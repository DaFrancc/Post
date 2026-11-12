//! The server fetches signed releases from GitHub, checks them, and hands
//! them out with resumable downloads, going as far by itself as its settings
//! allow (design section 12, "App updates").

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::routing::get;
use base64::Engine as _;
use post_identity::HybridSigningKey;
use post_release::{FileEntry, Manifest, platform};
use post_server::ReleaseConfig;
use zeroize::Zeroizing;

fn tarball(version: &str) -> Vec<u8> {
    format!("pretend this is post-{version}-linux-x86_64.tar.gz").into_bytes()
}

fn tarball_name(version: &str) -> String {
    format!("post-{version}-linux-x86_64.tar.gz")
}

/// A signed release: its manifest and signature.
fn release(key: &HybridSigningKey, version: &str) -> (Vec<u8>, Vec<u8>) {
    let m = Manifest {
        version: version.into(),
        platform: platform::LINUX_X86_64.into(),
        files: vec![FileEntry::describe(&tarball_name(version), &tarball(version))],
        min_version: "0.0.0".into(),
        notes: "Mentions.".into(),
        released_at_ms: 1,
    };
    let bytes = serde_json::to_vec_pretty(&m).expect("json");
    let sig = post_release::sign(key, &bytes).expect("sign");
    (bytes, sig)
}

/// A release copied into the directory by hand.
fn copy_in(releases: &Path, key: &HybridSigningKey, version: &str) {
    let (manifest, sig) = release(key, version);
    let dir = releases.join(version);
    std::fs::create_dir_all(&dir).expect("dir");
    std::fs::write(dir.join(tarball_name(version)), tarball(version)).expect("tarball");
    std::fs::write(dir.join("manifest-linux-x86_64.json.sig"), sig).expect("sig");
    std::fs::write(dir.join("manifest-linux-x86_64.json"), manifest).expect("manifest");
}

/// A stand-in for GitHub whose latest release is `version`, signed by `key`.
async fn fake_github(key: &HybridSigningKey, version: &str) -> String {
    let (manifest, sig) = release(key, version);
    let assets: Vec<(String, Vec<u8>)> = vec![
        ("manifest-linux-x86_64.json".into(), manifest),
        ("manifest-linux-x86_64.json.sig".into(), sig),
        (tarball_name(version), tarball(version)),
    ];
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let base = format!("http://{}", listener.local_addr().expect("addr"));
    let listing = serde_json::json!({
        "tag_name": format!("v{version}"),
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

fn config(dir: &Path, key: &HybridSigningKey, github: Option<String>) -> ReleaseConfig {
    ReleaseConfig {
        github_repo: github.is_some().then(|| "dafrancc/post".into()),
        github_api: github.unwrap_or_default(),
        ..ReleaseConfig::new(dir.join("releases"), Some(key.verifying_key()))
    }
}

async fn postd(dir: &Path, releases: ReleaseConfig) -> post_server::Running {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let domain = format!("127.0.0.1:{}", listener.local_addr().expect("addr").port());
    let config = post_server::Config { server_domain: domain, db_path: dir.join("server.db"), db_key: Zeroizing::new([5; 32]), releases, calls: None };
    post_server::spawn(listener, config).expect("server")
}

/// The version live for apps, if any.
async fn live(server: &post_server::Running) -> Option<String> {
    let res = reqwest::get(format!("{}/v1/releases/linux-x86_64", server.http_url())).await.expect("request");
    if res.status() == reqwest::StatusCode::NOT_FOUND {
        return None;
    }
    let v: serde_json::Value = serde_json::from_slice(&res.bytes().await.expect("body")).expect("json");
    Some(v["version"].as_str().expect("version").to_owned())
}

/// Wait (briefly) for `cond`, which background work makes true.
async fn eventually(mut cond: impl AsyncFnMut() -> bool) -> bool {
    for _ in 0..100 {
        if cond().await {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(30)).await;
    }
    false
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn by_default_a_new_release_downloads_but_goes_live_at_the_next_start() {
    let key = HybridSigningKey::generate().expect("key");
    let dir = tempfile::tempdir().expect("tempdir");
    let github = fake_github(&key, "1.2.0").await;
    let server = postd(dir.path(), config(dir.path(), &key, Some(github.clone()))).await;

    let manifest = dir.path().join("releases/1.2.0/manifest-linux-x86_64.json");
    assert!(eventually(async || manifest.exists()).await, "checked and downloaded by itself");
    assert_eq!(live(&server).await, None, "but not offered to apps while running");
    drop(server);

    let server = postd(dir.path(), config(dir.path(), &key, Some(github))).await;
    assert_eq!(live(&server).await.as_deref(), Some("1.2.0"), "live from the next start");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn live_while_running_offers_it_at_once_with_resumable_downloads() {
    let key = HybridSigningKey::generate().expect("key");
    let dir = tempfile::tempdir().expect("tempdir");
    let github = fake_github(&key, "1.2.0").await;
    let settings = ReleaseConfig { live_while_running: true, ..config(dir.path(), &key, Some(github)) };
    let server = postd(dir.path(), settings.clone()).await;
    assert!(eventually(async || live(&server).await.is_some()).await, "live once downloaded");

    // Exactly as signed.
    let (manifest, _) = release(&key, "1.2.0");
    let got: serde_json::Value =
        serde_json::from_slice(&reqwest::get(format!("{}/v1/releases/linux-x86_64", server.http_url())).await.expect("get").bytes().await.expect("body"))
            .expect("json");
    let b64 = base64::engine::general_purpose::STANDARD;
    assert_eq!(b64.decode(got["manifest"].as_str().expect("manifest")).expect("b64"), manifest);
    let sig = b64.decode(got["signature"].as_str().expect("signature")).expect("b64");
    post_release::verify_with(&key.verifying_key(), &manifest, &sig).expect("the signature verifies");

    // The file, whole, and from part way (a resumed download).
    let http = reqwest::Client::new();
    let url = format!("{}/v1/releases/linux-x86_64/1.2.0/{}", server.http_url(), tarball_name("1.2.0"));
    assert_eq!(http.get(&url).send().await.expect("get").bytes().await.expect("bytes"), tarball("1.2.0"));
    let tail = http.get(&url).header("Range", "bytes=10-").send().await.expect("get");
    assert_eq!(tail.status(), reqwest::StatusCode::PARTIAL_CONTENT);
    assert_eq!(tail.bytes().await.expect("bytes"), tarball("1.2.0")[10..]);
    // Nothing else is served from the directory.
    for other in ["manifest-linux-x86_64.json", "../server.db", "live-linux-x86_64", "nothing"] {
        let url = format!("{}/v1/releases/linux-x86_64/1.2.0/{other}", server.http_url());
        assert_ne!(http.get(&url).send().await.expect("get").status(), reqwest::StatusCode::OK, "{other}");
    }
    drop(server);

    // A file changed on disk: refused at the next start.
    std::fs::write(dir.path().join("releases/1.2.0").join(tarball_name("1.2.0")), b"tampered").expect("write");
    let server = postd(dir.path(), ReleaseConfig { github_repo: None, ..settings }).await;
    assert_eq!(live(&server).await, None, "a changed file is not handed out");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_reload_puts_a_release_copied_in_live_now() {
    let key = HybridSigningKey::generate().expect("key");
    let dir = tempfile::tempdir().expect("tempdir");
    let server = postd(dir.path(), config(dir.path(), &key, None)).await;
    assert_eq!(live(&server).await, None);

    copy_in(&dir.path().join("releases"), &key, "1.2.0");
    server.refresh_releases();
    assert!(eventually(async || live(&server).await.is_some()).await, "live after the reload, not at the next start");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn without_live_on_start_the_release_live_before_stays() {
    let key = HybridSigningKey::generate().expect("key");
    let dir = tempfile::tempdir().expect("tempdir");
    let releases = dir.path().join("releases");
    copy_in(&releases, &key, "1.2.0");
    let server = postd(dir.path(), config(dir.path(), &key, None)).await;
    assert_eq!(live(&server).await.as_deref(), Some("1.2.0"));
    drop(server);

    // A newer one arrives; the server restarts with live_on_start off.
    copy_in(&releases, &key, "1.3.0");
    let settings = ReleaseConfig { live_on_start: false, ..config(dir.path(), &key, None) };
    let server = postd(dir.path(), settings).await;
    assert_eq!(live(&server).await.as_deref(), Some("1.2.0"), "the release live before the restart");
    server.refresh_releases();
    assert!(eventually(async || live(&server).await.as_deref() == Some("1.3.0")).await, "the newest, once asked");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn without_automatic_downloads_nothing_downloads_until_asked() {
    let key = HybridSigningKey::generate().expect("key");
    let dir = tempfile::tempdir().expect("tempdir");
    let github = fake_github(&key, "1.2.0").await;
    let settings = ReleaseConfig { download: false, ..config(dir.path(), &key, Some(github)) };
    let server = postd(dir.path(), settings).await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(!dir.path().join("releases/1.2.0").exists(), "only checked");

    server.refresh_releases();
    assert!(eventually(async || live(&server).await.is_some()).await, "downloaded and live once asked");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_release_not_signed_by_the_release_key_is_refused() {
    let key = HybridSigningKey::generate().expect("key");
    let impostor = HybridSigningKey::generate().expect("key");
    let github = fake_github(&impostor, "1.2.0").await;
    let dir = tempfile::tempdir().expect("tempdir");
    let settings = ReleaseConfig { live_while_running: true, ..config(dir.path(), &key, Some(github)) };
    let server = postd(dir.path(), settings).await;
    server.refresh_releases();
    tokio::time::sleep(Duration::from_millis(800)).await;
    assert_eq!(live(&server).await, None);
    assert!(!dir.path().join("releases/1.2.0").join(tarball_name("1.2.0")).exists(), "nothing downloaded");
}
