//! The server fetches signed releases from GitHub, checks them, and hands
//! them out with resumable downloads (design section 12, "App updates").

use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::routing::get;
use base64::Engine as _;
use post_identity::HybridSigningKey;
use post_release::{FileEntry, Manifest, platform};
use zeroize::Zeroizing;

const TARBALL: &[u8] = b"pretend this is post-1.2.0-linux-x86_64.tar.gz";

/// A stand-in for GitHub: the latest release's JSON, and its assets.
async fn fake_github(assets: Vec<(&'static str, Vec<u8>)>) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let base = format!("http://{}", listener.local_addr().expect("addr"));
    let listing = serde_json::json!({
        "tag_name": "v1.2.0",
        "assets": assets.iter().map(|(name, _)| serde_json::json!({ "name": name, "browser_download_url": format!("{base}/download/{name}") })).collect::<Vec<_>>(),
    });
    let assets = Arc::new(assets);
    let app = Router::new()
        .route("/repos/dafrancc/post/releases/latest", get(move || async move { axum::Json(listing) }))
        .route(
            "/download/{name}",
            get(move |axum::extract::Path(name): axum::extract::Path<String>| async move {
                assets.iter().find(|(n, _)| *n == name).map(|(_, b)| b.clone()).ok_or(axum::http::StatusCode::NOT_FOUND)
            }),
        );
    tokio::spawn(async move { axum::serve(listener, app).await });
    base
}

fn release(key: &HybridSigningKey) -> (Vec<u8>, Vec<u8>) {
    let m = Manifest {
        version: "1.2.0".into(),
        platform: platform::LINUX_X86_64.into(),
        files: vec![FileEntry::describe("post-1.2.0-linux-x86_64.tar.gz", TARBALL)],
        min_version: "1.0.0".into(),
        notes: "Mentions.".into(),
        released_at_ms: 1,
    };
    let bytes = serde_json::to_vec_pretty(&m).expect("json");
    let sig = post_release::sign(key, &bytes).expect("sign");
    (bytes, sig)
}

async fn postd(dir: &std::path::Path, releases: post_server::ReleaseConfig) -> post_server::Running {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let domain = format!("127.0.0.1:{}", listener.local_addr().expect("addr").port());
    let config = post_server::Config { server_domain: domain, db_path: dir.join("server.db"), db_key: Zeroizing::new([5; 32]), releases };
    post_server::spawn(listener, config).expect("server")
}

async fn latest(http: &reqwest::Client, server: &post_server::Running) -> Option<serde_json::Value> {
    let res = http.get(format!("{}/v1/releases/linux-x86_64", server.http_url())).send().await.expect("request");
    if res.status() == reqwest::StatusCode::NOT_FOUND {
        return None;
    }
    Some(serde_json::from_slice(&res.bytes().await.expect("body")).expect("json"))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn releases_are_fetched_checked_and_handed_out() {
    let key = HybridSigningKey::generate().expect("key");
    let (manifest, sig) = release(&key);
    let github = fake_github(vec![
        ("manifest-linux-x86_64.json", manifest.clone()),
        ("manifest-linux-x86_64.json.sig", sig.clone()),
        ("post-1.2.0-linux-x86_64.tar.gz", TARBALL.to_vec()),
    ])
    .await;
    let dir = tempfile::tempdir().expect("tempdir");
    let releases_dir = dir.path().join("releases");
    let config = post_server::ReleaseConfig {
        dir: releases_dir.clone(),
        github_repo: Some("dafrancc/post".into()),
        github_api: github,
        key: Some(key.verifying_key()),
        every: Duration::from_secs(3600),
    };
    let server = postd(dir.path(), config.clone()).await;
    let http = reqwest::Client::new();

    // Fetched in the background on start.
    let mut found = None;
    for _ in 0..100 {
        found = latest(&http, &server).await;
        if found.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let found = found.expect("the release is handed out");
    let b64 = base64::engine::general_purpose::STANDARD;
    let got_manifest = b64.decode(found["manifest"].as_str().expect("manifest")).expect("b64");
    let got_sig = b64.decode(found["signature"].as_str().expect("signature")).expect("b64");
    assert_eq!((got_manifest.as_slice(), got_sig.as_slice()), (manifest.as_slice(), sig.as_slice()), "exactly as signed");

    // The file, whole, and from part way (a resumed download).
    let url = format!("{}/v1/releases/linux-x86_64/1.2.0/post-1.2.0-linux-x86_64.tar.gz", server.http_url());
    assert_eq!(http.get(&url).send().await.expect("get").bytes().await.expect("bytes").as_ref(), TARBALL);
    let tail = http.get(&url).header("Range", "bytes=10-").send().await.expect("get");
    assert_eq!(tail.status(), reqwest::StatusCode::PARTIAL_CONTENT);
    assert_eq!(tail.bytes().await.expect("bytes").as_ref(), &TARBALL[10..]);
    // Nothing else is served from the directory.
    for other in ["manifest-linux-x86_64.json", "../server.db", "nothing"] {
        let url = format!("{}/v1/releases/linux-x86_64/1.2.0/{other}", server.http_url());
        assert_ne!(http.get(&url).send().await.expect("get").status(), reqwest::StatusCode::OK, "{other}");
    }
    drop(server);

    // Kept: a restart serves it from the directory, without GitHub.
    let offline = post_server::ReleaseConfig { github_repo: None, ..config.clone() };
    let server = postd(dir.path(), offline).await;
    assert!(latest(&http, &server).await.is_some(), "served from the directory");
    drop(server);

    // A file changed on disk: refused at the next start.
    std::fs::write(releases_dir.join("1.2.0/post-1.2.0-linux-x86_64.tar.gz"), b"tampered").expect("write");
    let server = postd(dir.path(), post_server::ReleaseConfig { github_repo: None, ..config }).await;
    assert!(latest(&http, &server).await.is_none(), "a changed file is not handed out");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_refresh_picks_up_a_release_copied_in_while_running() {
    let key = HybridSigningKey::generate().expect("key");
    let dir = tempfile::tempdir().expect("tempdir");
    let releases = dir.path().join("releases");
    let config = post_server::ReleaseConfig {
        dir: releases.clone(),
        github_repo: None,
        github_api: String::new(),
        key: Some(key.verifying_key()),
        every: Duration::from_secs(3600),
    };
    let server = postd(dir.path(), config).await;
    let http = reqwest::Client::new();
    assert!(latest(&http, &server).await.is_none());

    let (manifest, sig) = release(&key);
    let v = releases.join("1.2.0");
    std::fs::create_dir_all(&v).expect("dir");
    std::fs::write(v.join("post-1.2.0-linux-x86_64.tar.gz"), TARBALL).expect("tarball");
    std::fs::write(v.join("manifest-linux-x86_64.json.sig"), sig).expect("sig");
    std::fs::write(v.join("manifest-linux-x86_64.json"), manifest).expect("manifest");
    server.refresh_releases();
    let mut found = false;
    for _ in 0..100 {
        if latest(&http, &server).await.is_some() {
            found = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(30)).await;
    }
    assert!(found, "handed out after the refresh, not at the next hour");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_release_not_signed_by_the_release_key_is_refused() {
    let key = HybridSigningKey::generate().expect("key");
    let impostor = HybridSigningKey::generate().expect("key");
    let (manifest, sig) = release(&impostor);
    let github = fake_github(vec![
        ("manifest-linux-x86_64.json", manifest),
        ("manifest-linux-x86_64.json.sig", sig),
        ("post-1.2.0-linux-x86_64.tar.gz", TARBALL.to_vec()),
    ])
    .await;
    let dir = tempfile::tempdir().expect("tempdir");
    let config = post_server::ReleaseConfig {
        dir: dir.path().join("releases"),
        github_repo: Some("dafrancc/post".into()),
        github_api: github,
        key: Some(key.verifying_key()),
        every: Duration::from_secs(3600),
    };
    let server = postd(dir.path(), config).await;
    tokio::time::sleep(Duration::from_millis(800)).await;
    assert!(latest(&reqwest::Client::new(), &server).await.is_none());
    assert!(!dir.path().join("releases/1.2.0/post-1.2.0-linux-x86_64.tar.gz").exists(), "nothing downloaded");
}
