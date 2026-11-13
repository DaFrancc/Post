//! Small blobs: profile pictures and banners (design section 7, "Avatar").
//!
//! Each is ciphertext the server cannot open, kept as a file named by its
//! random ID in the blobs folder, with a row saying when it expires and the
//! hash of the secret that renews or deletes it. **No row says whose it
//! is**: the upload comes over a session, to count against that account's
//! daily quota, and everything after it over anonymous connections.
//!
//! The file is written before its row and removed after it, so a row never
//! points at a missing file; a crash in between leaves at most a stray file,
//! which [`Blobs::sweep`] removes.

use std::path::{Path, PathBuf};

use post_identity::AccountId;
use post_proto::{self as proto, ErrorCode};
use rusqlite::{Connection, OptionalExtension, params};
use sha2::{Digest, Sha256};

use crate::Db;
use crate::db::{Internal as _, Result, err, now_ms, sql_i64, today, write_tx};

/// Where blobs live and how much room they may take.
#[derive(Clone, Debug)]
pub struct BlobConfig {
    /// The folder holding one file per blob.
    pub dir: PathBuf,
    /// The most all blobs together may take, in bytes.
    pub max_total_bytes: u64,
}

pub(crate) struct Blobs {
    config: BlobConfig,
}

fn blob_id(b: &[u8]) -> Result<[u8; 16]> {
    b.try_into().map_err(|_| err(ErrorCode::BadRequest, "a blob id is 16 bytes"))
}

fn io(e: &std::io::Error) -> proto::Error {
    tracing::error!(error = %e, "blob storage failed");
    err(ErrorCode::Internal, "storage error")
}

/// Whether `secret` is the one whose hash is `hash`, in constant time.
fn secret_matches(secret: &[u8], hash: &[u8]) -> bool {
    let got = Sha256::digest(secret);
    got.len() == hash.len() && got.iter().zip(hash).fold(0u8, |acc, (a, b)| acc | (a ^ b)) == 0
}

impl BlobConfig {
    /// A `blobs` folder beside the database, holding at most 4 GB.
    pub fn beside(db_path: &Path) -> Self {
        let dir = db_path.parent().unwrap_or_else(|| Path::new(".")).join("blobs");
        Self { dir, max_total_bytes: 4096 * 1024 * 1024 }
    }
}

impl Blobs {
    pub fn new(config: BlobConfig) -> std::io::Result<Self> {
        std::fs::create_dir_all(&config.dir)?;
        Ok(Self { config })
    }

    fn path(&self, id: &[u8; 16]) -> PathBuf {
        self.config.dir.join(hex::encode(id))
    }

    /// Store `req.data` for `account`. Returns its new ID and expiry.
    pub async fn upload(&self, db: &Db, account: AccountId, req: proto::UploadBlob) -> Result<proto::BlobStored> {
        if req.data.is_empty() || req.data.len() > proto::MAX_SMALL_BLOB_BYTES {
            return Err(err(ErrorCode::BadRequest, format!("a blob is 1 to {} bytes", proto::MAX_SMALL_BLOB_BYTES)));
        }
        if req.renew_hash.len() != 32 {
            return Err(err(ErrorCode::BadRequest, "the renewal hash is 32 bytes"));
        }
        let mut id = [0u8; 16];
        getrandom::fill(&mut id).map_err(|_| err(ErrorCode::Internal, "no randomness"))?;
        let size = req.data.len() as u64;
        // Check the quota before writing anything; the row's insert checks
        // it again, in case two uploads raced.
        let max = self.config.max_total_bytes;
        db.run(move |c| room_for(c, &account, size, max)).await?;
        let path = self.path(&id);
        write_file(&self.config.dir, &path, &req.data).await.map_err(|e| io(&e))?;
        let expires = now_ms() + proto::SMALL_BLOB_LIFETIME_MS;
        let stored = db
            .run(move |c| {
                let tx = write_tx(c)?;
                room_for(&tx, &account, size, max)?;
                tx.execute(
                    "INSERT INTO blobs (blob_id, size, renew_hash, expires_at) VALUES (?1, ?2, ?3, ?4)",
                    params![&id[..], sql_i64(size), req.renew_hash, sql_i64(expires)],
                )
                .db()?;
                tx.execute(
                    "INSERT INTO blob_uploads (account_id, day, count) VALUES (?1, ?2, 1)
                     ON CONFLICT (account_id, day) DO UPDATE SET count = count + 1",
                    params![&account.0[..], today()],
                )
                .db()?;
                tx.commit().db()
            })
            .await;
        if let Err(e) = stored {
            let _ = tokio::fs::remove_file(&path).await;
            return Err(e);
        }
        Ok(proto::BlobStored { blob_id: id.to_vec(), expires_at_ms: expires })
    }

    pub async fn fetch(&self, db: &Db, req: proto::FetchBlob) -> Result<proto::Blob> {
        let id = blob_id(&req.blob_id)?;
        let expires = db.run(move |c| live(c, &id)).await?.0;
        match tokio::fs::read(self.path(&id)).await {
            Ok(data) => Ok(proto::Blob { data, expires_at_ms: expires }),
            // Deleted between the row's check and the read.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(err(ErrorCode::NotFound, "no such blob")),
            Err(e) => Err(io(&e)),
        }
    }

    pub async fn renew(&self, db: &Db, req: proto::RenewBlob) -> Result<proto::BlobStored> {
        let id = blob_id(&req.blob_id)?;
        db.run(move |c| {
            let tx = write_tx(c)?;
            let (_, hash) = live(&tx, &id)?;
            if !secret_matches(&req.renew_secret, &hash) {
                return Err(err(ErrorCode::Forbidden, "wrong renewal secret"));
            }
            let expires = now_ms() + proto::SMALL_BLOB_LIFETIME_MS;
            tx.execute("UPDATE blobs SET expires_at = ?1 WHERE blob_id = ?2", params![sql_i64(expires), &id[..]]).db()?;
            tx.commit().db()?;
            Ok(proto::BlobStored { blob_id: id.to_vec(), expires_at_ms: expires })
        })
        .await
    }

    pub async fn delete(&self, db: &Db, req: proto::DeleteBlob) -> Result<()> {
        let id = blob_id(&req.blob_id)?;
        db.run(move |c| {
            let tx = write_tx(c)?;
            let (_, hash) = live(&tx, &id)?;
            if !secret_matches(&req.renew_secret, &hash) {
                return Err(err(ErrorCode::Forbidden, "wrong renewal secret"));
            }
            tx.execute("DELETE FROM blobs WHERE blob_id = ?1", [&id[..]]).db()?;
            tx.commit().db()
        })
        .await?;
        match tokio::fs::remove_file(self.path(&id)).await {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(io(&e)),
        }
    }

    /// Delete expired blobs, rows first. Returns how many went.
    pub async fn collect_garbage(&self, db: &Db) -> Result<usize> {
        let expired: Vec<Vec<u8>> = db
            .run(|c| {
                let tx = write_tx(c)?;
                let now = sql_i64(now_ms());
                let ids = tx
                    .prepare("SELECT blob_id FROM blobs WHERE expires_at < ?1")
                    .and_then(|mut s| s.query_map([now], |r| r.get(0))?.collect::<rusqlite::Result<Vec<Vec<u8>>>>())
                    .db()?;
                tx.execute("DELETE FROM blobs WHERE expires_at < ?1", [now]).db()?;
                tx.execute("DELETE FROM blob_uploads WHERE day < ?1", [today()]).db()?;
                tx.commit().db()?;
                Ok(ids)
            })
            .await?;
        for id in &expired {
            if let Ok(id) = blob_id(id) {
                let _ = tokio::fs::remove_file(self.path(&id)).await;
            }
        }
        Ok(expired.len())
    }

    /// Remove files with no row: left by a crash between writing a file and
    /// recording it, or between forgetting one and removing it. Files
    /// younger than an hour may be uploads in progress, so they stay.
    pub async fn sweep(&self, db: &Db) -> Result<usize> {
        let dir = self.config.dir.clone();
        let files = tokio::task::spawn_blocking(move || stray_candidates(&dir))
            .await
            .map_err(|_| err(ErrorCode::Internal, "sweep failed"))?
            .map_err(|e| io(&e))?;
        let mut removed = 0;
        for (path, id) in files {
            let known = db
                .run(move |c| c.query_row("SELECT 1 FROM blobs WHERE blob_id = ?1", [&id[..]], |_| Ok(())).optional().db())
                .await?
                .is_some();
            if !known && tokio::fs::remove_file(&path).await.is_ok() {
                removed += 1;
            }
        }
        Ok(removed)
    }
}

/// A blob's expiry and renewal hash, if it has not expired.
fn live(c: &Connection, id: &[u8; 16]) -> Result<(u64, Vec<u8>)> {
    c.query_row(
        "SELECT expires_at, renew_hash FROM blobs WHERE blob_id = ?1 AND expires_at >= ?2",
        params![&id[..], sql_i64(now_ms())],
        |r| Ok((r.get::<_, i64>(0)? as u64, r.get(1)?)),
    )
    .optional()
    .db()?
    .ok_or_else(|| err(ErrorCode::NotFound, "no such blob"))
}

/// Whether `account` may upload `size` more bytes today, and the server has
/// room for them.
fn room_for(c: &Connection, account: &AccountId, size: u64, max_total: u64) -> Result<()> {
    let used: u32 = c
        .query_row("SELECT count FROM blob_uploads WHERE account_id = ?1 AND day = ?2", params![&account.0[..], today()], |r| r.get(0))
        .optional()
        .db()?
        .unwrap_or(0);
    if used >= proto::SMALL_BLOB_UPLOADS_PER_DAY {
        return Err(err(ErrorCode::RateLimited, format!("at most {} pictures a day", proto::SMALL_BLOB_UPLOADS_PER_DAY)));
    }
    let total: i64 = c.query_row("SELECT COALESCE(SUM(size), 0) FROM blobs", [], |r| r.get(0)).db()?;
    if (total as u64).saturating_add(size) > max_total {
        tracing::warn!("blob storage is full: raise POST_BLOB_STORE_MB to allow more");
        return Err(err(ErrorCode::Unavailable, "this server has no room for more pictures right now"));
    }
    Ok(())
}

/// Write `data` to `path` whole or not at all: to a temporary file beside
/// it, flushed, then renamed.
async fn write_file(dir: &Path, path: &Path, data: &[u8]) -> std::io::Result<()> {
    let (dir, path, data) = (dir.to_path_buf(), path.to_path_buf(), data.to_vec());
    tokio::task::spawn_blocking(move || {
        use std::io::Write as _;
        let mut tmp = tempfile::NamedTempFile::new_in(&dir)?;
        tmp.write_all(&data)?;
        tmp.as_file().sync_all()?;
        tmp.persist(&path).map_err(|e| e.error)?;
        Ok(())
    })
    .await
    .map_err(std::io::Error::other)?
}

/// Blob files older than an hour, with their IDs.
fn stray_candidates(dir: &Path) -> std::io::Result<Vec<(PathBuf, [u8; 16])>> {
    let hour_ago = std::time::SystemTime::now() - std::time::Duration::from_secs(3600);
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let meta = entry.metadata()?;
        if !meta.is_file() || meta.modified().is_ok_and(|m| m > hour_ago) {
            continue;
        }
        let name = entry.file_name();
        let Some(id) = name.to_str().and_then(|n| hex::decode(n).ok()).and_then(|b| <[u8; 16]>::try_from(b).ok()) else {
            // Temporary files from an upload cut short.
            if name.to_str().is_some_and(|n| n.starts_with(".tmp")) {
                let _ = std::fs::remove_file(entry.path());
            }
            continue;
        };
        out.push((entry.path(), id));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secrets_must_match() {
        let secret = [7u8; 32];
        let hash = Sha256::digest(secret).to_vec();
        assert!(secret_matches(&secret, &hash));
        assert!(!secret_matches(&[8u8; 32], &hash));
        assert!(!secret_matches(&secret, &hash[..31]));
    }
}
