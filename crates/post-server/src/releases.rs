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
//! The operator decides how much happens alone. By default the server looks
//! for a new release every hour and downloads it, but puts it live (offers
//! it to apps) only when it next starts, or when the operator asks with a
//! reload (SIGHUP). Each step can be switched on or off (see
//! [`ReleaseConfig`]).
//!
//! The directory holds one folder per version, each with its files and,
//! per platform, `manifest-<platform>.json` and its `.sig`. A release
//! counts once its signature verifies and every file matches; the manifest
//! is written last, so a half-fetched release never looks complete.
//! `live-<platform>` remembers which version is live.

use std::collections::{BTreeMap, HashMap};
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

/// Where releases come from and are kept, and how much the server does with
/// them by itself.
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
    /// Look for a new release by itself, every `every`.
    pub check: bool,
    /// Download a new release it finds, ready to go live.
    pub download: bool,
    /// Put a release live (offer it to apps) as soon as it has downloaded.
    pub live_while_running: bool,
    /// On starting, put the newest downloaded release live. Otherwise the
    /// release live before the restart stays live.
    pub live_on_start: bool,
}

impl ReleaseConfig {
    /// Releases kept in `dir`, checked under `key`, with the defaults: look
    /// every hour and download, go live at the next start.
    pub fn new(dir: PathBuf, key: Option<HybridVerifyingKey>) -> Self {
        Self {
            dir,
            github_repo: None,
            github_api: "https://api.github.com".into(),
            key,
            every: Duration::from_secs(3600),
            check: true,
            download: true,
            live_while_running: false,
            live_on_start: true,
        }
    }

    /// Serve nothing (tests, and servers that do not hand out releases).
    pub fn none() -> Self {
        Self::new(PathBuf::new(), None)
    }
}

/// A complete, checked release for one platform.
#[derive(Clone)]
pub(crate) struct Published {
    pub version: Version,
    pub manifest: Manifest,
    pub manifest_bytes: Vec<u8>,
    pub signature: Vec<u8>,
}

/// The releases on disk, and the one live for each platform.
pub(crate) struct Releases {
    config: ReleaseConfig,
    /// Complete, checked releases in the directory, by platform and version.
    on_disk: RwLock<HashMap<String, BTreeMap<Version, Published>>>,
    /// The release offered to apps, by platform.
    live: RwLock<HashMap<String, Published>>,
    /// Look again now rather than at the next hour.
    now: tokio::sync::Notify,
}

impl Releases {
    pub fn new(config: ReleaseConfig) -> Self {
        Self { config, on_disk: RwLock::default(), live: RwLock::default(), now: tokio::sync::Notify::new() }
    }

    /// The operator asks: re-read the directory, fetch and download the
    /// latest release whatever the settings, and put the newest live.
    pub fn refresh(&self) {
        self.now.notify_one();
    }

    /// The release live for `platform`.
    pub fn latest(&self, platform: &str) -> Option<Published> {
        self.live.read().unwrap_or_else(|p| p.into_inner()).get(platform).cloned()
    }

    /// Where a file of the live release is, if `version` is the one live for
    /// `platform` and the file is one of its files.
    pub fn file_path(&self, platform: &str, version: &str, name: &str) -> Option<PathBuf> {
        let p = self.latest(platform)?;
        (p.manifest.version == version && p.manifest.file(name).is_some()).then(|| self.config.dir.join(version).join(name))
    }

    fn live_file(&self, platform: &str) -> PathBuf {
        self.config.dir.join(format!("live-{platform}"))
    }

    fn newest_on_disk(&self, platform: &str) -> Option<Published> {
        self.on_disk.read().unwrap_or_else(|p| p.into_inner()).get(platform)?.values().next_back().cloned()
    }

    /// Offer `p` to apps, and remember it as live.
    fn go_live(&self, p: Published) {
        let mut live = self.live.write().unwrap_or_else(|p| p.into_inner());
        if live.get(&p.manifest.platform).is_some_and(|c| c.version == p.version) {
            return;
        }
        tracing::info!(version = %p.version, platform = %p.manifest.platform, "release live: apps can now update to it");
        if let Err(e) = std::fs::write(self.live_file(&p.manifest.platform), format!("{}\n", p.version)) {
            tracing::warn!(error = %e, "could not remember which release is live");
        }
        live.insert(p.manifest.platform.clone(), p);
    }

    /// Put the newest release on disk live, for every platform.
    fn newest_live(&self) {
        let platforms: Vec<String> = self.on_disk.read().unwrap_or_else(|p| p.into_inner()).keys().cloned().collect();
        for platform in platforms {
            if let Some(p) = self.newest_on_disk(&platform) {
                self.go_live(p);
            }
        }
    }

    /// On starting: read the directory, then put live the newest release
    /// (`live_on_start`) or the one live before.
    pub fn start(&self) {
        self.load();
        if self.config.live_on_start {
            return self.newest_live();
        }
        let platforms: Vec<String> = self.on_disk.read().unwrap_or_else(|p| p.into_inner()).keys().cloned().collect();
        for platform in platforms {
            let before = std::fs::read_to_string(self.live_file(&platform)).ok().and_then(|s| s.trim().parse::<Version>().ok());
            let release = before.and_then(|v| self.on_disk.read().unwrap_or_else(|p| p.into_inner()).get(&platform)?.get(&v).cloned());
            match release {
                Some(p) => self.go_live(p),
                None => tracing::info!(platform, "no release live: put the newest live with a reload (SIGHUP)"),
            }
        }
    }

    /// Read every complete, valid release in the directory.
    fn load(&self) {
        let Some(key) = &self.config.key else { return };
        let mut found: HashMap<String, BTreeMap<Version, Published>> = HashMap::new();
        if let Ok(entries) = std::fs::read_dir(&self.config.dir) {
            for entry in entries.flatten() {
                let Some(version) = entry.file_name().to_str().and_then(|s| s.parse::<Version>().ok()) else { continue };
                let Ok(files) = std::fs::read_dir(entry.path()) else { continue };
                for f in files.flatten() {
                    let name = f.file_name();
                    let Some(platform) = name.to_str().and_then(|n| n.strip_prefix("manifest-")).and_then(|n| n.strip_suffix(".json"))
                    else {
                        continue;
                    };
                    match load_release(key, &entry.path(), platform, version) {
                        Ok(p) => {
                            found.entry(platform.to_owned()).or_default().insert(version, p);
                        }
                        Err(e) => tracing::warn!(%version, platform, error = %e, "release in the directory refused"),
                    }
                }
            }
        }
        *self.on_disk.write().unwrap_or_else(|p| p.into_inner()) = found;
    }

    /// Ask GitHub for the latest published release and, if it is newer than
    /// any on disk, download it (if `download`, or asked) and put it live:
    /// if asked, if `live_while_running`, or, for the look the server takes
    /// as it starts, if `live_on_start`. That look is part of starting, so a
    /// release it finds is the newest at start.
    async fn fetch(&self, http: &reqwest::Client, why: Look) -> anyhow::Result<()> {
        let asked = why == Look::Asked;
        let go_live = asked || self.config.live_while_running || (why == Look::Starting && self.config.live_on_start);
        let (Some(repo), Some(key)) = (&self.config.github_repo, &self.config.key) else { return Ok(()) };
        let url = format!("{}/repos/{repo}/releases/latest", self.config.github_api);
        let latest: GithubRelease = serde_json::from_slice(&get_capped(http, &url, 1 << 20).await?)?;
        let asset = |name: &str| latest.assets.iter().find(|a| a.name == name);
        let mut downloaded = false;
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
            if self.newest_on_disk(platform).is_some_and(|c| c.version >= version) {
                continue;
            }
            if !self.config.download && !asked {
                tracing::info!(%version, platform, "a new release is out; automatic downloads are off, so download it with a reload (SIGHUP)");
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
            let p = Published { version, manifest, manifest_bytes, signature };
            self.on_disk.write().unwrap_or_else(|p| p.into_inner()).entry(platform.to_owned()).or_default().insert(version, p.clone());
            downloaded = true;
            if go_live {
                self.go_live(p);
            } else {
                tracing::info!(%version, platform, "release downloaded; it goes live at the next start, or now with a reload (SIGHUP)");
            }
        }
        if downloaded {
            self.prune();
        }
        Ok(())
    }

    /// Delete versions neither live nor newest, keeping one more behind them
    /// for going back.
    fn prune(&self) {
        let mut keep: Vec<Version> = self.live.read().unwrap_or_else(|p| p.into_inner()).values().map(|p| p.version).collect();
        keep.extend(self.on_disk.read().unwrap_or_else(|p| p.into_inner()).values().filter_map(|m| m.keys().next_back().copied()));
        let Some(oldest_kept) = keep.iter().min().copied() else { return };
        let Ok(entries) = std::fs::read_dir(&self.config.dir) else { return };
        let mut older: Vec<(Version, PathBuf)> = entries
            .flatten()
            .filter_map(|e| Some((e.file_name().to_str()?.parse::<Version>().ok()?, e.path())))
            .filter(|(v, _)| *v < oldest_kept)
            .collect();
        older.sort_by_key(|(v, _)| std::cmp::Reverse(*v));
        for (v, path) in older.into_iter().skip(1) {
            match std::fs::remove_dir_all(&path) {
                Ok(()) => {
                    for m in self.on_disk.write().unwrap_or_else(|p| p.into_inner()).values_mut() {
                        m.remove(&v);
                    }
                    tracing::info!(version = %v, "old release deleted");
                }
                Err(e) => tracing::warn!(version = %v, error = %e, "could not delete an old release"),
            }
        }
    }

    /// For as long as the server runs: look for new releases every hour if
    /// `check`, and do everything at once when [`Self::refresh`] asks.
    pub async fn keep_fetching(self: std::sync::Arc<Self>) {
        if self.config.key.is_none() {
            return;
        }
        let http = match reqwest::Client::builder().user_agent("postd").timeout(Duration::from_secs(600)).build() {
            Ok(h) => h,
            Err(e) => return tracing::error!(error = %e, "no HTTP client for fetching releases"),
        };
        // The first tick is at once: the look the server takes as it starts.
        let mut every = tokio::time::interval(self.config.every);
        let mut starting = true;
        loop {
            let why = tokio::select! {
                _ = every.tick() => if std::mem::take(&mut starting) { Look::Starting } else { Look::Timer },
                () = self.now.notified() => Look::Asked,
            };
            let asked = why == Look::Asked;
            if asked {
                tracing::info!("looking for releases now, as asked");
                // Releases copied in by hand; hashing them is file work.
                let me = self.clone();
                if tokio::task::spawn_blocking(move || me.load()).await.is_err() {
                    tracing::error!("re-reading the releases directory failed");
                }
            }
            if (asked || self.config.check)
                && let Err(e) = self.fetch(&http, why).await
            {
                tracing::warn!(error = %e, "fetching the latest release failed");
            }
            if asked {
                self.newest_live();
            }
        }
    }
}

/// Why the server looks for a new release.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Look {
    /// As it starts: part of starting, for `live_on_start`.
    Starting,
    /// The hourly look while running.
    Timer,
    /// The operator asked (a reload), so it does everything.
    Asked,
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
