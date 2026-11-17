// SPDX-License-Identifier: AGPL-3.0-or-later
//! Automatic updates with [Velopack](https://velopack.io), from Inlet's GitHub releases.
//!
//! - **What happens.** Shortly after start, and then at most every [`CHECK_EVERY`] (6 hours),
//!   Inlet asks GitHub for the newest release on its own channel. A newer version is
//!   downloaded in the background and checked against its size and SHA-256. It is
//!   installed the next time Inlet starts ([`apply_pending_at_start`]), or at once when the
//!   user picks "Restart to update". The Desktop settings tab can switch the whole thing
//!   off ([`set_enabled`]) and shows the changelog of the update ([`Release::notes`]).
//! - **What cannot happen.** Inlet never downgrades and never changes channel: the
//!   options that allow either are not set, and [`Checked`] refuses any feed that names
//!   another package, another channel, a file name that is not a plain package name, an
//!   older version or a hash that is not SHA-256. Every request goes over HTTPS only
//!   ([`ReleaseSource`] refuses a plain `http` address and redirects to one), to GitHub's
//!   API and release downloads of this repository and nowhere else. The package size is
//!   capped at what the feed announces, and the download is also compared with the digest
//!   GitHub computes for the asset when it is uploaded.
//! - **Not installed.** A copy that was not installed by Velopack (a development build, an
//!   unpacked tarball) has nothing to update: [`Status::Unavailable`].
//! - **Hooks.** [`startup`] runs first in `main`: it handles the installer's hook arguments
//!   (install, update, uninstall) and, on Windows, removes the registry keys Inlet wrote
//!   itself when the app is uninstalled.
//!
//! The release feed (`releases.<channel>.json`) and the full package are release assets,
//! put there by `release.yml`. The channel is `beta-<os>`: one feed file per operating
//! system, because every release holds the feeds of all of them.
//!
//! Settings: `"update"` in `settings.json` (`enabled`, `last_check`).

use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use gpui::{App, AppContext, Global};
use serde::{Deserialize, Serialize};
use velopack::bundle::Manifest;
use velopack::sources::UpdateSource;
use velopack::{
    Error as VeloError, UpdateCheck, UpdateManager, UpdateOptions, VelopackApp, VelopackAsset,
    VelopackAssetFeed,
};

use crate::backend::platform::prefs;

/// The Velopack package id (the same as the app id).
pub const PACKAGE_ID: &str = "no.opland.inlet";
/// How long Inlet waits between two automatic checks.
pub const CHECK_EVERY: Duration = Duration::from_secs(6 * 60 * 60);
/// How long after start the first automatic check waits, so it never competes with the
/// sign-in.
const FIRST_CHECK_DELAY: Duration = Duration::from_secs(45);
/// How often the scheduler looks at the clock.
const TICK: Duration = Duration::from_secs(15 * 60);
/// After a failed check, the next automatic one waits this long.
const RETRY_AFTER_FAILURE: Duration = Duration::from_secs(60 * 60);
/// Two checks the user asks for must be this far apart.
const MANUAL_COOLDOWN: Duration = Duration::from_secs(20);

/// The releases of the repository, newest first (published ones only: a draft is not
/// listed without a token).
const RELEASES_URL: &str = "https://api.github.com/repos/Ferdinand99/Inlet/releases?per_page=10";
/// Where the assets of this repository's releases are downloaded from.
const DOWNLOAD_PREFIX: &str = "https://github.com/Ferdinand99/Inlet/releases/download/";

/// The most a feed (`releases.<channel>.json`) or the releases list may be.
const MAX_FEED_BYTES: u64 = 1 << 20;
const MAX_LIST_BYTES: u64 = 4 << 20;
/// The largest package Inlet downloads.
const MAX_PACKAGE_BYTES: u64 = 300 << 20;
/// The most of a release's notes that is kept.
const MAX_NOTES_BYTES: usize = 16 * 1024;
/// The most of the notes that is shown.
pub const MAX_NOTES_SHOWN: usize = 6000;

/// The update channel of this operating system. One feed file per system in each release,
/// so the systems do not overwrite each other's.
pub const fn channel() -> &'static str {
    if cfg!(windows) {
        "beta-win"
    } else if cfg!(target_os = "macos") {
        "beta-osx"
    } else {
        "beta-linux"
    }
}

// ---- settings ------------------------------------------------------------------------

/// The update settings.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Prefs {
    /// Check, download and install updates by themselves. On by default.
    pub enabled: bool,
    /// When the last successful check was, in seconds since 1970. Kept so a restart does
    /// not check again within [`CHECK_EVERY`].
    pub last_check: Option<u64>,
}

impl Default for Prefs {
    fn default() -> Self {
        Prefs {
            enabled: true,
            last_check: None,
        }
    }
}

impl Prefs {
    /// Reads the settings; the defaults for a missing or broken file.
    pub fn load() -> Self {
        prefs::settings_path()
            .map(|p| prefs::load_section::<Prefs>(&p, "update"))
            .unwrap_or_default()
    }

    fn save(&self) {
        if let Some(path) = prefs::settings_path() {
            if let Err(err) = prefs::save_section(&path, "update", self) {
                log::warn!("could not write the update settings: {err}");
            }
        }
    }
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Whether an automatic check is due: never done, or [`CHECK_EVERY`] ago. A last check in
/// the future (the clock was set back) counts as due.
pub fn is_due(last_check: Option<u64>, now: u64) -> bool {
    match last_check {
        None => true,
        Some(last) => last > now || now - last >= CHECK_EVERY.as_secs(),
    }
}

// ---- what the user sees --------------------------------------------------------------

/// A release that was found: its version and its changelog.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
    pub version: String,
    /// The changelog as plain text ([`clean_notes`]), at most [`MAX_NOTES_SHOWN`] characters.
    pub notes: String,
}

impl Release {
    fn from_asset(asset: &VelopackAsset) -> Release {
        Release {
            version: asset.Version.clone(),
            notes: clean_notes(&asset.NotesMarkdown),
        }
    }
}

/// Where the updater stands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    /// Nothing known yet.
    Idle,
    /// This copy cannot update itself (not installed by the installer, or another channel).
    Unavailable,
    Checking,
    /// A newer release is being downloaded.
    Downloading(Release, u8),
    UpToDate,
    /// Downloaded: installed at the next start, or when the user restarts.
    Ready(Release),
    /// The last check or download failed (a short reason).
    Failed(String),
}

/// Turns release notes (Markdown, from the changelog) into plain text for the settings
/// tab: headings and bullets become lines, emphasis and links lose their markup, control
/// characters and HTML are dropped. The text is never parsed as Markdown for display and
/// never opens a link.
pub fn clean_notes(markdown: &str) -> String {
    let mut out = String::new();
    let mut blank = true;
    for raw in markdown.lines() {
        let line = raw.trim();
        if line.starts_with("<!--") || line.is_empty() {
            if !blank {
                out.push('\n');
                blank = true;
            }
            continue;
        }
        let (bullet, body) = match line {
            l if l.starts_with("* ") || l.starts_with("- ") => (true, &l[2..]),
            l => (false, l.trim_start_matches('#').trim_start()),
        };
        let text = strip_inline(body);
        let text = text.trim();
        if text.is_empty() {
            continue;
        }
        if bullet {
            out.push('\u{2022}');
            out.push(' ');
        }
        out.push_str(text);
        out.push('\n');
        blank = false;
    }
    let mut out: String = out
        .chars()
        .filter(|c| *c == '\n' || !c.is_control())
        .collect();
    out.truncate(out.trim_end().len());
    if out.chars().count() > MAX_NOTES_SHOWN {
        out = out.chars().take(MAX_NOTES_SHOWN).collect();
        out.push('\u{2026}');
    }
    out
}

/// `[text](url)` becomes `text`; `**`, `__` and backticks go; `<...>` tags go.
fn strip_inline(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            '[' => {
                // A link: keep the text, drop the address.
                if let Some(close) = chars[i..].iter().position(|c| *c == ']') {
                    let close = i + close;
                    if chars.get(close + 1) == Some(&'(') {
                        if let Some(end) = chars[close..].iter().position(|c| *c == ')') {
                            out.extend(&chars[i + 1..close]);
                            i = close + end + 1;
                            continue;
                        }
                    }
                }
                out.push('[');
            }
            '<' => match chars[i..].iter().position(|c| *c == '>') {
                Some(end) => {
                    i += end + 1;
                    continue;
                }
                None => out.push('<'),
            },
            '*' | '`' => {}
            '_' if chars.get(i + 1) == Some(&'_') => {
                i += 2;
                continue;
            }
            c => out.push(c),
        }
        i += 1;
    }
    out
}

fn short(err: impl std::fmt::Display) -> String {
    let text = err.to_string().replace(['\n', '\r'], " ");
    text.chars().take(200).collect()
}

// ---- what a feed may contain ---------------------------------------------------------

/// Why an asset of a feed is refused, if it is. The rules:
/// the package id is ours, the type is a full package (no delta, so no patch tool runs),
/// the version is a valid version, the file name is a plain package name for this app (no
/// path), the SHA-256 is there and well formed, and the size is sane.
fn refuse_reason(asset: &VelopackAsset) -> Option<&'static str> {
    if asset.PackageId != PACKAGE_ID {
        return Some("another package id");
    }
    if !asset.Type.eq_ignore_ascii_case("Full") {
        return Some("not a full package");
    }
    if semver::Version::parse(&asset.Version).is_err() {
        return Some("version is not valid");
    }
    let name = &asset.FileName;
    let plain = !name.is_empty()
        && name.len() <= 128
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_' | b'+'))
        && !name.contains("..")
        && name.starts_with(&format!("{PACKAGE_ID}-"))
        && name.ends_with("-full.nupkg");
    if !plain {
        return Some("file name is not a package name");
    }
    if asset.SHA256.len() != 64 || !asset.SHA256.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Some("no SHA-256");
    }
    if asset.Size == 0 || asset.Size > MAX_PACKAGE_BYTES {
        return Some("size is out of range");
    }
    None
}

/// An update source that only passes on what [`refuse_reason`] accepts, and only for the
/// channel it was made for. Wraps the real source (GitHub in the app, a folder in tests).
pub struct Checked<S> {
    inner: S,
    channel: String,
}

impl<S> Checked<S> {
    pub fn new(inner: S, channel: &str) -> Self {
        Checked {
            inner,
            channel: channel.to_owned(),
        }
    }
}

impl<S: UpdateSource> UpdateSource for Checked<S> {
    fn get_release_feed(
        &self,
        channel: &str,
        app: &Manifest,
        staged_user_id: &str,
    ) -> Result<VelopackAssetFeed, VeloError> {
        // No channel switch: only the channel this build was packed for.
        if channel != self.channel {
            return Err(VeloError::Other(format!(
                "updates are not offered for the channel {channel:?}"
            )));
        }
        let feed = self.inner.get_release_feed(channel, app, staged_user_id)?;
        let assets = feed
            .Assets
            .into_iter()
            .filter_map(|mut asset| match refuse_reason(&asset) {
                None => {
                    asset
                        .NotesMarkdown
                        .truncate(floor_boundary(&asset.NotesMarkdown, MAX_NOTES_BYTES));
                    // Only the markdown is used; HTML from a feed is never shown.
                    asset.NotesHtml.clear();
                    Some(asset)
                }
                Some(reason) => {
                    log::warn!("update feed: {} left out ({reason})", asset.FileName);
                    None
                }
            })
            .collect();
        Ok(VelopackAssetFeed { Assets: assets })
    }

    fn download_release_entry(
        &self,
        asset: &VelopackAsset,
        local_file: &Path,
        progress_sender: Option<Sender<i16>>,
    ) -> Result<(), VeloError> {
        if let Some(reason) = refuse_reason(asset) {
            return Err(VeloError::Other(format!(
                "{} is refused ({reason})",
                asset.FileName
            )));
        }
        self.inner
            .download_release_entry(asset, local_file, progress_sender)
    }
}

/// The largest index at most `max` that is a character boundary of `s`.
fn floor_boundary(s: &str, max: usize) -> usize {
    let mut at = max.min(s.len());
    while !s.is_char_boundary(at) {
        at -= 1;
    }
    at
}

// ---- GitHub --------------------------------------------------------------------------

#[derive(Deserialize)]
struct GhRelease {
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    assets: Vec<GhAsset>,
}

#[derive(Deserialize)]
struct GhAsset {
    name: String,
    browser_download_url: String,
    /// `sha256:<hex>`, computed by GitHub when the asset is uploaded.
    #[serde(default)]
    digest: Option<String>,
}

/// Where a package was found in the last feed.
struct Found {
    url: String,
    digest: Option<String>,
}

/// Reads the feed files and packages of the releases of this repository over HTTPS.
///
/// One request lists the newest releases; each release's `releases.<channel>.json` asset is
/// the feed of that release, and the packages are the assets named in the feeds. A feed
/// entry whose SHA-256 differs from the digest GitHub computed for the same file is left
/// out. Nothing is sent but a `GET` with a user agent: no token, no cookie, no identifier.
pub struct ReleaseSource {
    list_url: String,
    download_prefix: String,
    https_only: bool,
    client: OnceLock<Result<reqwest::blocking::Client, String>>,
    found: Mutex<HashMap<String, Found>>,
}

impl ReleaseSource {
    /// The real source: Inlet's releases on GitHub, HTTPS only.
    pub fn github() -> Self {
        Self::with_urls(RELEASES_URL, DOWNLOAD_PREFIX, true)
    }

    fn with_urls(list_url: &str, download_prefix: &str, https_only: bool) -> Self {
        ReleaseSource {
            list_url: list_url.to_owned(),
            download_prefix: download_prefix.to_owned(),
            https_only,
            client: OnceLock::new(),
            found: Mutex::new(HashMap::new()),
        }
    }

    /// Built on first use, so that starting Inlet does not start the HTTP machinery.
    fn client(&self) -> Result<&reqwest::blocking::Client, VeloError> {
        let built = self.client.get_or_init(|| {
            reqwest::blocking::Client::builder()
                .user_agent(concat!("Inlet/", env!("CARGO_PKG_VERSION")))
                .https_only(self.https_only)
                .redirect(reqwest::redirect::Policy::limited(5))
                .connect_timeout(Duration::from_secs(15))
                .timeout(Duration::from_secs(20 * 60))
                .build()
                .map_err(short)
        });
        built.as_ref().map_err(|e| VeloError::Other(e.clone()))
    }

    fn check_url(&self, url: &str, prefix: &str) -> Result<(), VeloError> {
        let ok = url.starts_with(prefix) && (url.starts_with("https://") || !self.https_only);
        if ok {
            Ok(())
        } else {
            Err(VeloError::Other(format!("address not allowed: {url}")))
        }
    }

    fn get(&self, url: &str, accept: &str) -> Result<reqwest::blocking::Response, VeloError> {
        let response = self
            .client()?
            .get(url)
            .header(reqwest::header::ACCEPT, accept)
            .send()
            .and_then(|r| r.error_for_status())
            .map_err(|e| VeloError::Other(short(e)))?;
        if self.https_only && response.url().scheme() != "https" {
            return Err(VeloError::Other("answer did not come over https".into()));
        }
        Ok(response)
    }

    fn get_bytes(&self, url: &str, accept: &str, cap: u64) -> Result<Vec<u8>, VeloError> {
        let mut body = Vec::new();
        self.get(url, accept)?
            .take(cap + 1)
            .read_to_end(&mut body)
            .map_err(|e| VeloError::Other(short(e)))?;
        if body.len() as u64 > cap {
            return Err(VeloError::Other("answer is too large".into()));
        }
        Ok(body)
    }
}

fn digest_matches(asset: &VelopackAsset, digest: &Option<String>) -> bool {
    match digest.as_deref().and_then(|d| d.strip_prefix("sha256:")) {
        Some(hex) => hex.eq_ignore_ascii_case(&asset.SHA256),
        None => true,
    }
}

impl UpdateSource for ReleaseSource {
    fn get_release_feed(
        &self,
        channel: &str,
        _app: &Manifest,
        _staged_user_id: &str,
    ) -> Result<VelopackAssetFeed, VeloError> {
        if channel.is_empty()
            || !channel
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        {
            return Err(VeloError::Other("channel name is not valid".into()));
        }
        let feed_name = format!("releases.{channel}.json");
        let list = self.get_bytes(
            &self.list_url,
            "application/vnd.github+json",
            MAX_LIST_BYTES,
        )?;
        let releases: Vec<GhRelease> = serde_json::from_slice(&list)?;

        let mut assets = Vec::new();
        let mut found = HashMap::new();
        for release in releases.iter().filter(|r| !r.draft) {
            let Some(feed_asset) = release.assets.iter().find(|a| a.name == feed_name) else {
                continue;
            };
            if self
                .check_url(&feed_asset.browser_download_url, &self.download_prefix)
                .is_err()
            {
                log::warn!("update feed: an address was refused");
                continue;
            }
            let bytes = match self.get_bytes(
                &feed_asset.browser_download_url,
                "application/octet-stream",
                MAX_FEED_BYTES,
            ) {
                Ok(bytes) => bytes,
                Err(err) => {
                    log::info!("update feed of one release not read: {err}");
                    continue;
                }
            };
            let Ok(feed) = serde_json::from_slice::<VelopackAssetFeed>(&bytes) else {
                log::info!("update feed of one release is not valid");
                continue;
            };
            for asset in feed.Assets {
                let Some(file) = release.assets.iter().find(|a| a.name == asset.FileName) else {
                    continue;
                };
                if !digest_matches(&asset, &file.digest) {
                    log::warn!(
                        "update feed: {} differs from GitHub's digest",
                        asset.FileName
                    );
                    continue;
                }
                if self
                    .check_url(&file.browser_download_url, &self.download_prefix)
                    .is_err()
                {
                    continue;
                }
                found.insert(
                    asset.FileName.clone(),
                    Found {
                        url: file.browser_download_url.clone(),
                        digest: file.digest.clone(),
                    },
                );
                assets.push(asset);
            }
        }
        *self.found.lock().unwrap_or_else(|e| e.into_inner()) = found;
        Ok(VelopackAssetFeed { Assets: assets })
    }

    fn download_release_entry(
        &self,
        asset: &VelopackAsset,
        local_file: &Path,
        progress_sender: Option<Sender<i16>>,
    ) -> Result<(), VeloError> {
        let (url, digest) = {
            let found = self.found.lock().unwrap_or_else(|e| e.into_inner());
            let one = found
                .get(&asset.FileName)
                .ok_or_else(|| VeloError::Other("the package is not in the last feed".into()))?;
            (one.url.clone(), one.digest.clone())
        };
        if !digest_matches(asset, &digest) {
            return Err(VeloError::Other("digest differs".into()));
        }
        self.check_url(&url, &self.download_prefix)?;
        let mut response = self.get(&url, "application/octet-stream")?;
        if response
            .content_length()
            .is_some_and(|len| len != asset.Size)
        {
            return Err(VeloError::Other("the size differs from the feed".into()));
        }
        let mut file = std::fs::File::create(local_file)?;
        let mut buffer = vec![0u8; 256 * 1024];
        let mut done: u64 = 0;
        let mut reported = 0i16;
        loop {
            let n = response
                .read(&mut buffer)
                .map_err(|e| VeloError::Other(short(e)))?;
            if n == 0 {
                break;
            }
            done += n as u64;
            // Never more than the feed announced: Velopack then checks size and hash.
            if done > asset.Size {
                return Err(VeloError::Other("more data than the feed announced".into()));
            }
            file.write_all(&buffer[..n])?;
            let percent = (done * 100 / asset.Size) as i16;
            if percent >= reported + 5 {
                reported = percent;
                if let Some(tx) = &progress_sender {
                    let _ = tx.send(percent);
                }
            }
        }
        file.flush()?;
        Ok(())
    }
}

// ---- the engine ----------------------------------------------------------------------

/// What one check found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    UpToDate,
    /// A newer release is downloaded and waits for the next start.
    Ready(Release),
}

/// Velopack's update manager with Inlet's rules. Blocking: run it off the UI thread.
pub struct Engine {
    manager: UpdateManager,
}

impl Engine {
    /// The engine for the installed copy, with GitHub as the source. `Err` when this copy
    /// was not installed by Velopack.
    pub fn production() -> Result<Engine, String> {
        Self::with_source(ReleaseSource::github())
    }

    /// The engine with another source (a folder, in tests), for the same rules.
    pub fn with_source<S: UpdateSource + 'static>(source: S) -> Result<Engine, String> {
        // No downgrade (the default, stated here), no explicit channel (the one this copy
        // was packed for), no deltas (nothing is patched on the user's machine).
        let options = UpdateOptions {
            AllowVersionDowngrade: false,
            ExplicitChannel: None,
            MaximumDeltasBeforeFallback: -1,
        };
        let manager = UpdateManager::new(Checked::new(source, channel()), Some(options), None)
            .map_err(short)?;
        Ok(Engine { manager })
    }

    /// The installed version.
    pub fn current_version(&self) -> String {
        self.manager.get_current_version_as_string()
    }

    /// A release that is downloaded and waits for a restart.
    pub fn pending(&self) -> Option<Release> {
        self.manager
            .get_update_pending_restart()
            .map(|a| Release::from_asset(&a))
    }

    /// Looks for a newer release and downloads it. `progress` gets 0 to 100 while the
    /// package comes in (called from another thread).
    pub fn check_and_download(
        &self,
        progress: impl Fn(Release, u8) + Send + 'static,
    ) -> Result<Outcome, String> {
        let current = self.manager.get_current_version();
        match self.manager.check_for_updates().map_err(short)? {
            UpdateCheck::RemoteIsEmpty | UpdateCheck::NoUpdateAvailable => {
                Ok(self.pending().map_or(Outcome::UpToDate, Outcome::Ready))
            }
            UpdateCheck::UpdateAvailable(info) => {
                let target =
                    semver::Version::parse(&info.TargetFullRelease.Version).map_err(short)?;
                // Belt and braces: the options already forbid a downgrade.
                if info.IsDowngrade || target <= current {
                    return Err("the offered version is not newer".into());
                }
                let release = Release::from_asset(&info.TargetFullRelease);
                let (tx, rx) = std::sync::mpsc::channel::<i16>();
                let shown = release.clone();
                let relay = std::thread::spawn(move || {
                    for percent in rx {
                        progress(shown.clone(), percent.clamp(0, 100) as u8);
                    }
                });
                let result = self.manager.download_updates(&info, Some(tx));
                let _ = relay.join();
                result.map_err(short)?;
                Ok(Outcome::Ready(release))
            }
        }
    }

    /// Hands the downloaded package to the updater, which waits for this process to end,
    /// installs it and starts Inlet again with `restart_args`. The caller ends the
    /// process right after.
    pub fn apply(&self, restart_args: &[String]) -> Result<(), String> {
        let asset = self
            .manager
            .get_update_pending_restart()
            .ok_or("no update is downloaded")?;
        self.manager
            .wait_exit_then_apply_updates(&asset, true, true, restart_args)
            .map_err(short)
    }
}

// ---- start-up ------------------------------------------------------------------------

/// Set when Velopack started this process after an update (it sets `VELOPACK_RESTART`,
/// which `VelopackApp::run` removes again).
static RESTARTED: AtomicBool = AtomicBool::new(false);

/// Runs first in `main`. Handles the installer's hook arguments (`--veloapp-install`,
/// `-updated`, `-obsolete`, `-uninstall`): the process exits after the hook. On Windows
/// the uninstall hook removes what Inlet wrote itself outside its folder (the `inlet://`
/// handler, the login item and the notification id in the user's registry).
///
/// It does not apply a waiting update: that happens in [`apply_pending_at_start`], once
/// this process is the only Inlet, and only when updates are on.
pub fn startup() {
    RESTARTED.store(
        std::env::var_os("VELOPACK_RESTART").is_some(),
        Ordering::Relaxed,
    );
    let app = VelopackApp::build().set_auto_apply_on_startup(false);
    #[cfg(windows)]
    let app = app.on_before_uninstall_fast_callback(|_| inlet_platform::uninstall::cleanup());
    let mut app = app;
    app.run();
}

/// If updates are on and one is downloaded, installs it now: the updater takes over and
/// this process ends. Called after this process became the one running Inlet (a second
/// start, which only hands its command line on, must not replace the running copy). Not
/// done again right after an update (a package that does not install would loop).
pub fn apply_pending_at_start(args: &[String]) {
    if RESTARTED.load(Ordering::Relaxed) || !Prefs::load().enabled {
        return;
    }
    let Ok(engine) = Engine::production() else {
        return;
    };
    if engine.pending().is_none() {
        return;
    }
    match engine.apply(args) {
        Ok(()) => std::process::exit(0),
        Err(err) => log::warn!("could not install the waiting update: {err}"),
    }
}

// ---- the app side --------------------------------------------------------------------

/// The updater of the running app, a gpui global. The settings tab reads it.
pub struct Updater {
    pub prefs: Prefs,
    pub status: Status,
    /// The installed version, when this copy can update.
    pub current: Option<String>,
    engine: Option<Arc<Engine>>,
    busy: bool,
    last_manual: Option<std::time::Instant>,
    retry_after: Option<std::time::Instant>,
    persist: bool,
}

impl Global for Updater {}

impl Updater {
    fn new(persist: bool) -> Self {
        Updater {
            prefs: if persist {
                Prefs::load()
            } else {
                Prefs::default()
            },
            status: Status::Idle,
            current: None,
            engine: None,
            busy: false,
            last_manual: None,
            retry_after: None,
            persist,
        }
    }

    /// Whether a check or a download is running.
    pub fn busy(&self) -> bool {
        self.busy
    }

    /// Whether the user may ask for a check now.
    pub fn can_check(&self) -> bool {
        self.engine.is_some()
            && !self.busy
            && self
                .last_manual
                .is_none_or(|t| t.elapsed() >= MANUAL_COOLDOWN)
    }

    /// Whether a downloaded update waits for a restart.
    pub fn ready(&self) -> Option<&Release> {
        match &self.status {
            Status::Ready(r) => Some(r),
            _ => None,
        }
    }

    fn save(&self) {
        if self.persist {
            self.prefs.save();
        }
    }
}

/// Sets up the updater, before the main window opens. `persist` is off for demo runs.
/// A demo run can show a state with `INLET_DEMO_UPDATE` (`ready`, `downloading`,
/// `uptodate`, `failed`); a normal demo run has no updater.
pub fn init(cx: &mut App) {
    let demo = cfg!(feature = "demo") && std::env::var_os("INLET_DEMO").is_some();
    if demo {
        #[cfg(feature = "demo")]
        demo_state(cx);
        return;
    }
    let mut updater = Updater::new(true);
    match Engine::production() {
        Ok(engine) => {
            updater.current = Some(engine.current_version());
            updater.status = engine.pending().map_or(Status::Idle, Status::Ready);
            updater.engine = Some(Arc::new(engine));
        }
        Err(err) => {
            log::info!("updates are not available for this copy: {err}");
            updater.status = Status::Unavailable;
        }
    }
    let available = updater.engine.is_some();
    cx.set_global(updater);
    if available {
        schedule(cx);
    }
}

#[cfg(feature = "demo")]
fn demo_state(cx: &mut App) {
    let Ok(which) = std::env::var("INLET_DEMO_UPDATE") else {
        return;
    };
    let release = Release {
        version: "0.2.0".into(),
        notes: clean_notes(
            "### Features\n\n* **chat:** reply previews as plain text ([abc1234](https://example.test))\n* **settings:** a Desktop tab with updates\n\n### Bug Fixes\n\n* **media:** GIFs always end as a picture, a still or an error tile\n",
        ),
    };
    let mut updater = Updater::new(false);
    updater.current = Some("0.1.0".into());
    updater.engine = None;
    updater.status = match which.as_str() {
        "downloading" => Status::Downloading(release, 40),
        "uptodate" => Status::UpToDate,
        "failed" => Status::Failed("Network error".into()),
        _ => Status::Ready(release),
    };
    cx.set_global(updater);
}

/// The scheduler: the first check shortly after start, then whenever one is due.
fn schedule(cx: &mut App) {
    cx.spawn(async move |cx| {
        cx.background_executor().timer(FIRST_CHECK_DELAY).await;
        loop {
            cx.update(check_if_due);
            cx.background_executor().timer(TICK).await;
        }
    })
    .detach();
}

fn check_if_due(cx: &mut App) {
    let u = cx.global::<Updater>();
    let due = u.prefs.enabled
        && !u.busy
        && u.ready().is_none()
        && is_due(u.prefs.last_check, now_secs())
        && u.retry_after.is_none_or(|t| t <= std::time::Instant::now());
    if due {
        start_check(cx);
    }
}

/// Turns the automatic updates on or off. Off: no check, no download, and a downloaded
/// update is not installed at the next start.
pub fn set_enabled(on: bool, cx: &mut App) {
    let u = cx.global_mut::<Updater>();
    u.prefs.enabled = on;
    u.save();
    cx.refresh_windows();
}

/// The user asked for a check. Allowed also while the automatic updates are off.
pub fn check_now(cx: &mut App) {
    if !cx.global::<Updater>().can_check() {
        return;
    }
    cx.global_mut::<Updater>().last_manual = Some(std::time::Instant::now());
    start_check(cx);
}

fn start_check(cx: &mut App) {
    let u = cx.global_mut::<Updater>();
    let Some(engine) = u.engine.clone() else {
        return;
    };
    u.busy = true;
    u.status = Status::Checking;
    cx.refresh_windows();

    let (tx, rx) = futures::channel::mpsc::unbounded::<(Release, u8)>();
    let work = cx.background_spawn(async move {
        engine.check_and_download(move |release, percent| {
            let _ = tx.unbounded_send((release, percent));
        })
    });
    cx.spawn(async move |cx| {
        use futures::{FutureExt, StreamExt};
        let mut work = work.fuse();
        let mut rx = rx.fuse();
        let result = loop {
            futures::select! {
                shown = rx.next() => {
                    if let Some((release, percent)) = shown {
                        cx.update(|cx| {
                            cx.global_mut::<Updater>().status = Status::Downloading(release, percent);
                            cx.refresh_windows();
                        });
                    }
                }
                result = work => break result,
            }
        };
        cx.update(|cx| finish_check(result, cx));
    })
    .detach();
}

fn finish_check(result: Result<Outcome, String>, cx: &mut App) {
    let u = cx.global_mut::<Updater>();
    u.busy = false;
    match result {
        Ok(outcome) => {
            u.prefs.last_check = Some(now_secs());
            u.retry_after = None;
            u.save();
            u.status = match outcome {
                Outcome::UpToDate => Status::UpToDate,
                Outcome::Ready(release) => Status::Ready(release),
            };
        }
        Err(err) => {
            log::info!("update check failed: {err}");
            u.retry_after = Some(std::time::Instant::now() + RETRY_AFTER_FAILURE);
            u.status = Status::Failed(err);
        }
    }
    cx.refresh_windows();
}

/// Installs the downloaded update now: Inlet ends, the updater installs and starts it again.
pub fn restart_now(cx: &mut App) {
    let Some(engine) = cx.global::<Updater>().engine.clone() else {
        return;
    };
    match engine.apply(&[]) {
        Ok(()) => crate::desktop::quit(cx),
        Err(err) => {
            log::warn!("could not start the update: {err}");
            cx.global_mut::<Updater>().status = Status::Failed(err);
            cx.refresh_windows();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn asset(name: &str, version: &str) -> VelopackAsset {
        VelopackAsset {
            PackageId: PACKAGE_ID.into(),
            Version: version.into(),
            Type: "Full".into(),
            FileName: name.into(),
            SHA1: "0".repeat(40),
            SHA256: "ab".repeat(32),
            Size: 1000,
            NotesMarkdown: String::new(),
            NotesHtml: String::new(),
        }
    }

    fn good() -> VelopackAsset {
        asset("no.opland.inlet-0.2.0-beta-linux-full.nupkg", "0.2.0")
    }

    #[test]
    fn the_channel_belongs_to_the_system() {
        assert!(channel().starts_with("beta-"));
        if cfg!(windows) {
            assert_eq!(channel(), "beta-win");
        } else if cfg!(target_os = "macos") {
            assert_eq!(channel(), "beta-osx");
        } else {
            assert_eq!(channel(), "beta-linux");
        }
    }

    #[test]
    fn a_check_is_due_after_six_hours() {
        let now = 1_000_000;
        assert!(is_due(None, now));
        assert!(!is_due(Some(now), now));
        assert!(!is_due(Some(now - 6 * 3600 + 1), now));
        assert!(is_due(Some(now - 6 * 3600), now));
        // The clock was set back: check again rather than wait for ever.
        assert!(is_due(Some(now + 1), now));
    }

    #[test]
    fn defaults_are_on_and_a_broken_section_falls_back() {
        assert!(Prefs::default().enabled);
        let path = std::env::temp_dir().join(format!("inlet-update-{}.json", std::process::id()));
        std::fs::write(
            &path,
            r#"{"update": {"enabled": false, "last_check": 5}, "x": 1}"#,
        )
        .unwrap();
        let p = prefs::load_section::<Prefs>(&path, "update");
        assert!(!p.enabled);
        assert_eq!(p.last_check, Some(5));
        std::fs::write(&path, r#"{"update": "oops"}"#).unwrap();
        assert_eq!(
            prefs::load_section::<Prefs>(&path, "update"),
            Prefs::default()
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_good_asset_is_accepted_and_others_are_refused() {
        assert_eq!(refuse_reason(&good()), None);

        let mut a = good();
        a.PackageId = "other.app".into();
        assert!(refuse_reason(&a).is_some());

        let mut a = good();
        a.Type = "Delta".into();
        assert!(refuse_reason(&a).is_some());

        let mut a = good();
        a.Version = "not-a-version".into();
        assert!(refuse_reason(&a).is_some());

        for name in [
            "../no.opland.inlet-0.2.0-full.nupkg",
            "dir/no.opland.inlet-0.2.0-full.nupkg",
            "dir\\no.opland.inlet-0.2.0-full.nupkg",
            "no.opland.inlet-0.2.0-delta.nupkg",
            "other-0.2.0-full.nupkg",
            "no.opland.inlet-..-full.nupkg",
            "no.opland.inlet-0.2.0-full.nupkg?x=1",
            "",
        ] {
            let mut a = good();
            a.FileName = name.into();
            assert!(refuse_reason(&a).is_some(), "{name:?} must be refused");
        }

        // The hash must be a SHA-256: a missing or short one (SHA-1 only) is refused.
        let mut a = good();
        a.SHA256 = String::new();
        assert!(refuse_reason(&a).is_some());
        a.SHA256 = "ab".repeat(20);
        assert!(refuse_reason(&a).is_some());
        a.SHA256 = "zz".repeat(32);
        assert!(refuse_reason(&a).is_some());

        let mut a = good();
        a.Size = 0;
        assert!(refuse_reason(&a).is_some());
        a.Size = MAX_PACKAGE_BYTES + 1;
        assert!(refuse_reason(&a).is_some());
    }

    /// A source that returns what it is given.
    struct Fixed(Vec<VelopackAsset>);

    impl UpdateSource for Fixed {
        fn get_release_feed(
            &self,
            _: &str,
            _: &Manifest,
            _: &str,
        ) -> Result<VelopackAssetFeed, VeloError> {
            Ok(VelopackAssetFeed {
                Assets: self.0.clone(),
            })
        }
        fn download_release_entry(
            &self,
            _: &VelopackAsset,
            _: &Path,
            _: Option<Sender<i16>>,
        ) -> Result<(), VeloError> {
            Ok(())
        }
    }

    fn manifest() -> Manifest {
        Manifest::default()
    }

    #[test]
    fn the_checked_source_passes_only_good_assets_for_its_own_channel() {
        let mut bad = good();
        bad.PackageId = "other".into();
        let mut long_notes = good();
        long_notes.NotesMarkdown = "é".repeat(MAX_NOTES_BYTES);
        long_notes.NotesHtml = "<script>x</script>".into();
        let source = Checked::new(Fixed(vec![good(), bad, long_notes]), "beta-linux");

        let feed = source
            .get_release_feed("beta-linux", &manifest(), "")
            .unwrap();
        assert_eq!(feed.Assets.len(), 2);
        assert!(feed.Assets[1].NotesMarkdown.len() <= MAX_NOTES_BYTES);
        assert!(feed.Assets[1].NotesHtml.is_empty());

        // No channel switch.
        assert!(source.get_release_feed("stable", &manifest(), "").is_err());
        assert!(source
            .get_release_feed("beta-win", &manifest(), "")
            .is_err());
        assert!(source.get_release_feed("", &manifest(), "").is_err());

        // A refused package is not downloaded either.
        let mut bad = good();
        bad.SHA256 = String::new();
        assert!(source
            .download_release_entry(&bad, Path::new("x.partial"), None)
            .is_err());
        assert!(source
            .download_release_entry(&good(), Path::new("x.partial"), None)
            .is_ok());
    }

    #[test]
    fn notes_become_plain_text() {
        let md = "<!-- comment -->\n## [0.2.0](https://github.com/x/y/compare/v0.1.0...v0.2.0) (2026-11-17)\n\n\n### Features\n\n* **chat:** reply previews ([abc1234](https://github.com/x/y/commit/abc1234))\n- plain __bold__ `code` <b>tag</b>\n\u{7}bell\n";
        let text = clean_notes(md);
        assert_eq!(
            text,
            "0.2.0 (2026-11-17)\n\nFeatures\n\n\u{2022} chat: reply previews (abc1234)\n\u{2022} plain bold code tag\nbell"
        );
        assert!(!text.contains("http"));
        assert_eq!(clean_notes(""), "");
        assert_eq!(clean_notes("   \n\n"), "");
    }

    #[test]
    fn long_notes_are_cut() {
        let text = clean_notes(&"word ".repeat(10_000));
        assert!(text.chars().count() <= MAX_NOTES_SHOWN + 1);
        assert!(text.ends_with('\u{2026}'));
    }

    #[test]
    fn the_real_source_is_https_only_and_pinned_to_the_repository() {
        let source = ReleaseSource::github();
        assert!(source.https_only);
        assert!(source
            .list_url
            .starts_with("https://api.github.com/repos/Ferdinand99/Inlet/"));
        assert!(source
            .download_prefix
            .starts_with("https://github.com/Ferdinand99/Inlet/"));
        assert!(source
            .check_url(
                "https://github.com/Ferdinand99/Inlet/releases/download/v1/a",
                DOWNLOAD_PREFIX
            )
            .is_ok());
        for url in [
            "http://github.com/Ferdinand99/Inlet/releases/download/v1/a",
            "https://github.com/Ferdinand99/Inlet2/releases/download/v1/a",
            "https://github.com/other/Inlet/releases/download/v1/a",
            "https://example.com/Ferdinand99/Inlet/releases/download/v1/a",
        ] {
            assert!(source.check_url(url, DOWNLOAD_PREFIX).is_err(), "{url}");
        }
    }

    #[test]
    fn github_digests_must_agree_with_the_feed() {
        let a = good();
        assert!(digest_matches(&a, &None));
        assert!(digest_matches(
            &a,
            &Some(format!("sha256:{}", "AB".repeat(32)))
        ));
        assert!(!digest_matches(
            &a,
            &Some(format!("sha256:{}", "cd".repeat(32)))
        ));
        // A digest of another kind is not one GitHub gives for assets; it is ignored.
        assert!(digest_matches(&a, &Some("sha1:abc".into())));
    }

    // ---- ReleaseSource against a mock server (plain http, allowed only for the test) ----

    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn feed_json(assets: &[VelopackAsset]) -> String {
        let items: Vec<_> = assets
            .iter()
            .map(|a| {
                serde_json::json!({
                    "PackageId": a.PackageId, "Version": a.Version, "Type": a.Type,
                    "FileName": a.FileName, "SHA1": a.SHA1, "SHA256": a.SHA256,
                    "Size": a.Size, "NotesMarkdown": "", "NotesHtml": ""
                })
            })
            .collect();
        serde_json::json!({ "Assets": items }).to_string()
    }

    async fn serve(server: &MockServer, body: Vec<u8>, name: &str, status: u16) {
        Mock::given(method("GET"))
            .and(path(format!("/dl/{name}")))
            .respond_with(ResponseTemplate::new(status).set_body_bytes(body))
            .mount(server)
            .await;
    }

    /// Runs `f` on a thread that is not a tokio worker (the blocking client needs that).
    async fn off_runtime<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
        tokio::task::spawn_blocking(f).await.unwrap()
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn the_release_source_reads_the_feed_and_downloads_a_package() {
        let server = MockServer::start().await;
        let base = server.uri();
        let package = vec![7u8; 1000];
        let mut asset = good();
        asset.Size = package.len() as u64;
        asset.SHA256 = "ab".repeat(32);

        let list = serde_json::json!([
            // A draft is never used.
            {"draft": true, "assets": [{
                "name": "releases.beta-linux.json",
                "browser_download_url": format!("{base}/dl/draft.json")}]},
            {"draft": false, "assets": [
                {"name": "releases.beta-linux.json",
                 "browser_download_url": format!("{base}/dl/releases.beta-linux.json")},
                {"name": asset.FileName,
                 "browser_download_url": format!("{base}/dl/{}", asset.FileName),
                 "digest": format!("sha256:{}", asset.SHA256)},
            ]},
        ]);
        Mock::given(method("GET"))
            .and(path("/list"))
            .respond_with(ResponseTemplate::new(200).set_body_string(list.to_string()))
            .mount(&server)
            .await;
        serve(
            &server,
            feed_json(std::slice::from_ref(&asset)).into_bytes(),
            "releases.beta-linux.json",
            200,
        )
        .await;
        serve(&server, package.clone(), &asset.FileName, 200).await;

        let (list_url, prefix) = (format!("{base}/list"), format!("{base}/dl/"));
        let name = asset.FileName.clone();
        let asset2 = asset.clone();
        let (feed, file) = off_runtime(move || {
            let source = ReleaseSource::with_urls(&list_url, &prefix, false);
            let feed = source
                .get_release_feed("beta-linux", &manifest(), "")
                .unwrap();
            let dir = std::env::temp_dir().join(format!("inlet-update-dl-{}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            let target = dir.join("pkg.partial");
            source
                .download_release_entry(&asset2, &target, None)
                .unwrap();
            let file = std::fs::read(&target).unwrap();
            let _ = std::fs::remove_dir_all(&dir);
            (feed, file)
        })
        .await;
        assert_eq!(feed.Assets.len(), 1);
        assert_eq!(feed.Assets[0].FileName, name);
        assert_eq!(file, package);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_digest_that_differs_drops_the_package_and_a_big_download_stops() {
        let server = MockServer::start().await;
        let base = server.uri();
        let mut asset = good();
        asset.Size = 100;
        let list = serde_json::json!([{"draft": false, "assets": [
            {"name": "releases.beta-linux.json",
             "browser_download_url": format!("{base}/dl/releases.beta-linux.json")},
            {"name": asset.FileName,
             "browser_download_url": format!("{base}/dl/{}", asset.FileName),
             "digest": format!("sha256:{}", "cd".repeat(32))},
        ]}]);
        Mock::given(method("GET"))
            .and(path("/list"))
            .respond_with(ResponseTemplate::new(200).set_body_string(list.to_string()))
            .mount(&server)
            .await;
        serve(
            &server,
            feed_json(std::slice::from_ref(&asset)).into_bytes(),
            "releases.beta-linux.json",
            200,
        )
        .await;
        // The package is longer than the feed says.
        serve(&server, vec![1u8; 500], &asset.FileName, 200).await;

        let (list_url, prefix) = (format!("{base}/list"), format!("{base}/dl/"));
        off_runtime(move || {
            let source = ReleaseSource::with_urls(&list_url, &prefix, false);
            let feed = source
                .get_release_feed("beta-linux", &manifest(), "")
                .unwrap();
            assert!(feed.Assets.is_empty(), "GitHub's digest differs");

            // The same asset with a matching digest: the longer body is cut off.
            let mut a = good();
            a.Size = 100;
            source.found.lock().unwrap().insert(
                a.FileName.clone(),
                Found {
                    url: format!("{prefix}{}", a.FileName),
                    digest: None,
                },
            );
            let target =
                std::env::temp_dir().join(format!("inlet-update-big-{}", std::process::id()));
            let err = source
                .download_release_entry(&a, &target, None)
                .unwrap_err()
                .to_string();
            let _ = std::fs::remove_file(&target);
            assert!(
                err.contains("size differs") || err.contains("more data"),
                "{err}"
            );
        })
        .await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_failing_server_is_an_error_not_a_panic() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(503))
            .mount(&server)
            .await;
        let (list_url, prefix) = (
            format!("{}/list", server.uri()),
            format!("{}/dl/", server.uri()),
        );
        off_runtime(move || {
            let source = ReleaseSource::with_urls(&list_url, &prefix, false);
            assert!(source
                .get_release_feed("beta-linux", &manifest(), "")
                .is_err());
            assert!(source
                .get_release_feed("bad channel!", &manifest(), "")
                .is_err());
        })
        .await;
    }

    #[test]
    fn https_only_refuses_a_plain_address_even_for_the_list() {
        let source =
            ReleaseSource::with_urls("http://127.0.0.1:1/list", "http://127.0.0.1:1/dl/", true);
        assert!(source
            .get_release_feed("beta-linux", &manifest(), "")
            .is_err());
    }
}
