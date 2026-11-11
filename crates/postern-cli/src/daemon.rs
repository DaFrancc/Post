//! `postern daemon`: the local API as JSON-RPC 2.0, one JSON object per line.
//!
//! Transports: stdin/stdout (a wrapper spawns the daemon), or a Unix socket
//! with mode 0600 (several local tools share one daemon). Anyone who can
//! reach the transport has the full power of the account, so access is left
//! to operating-system permissions (design section 11).
//!
//! Beyond the API methods, two daemon methods:
//! - `subscribe`: send this client `event` notifications from now on.
//! - `schema`: the OpenRPC document.

use postern_api::{ApiError, Needs};
use postern_core::Core;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt as _, AsyncRead, AsyncWrite, AsyncWriteExt as _, BufReader};
use tokio::sync::mpsc;

/// Keep the core connected, reconnecting with backoff. Runs until cancelled.
pub async fn supervise(core: Core) {
    core.run().await;
}

fn error_response(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

async fn handle(core: &Core, method: &str, params: Value) -> Result<Value, ApiError> {
    let info = postern_api::method(method).ok_or_else(|| ApiError { code: -32601, message: format!("unknown method {method}") })?;
    if info.needs == Needs::Connection && !core.is_connected().await {
        core.connect().await?;
    }
    match postern_api::call(core, method, params.clone()).await {
        // A local method that needed the server after all, such as looking
        // up a user this device has never seen.
        Err(e) if e.code == 1003 && info.needs == Needs::Local => {
            core.connect().await?;
            postern_api::call(core, method, params).await
        }
        r => r,
    }
}

/// Serve one client over a byte stream until it closes.
pub async fn serve<R, W>(core: Core, reader: R, writer: W)
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin + Send + 'static,
{
    let (out_tx, mut out_rx) = mpsc::channel::<Value>(256);
    let writer_task = tokio::spawn(async move {
        let mut writer = writer;
        while let Some(v) = out_rx.recv().await {
            let mut line = v.to_string();
            line.push('\n');
            if writer.write_all(line.as_bytes()).await.is_err() || writer.flush().await.is_err() {
                break;
            }
        }
    });

    let mut events_task: Option<tokio::task::JoinHandle<()>> = None;
    let mut lines = BufReader::new(reader).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        if line.trim().is_empty() {
            continue;
        }
        let req: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(e) => {
                let _ = out_tx.send(error_response(Value::Null, -32700, &format!("parse error: {e}"))).await;
                continue;
            }
        };
        let id = req.get("id").cloned();
        let Some(method) = req.get("method").and_then(Value::as_str).map(str::to_owned) else {
            let _ = out_tx.send(error_response(id.unwrap_or(Value::Null), -32600, "request has no method")).await;
            continue;
        };
        let params = req.get("params").cloned().unwrap_or(Value::Null);

        let result = match method.as_str() {
            "subscribe" => {
                if events_task.is_none() {
                    let mut rx = core.subscribe();
                    let tx = out_tx.clone();
                    events_task = Some(tokio::spawn(async move {
                        loop {
                            match rx.recv().await {
                                Ok(e) => {
                                    let n = json!({ "jsonrpc": "2.0", "method": "event", "params": postern_api::event(e) });
                                    if tx.send(n).await.is_err() {
                                        break;
                                    }
                                }
                                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                                    let n = json!({ "jsonrpc": "2.0", "method": "event", "params": { "type": "lagged", "missed": n } });
                                    if tx.send(n).await.is_err() {
                                        break;
                                    }
                                }
                                Err(_) => break,
                            }
                        }
                    }));
                }
                Ok(json!({}))
            }
            "schema" => Ok(postern_api::openrpc()),
            m => handle(&core, m, params).await,
        };
        // A request without an id is a notification: no response.
        let Some(id) = id else { continue };
        let response = match result {
            Ok(v) => json!({ "jsonrpc": "2.0", "id": id, "result": v }),
            Err(e) => error_response(id, e.code, &e.message),
        };
        if out_tx.send(response).await.is_err() {
            break;
        }
    }
    if let Some(t) = events_task {
        t.abort();
    }
    drop(out_tx);
    let _ = writer_task.await;
}

/// Serve clients on a Unix socket at `path`, readable only by this user.
#[cfg(unix)]
pub async fn serve_socket(core: Core, path: &std::path::Path) -> anyhow::Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    use anyhow::Context as _;

    if path.exists() {
        // A socket left by a daemon that died. Refuse if one is still live.
        if tokio::net::UnixStream::connect(path).await.is_ok() {
            anyhow::bail!("a daemon is already listening on {}", path.display());
        }
        std::fs::remove_file(path).with_context(|| format!("removing stale socket {}", path.display()))?;
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let listener = tokio::net::UnixListener::bind(path).with_context(|| format!("binding {}", path.display()))?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    tracing::info!(socket = %path.display(), "daemon listening");
    loop {
        let (stream, _) = listener.accept().await?;
        let (r, w) = stream.into_split();
        tokio::spawn(serve(core.clone(), r, w));
    }
}
