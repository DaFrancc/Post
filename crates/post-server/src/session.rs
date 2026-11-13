//! One WebSocket connection.
//!
//! 1. Send `Hello` with a fresh nonce.
//! 2. The first request decides the connection's kind for good:
//!    - `Register` or `Auth` (signed over the nonce): a **session** for that
//!      device, allowing only device-scoped requests.
//!    - Any conversation request: an **anonymous** connection, allowing only
//!      conversation requests, authorized by access keys or tokens.
//! 3. Handle requests in order; forward wakes; close on kick.
//!
//! Nothing here logs addresses or payloads.

use std::sync::Arc;
use std::time::Duration;

use axum::extract::ws::{Message, WebSocket};
use futures_util::SinkExt as _;
use post_identity::{AccountId, Chain, DeviceId, HybridSignature, labels};
use post_proto::{self as proto, ClientFrame, ErrorCode, Reply, Request, ServerFrame, ServerKind};
use prost::Message as _;

use crate::announce::{self, By};
use crate::{Inbox, State, db, tokens};

/// How long a new connection has to send its first request.
const FIRST_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

enum Kind {
    Session { account: AccountId, device: DeviceId },
    Anonymous { subscribed: Vec<Vec<u8>>, limiter: Limiter, links_created: u32 },
}

/// Ephemeral signals one anonymous connection may send per second. Typing
/// sends one every few seconds per conversation; presence, one per group
/// every ten minutes.
const EPHEMERAL_PER_SECOND: u32 = 20;

/// A fixed-window rate limiter.
struct Limiter {
    window: std::time::Instant,
    count: u32,
}

impl Limiter {
    fn new() -> Self {
        Self { window: std::time::Instant::now(), count: 0 }
    }

    fn allow(&mut self) -> bool {
        if self.window.elapsed() >= Duration::from_secs(1) {
            self.window = std::time::Instant::now();
            self.count = 0;
        }
        self.count += 1;
        self.count <= EPHEMERAL_PER_SECOND
    }
}

struct Conn {
    state: Arc<State>,
    id: u64,
    inbox: Arc<Inbox>,
    kind: Kind,
}

async fn send(socket: &mut WebSocket, kind: ServerKind) -> bool {
    let frame = ServerFrame { kind: Some(kind) }.encode_to_vec();
    socket.send(Message::Binary(frame.into())).await.is_ok()
}

async fn reply(socket: &mut WebSocket, id: u64, reply: Reply) -> bool {
    send(socket, ServerKind::Response(proto::Response { id, reply: Some(reply) })).await
}

/// Next binary frame, decoded. `None` when the connection ends or misbehaves.
async fn next_frame(socket: &mut WebSocket) -> Option<ClientFrame> {
    loop {
        match socket.recv().await? {
            Ok(Message::Binary(b)) => return ClientFrame::decode(b.as_ref()).ok(),
            Ok(Message::Ping(_) | Message::Pong(_)) => continue,
            Ok(Message::Text(_) | Message::Close(_)) | Err(_) => return None,
        }
    }
}

fn verify_login(chain: &Chain, device: &DeviceId, domain: &str, nonce: &[u8], ed: &[u8], ml: &[u8]) -> bool {
    let Ok(sig) = HybridSignature::from_parts(ed, ml) else { return false };
    let Some(d) = chain.device(device).filter(|d| d.is_active()) else { return false };
    d.entry.chain_key.verify(labels::AUTH, &proto::auth_message(domain, nonce), &sig).is_ok()
}

fn wrong_connection(session: bool) -> proto::Error {
    let msg = if session {
        "conversation requests must use an anonymous connection"
    } else {
        "device requests must use a logged-in session"
    };
    proto::Error::new(ErrorCode::WrongConnection, msg)
}

pub(crate) async fn run(mut socket: WebSocket, state: Arc<State>) {
    let mut nonce = [0u8; 32];
    if getrandom::fill(&mut nonce).is_err() {
        return;
    }
    let hello = proto::Hello { protocol_version: proto::PROTOCOL_VERSION, server_domain: state.domain.clone(), nonce: nonce.to_vec() };
    if !send(&mut socket, ServerKind::Hello(hello)).await {
        return;
    }
    let Ok(Some(first)) = tokio::time::timeout(FIRST_REQUEST_TIMEOUT, next_frame(&mut socket)).await else { return };
    let Some(request) = first.request else { return };

    let (id, inbox) = state.hub.new_connection();
    let mut conn = if request.is_session() {
        match login(&state, &nonce, request).await {
            Ok((account, device, authenticated)) => {
                if !reply(&mut socket, first.id, Reply::Authenticated(authenticated)).await {
                    return;
                }
                state.hub.join_session(device, id, inbox.clone());
                Conn { state: state.clone(), id, inbox, kind: Kind::Session { account, device } }
            }
            Err(e) => {
                reply(&mut socket, first.id, Reply::Error(e)).await;
                return;
            }
        }
    } else {
        let mut conn =
            Conn { state: state.clone(), id, inbox, kind: Kind::Anonymous { subscribed: Vec::new(), limiter: Limiter::new(), links_created: 0 } };
        let r = conn.handle(request).await.unwrap_or_else(Reply::Error);
        if !reply(&mut socket, first.id, r).await {
            conn.close();
            return;
        }
        conn
    };

    loop {
        tokio::select! {
            frame = next_frame(&mut socket) => {
                let Some(frame) = frame else { break };
                let r = match frame.request {
                    Some(req) => conn.handle(req).await.unwrap_or_else(Reply::Error),
                    None => Reply::Error(proto::Error::new(ErrorCode::BadRequest, "empty request")),
                };
                if !reply(&mut socket, frame.id, r).await {
                    break;
                }
            }
            () = conn.inbox.notify.notified() => {
                let p = conn.inbox.take();
                if p.kicked {
                    break;
                }
                let mut ok = true;
                for (group_id, payload) in p.ephemeral {
                    ok &= send(&mut socket, ServerKind::Ephemeral(proto::Ephemeral { group_id, payload })).await;
                }
                if p.mailbox || p.announcements || !p.groups.is_empty() {
                    let wake = proto::Wake { mailbox: p.mailbox, groups: p.groups, announcements: p.announcements };
                    ok &= send(&mut socket, ServerKind::Wake(wake)).await;
                }
                if !ok {
                    break;
                }
            }
        }
    }
    conn.close();
    let _ = socket.close().await;
}

async fn login(state: &Arc<State>, nonce: &[u8; 32], req: Request) -> db::Result<(AccountId, DeviceId, proto::Authenticated)> {
    let domain = state.domain.clone();
    let nonce = *nonce;
    let (account, device) = match req {
        Request::Register(r) => {
            state
                .db
                .run(move |c| {
                    db::register(c, &domain, &r.invite_code, &r.genesis, |chain| {
                        let id = chain.devices()[0].id;
                        verify_login(chain, &id, &domain, &nonce, &r.auth_sig_ed25519, &r.auth_sig_mldsa65)
                    })
                })
                .await?
        }
        Request::Auth(a) => {
            let device =
                DeviceId::from_slice(&a.device_id).map_err(|_| proto::Error::new(ErrorCode::BadRequest, "device id must be 16 bytes"))?;
            state
                .db
                .run(move |c| {
                    let (account, chain) = db::device_for_login(c, &device)?;
                    if verify_login(&chain, &device, &domain, &nonce, &a.sig_ed25519, &a.sig_mldsa65) {
                        Ok((account, device))
                    } else {
                        Err(proto::Error::new(ErrorCode::Unauthenticated, "bad login signature"))
                    }
                })
                .await?
        }
        _ => return Err(proto::Error::new(ErrorCode::Unauthenticated, "log in first")),
    };
    let (remaining, has_last_resort) = state.db.run(move |c| db::key_package_counts(c, &device)).await?;
    Ok((account, device, proto::Authenticated { account_id: account.0.to_vec(), key_packages_remaining: remaining, has_last_resort }))
}

impl Conn {
    fn close(&self) {
        match &self.kind {
            Kind::Session { device, .. } => self.state.hub.leave_session(*device, self.id),
            Kind::Anonymous { subscribed, .. } => self.state.hub.subscribe(self.id, &self.inbox, subscribed, &[]),
        }
    }

    async fn handle(&mut self, req: Request) -> db::Result<Reply> {
        if matches!(req, Request::Register(_) | Request::Auth(_)) {
            return Err(proto::Error::new(ErrorCode::BadRequest, "login must be the first request"));
        }
        let session = matches!(self.kind, Kind::Session { .. });
        if req.is_session() != session {
            return Err(wrong_connection(session));
        }
        match &mut self.kind {
            Kind::Session { account, device } => Self::session_request(&self.state, *account, *device, req).await,
            Kind::Anonymous { subscribed, limiter, links_created } => {
                let ctx = Anon { id: self.id, inbox: &self.inbox, subscribed, limiter, links_created };
                Self::anonymous_request(&self.state, ctx, req).await
            }
        }
    }

    async fn session_request(st: &Arc<State>, account: AccountId, device: DeviceId, req: Request) -> db::Result<Reply> {
        Ok(match req {
            Request::Directory(r) => Reply::DirectoryUpdate(st.db.run(move |c| db::directory(c, r.since_version)).await?),
            Request::AppendChain(r) => {
                let update = st.db.run(move |c| db::append_chain(c, &account, &r.link)).await?;
                st.hub.kick(&update.removed);
                Reply::Empty(proto::Empty {})
            }
            Request::UploadKeyPackages(r) => {
                let last = (!r.last_resort.is_empty()).then_some(r.last_resort);
                let remaining =
                    st.db.run(move |c| db::upload_key_packages(c, &account, &device, &r.key_packages, last.as_deref())).await?;
                Reply::KeyPackageCount(proto::KeyPackageCount { remaining })
            }
            Request::FetchMailbox(r) => Reply::Mailbox(st.db.run(move |c| db::fetch_mailbox(c, &device, r.limit)).await?),
            Request::AckMailbox(r) => {
                st.db.run(move |c| db::ack_mailbox(c, &device, r.seq)).await?;
                Reply::Empty(proto::Empty {})
            }
            Request::GetTokenKey(_) => {
                let k = st.db.run(|c| tokens::current_key(c)).await?;
                Reply::TokenKey(proto::TokenKey {
                    version: proto::TOKEN_VERSION_RSABSSA_SHA384,
                    key_id: k.key_id,
                    public_key: k.public_der,
                    valid_until_ms: k.valid_until_ms,
                })
            }
            Request::IssueTokens(r) => {
                let sigs = st.db.run(move |c| tokens::issue(c, &device.0, &r.key_id, &r.blinded)).await?;
                Reply::IssuedTokens(proto::IssuedTokens { blind_signatures: sigs })
            }
            Request::FetchAnnouncements(r) => {
                Reply::Announcements(st.db.run(move |c| announce::fetch(c, &account, r.after_id, r.limit)).await?)
            }
            Request::Announce(r) => {
                let v = st.db.run(move |c| announce::announce(c, By::Host(&account), Some(&r.client_msg_id), &r.text)).await?;
                st.announced(v);
                Reply::Empty(proto::Empty {})
            }
            Request::PlanMaintenance(r) => {
                let v = st.db.run(move |c| announce::plan(c, By::Host(&account), &r)).await?;
                st.announced(v);
                Reply::Empty(proto::Empty {})
            }
            Request::CreateInvite(r) => {
                let days = if r.days == 0 { 7 } else { r.days };
                if days > proto::MAX_INVITE_DAYS {
                    return Err(proto::Error::new(ErrorCode::BadRequest, "an invite lasts at most 30 days"));
                }
                let valid_ms = u64::from(days) * 24 * 3600 * 1000;
                let code = st
                    .db
                    .run(move |c| {
                        announce::check_host(c, By::Host(&account))?;
                        db::create_invite(c, r.bot, valid_ms).map_err(|e| {
                            tracing::error!(error = %e, "creating an invite failed");
                            proto::Error::new(ErrorCode::Internal, "database error")
                        })
                    })
                    .await?;
                tracing::info!("a host made an invite");
                Reply::Invite(proto::Invite { code, expires_at_ms: db::now_ms() + valid_ms })
            }
            Request::UploadBlob(r) => Reply::BlobStored(st.blobs.upload(&st.db, account, r).await?),
            Request::EndMaintenance(_) => {
                let (_, v) = st.db.run(move |c| announce::end(c, By::Host(&account))).await?;
                st.announced(v);
                Reply::Empty(proto::Empty {})
            }
            _ => return Err(wrong_connection(true)),
        })
    }

    async fn anonymous_request(st: &Arc<State>, ctx: Anon<'_>, req: Request) -> db::Result<Reply> {
        let Anon { id, inbox, subscribed, limiter, links_created } = ctx;
        Ok(match req {
            Request::CreateGroup(r) => {
                let welcomed = st.db.run(move |c| db::create_group(c, &r)).await?;
                st.hub.wake_mailboxes(&welcomed);
                Reply::GroupCreated(proto::GroupCreated { epoch: 1 })
            }
            Request::Send(r) => {
                let group = r.group_id.clone();
                let (seq, epoch, new) = st.db.run(move |c| db::send(c, &r)).await?;
                if new {
                    st.hub.wake_group(&group, id);
                }
                Reply::Accepted(proto::Accepted { seq, epoch })
            }
            Request::Commit(r) => {
                let group = r.group_id.clone();
                let (seq, epoch, welcomed, new) = st.db.run(move |c| db::commit(c, &r)).await?;
                if new {
                    st.hub.wake_group(&group, id);
                    st.hub.wake_mailboxes(&welcomed);
                }
                Reply::Accepted(proto::Accepted { seq, epoch })
            }
            Request::FetchGroups(r) => Reply::GroupBatch(st.db.run(move |c| db::fetch_groups(c, &r)).await?),
            Request::Subscribe(r) => {
                let groups = st.db.run(move |c| db::valid_subscriptions(c, &r.groups)).await?;
                st.hub.subscribe(id, inbox, subscribed, &groups);
                *subscribed = groups;
                Reply::Empty(proto::Empty {})
            }
            Request::ClaimKeyPackages(r) => {
                let kps = st.db.run(move |c| db::claim_key_packages(c, &r)).await?;
                Reply::ClaimedKeyPackages(proto::ClaimedKeyPackages { key_packages: kps })
            }
            Request::SendEphemeral(r) => {
                if r.payload.len() > proto::MAX_EPHEMERAL_BYTES {
                    return Err(proto::Error::new(ErrorCode::BadRequest, "ephemeral payload too large"));
                }
                if !limiter.allow() {
                    return Err(proto::Error::new(ErrorCode::RateLimited, "too many ephemeral signals"));
                }
                let (group, access) = (r.group_id.clone(), r.access_key.clone());
                st.db.run(move |c| db::check_current_access(c, &group, &access)).await?;
                st.hub.relay(&r.group_id, &r.payload, id);
                Reply::Empty(proto::Empty {})
            }
            Request::LinkPut(r) => {
                st.links.put(&r, links_created)?;
                Reply::Empty(proto::Empty {})
            }
            Request::LinkGet(r) => Reply::LinkSlot(st.links.get(&r).await?),
            // Requests on one connection are handled one at a time, so a
            // connection downloads one picture at a time.
            Request::FetchBlob(r) => Reply::Blob(st.blobs.fetch(&st.db, r).await?),
            Request::RenewBlob(r) => Reply::BlobStored(st.blobs.renew(&st.db, r).await?),
            Request::DeleteBlob(r) => {
                st.blobs.delete(&st.db, r).await?;
                Reply::Empty(proto::Empty {})
            }
            Request::CallToken(r) => {
                let Some(calls) = &st.calls else {
                    return Err(proto::Error::new(ErrorCode::Unavailable, "this server does not offer calls"));
                };
                if r.call_id.len() != 16 || r.participant_id.len() != 16 {
                    return Err(proto::Error::new(ErrorCode::BadRequest, "call and participant IDs are 16 bytes"));
                }
                if !limiter.allow() {
                    return Err(proto::Error::new(ErrorCode::RateLimited, "too many requests"));
                }
                let (group, access) = (r.group_id.clone(), r.access_key.clone());
                st.db.run(move |c| db::check_current_access(c, &group, &access)).await?;
                let room = crate::calls::room_name(&r.group_id, &r.call_id);
                let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
                Reply::CallAccess(proto::CallAccess {
                    url: calls.url.clone(),
                    token: crate::calls::token(calls, &room, &hex::encode(&r.participant_id), now),
                    max_bitrate_kbps: calls.max_bitrate_kbps,
                    max_participants: calls.max_participants,
                })
            }
            _ => return Err(wrong_connection(false)),
        })
    }
}

/// An anonymous connection's own state, borrowed for one request.
struct Anon<'a> {
    id: u64,
    inbox: &'a Arc<Inbox>,
    subscribed: &'a mut Vec<Vec<u8>>,
    limiter: &'a mut Limiter,
    links_created: &'a mut u32,
}
