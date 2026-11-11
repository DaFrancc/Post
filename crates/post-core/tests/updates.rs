//! App updates end to end: a server hands out a signed release, and the app
//! checks, downloads (resuming), and installs it (design section 12).

use std::path::{Path, PathBuf};

use post_core::update::Updater;
use post_identity::HybridSigningKey;
use post_release::{FileEntry, Manifest};
use zeroize::Zeroizing;

/// A release tarball: one folder holding the program.
fn tarball(version: &str) -> Vec<u8> {
    let gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    let mut b = tar::Builder::new(gz);
    for (name, bytes, mode) in [("post", &b"#!/bin/sh\necho post\n"[..], 0o755), ("lib/libapp.so", &[7u8; 5000][..], 0o644)] {
        let mut h = tar::Header::new_gnu();
        h.set_size(bytes.len() as u64);
        h.set_mode(mode);
        h.set_entry_type(tar::EntryType::Regular);
        h.set_cksum();
        b.append_data(&mut h, format!("post-{version}-linux-x86_64/{name}"), bytes).expect("append");
    }
    b.into_inner().expect("tar").finish().expect("gz")
}

/// Put a signed release where the server keeps them.
fn publish(releases: &Path, key: &HybridSigningKey, version: &str, notes: &str) -> Vec<u8> {
    let name = format!("post-{version}-linux-x86_64.tar.gz");
    let bytes = tarball(version);
    let dir = releases.join(version);
    std::fs::create_dir_all(&dir).expect("dir");
    std::fs::write(dir.join(&name), &bytes).expect("tarball");
    let m = Manifest {
        version: version.into(),
        platform: "linux-x86_64".into(),
        files: vec![FileEntry::describe(&name, &bytes)],
        min_version: "1.1.0".into(),
        notes: notes.into(),
        released_at_ms: 1,
    };
    let manifest = serde_json::to_vec_pretty(&m).expect("json");
    std::fs::write(dir.join("manifest-linux-x86_64.json.sig"), post_release::sign(key, &manifest).expect("sign")).expect("sig");
    std::fs::write(dir.join("manifest-linux-x86_64.json"), &manifest).expect("manifest");
    bytes
}

async fn server(dir: &Path, key: &HybridSigningKey) -> post_server::Running {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let domain = format!("127.0.0.1:{}", listener.local_addr().expect("addr").port());
    let releases = post_server::ReleaseConfig::new(dir.join("releases"), Some(key.verifying_key()));
    let config = post_server::Config { server_domain: domain, db_path: dir.join("server.db"), db_key: Zeroizing::new([1; 32]), releases };
    post_server::spawn(listener, config).expect("server")
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_older_app_finds_downloads_and_installs_the_release() {
    let key = HybridSigningKey::generate().expect("key");
    let dir = tempfile::tempdir().expect("tempdir");
    let tarball = publish(&dir.path().join("releases"), &key, "1.2.0", "Mentions.");
    let running = server(dir.path(), &key).await;
    let downloads: PathBuf = dir.path().join("app/updates");

    // Up to date: nothing to do.
    let same = Updater::with_key(&running.url(), "1.2.0", downloads.clone(), key.verifying_key()).expect("updater");
    assert!(same.check().await.expect("check").is_none());

    // Older, and below the release's min_version.
    let updater = Updater::with_key(&running.url(), "1.0.0", downloads.clone(), key.verifying_key()).expect("updater");
    let found = updater.check().await.expect("check").expect("an update");
    assert_eq!((found.manifest.version.as_str(), found.manifest.notes.as_str()), ("1.2.0", "Mentions."));
    assert!(found.required, "1.0.0 is below min_version 1.1.0");
    assert_eq!(found.size, tarball.len() as u64);

    // A download that stopped part way resumes from where it was.
    let staged = updater.staged(&found);
    std::fs::create_dir_all(&staged).expect("dir");
    std::fs::write(staged.join("post-1.2.0-linux-x86_64.tar.gz.part"), &tarball[..100]).expect("part");
    let mut seen = Vec::new();
    updater.download(&found, |done, total| seen.push((done, total))).await.expect("download");
    assert_eq!(std::fs::read(staged.join("post-1.2.0-linux-x86_64.tar.gz")).expect("file"), tarball);
    assert_eq!(seen.last(), Some(&(tarball.len() as u64, tarball.len() as u64)));
    assert!(seen.first().is_some_and(|(done, _)| *done > 100), "picked up after the 100 bytes already there");

    // Installed beside nothing: the first version under this root.
    let root = dir.path().join("install");
    let program = updater.install(&found, &root).expect("install");
    assert_eq!(program, root.join("current/post"));
    assert!(program.is_file());
    assert_eq!(std::fs::read_link(root.join("current")).expect("link"), Path::new("versions/1.2.0"));
    assert!(!staged.exists(), "the download is cleared away once installed");
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_release_changed_after_it_was_seen_is_refused() {
    let key = HybridSigningKey::generate().expect("key");
    let dir = tempfile::tempdir().expect("tempdir");
    let downloads = dir.path().join("app/updates");
    publish(&dir.path().join("releases"), &key, "1.2.0", "Mentions.");
    let running = server(dir.path(), &key).await;
    let updater = Updater::with_key(&running.url(), "1.0.0", downloads.clone(), key.verifying_key()).expect("updater");
    assert!(updater.check().await.expect("check").is_some());
    drop(running);

    // The same version, signed again with other contents: even signed, a
    // published release never changes, so this is refused.
    publish(&dir.path().join("releases"), &key, "1.2.0", "Something else.");
    let running = server(dir.path(), &key).await;
    let updater = Updater::with_key(&running.url(), "1.0.0", downloads, key.verifying_key()).expect("updater");
    let e = updater.check().await.expect_err("refused");
    assert!(e.to_string().contains("different contents"), "{e}");

    // And one signed by another key never gets past the check.
    let other = HybridSigningKey::generate().expect("key");
    let fresh = Updater::with_key(&running.url(), "1.0.0", dir.path().join("fresh"), other.verifying_key()).expect("updater");
    let e = fresh.check().await.expect_err("refused");
    assert!(e.to_string().contains("not genuine"), "{e}");
}
