//! The app releases this server hands out (design section 12, "App updates").
//!
//! Apps ask their own server for updates rather than a third party, so no one
//! else learns who runs Post or when. The server fetches signed releases
//! from GitHub (or an operator copies them into its releases directory) and
//! serves them over plain HTTPS, where downloads can resume.
//!
//! The server only distributes. It checks each release under the release
//! key before serving it, so it never hands out junk, but apps check
//! everything again themselves: a server cannot make a build apps accept.
//!
//! The directory holds one folder per version, each with its files and,
//! per platform, `manifest-<platform>.json` and its `.sig`. A release
//! counts once its signature verifies and every file matches; the manifest
//! is written last, so a half-fetched release never looks complete.

use std::collections::HashMap;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::sync::RwLock;
use std::time::Duration;

use futures_util::StreamExt as _;
use post_identity::HybridVerifyingKey;
use post_release::{Manifest, Version};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt as _;

/// Where releases come from and are kept.
#[derive(Clone)]
pub struct ReleaseConfig {
    /// The releases directory.
    pub dir: PathBuf,
    /// The GitHub repository (`owner/name`) to fetch published releases
    /// from, or none to serve only what is in the directory.
    pub github_repo: Option<String>,
    /// GitHub's API, changed only by tests.
    pub github_api: String,
    /// The release key releases must be signed with. Without one, nothing
    /// is served.
    pub key: Option<HybridVerifyingKey>,
    /// How often to look for a new release.
    pub every: Duration,
}

impl ReleaseConfig {
    /// Serve nothing (tests, and servers that do not hand out releases).
    pub fn none() -> Self {
        Self { dir: PathBuf::new(), github_repo: None, github_api: String::new(), key: None, every: Duration::from_secs(3600) }
    }
}

/// A release ready to hand out for one platform.
#[derive(Clone)]
pub(crate) struct Published {
    pub version: Version,
    pub manifest: Manifest,
    pub manifest_bytes: Vec<u8>,
    pub signature: Vec<u8>,
}

/// The newest complete release for each platform.
pub(crate) struct Releases {
    config: ReleaseConfig,
    current: RwLock<HashMap<String, Published>>,
    /// Look again now rather than at the next hour.
    now: tokio::sync::Notify,
}

impl Releases {
    pub fn new(config: ReleaseConfig) -> Self {
        Self { config, current: RwLock::default(), now: tokio::sync::Notify::new() }
    }

    /// Re-read the releases directory (for releases copied in by hand) and
    /// ask GitHub, now.
    pub fn refresh(&self) {
        self.now.notify_one();
    }

    /// The newest release for `platform`.
    pub fn latest(&self, platform: &str) -> Option<Published> {
        self.current.read().unwrap_or_else(|p| p.into_inner()).get(platform).cloned()
    }

    /// Where a file of a release being served is, if `version` is the one
    /// served for `platform` and the file is one of its files.
    pub fn file_path(&self, platform: &str, version: &str, name: &str) -> Option<PathBuf> {
        let p = self.latest(platform)?;
        (p.manifest.version == version && p.manifest.file(name).is_some()).then(|| self.config.dir.join(version).join(name))
    }

    fn offer(&self, p: Published) -> bool {
        let mut current = self.current.write().unwrap_or_else(|p| p.into_inner());
        let newer = current.get(&p.manifest.platform).is_none_or(|c| c.version < p.version);
        if newer {
            tracing::info!(version = %p.version, platform = %p.manifest.platform, "release ready to hand out");
            current.insert(p.manifest.platform.clone(), p);
        }
        newer
    }

    /// Read every complete, valid release in the directory, keeping the
    /// newest for each platform.
    pub fn load(&self) {
        let Some(key) = &self.config.key else { return };
        let Ok(entries) = std::fs::read_dir(&self.config.dir) else { return };
        for entry in entries.flatten() {
            let Some(version) = entry.file_name().to_str().and_then(|s| s.parse::<Version>().ok()) else { continue };
            let Ok(files) = std::fs::read_dir(entry.path()) else { continue };
            for f in files.flatten() {
                let name = f.file_name();
                let Some(platform) = name.to_str().and_then(|n| n.strip_prefix("manifest-")).and_then(|n| n.strip_suffix(".json")) else { continue };
                match load_release(key, &entry.path(), platform, version) {
                    Ok(p) => {
                        self.offer(p);
                    }
                    Err(e) => tracing::warn!(%version, platform, error = %e, "release in the directory refused"),
                }
            }
        }
    }

    /// Fetch the latest published release from GitHub, if it is newer than
    /// what is served. Returns how many platforms it brought up to date.
    pub async fn fetch(&self, http: &reqwest::Client) -> anyhow::Result<usize> {
        let (Some(repo), Some(key)) = (&self.config.github_repo, &self.config.key) else { return Ok(0) };
        let url = format!("{}/repos/{repo}/releases/latest", self.config.github_api);
        let latest: GithubRelease = serde_json::from_slice(&get_capped(http, &url, 1 << 20).await?)?;
        let asset = |name: &str| latest.assets.iter().find(|a| a.name == name);
        let mut updated = 0;
        for a in &latest.assets {
            let Some(platform) = a.name.strip_prefix("manifest-").and_then(|n| n.strip_suffix(".json")) else { continue };
            let Some(sig_asset) = asset(&post_release::signature_name(platform)) else { continue };
            let manifest_bytes = get_capped(http, &a.browser_download_url, post_release::MAX_MANIFEST).await?;
            let signature = get_capped(http, &sig_asset.browser_download_url, 8 * 1024).await?;
            let manifest = post_release::verify_with(key, &manifest_bytes, &signature)?;
            let version = manifest.version()?;
            if manifest.platform != platform {
                anyhow::bail!("{} describes {}", a.name, manifest.platform);
            }
            if self.latest(platform).is_some_and(|c| c.version >= version) {
                continue;
            }
            let dir = self.config.dir.join(version.to_string());
            tokio::fs::create_dir_all(&dir).await?;
            for f in &manifest.files {
                let Some(file_asset) = asset(&f.name) else { anyhow::bail!("the release has no {}", f.name) };
                download(http, &file_asset.browser_download_url, f, &dir).await?;
            }
            // The signature, then the manifest: once it is there the release
            // is complete.
            tokio::fs::write(dir.join(post_release::signature_name(platform)), &signature).await?;
            write_atomically(&dir.join(post_release::manifest_name(platform)), &manifest_bytes).await?;
            if self.offer(Published { version, manifest, manifest_bytes, signature }) {
                updated += 1;
            }
        }
        if updated > 0 {
            self.prune();
        }
        Ok(updated)
    }

    /// Delete versions no longer served for any platform, keeping one
    /// older version behind the newest.
    fn prune(&self) {
        let served: Vec<Version> = self.current.read().unwrap_or_else(|p| p.into_inner()).values().map(|p| p.version).collect();
        let Some(oldest_served) = served.iter().min().copied() else { return };
        let Ok(entries) = std::fs::read_dir(&self.config.dir) else { return };
        let mut older: Vec<(Version, PathBuf)> = entries
            .flatten()
            .filter_map(|e| Some((e.file_name().to_str()?.parse::<Version>().ok()?, e.path())))
            .filter(|(v, _)| *v < oldest_served)
            .collect();
        older.sort_by_key(|(v, _)| std::cmp::Reverse(*v));
        for (v, path) in older.into_iter().skip(1) {
            match std::fs::remove_dir_all(&path) {
                Ok(()) => tracing::info!(version = %v, "old release deleted"),
                Err(e) => tracing::warn!(version = %v, error = %e, "could not delete an old release"),
            }
        }
    }

    /// Look for new releases now and then, for as long as the server runs,
    /// and at once when [`Self::refresh`] asks.
    pub async fn keep_fetching(self: std::sync::Arc<Self>) {
        if self.config.key.is_none() {
            return;
        }
        let http = match reqwest::Client::builder().user_agent("postd").timeout(Duration::from_secs(600)).build() {
            Ok(h) => h,
            Err(e) => return tracing::error!(error = %e, "no HTTP client for fetching releases"),
        };
        let mut every = tokio::time::interval(self.config.every);
        loop {
            tokio::select! {
                _ = every.tick() => {}
                () = self.now.notified() => {
                    tracing::info!("looking for releases now, as asked");
                    // Releases copied in by hand; hashing them is file work.
                    let me = self.clone();
                    if tokio::task::spawn_blocking(move || me.load()).await.is_err() {
                        tracing::error!("re-reading the releases directory failed");
                    }
                }
            }
            if self.config.github_repo.is_some()
                && let Err(e) = self.fetch(&http).await
            {
                tracing::warn!(error = %e, "fetching the latest release failed");
            }
        }
    }
}

/// Check one platform's release in a version's folder: its signature, that
/// it is the version and platform it is filed under, and every file.
fn load_release(key: &HybridVerifyingKey, dir: &Path, platform: &str, version: Version) -> anyhow::Result<Published> {
    let manifest_bytes = std::fs::read(dir.join(post_release::manifest_name(platform)))?;
    let signature = std::fs::read(dir.join(post_release::signature_name(platform)))?;
    let manifest = post_release::verify_with(key, &manifest_bytes, &signature)?;
    if manifest.version()? != version || manifest.platform != platform {
        anyhow::bail!("filed under the wrong version or platform");
    }
    for f in &manifest.files {
        let mut file = std::fs::File::open(dir.join(&f.name))?;
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
        f.check(size, &h.finalize().into())?;
    }
    Ok(Published { version, manifest, manifest_bytes, signature })
}

#[derive(Deserialize)]
struct GithubRelease {
    assets: Vec<GithubAsset>,
}

#[derive(Deserialize)]
struct GithubAsset {
    name: String,
    browser_download_url: String,
}

/// A small download, refused past `cap` bytes.
async fn get_capped(http: &reqwest::Client, url: &str, cap: usize) -> anyhow::Result<Vec<u8>> {
    let res = http.get(url).header("Accept", "application/vnd.github+json, application/octet-stream").send().await?.error_for_status()?;
    let mut out = Vec::new();
    let mut body = res.bytes_stream();
    while let Some(chunk) = body.next().await {
        let chunk = chunk?;
        if out.len() + chunk.len() > cap {
            anyhow::bail!("{url} is larger than expected");
        }
        out.extend_from_slice(&chunk);
    }
    Ok(out)
}

/// Download a release file into `dir`, checking it against the manifest as
/// it comes; it takes its name only once it matches.
async fn download(http: &reqwest::Client, url: &str, f: &post_release::FileEntry, dir: &Path) -> anyhow::Result<()> {
    let target = dir.join(&f.name);
    if tokio::fs::try_exists(&target).await? {
        return Ok(());
    }
    let part = dir.join(format!("{}.part", f.name));
    let mut out = tokio::fs::File::create(&part).await?;
    let mut h = Sha256::new();
    let mut size = 0u64;
    let mut body = http.get(url).send().await?.error_for_status()?.bytes_stream();
    while let Some(chunk) = body.next().await {
        let chunk = chunk?;
        size += chunk.len() as u64;
        if size > f.size {
            anyhow::bail!("{} is larger than the release says", f.name);
        }
        h.update(&chunk);
        out.write_all(&chunk).await?;
    }
    out.sync_all().await?;
    drop(out);
    f.check(size, &h.finalize().into())?;
    tokio::fs::rename(&part, &target).await?;
    Ok(())
}

async fn write_atomically(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension("tmp");
    tokio::fs::write(&tmp, bytes).await?;
    tokio::fs::rename(&tmp, path).await
}
