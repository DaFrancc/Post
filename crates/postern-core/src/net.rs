//! The connection to the server: one WebSocket carrying protobuf frames
//! (design section 8). Requests are matched to responses by id; `Wake`
//! frames are surfaced through [`ServerConn::wake`].

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::{SinkExt as _, StreamExt as _};
use postern_proto::{ClientFrame, Hello, Reply, Request, ServerFrame, ServerKind};
use prost::Message as _;
use tokio::sync::{Notify, mpsc, oneshot, watch};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;

use crate::{Error, Result};

/// How long to wait for any one response.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

type Pending = Arc<Mutex<HashMap<u64, oneshot::Sender<Reply>>>>;

/// A live server connection: a session once logged in, or anonymous.
pub struct ServerConn {
    out: mpsc::Sender<Vec<u8>>,
    pending: Pending,
    next_id: AtomicU64,
    /// Notified when the server says new entries are waiting.
    pub wake: Arc<Notify>,
    closed: watch::Receiver<bool>,
    tasks: Vec<tokio::task::JoinHandle<()>>,
}

impl Drop for ServerConn {
    fn drop(&mut self) {
        for t in &self.tasks {
            t.abort();
        }
    }
}

impl ServerConn {
    /// Connect and read the server's `Hello`.
    pub async fn connect(url: &str) -> Result<(Self, Hello)> {
        let config = WebSocketConfig::default()
            .max_message_size(Some(postern_proto::MAX_FRAME_BYTES))
            .max_frame_size(Some(postern_proto::MAX_FRAME_BYTES));
        let (ws, _) = tokio_tungstenite::connect_async_with_config(url, Some(config), false)
            .await
            .map_err(|e| Error::Network(format!("connecting to {url}: {e}")))?;
        let (mut sink, mut stream) = ws.split();

        let hello = loop {
            match tokio::time::timeout(REQUEST_TIMEOUT, stream.next()).await {
                Ok(Some(Ok(Message::Binary(b)))) => match ServerFrame::decode(b.as_ref()) {
                    Ok(ServerFrame { kind: Some(ServerKind::Hello(h)) }) => break h,
                    _ => return Err(Error::Protocol("expected Hello".into())),
                },
                Ok(Some(Ok(Message::Ping(_) | Message::Pong(_)))) => continue,
                Ok(_) => return Err(Error::Network("connection closed before Hello".into())),
                Err(_) => return Err(Error::Network("timed out waiting for Hello".into())),
            }
        };
        if hello.protocol_version != postern_proto::PROTOCOL_VERSION {
            return Err(Error::Protocol(format!("server speaks protocol {}", hello.protocol_version)));
        }

        let pending: Pending = Arc::default();
        let wake = Arc::new(Notify::new());
        let (closed_tx, closed) = watch::channel(false);
        let (out, mut out_rx) = mpsc::channel::<Vec<u8>>(64);

        let writer = tokio::spawn(async move {
            while let Some(frame) = out_rx.recv().await {
                if sink.send(Message::Binary(frame.into())).await.is_err() {
                    break;
                }
            }
            let _ = sink.close().await;
        });
        let (r_pending, r_wake) = (pending.clone(), wake.clone());
        let reader = tokio::spawn(async move {
            while let Some(Ok(msg)) = stream.next().await {
                let Message::Binary(b) = msg else { continue };
                match ServerFrame::decode(b.as_ref()).map(|f| f.kind) {
                    Ok(Some(ServerKind::Response(r))) => {
                        let tx = r_pending.lock().unwrap_or_else(|p| p.into_inner()).remove(&r.id);
                        if let (Some(tx), Some(reply)) = (tx, r.reply) {
                            let _ = tx.send(reply);
                        }
                    }
                    Ok(Some(ServerKind::Wake(_))) => r_wake.notify_one(),
                    // Typing and presence are handled from phase 4.
                    Ok(Some(ServerKind::Ephemeral(_))) => {}
                    _ => break,
                }
            }
            // Dropping the senders fails every waiting request.
            r_pending.lock().unwrap_or_else(|p| p.into_inner()).clear();
            let _ = closed_tx.send(true);
            r_wake.notify_one();
        });

        Ok((Self { out, pending, next_id: AtomicU64::new(1), wake, closed, tasks: vec![writer, reader] }, hello))
    }

    /// Send a request and wait for its reply. A protocol `Error` reply comes
    /// back as [`Error::Server`].
    pub async fn request(&self, req: Request) -> Result<Reply> {
        if self.is_closed() {
            return Err(Error::NotConnected);
        }
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().unwrap_or_else(|p| p.into_inner()).insert(id, tx);
        let frame = ClientFrame { id, request: Some(req) }.encode_to_vec();
        if self.out.send(frame).await.is_err() {
            return Err(Error::NotConnected);
        }
        match tokio::time::timeout(REQUEST_TIMEOUT, rx).await {
            Ok(Ok(Reply::Error(e))) => Err(Error::Server(e)),
            Ok(Ok(reply)) => Ok(reply),
            Ok(Err(_)) => Err(Error::NotConnected),
            Err(_) => {
                self.pending.lock().unwrap_or_else(|p| p.into_inner()).remove(&id);
                Err(Error::Network("request timed out".into()))
            }
        }
    }

    /// Whether the connection has ended.
    pub fn is_closed(&self) -> bool {
        *self.closed.borrow()
    }

    /// A future that resolves when the connection ends, usable after the
    /// connection itself is out of reach.
    pub fn closed_signal(&self) -> Closed {
        let mut rx = self.closed.clone();
        Box::pin(async move {
            let _ = rx.wait_for(|c| *c).await;
        })
    }
}

/// Resolves when a connection ends.
pub type Closed = std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>;

/// Unwrap the expected reply variant, or fail with a protocol error.
macro_rules! expect_reply {
    ($reply:expr, $variant:path) => {
        match $reply {
            $variant(inner) => Ok(inner),
            other => Err($crate::Error::Protocol(format!("unexpected reply: {other:?}"))),
        }
    };
}
pub(crate) use expect_reply;
