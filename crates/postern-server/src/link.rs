//! Device-link rendezvous (design section 5, "Linking a device").
//!
//! A rendezvous is a set of write-once slots, named by a 16-byte ID derived
//! from the link code. The two devices take turns filling slots with
//! payloads sealed under a key derived from the same code, which the server
//! never sees. Everything lives in memory and expires after [`TTL`]: a
//! restart forgets it, and the devices simply start again.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use postern_proto::{self as proto, ErrorCode, LINK_SLOTS, MAX_LINK_SLOT_BYTES, MAX_LINK_WAIT_MS};
use tokio::sync::Notify;

use crate::db::err;

/// How long a rendezvous lives after it is created.
pub const TTL: Duration = Duration::from_secs(600);
/// Most rendezvous held at once, across all connections. Linking is rare,
/// so this only bounds memory if someone floods the server.
pub const MAX_RENDEZVOUS: usize = 256;
/// Most rendezvous one connection may create.
pub const MAX_PER_CONNECTION: u32 = 4;

struct Rendezvous {
    created: Instant,
    slots: Vec<Option<Vec<u8>>>,
    notify: Arc<Notify>,
}

/// Every live rendezvous.
#[derive(Default)]
pub(crate) struct Links {
    map: Mutex<HashMap<[u8; 16], Rendezvous>>,
}

fn rendezvous_id(b: &[u8]) -> Result<[u8; 16], proto::Error> {
    b.try_into().map_err(|_| err(ErrorCode::BadRequest, "rendezvous id must be 16 bytes"))
}

fn slot_index(slot: u32) -> Result<usize, proto::Error> {
    if slot < LINK_SLOTS { Ok(slot as usize) } else { Err(err(ErrorCode::BadRequest, "no such link slot")) }
}

fn gone() -> proto::Error {
    err(ErrorCode::NotFound, "no such link request, or it expired")
}

impl Links {
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<[u8; 16], Rendezvous>> {
        let mut map = self.map.lock().unwrap_or_else(|p| p.into_inner());
        map.retain(|_, r| r.created.elapsed() < TTL);
        map
    }

    /// Write a slot. Slot 0 creates the rendezvous; `created` counts how many
    /// this connection has created.
    pub fn put(&self, req: &proto::LinkPut, created: &mut u32) -> Result<(), proto::Error> {
        let id = rendezvous_id(&req.rendezvous)?;
        let slot = slot_index(req.slot)?;
        if req.payload.is_empty() || req.payload.len() > MAX_LINK_SLOT_BYTES {
            return Err(err(ErrorCode::BadRequest, "link payload is empty or too large"));
        }
        let mut map = self.lock();
        if slot == 0 {
            if map.contains_key(&id) {
                return Err(err(ErrorCode::Conflict, "link request already exists"));
            }
            if map.len() >= MAX_RENDEZVOUS || *created >= MAX_PER_CONNECTION {
                return Err(err(ErrorCode::RateLimited, "too many link requests; try again later"));
            }
            *created += 1;
            let mut slots = vec![None; LINK_SLOTS as usize];
            slots[0] = Some(req.payload.clone());
            map.insert(id, Rendezvous { created: Instant::now(), slots, notify: Arc::new(Notify::new()) });
            return Ok(());
        }
        let r = map.get_mut(&id).ok_or_else(gone)?;
        if r.slots[slot].is_some() {
            return Err(err(ErrorCode::Conflict, "link slot already written"));
        }
        r.slots[slot] = Some(req.payload.clone());
        r.notify.notify_waiters();
        Ok(())
    }

    fn read(&self, id: &[u8; 16], slot: usize) -> Result<(Option<Vec<u8>>, Arc<Notify>), proto::Error> {
        let map = self.lock();
        let r = map.get(id).ok_or_else(gone)?;
        Ok((r.slots[slot].clone(), r.notify.clone()))
    }

    /// Read a slot, waiting up to the requested time for it to be written.
    pub async fn get(&self, req: &proto::LinkGet) -> Result<proto::LinkSlot, proto::Error> {
        let id = rendezvous_id(&req.rendezvous)?;
        let slot = slot_index(req.slot)?;
        let deadline = tokio::time::Instant::now() + Duration::from_millis(u64::from(req.wait_ms.min(MAX_LINK_WAIT_MS)));
        loop {
            let (value, notify) = self.read(&id, slot)?;
            if let Some(payload) = value {
                return Ok(proto::LinkSlot { present: true, payload });
            }
            // Register for the wake-up before checking again, so a write
            // between the two checks is not missed.
            let notified = notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if let (Some(payload), _) = self.read(&id, slot)? {
                return Ok(proto::LinkSlot { present: true, payload });
            }
            tokio::select! {
                () = &mut notified => {}
                () = tokio::time::sleep_until(deadline) => return Ok(proto::LinkSlot { present: false, payload: Vec::new() }),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn put(r: [u8; 16], slot: u32, payload: &[u8]) -> proto::LinkPut {
        proto::LinkPut { rendezvous: r.to_vec(), slot, payload: payload.to_vec() }
    }

    fn get(r: [u8; 16], slot: u32, wait_ms: u32) -> proto::LinkGet {
        proto::LinkGet { rendezvous: r.to_vec(), slot, wait_ms }
    }

    #[tokio::test]
    async fn slots_are_write_once_and_waitable() {
        let links = Arc::new(Links::default());
        let mut created = 0;
        let code = |e: Result<_, proto::Error>| e.err().map(|e| e.code());

        // Only slot 0 creates a rendezvous.
        assert_eq!(code(links.put(&put([1; 16], 1, b"x"), &mut created)), Some(ErrorCode::NotFound));
        links.put(&put([1; 16], 0, b"offer"), &mut created).expect("create");
        assert_eq!(code(links.put(&put([1; 16], 0, b"again"), &mut created)), Some(ErrorCode::Conflict));
        assert_eq!(links.get(&get([1; 16], 0, 0)).await.expect("read").payload, b"offer");

        // An empty slot times out without error.
        assert!(!links.get(&get([1; 16], 1, 10)).await.expect("wait").present);

        // A waiting reader wakes when the slot is written.
        let reader = {
            let links = links.clone();
            tokio::spawn(async move { links.get(&get([1; 16], 1, 5_000)).await })
        };
        tokio::time::sleep(Duration::from_millis(20)).await;
        links.put(&put([1; 16], 1, b"answer"), &mut created).expect("answer");
        assert_eq!(reader.await.expect("join").expect("read").payload, b"answer");
        assert_eq!(code(links.put(&put([1; 16], 1, b"forged"), &mut created)), Some(ErrorCode::Conflict));

        // Bounds.
        assert_eq!(code(links.put(&put([1; 16], LINK_SLOTS, b"x"), &mut created)), Some(ErrorCode::BadRequest));
        assert_eq!(code(links.put(&put([1; 16], 2, b""), &mut created)), Some(ErrorCode::BadRequest));
        let big = vec![0; MAX_LINK_SLOT_BYTES + 1];
        assert_eq!(code(links.put(&put([1; 16], 2, &big), &mut created)), Some(ErrorCode::BadRequest));
        assert_eq!(links.get(&get([9; 16], 0, 0)).await.err().map(|e| e.code()), Some(ErrorCode::NotFound));
    }

    #[test]
    fn creation_is_limited_per_connection() {
        let links = Links::default();
        let mut created = 0;
        for i in 0..MAX_PER_CONNECTION as u8 {
            links.put(&put([i; 16], 0, b"x"), &mut created).expect("create");
        }
        let e = links.put(&put([200; 16], 0, b"x"), &mut created).err().map(|e| e.code());
        assert_eq!(e, Some(ErrorCode::RateLimited));
    }
}
