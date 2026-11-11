//! App updates on the device (design section 12, "App updates").
//!
//! The app asks its own server for the newest release, never a third party.
//! Nothing the server says is trusted: the manifest must verify under the
//! release key built into this app, the version must be newer than this
//! one, a version once seen must never come back with other contents, and
//! every downloaded file must match the manifest before it is used.
//!
//! On Linux, Post installs per user as a tarball:
//!
//! ```text
//! ~/.local/share/post/
//!     versions/1.2.0/post        each version unpacked whole
//!     versions/1.3.0/post
//!     current -> versions/1.3.0  what the desktop entry starts
//! ```
//!
//! Installing unpacks the new version beside the old and then swaps the
//! `current` link in one rename, so a failure at any point leaves the app as
//! it was, and the previous version stays for going back.

use std::collections::BTreeMap;
use std::io::Read as _;
use std::path::{Component, Path, PathBuf};

use base64::Engine as _;
use futures_util::StreamExt as _;
use post_identity::HybridVerifyingKey;
use post_release::{Manifest, Version};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt as _;

use crate::{Error, Result};

fn fail(s: impl Into<String>) -> Error {
    Error::Update(s.into())
}

/// A newer release, checked, ready to download.
#[derive(Clone, Debug)]
pub struct Available {
    /// What it says about itself; checked against the release key.
    pub manifest: Manifest,
    /// The total size of its files.
    pub size: u64,
    /// This app is older than the release's `min_version`: it can no longer
    /// talk to servers running the new protocol.
    pub required: bool,
}

/// Checks for, fetches and installs releases from one server.
pub struct Updater {
    base: String,
    platform: &'static str,
    current: Version,
    dir: PathBuf,
    key: HybridVerifyingKey,
    http: reqwest::Client,
}

impl Updater {
    /// An updater for the app at version `current`, asking the server at
    /// `server_url` (the `wss://` address it connects to), keeping
    /// downloads in `dir`.
    pub fn new(server_url: &str, current: &str, dir: PathBuf) -> Result<Self> {
        let key = post_release::release_key().map_err(|_| fail("This build of Post cannot check for updates (it has no release key)."))?;
        Self::with_key(server_url, current, dir, key)
    }

    /// As [`Self::new`], trusting `key` (tests).
    pub fn with_key(server_url: &str, current: &str, dir: PathBuf, key: HybridVerifyingKey) -> Result<Self> {
        let platform = post_release::platform::current().ok_or_else(|| fail("Post has no releases for this kind of computer yet."))?;
        let current = current.parse().map_err(|_| fail(format!("This app's version ({current}) is not one releases can be compared with.")))?;
        let http = reqwest::Client::builder()
            .user_agent("Post")
            .connect_timeout(std::time::Duration::from_secs(20))
            .build()
            .map_err(|e| fail(format!("No connection for updates: {e}.")))?;
        Ok(Self { base: http_base(server_url)?, platform, current, dir, key, http })
    }

    /// The newest release, if it is newer than this app.
    pub async fn check(&self) -> Result<Option<Available>> {
        #[derive(Deserialize)]
        struct Latest {
            manifest: String,
            signature: String,
        }
        let url = format!("{}/v1/releases/{}", self.base, self.platform);
        let res = self.http.get(&url).send().await.map_err(|e| Error::Network(e.to_string()))?;
        if res.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        let res = res.error_for_status().map_err(|e| Error::Network(e.to_string()))?;
        let body = res.bytes().await.map_err(|e| Error::Network(e.to_string()))?;
        if body.len() > 4 * post_release::MAX_MANIFEST {
            return Err(fail("The server's answer about updates was too large."));
        }
        let latest: Latest = serde_json::from_slice(&body).map_err(|_| fail("The server's answer about updates did not make sense."))?;
        let b64 = base64::engine::general_purpose::STANDARD;
        let (manifest_bytes, signature) = match (b64.decode(&latest.manifest), b64.decode(&latest.signature)) {
            (Ok(m), Ok(s)) => (m, s),
            _ => return Err(fail("The server's answer about updates did not make sense.")),
        };
        let manifest = post_release::verify_with(&self.key, &manifest_bytes, &signature)
            .map_err(|e| fail(format!("The update the server offered is not genuine ({e}); it was ignored.")))?;
        if manifest.platform != self.platform {
            return Err(fail("The server offered an update for another kind of computer."));
        }
        let version = manifest.version().map_err(|_| fail("The update's version is unreadable."))?;
        self.remember(&manifest.version, &manifest_bytes)?;
        if version <= self.current {
            return Ok(None);
        }
        let required = manifest.min_version().is_ok_and(|min| self.current < min);
        Ok(Some(Available { size: manifest.files.iter().map(|f| f.size).sum(), required, manifest }))
    }

    /// Refuse a version seen before with other contents: a release is never
    /// changed once published, so that would mean someone is tampering.
    fn remember(&self, version: &str, manifest: &[u8]) -> Result<()> {
        let path = self.dir.join("seen.json");
        let mut seen: BTreeMap<String, String> = std::fs::read(&path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        let digest = hex::encode(Sha256::digest(manifest));
        match seen.get(version) {
            Some(d) if *d == digest => Ok(()),
            Some(_) => Err(fail(format!("Version {version} came back with different contents than before; it will not be installed."))),
            None => {
                seen.insert(version.into(), digest);
                std::fs::create_dir_all(&self.dir).map_err(|e| fail(format!("Could not keep track of updates: {e}.")))?;
                std::fs::write(&path, serde_json::to_vec(&seen).unwrap_or_default()).map_err(|e| fail(format!("Could not keep track of updates: {e}.")))
            }
        }
    }

    /// Where a release's files are downloaded to.
    pub fn staged(&self, a: &Available) -> PathBuf {
        self.dir.join(&a.manifest.version)
    }

    /// Download a release's files, resuming any left part way, each checked
    /// against the manifest. `progress` hears bytes done and bytes in all.
    pub async fn download(&self, a: &Available, mut progress: impl FnMut(u64, u64) + Send) -> Result<()> {
        let dir = self.staged(a);
        tokio::fs::create_dir_all(&dir).await.map_err(|e| fail(format!("Could not save the update: {e}.")))?;
        let mut done = 0u64;
        for f in &a.manifest.files {
            let target = dir.join(&f.name);
            if hash_file(&target).is_ok_and(|(size, h)| f.check(size, &h).is_ok()) {
                done += f.size;
                progress(done, a.size);
                continue;
            }
            let part = dir.join(format!("{}.part", f.name));
            // Pick up where a stopped download left off: hash what is there,
            // then ask for the rest.
            let (have, mut hash) = match std::fs::File::open(&part) {
                Ok(mut file) => {
                    let mut h = Sha256::new();
                    let mut buf = vec![0u8; 1 << 16];
                    let mut n_total = 0u64;
                    loop {
                        let n = file.read(&mut buf).map_err(|e| fail(format!("Could not read the update: {e}.")))?;
                        if n == 0 {
                            break;
                        }
                        n_total += n as u64;
                        h.update(&buf[..n]);
                    }
                    (n_total, h)
                }
                Err(_) => (0, Sha256::new()),
            };
            let url = format!("{}/v1/releases/{}/{}/{}", self.base, self.platform, a.manifest.version, f.name);
            let mut req = self.http.get(&url);
            if have > 0 && have < f.size {
                req = req.header("Range", format!("bytes={have}-"));
            }
            let res = req.send().await.and_then(|r| r.error_for_status()).map_err(|e| Error::Network(e.to_string()))?;
            let resumed = res.status() == reqwest::StatusCode::PARTIAL_CONTENT;
            let mut size = if resumed { have } else { 0 };
            if !resumed {
                hash = Sha256::new();
            }
            let mut out = tokio::fs::OpenOptions::new()
                .create(true)
                .write(true)
                .append(resumed)
                .truncate(!resumed)
                .open(&part)
                .await
                .map_err(|e| fail(format!("Could not save the update: {e}.")))?;
            let mut body = res.bytes_stream();
            while let Some(chunk) = body.next().await {
                let chunk = chunk.map_err(|e| Error::Network(e.to_string()))?;
                size += chunk.len() as u64;
                if size > f.size {
                    drop(out);
                    let _ = std::fs::remove_file(&part);
                    return Err(fail("The update the server sent is larger than the release says; it was thrown away."));
                }
                hash.update(&chunk);
                out.write_all(&chunk).await.map_err(|e| fail(format!("Could not save the update: {e}.")))?;
                progress(done + size, a.size);
            }
            out.sync_all().await.map_err(|e| fail(format!("Could not save the update: {e}.")))?;
            drop(out);
            if f.check(size, &hash.finalize().into()).is_err() {
                let _ = std::fs::remove_file(&part);
                return Err(fail("The update the server sent does not match the release; it was thrown away."));
            }
            std::fs::rename(&part, &target).map_err(|e| fail(format!("Could not save the update: {e}.")))?;
            done += f.size;
        }
        Ok(())
    }

    /// Install a downloaded release under `root` (see [`install_root`]),
    /// checking its files once more first. Returns the new version's
    /// program, to start in place of this one.
    pub fn install(&self, a: &Available, root: &Path) -> Result<PathBuf> {
        let dir = self.staged(a);
        for f in &a.manifest.files {
            let ok = hash_file(&dir.join(&f.name)).is_ok_and(|(size, h)| f.check(size, &h).is_ok());
            if !ok {
                return Err(fail("The downloaded update does not match the release; download it again."));
            }
        }
        let tarball = match a.manifest.files.as_slice() {
            [f] if f.name.ends_with(".tar.gz") => dir.join(&f.name),
            _ => return Err(fail("This release is not a tarball this app knows how to install.")),
        };
        let program = install_tarball(&tarball, root, &a.manifest.version)?;
        // The download is not needed once installed.
        let _ = std::fs::remove_dir_all(&dir);
        Ok(program)
    }
}

/// The plain HTTP(S) address of a server, from the WebSocket address the app
/// connects to: `wss://host/v1/ws` is served at `https://host`.
pub fn http_base(server_url: &str) -> Result<String> {
    let url = url::Url::parse(server_url).map_err(|_| fail("The server address is unreadable."))?;
    let scheme = match url.scheme() {
        "wss" => "https",
        "ws" => "http",
        _ => return Err(fail("The server address is unreadable.")),
    };
    let host = url.host_str().ok_or_else(|| fail("The server address is unreadable."))?;
    Ok(match url.port() {
        Some(p) => format!("{scheme}://{host}:{p}"),
        None => format!("{scheme}://{host}"),
    })
}

/// Where this running app is installed, if it is installed the way updates
/// can replace: run from `<root>/versions/<version>/`, with a `current`
/// link beside `versions`. A build run from anywhere else (development, a
/// distribution package) is not replaced by the app.
pub fn install_root() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?.canonicalize().ok()?;
    let versions = exe.parent()?.parent()?;
    let root = versions.parent()?;
    let linked = std::fs::symlink_metadata(root.join("current")).is_ok_and(|m| m.file_type().is_symlink());
    (versions.file_name()? == "versions" && linked).then(|| root.to_path_buf())
}

/// The program's name inside an installed version.
const PROGRAM: &str = "post";

/// Unpack `tarball` (one top-level folder holding the app) into
/// `<root>/versions/<version>`, then point `current` at it, then delete all
/// but it and the version it replaced.
pub fn install_tarball(tarball: &Path, root: &Path, version: &str) -> Result<PathBuf> {
    version.parse::<Version>().map_err(|_| fail("The update's version is unreadable."))?;
    let versions = root.join("versions");
    let unpacking = versions.join(format!(".{version}.unpacking"));
    let installed = versions.join(version);
    let _ = std::fs::remove_dir_all(&unpacking);
    std::fs::create_dir_all(&unpacking).map_err(|e| fail(format!("Could not install the update: {e}.")))?;
    if let Err(e) = unpack(tarball, &unpacking) {
        let _ = std::fs::remove_dir_all(&unpacking);
        return Err(e);
    }
    if !unpacking.join(PROGRAM).is_file() {
        let _ = std::fs::remove_dir_all(&unpacking);
        return Err(fail("The update holds no Post program; it was not installed."));
    }
    let _ = std::fs::remove_dir_all(&installed);
    std::fs::rename(&unpacking, &installed).map_err(|e| fail(format!("Could not install the update: {e}.")))?;

    // Swap the link in one step: a new link beside it, renamed over it.
    let current = root.join("current");
    let previous = std::fs::read_link(&current).ok();
    let next = root.join(".current.next");
    let _ = std::fs::remove_file(&next);
    symlink(Path::new("versions").join(version), &next)?;
    std::fs::rename(&next, &current).map_err(|e| fail(format!("Could not switch to the update: {e}.")))?;

    // Keep the new version and the one it replaced, for going back.
    let keep: Vec<PathBuf> = [Some(Path::new("versions").join(version)), previous].into_iter().flatten().map(|p| root.join(p)).collect();
    if let Ok(entries) = std::fs::read_dir(&versions) {
        for e in entries.flatten() {
            let path = e.path();
            if !keep.contains(&path) {
                let _ = std::fs::remove_dir_all(&path);
            }
        }
    }
    Ok(current.join(PROGRAM))
}

#[cfg(unix)]
fn symlink(target: impl AsRef<Path>, link: &Path) -> Result<()> {
    std::os::unix::fs::symlink(target, link).map_err(|e| fail(format!("Could not switch to the update: {e}.")))
}

#[cfg(not(unix))]
fn symlink(_target: impl AsRef<Path>, _link: &Path) -> Result<()> {
    Err(fail("Installing updates is not supported on this system yet."))
}

/// Unpack a `.tar.gz` into `into`, dropping its one top-level folder. Only
/// plain files and folders, with plain relative names: nothing that could
/// point outside `into`.
fn unpack(tarball: &Path, into: &Path) -> Result<()> {
    let bad = || fail("The update's archive is damaged; it was not installed.");
    let file = std::fs::File::open(tarball).map_err(|e| fail(format!("Could not read the update: {e}.")))?;
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(file));
    for entry in archive.entries().map_err(|_| bad())? {
        let mut entry = entry.map_err(|_| bad())?;
        let path = entry.path().map_err(|_| bad())?.into_owned();
        let mut parts = path.components();
        match parts.next() {
            Some(Component::Normal(_)) => {}
            _ => return Err(bad()),
        }
        let rest: PathBuf = parts.as_path().to_path_buf();
        if rest.components().any(|c| !matches!(c, Component::Normal(_))) {
            return Err(bad());
        }
        if rest.as_os_str().is_empty() {
            continue;
        }
        let dest = into.join(&rest);
        match entry.header().entry_type() {
            tar::EntryType::Directory => std::fs::create_dir_all(&dest).map_err(|e| fail(format!("Could not install the update: {e}.")))?,
            tar::EntryType::Regular => {
                if let Some(parent) = dest.parent() {
                    std::fs::create_dir_all(parent).map_err(|e| fail(format!("Could not install the update: {e}.")))?;
                }
                entry.unpack(&dest).map_err(|e| fail(format!("Could not install the update: {e}.")))?;
            }
            _ => return Err(bad()),
        }
    }
    Ok(())
}

fn hash_file(path: &Path) -> std::io::Result<(u64, [u8; 32])> {
    let mut file = std::fs::File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 16];
    let mut size = 0u64;
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        size += n as u64;
        h.update(&buf[..n]);
    }
    Ok((size, h.finalize().into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tarball of `files` (path, contents, executable) under one folder.
    fn tarball(dir: &Path, top: &str, files: &[(&str, &[u8], bool)]) -> PathBuf {
        let path = dir.join(format!("{top}.tar.gz"));
        let gz = flate2::write::GzEncoder::new(std::fs::File::create(&path).expect("create"), flate2::Compression::fast());
        let mut b = tar::Builder::new(gz);
        for (name, bytes, exe) in files {
            let mut h = tar::Header::new_gnu();
            h.set_size(bytes.len() as u64);
            h.set_mode(if *exe { 0o755 } else { 0o644 });
            h.set_entry_type(tar::EntryType::Regular);
            h.set_cksum();
            b.append_data(&mut h, format!("{top}/{name}"), *bytes).expect("append");
        }
        b.into_inner().expect("tar").finish().expect("gz");
        path
    }

    #[test]
    fn server_addresses_become_http_ones() {
        assert_eq!(http_base("wss://chat.example.org/v1/ws").expect("url"), "https://chat.example.org");
        assert_eq!(http_base("ws://127.0.0.1:8080/v1/ws").expect("url"), "http://127.0.0.1:8080");
        assert!(http_base("https://x").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn installing_swaps_versions_and_keeps_the_last_one() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path().join("post");
        let files: &[(&str, &[u8], bool)] = &[("post", b"#!/bin/sh\n", true), ("lib/libapp.so", b"code", false)];
        for v in ["1.0.0", "1.1.0", "1.2.0"] {
            let t = tarball(dir.path(), &format!("post-{v}-linux-x86_64"), files);
            let program = install_tarball(&t, &root, v).expect("install");
            assert_eq!(program, root.join("current/post"));
        }
        assert_eq!(std::fs::read_link(root.join("current")).expect("link"), Path::new("versions/1.2.0"));
        let mode = std::fs::metadata(root.join("current/post")).expect("program").permissions().mode();
        assert!(mode & 0o111 != 0, "the program stays executable");
        assert_eq!(std::fs::read(root.join("current/lib/libapp.so")).expect("lib"), b"code");
        let mut kept: Vec<String> = std::fs::read_dir(root.join("versions")).expect("dir").flatten().map(|e| e.file_name().to_string_lossy().into()).collect();
        kept.sort();
        assert_eq!(kept, ["1.1.0", "1.2.0"], "the new version and the one before it");
    }

    #[test]
    fn archives_that_reach_outside_are_refused() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path().join("post");
        // No program in it.
        let t = tarball(dir.path(), "post-1.0.0", &[("readme", b"hi", false)]);
        assert!(install_tarball(&t, &root, "1.0.0").is_err());
        assert!(!root.join("current").exists());
        // A path climbing out of the folder (written raw: the tar crate's
        // own builder refuses to make one).
        let path = dir.path().join("evil.tar.gz");
        let gz = flate2::write::GzEncoder::new(std::fs::File::create(&path).expect("create"), flate2::Compression::fast());
        let mut b = tar::Builder::new(gz);
        let mut h = tar::Header::new_gnu();
        h.as_gnu_mut().expect("gnu").name[..17].copy_from_slice(b"top/../../escaped");
        h.set_size(1);
        h.set_mode(0o644);
        h.set_entry_type(tar::EntryType::Regular);
        h.set_cksum();
        b.append(&h, &b"x"[..]).expect("append");
        b.into_inner().expect("tar").finish().expect("gz");
        assert!(install_tarball(&path, &root, "1.0.1").is_err());
        assert!(!dir.path().join("escaped").exists());
        assert!(install_tarball(&path, &root, "../1.0.1").is_err(), "a version is a version, not a path");
    }
}
