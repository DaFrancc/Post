//! `posternd`: the Postern relay and storage server (design section 8).
//!
//! The server stores ciphertext, orders it, and checks what it can without
//! keys: device chains, KeyPackages, MLS headers, group access keys and
//! anonymous tokens. It never holds a key that could decrypt a message, and
//! it keeps no record of who is in which conversation.

pub mod db;
mod link;
mod session;
mod tokens;

use std::collections::{BTreeSet, HashMap, VecDeque};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use axum::Router;
use axum::extract::{State as AxState, WebSocketUpgrade};
use axum::response::IntoResponse;
use axum::routing::get;
use postern_identity::DeviceId;
use rusqlite::Connection;
use tokio::net::TcpListener;
use tokio::sync::Notify;
use zeroize::Zeroizing;

/// Server configuration.
#[derive(Clone)]
pub struct Config {
    /// The domain clients use for this server, such as `chat.example.org`.
    /// Account genesis links must name it.
    pub server_domain: String,
    /// Path to the SQLite database.
    pub db_path: PathBuf,
    /// Key encrypting the database at rest.
    pub db_key: Zeroizing<[u8; 32]>,
}

/// Shared server state.
pub(crate) struct State {
    pub domain: String,
    pub db: Db,
    pub hub: Hub,
    pub links: link::Links,
}

/// The database, used from blocking threads one request at a time.
#[derive(Clone)]
pub(crate) struct Db(Arc<Mutex<Connection>>);

impl Db {
    /// Run `f` with the connection on a blocking thread.
    pub async fn run<T: Send + 'static>(&self, f: impl FnOnce(&mut Connection) -> T + Send + 'static) -> T {
        let inner = self.0.clone();
        tokio::task::spawn_blocking(move || {
            let mut conn = inner.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            f(&mut conn)
        })
        .await
        .expect("database task panicked")
    }
}

/// Most relayed ephemeral signals queued for one connection. Beyond this the
/// oldest are dropped: they are only worth anything while fresh.
const EPHEMERAL_QUEUE: usize = 64;

/// Pending signals for one connection. Wakes coalesce: one wake covers
/// everything that arrived since the last.
#[derive(Default)]
pub(crate) struct Inbox {
    groups: Mutex<BTreeSet<Vec<u8>>>,
    ephemeral: Mutex<VecDeque<(Vec<u8>, Vec<u8>)>>,
    mailbox: AtomicBool,
    kicked: AtomicBool,
    pub notify: Notify,
}

/// What is pending for a connection.
pub(crate) struct Pending {
    pub kicked: bool,
    pub mailbox: bool,
    pub groups: Vec<Vec<u8>>,
    pub ephemeral: Vec<(Vec<u8>, Vec<u8>)>,
}

impl Inbox {
    pub fn take(&self) -> Pending {
        Pending {
            kicked: self.kicked.load(Ordering::Acquire),
            mailbox: self.mailbox.swap(false, Ordering::AcqRel),
            groups: std::mem::take(&mut *lock(&self.groups)).into_iter().collect(),
            ephemeral: std::mem::take(&mut *lock(&self.ephemeral)).into_iter().collect(),
        }
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

type Conns = HashMap<u64, Arc<Inbox>>;

/// Live connections: device sessions, and anonymous group subscriptions.
/// Held in memory only; subscriptions are never written anywhere.
#[derive(Default)]
pub(crate) struct Hub {
    sessions: Mutex<HashMap<DeviceId, Conns>>,
    subscriptions: Mutex<HashMap<Vec<u8>, Conns>>,
    next_id: AtomicU64,
}

impl Hub {
    pub fn new_connection(&self) -> (u64, Arc<Inbox>) {
        (self.next_id.fetch_add(1, Ordering::Relaxed), Arc::new(Inbox::default()))
    }

    pub fn join_session(&self, device: DeviceId, id: u64, inbox: Arc<Inbox>) {
        lock(&self.sessions).entry(device).or_default().insert(id, inbox);
    }

    pub fn leave_session(&self, device: DeviceId, id: u64) {
        let mut s = lock(&self.sessions);
        if let Some(m) = s.get_mut(&device) {
            m.remove(&id);
            if m.is_empty() {
                s.remove(&device);
            }
        }
    }

    /// Replace connection `id`'s subscriptions with `groups`.
    pub fn subscribe(&self, id: u64, inbox: &Arc<Inbox>, old: &[Vec<u8>], groups: &[Vec<u8>]) {
        let mut s = lock(&self.subscriptions);
        for g in old {
            if let Some(m) = s.get_mut(g) {
                m.remove(&id);
                if m.is_empty() {
                    s.remove(g);
                }
            }
        }
        for g in groups {
            s.entry(g.clone()).or_default().insert(id, inbox.clone());
        }
    }

    /// Wake every connection subscribed to `group` except `except`.
    pub fn wake_group(&self, group: &[u8], except: u64) {
        let s = lock(&self.subscriptions);
        for (id, inbox) in s.get(group).into_iter().flatten() {
            if *id != except {
                lock(&inbox.groups).insert(group.to_vec());
                inbox.notify.notify_one();
            }
        }
    }

    /// Relay an ephemeral signal to every connection subscribed to `group`
    /// except `except`. Held only in those connections' memory queues.
    pub fn relay(&self, group: &[u8], payload: &[u8], except: u64) {
        let s = lock(&self.subscriptions);
        for (id, inbox) in s.get(group).into_iter().flatten() {
            if *id != except {
                let mut q = lock(&inbox.ephemeral);
                if q.len() == EPHEMERAL_QUEUE {
                    q.pop_front();
                }
                q.push_back((group.to_vec(), payload.to_vec()));
                drop(q);
                inbox.notify.notify_one();
            }
        }
    }

    /// Tell these devices' sessions their mailbox has something new.
    pub fn wake_mailboxes(&self, devices: &[DeviceId]) {
        let s = lock(&self.sessions);
        for inbox in devices.iter().filter_map(|d| s.get(d)).flat_map(|m| m.values()) {
            inbox.mailbox.store(true, Ordering::Release);
            inbox.notify.notify_one();
        }
    }

    /// Disconnect these devices' sessions.
    pub fn kick(&self, devices: &[DeviceId]) {
        let s = lock(&self.sessions);
        for inbox in devices.iter().filter_map(|d| s.get(d)).flat_map(|m| m.values()) {
            inbox.kicked.store(true, Ordering::Release);
            inbox.notify.notify_one();
        }
    }
}

async fn ws_handler(ws: WebSocketUpgrade, AxState(state): AxState<Arc<State>>) -> impl IntoResponse {
    ws.max_message_size(postern_proto::MAX_FRAME_BYTES)
        .max_frame_size(postern_proto::MAX_FRAME_BYTES)
        .on_upgrade(move |socket| session::run(socket, state))
}

/// A server running in the background. Stops when dropped.
pub struct Running {
    /// The address it listens on.
    pub addr: SocketAddr,
    task: tokio::task::JoinHandle<()>,
}

impl Running {
    /// The WebSocket URL clients connect to.
    pub fn url(&self) -> String {
        format!("ws://{}/v1/ws", self.addr)
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// Errors starting the server.
#[derive(Debug, thiserror::Error)]
pub enum StartError {
    /// The configured domain is not valid.
    #[error("invalid server domain")]
    Domain,
    /// The database could not be opened, or the key is wrong.
    #[error("database: {0}")]
    Db(#[from] rusqlite::Error),
    /// Binding or serving failed.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

/// Start serving on `listener` in the background.
pub fn spawn(listener: TcpListener, config: Config) -> Result<Running, StartError> {
    postern_identity::chain::validate_domain(&config.server_domain).map_err(|_| StartError::Domain)?;
    let conn = db::open(&config.db_path, &config.db_key)?;
    let state = Arc::new(State {
        domain: config.server_domain,
        db: Db(Arc::new(Mutex::new(conn))),
        hub: Hub::default(),
        links: link::Links::default(),
    });
    let addr = listener.local_addr()?;

    let gc_db = state.db.clone();
    let app = Router::new().route("/v1/ws", get(ws_handler)).with_state(state);
    let task = tokio::spawn(async move {
        let gc = tokio::spawn(async move {
            let mut every = tokio::time::interval(Duration::from_secs(3600));
            loop {
                every.tick().await;
                match gc_db.run(db::collect_garbage).await {
                    Ok(n) if n > 0 => tracing::info!(rows = n, "expired data deleted"),
                    Ok(_) => {}
                    Err(e) => tracing::error!(error = %e, "garbage collection failed"),
                }
                match gc_db.run(db::mark_stale_devices).await {
                    Ok(n) if n > 0 => tracing::info!(devices = n, "devices marked stale"),
                    Ok(_) => {}
                    Err(e) => tracing::error!(error = %e, "marking stale devices failed"),
                }
            }
        });
        if let Err(e) = axum::serve(listener, app).await {
            tracing::error!(error = %e, "server stopped");
        }
        gc.abort();
    });
    Ok(Running { addr, task })
}
