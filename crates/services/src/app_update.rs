//! New versions of Irori itself. Finds out how Irori was installed, asks
//! GitHub at most once a day whether a newer release is out, and remembers
//! the version the user dismissed and the version that ran last.
//!
//! Package managers' installs are updated by their package manager: Irori
//! points at its command. The installs nothing else updates (the AppImage,
//! the tarball, the macOS app, the Windows zips and setup) can download and
//! check the new release and put it in at the next restart
//! ([`crate::self_update`]); the release page stays offered too. Installs
//! that update with the system (Flatpak, distribution packages, Nix) are
//! never checked automatically or announced in popups; Flatpak and Linux
//! distribution packages still allow a manual check and show the detected
//! package manager's update command in Settings > About. Packagers who
//! build Irori themselves turn the check off by setting
//! `IRORI_UPDATE_EXPLANATION` when building (or in the environment); its text
//! is shown instead of checking.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::Result;
use irori_core::{json_store, util};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};

/// GitHub's newest published release: never a draft or a pre-release.
pub const RELEASES_API: &str = "https://api.github.com/repos/irori-app/irori/releases/latest";
const RELEASES_PAGE: &str = "https://github.com/irori-app/irori/releases/tag/v";
/// Seconds between two automatic checks.
const CHECK_EVERY: u64 = 24 * 60 * 60;

/// The distribution package manager that updates a Linux `/usr` install.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SystemPackageManager {
    Pacman,
    Apt,
    Dnf,
}

impl SystemPackageManager {
    /// The command that updates Irori through this package manager.
    pub fn update_command(self) -> &'static str {
        match self {
            Self::Pacman => "sudo pacman -Syu irori",
            Self::Apt => "sudo apt update && sudo apt install irori",
            Self::Dnf => "sudo dnf upgrade irori",
        }
    }

    /// Short display name for the install channel indicator.
    pub fn name(self) -> &'static str {
        match self {
            Self::Pacman => "pacman",
            Self::Apt => "apt",
            Self::Dnf => "dnf",
        }
    }
}

/// How this copy of Irori was installed, which decides who updates it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallKind {
    Flatpak,
    /// A distribution package under `/usr`: apt, dnf (Irori's repository
    /// or Fedora COPR), pacman (Irori's repository or the AUR).
    SystemPackage(Option<SystemPackageManager>),
    Nix,
    /// Run from a Cargo `target` folder.
    Source,
    /// A packager set `IRORI_UPDATE_EXPLANATION`; their text, untranslated.
    Packaged(String),
    Homebrew,
    Scoop,
    Winget,
    /// The tarball, a disk image, a zip or the Windows setup: the user
    /// updates by hand.
    Manual,
}

impl InstallKind {
    /// Look at this process.
    pub fn detect() -> Self {
        let exe = std::env::current_exe()
            .ok()
            .map(|p| std::fs::canonicalize(&p).unwrap_or(p))
            .unwrap_or_default();
        let is_system_usr = exe.starts_with("/usr") && !exe.starts_with("/usr/local");
        let system_pkg_manager = if cfg!(target_os = "linux") && is_system_usr {
            if Path::new("/run/ostree-booted").exists() {
                None
            } else {
                let os_release = std::fs::read_to_string("/etc/os-release").ok();
                detect_system_package_manager(os_release.as_deref(), |name| {
                    Path::new("/usr/bin").join(name).is_file()
                })
            }
        } else {
            None
        };
        let probe = Probe {
            explanation: option_env!("IRORI_UPDATE_EXPLANATION")
                .map(str::to_string)
                .or_else(|| std::env::var("IRORI_UPDATE_EXPLANATION").ok())
                .filter(|s| !s.trim().is_empty()),
            flatpak: std::env::var_os("FLATPAK_ID").is_some()
                || Path::new("/.flatpak-info").exists(),
            source_build: exe.ancestors().any(|a| {
                a.file_name().is_some_and(|n| n == "target")
                    && a.parent().is_some_and(|p| p.join("Cargo.toml").is_file())
            }),
            homebrew_cask: cfg!(target_os = "macos")
                && ["/opt/homebrew/Caskroom/irori", "/usr/local/Caskroom/irori"]
                    .iter()
                    .any(|p| Path::new(p).is_dir()),
            install_ini: exe
                .parent()
                .and_then(|dir| std::fs::read_to_string(dir.join("install.ini")).ok()),
            scoop_files: exe.parent().is_some_and(|dir| {
                dir.join("manifest.json").is_file() && dir.join("install.json").is_file()
            }),
            system_pkg_manager,
        };
        classify(&exe, &probe)
    }

    /// Whether Irori checks GitHub automatically and pops up update notices.
    pub fn checks(&self) -> bool {
        matches!(
            self,
            Self::Homebrew | Self::Scoop | Self::Winget | Self::Manual
        )
    }

    /// Whether the user can manually check GitHub for newer releases.
    pub fn can_check_manually(&self) -> bool {
        self.checks() || matches!(self, Self::Flatpak | Self::SystemPackage(_))
    }

    /// The command that updates Irori, for package managers the user runs.
    pub fn update_command(&self) -> Option<&'static str> {
        match self {
            Self::Flatpak => Some("flatpak update app.irori.Irori"),
            Self::SystemPackage(Some(pm)) => Some(pm.update_command()),
            Self::Homebrew => Some("brew upgrade --cask irori"),
            Self::Scoop => Some("scoop update irori"),
            Self::Winget => Some("winget upgrade Irori.Irori"),
            _ => None,
        }
    }
}

/// Detect the Linux distribution's package manager from `/etc/os-release`
/// (with a fallback to checking package manager binaries under `/usr/bin`).
fn detect_system_package_manager(
    os_release: Option<&str>,
    has_bin: impl Fn(&str) -> bool,
) -> Option<SystemPackageManager> {
    if let Some(content) = os_release {
        let value = |key: &str| {
            content.lines().find_map(|l| {
                let v = l.trim().strip_prefix(key)?.strip_prefix('=')?;
                Some(v.trim().trim_matches(['"', '\'']).to_ascii_lowercase())
            })
        };
        let mut ids: Vec<String> = Vec::new();
        for v in [value("ID"), value("ID_LIKE"), value("VARIANT_ID")]
            .into_iter()
            .flatten()
        {
            for id in v.split_whitespace() {
                if !ids.iter().any(|i| i == id) {
                    ids.push(id.to_string());
                }
            }
        }
        const IMAGE_BASED: &[&str] = &[
            "steamos",
            "bazzite",
            "silverblue",
            "kinoite",
            "sericea",
            "onyx",
            "aurora",
            "bluefin",
            "nixos",
        ];
        if ids
            .iter()
            .any(|i| IMAGE_BASED.contains(&i.as_str()) || i.ends_with("-atomic"))
        {
            return None;
        }
        if let Some(pm) = ids.iter().find_map(|id| match id.as_str() {
            "arch" | "manjaro" | "endeavouros" | "cachyos" | "garuda" => {
                Some(SystemPackageManager::Pacman)
            }
            "debian" | "ubuntu" | "pop" | "linuxmint" | "elementary" | "zorin" => {
                Some(SystemPackageManager::Apt)
            }
            "fedora" | "rhel" | "centos" | "rocky" | "almalinux" | "nobara" | "ultramarine" => {
                Some(SystemPackageManager::Dnf)
            }
            _ => None,
        }) {
            return Some(pm);
        }
    }
    if has_bin("pacman") {
        Some(SystemPackageManager::Pacman)
    } else if has_bin("apt") {
        Some(SystemPackageManager::Apt)
    } else if has_bin("dnf") {
        Some(SystemPackageManager::Dnf)
    } else {
        None
    }
}

/// What [`classify`] needs to know besides the executable's path.
#[derive(Debug, Default)]
struct Probe {
    explanation: Option<String>,
    flatpak: bool,
    source_build: bool,
    homebrew_cask: bool,
    /// `install.ini` beside the executable, written by the Windows setup.
    install_ini: Option<String>,
    /// Scoop's `manifest.json` and `install.json` beside the executable,
    /// which every Scoop app version folder has, wherever `SCOOP` puts it.
    scoop_files: bool,
    /// Detected distribution package manager on Linux (`/usr` installs).
    system_pkg_manager: Option<SystemPackageManager>,
}

fn classify(exe: &Path, probe: &Probe) -> InstallKind {
    if let Some(text) = &probe.explanation {
        return InstallKind::Packaged(text.clone());
    }
    if probe.flatpak {
        return InstallKind::Flatpak;
    }
    if probe.source_build {
        return InstallKind::Source;
    }
    let path = exe.to_string_lossy().replace('\\', "/").to_lowercase();
    if path.starts_with("/nix/store/") {
        return InstallKind::Nix;
    }
    if path.starts_with("/usr/") && !path.starts_with("/usr/local/") {
        return InstallKind::SystemPackage(probe.system_pkg_manager);
    }
    if probe.homebrew_cask {
        return InstallKind::Homebrew;
    }
    if path.contains("/scoop/apps/irori/") || (probe.scoop_files && path.contains("/apps/irori/")) {
        return InstallKind::Scoop;
    }
    if probe
        .install_ini
        .as_deref()
        .and_then(ini_channel)
        .is_some_and(|c| c.eq_ignore_ascii_case("winget"))
    {
        return InstallKind::Winget;
    }
    InstallKind::Manual
}

/// `Channel=` of the Windows setup's `install.ini`.
fn ini_channel(ini: &str) -> Option<&str> {
    ini.lines().find_map(|line| {
        let (key, value) = line.split_once('=')?;
        key.trim()
            .eq_ignore_ascii_case("channel")
            .then(|| value.trim())
    })
}

/// A published version of Irori.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Release {
    /// `X.Y.Z`, without the tag's `v`.
    pub version: String,
    /// The release's page.
    pub url: String,
}

/// Where installing an update by Irori itself stands.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum SelfInstall {
    /// This install is updated another way, or no package is built for it.
    #[default]
    Unavailable,
    /// Irori can install the update.
    Idle,
    Downloading {
        percent: Option<u8>,
    },
    /// Checked and unpacked: it goes in at the next restart.
    Ready {
        version: String,
    },
    Failed {
        error: String,
    },
    /// The update to `version` didn't start, so this version came back.
    RolledBack {
        version: String,
    },
}

/// What the UI shows about Irori's own updates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppUpdateStatus {
    /// The running version.
    pub current: String,
    pub kind: InstallKind,
    /// Package format for self-updatable manual installs (AppImage, tarball, etc.).
    pub self_update_channel: Option<crate::self_update::SelfUpdateChannel>,
    /// A release newer than this one, from the last check.
    pub available: Option<Release>,
    /// The user closed the notice about `available`.
    pub dismissed: bool,
    /// Tell the user about `available` now: the check is on, this install
    /// checks, and they haven't dismissed it.
    pub announce: bool,
    /// Irori runs a newer version than last time.
    pub updated: bool,
    pub checking: bool,
    /// When the last check answered, in Unix seconds.
    pub checked_at: Option<u64>,
    /// Installing `available` from Irori.
    pub install: SelfInstall,
}

impl AppUpdateStatus {
    /// Whether a newer release of Irori is available or staged for restart.
    pub fn has_update(&self) -> bool {
        (self.kind.can_check_manually() && self.available.is_some())
            || matches!(self.install, SelfInstall::Ready { .. })
    }
}

/// Saved in `state/app-update.json`.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct Saved {
    checked_at: u64,
    latest: Option<Release>,
    dismissed: Option<String>,
    last_run_version: Option<String>,
}

pub(crate) struct AppUpdater {
    endpoint: String,
    file: PathBuf,
    current: String,
    kind: InstallKind,
    saved: RwLock<Saved>,
    updated: bool,
    checking: AtomicBool,
    restarting: AtomicBool,
    /// How this install is laid out, when Irori can update it itself.
    layout: Option<crate::self_update::Layout>,
    install: RwLock<SelfInstall>,
    /// The staged update's record (`state/app-update-staged.json`).
    staged: PathBuf,
}

impl AppUpdater {
    /// `app` is true for the app window: only it records the version that
    /// ran, so a command without a window doesn't swallow the "updated" note.
    pub(crate) fn load(app: bool) -> Self {
        let kind = InstallKind::detect();
        let layout = (kind == InstallKind::Manual)
            .then(crate::self_update::detect)
            .flatten();
        let mut updater = Self::with(
            RELEASES_API,
            irori_core::Paths::get().state_dir().join("app-update.json"),
            env!("CARGO_PKG_VERSION"),
            kind,
            app,
        );
        updater.layout = layout;
        updater
    }

    /// Read the saved state, and with `record_run` record this version as
    /// the one that ran.
    fn with(
        endpoint: &str,
        file: PathBuf,
        current: &str,
        kind: InstallKind,
        record_run: bool,
    ) -> Self {
        let mut saved: Saved = json_store::read_or_default(&file);
        // The first start of all has nothing to compare with, and a source
        // build has no release notes.
        let updated = record_run
            && kind != InstallKind::Source
            && saved
                .last_run_version
                .as_deref()
                .is_some_and(|last| newer(current, last));
        if record_run && saved.last_run_version.as_deref() != Some(current) {
            saved.last_run_version = Some(current.to_string());
            if let Err(e) = json_store::write(&file, &saved) {
                tracing::warn!("saving the update state: {e:#}");
            }
        }
        let staged = file.with_file_name("app-update-staged.json");
        // Only the app settles what the restart left: a command without a
        // window may run beside it.
        let install = if record_run {
            settle_staged(&staged, current)
        } else {
            SelfInstall::Idle
        };
        Self {
            endpoint: endpoint.to_string(),
            file,
            current: current.to_string(),
            kind,
            saved: RwLock::new(saved),
            updated,
            checking: AtomicBool::new(false),
            restarting: AtomicBool::new(false),
            layout: None,
            install: RwLock::new(install),
            staged,
        }
    }

    pub(crate) fn layout(&self) -> Option<&crate::self_update::Layout> {
        self.layout.as_ref()
    }

    pub(crate) fn staged_record(&self) -> &Path {
        &self.staged
    }

    pub(crate) fn current(&self) -> &str {
        &self.current
    }

    pub(crate) fn try_begin_install(&self) -> bool {
        let mut guard = self.install.write();
        if matches!(*guard, SelfInstall::Downloading { .. }) {
            return false;
        }
        *guard = SelfInstall::Downloading { percent: None };
        true
    }

    pub(crate) fn try_begin_restart(&self) -> bool {
        self.restarting
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }

    pub(crate) fn finish_restart_failed(&self) {
        self.restarting.store(false, Ordering::Release);
    }

    pub(crate) fn set_install(&self, state: SelfInstall) {
        *self.install.write() = state;
    }

    pub(crate) fn kind(&self) -> &InstallKind {
        &self.kind
    }

    pub(crate) fn status(&self, enabled: bool) -> AppUpdateStatus {
        let saved = self.saved.read();
        let available = self
            .kind
            .can_check_manually()
            .then(|| saved.latest.clone())
            .flatten()
            .filter(|r| newer(&r.version, &self.current));
        let dismissed = available
            .as_ref()
            .is_some_and(|r| saved.dismissed.as_deref() == Some(r.version.as_str()));
        AppUpdateStatus {
            current: self.current.clone(),
            kind: self.kind.clone(),
            self_update_channel: self
                .layout
                .as_ref()
                .map(crate::self_update::Layout::channel),
            announce: available.is_some() && !dismissed && enabled && self.kind.checks(),
            available,
            dismissed,
            updated: self.updated,
            checking: self.checking.load(Ordering::Relaxed),
            checked_at: (saved.checked_at > 0).then_some(saved.checked_at),
            install: if self.layout.is_some() {
                self.install.read().clone()
            } else {
                SelfInstall::Unavailable
            },
        }
    }

    /// The last check is older than a day.
    pub(crate) fn due(&self, now: u64) -> bool {
        now.saturating_sub(self.saved.read().checked_at) >= CHECK_EVERY
    }

    pub(crate) fn set_checking(&self, checking: bool) {
        self.checking.store(checking, Ordering::Relaxed);
    }

    /// Ask GitHub for the newest release and save the answer.
    pub(crate) async fn check(&self, check: irori_core::github_releases::CheckKind) -> Result<()> {
        let has_cached = self.saved.read().latest.is_some();
        let fetched = fetch(&self.endpoint, has_cached, check).await?;
        let mut saved = self.saved.read().clone();
        saved.checked_at = util::unix_now();
        if let Some(latest) = fetched {
            saved.latest = latest;
        }
        if let Err(e) = json_store::write(&self.file, &saved) {
            tracing::warn!("saving the update state: {e:#}");
        }
        *self.saved.write() = saved;
        Ok(())
    }

    /// Stop announcing `version`.
    pub(crate) fn dismiss(&self, version: &str) {
        let mut saved = self.saved.read().clone();
        if saved.dismissed.as_deref() == Some(version) {
            return;
        }
        saved.dismissed = Some(version.to_string());
        if let Err(e) = json_store::write(&self.file, &saved) {
            tracing::warn!("saving the update state: {e:#}");
        }
        *self.saved.write() = saved;
    }
}

#[derive(Deserialize)]
struct GhRelease {
    tag_name: String,
    #[serde(default)]
    html_url: Option<String>,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
}

/// Turn a non-success GitHub API response into a clean, user-facing error
/// without leaking raw `reqwest` URL details.
pub(crate) fn github_http_error(
    status: u16,
    headers: &reqwest::header::HeaderMap,
    body: Option<&str>,
    not_found: Option<&str>,
    now_unix: u64,
) -> anyhow::Error {
    let header_str = |name: &str| {
        headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(str::trim)
    };
    let remaining_zero = header_str("x-ratelimit-remaining") == Some("0");
    let retry_after_raw = header_str("retry-after").filter(|s| !s.is_empty());
    let retry_after = retry_after_raw.and_then(|s| {
        s.parse::<u64>().ok().or_else(|| {
            chrono::DateTime::parse_from_rfc2822(s)
                .ok()
                .map(|dt| (dt.timestamp().max(0) as u64).saturating_sub(now_unix))
        })
    });
    let reset_secs = header_str("x-ratelimit-reset")
        .and_then(|s| s.parse::<u64>().ok())
        .map(|reset| reset.saturating_sub(now_unix));
    let body_mentions_rate_limit =
        body.is_some_and(|b| b.to_ascii_lowercase().contains("rate limit"));
    let is_rate_limit = status == 429
        || (status == 403
            && (remaining_zero || retry_after_raw.is_some() || body_mentions_rate_limit));
    if is_rate_limit {
        let wait_secs = retry_after
            .filter(|&s| s > 0)
            .or_else(|| reset_secs.filter(|&s| s > 0));
        if let Some(secs) = wait_secs {
            let minutes = secs.div_ceil(60).max(1);
            let unit = if minutes == 1 { "minute" } else { "minutes" };
            return anyhow::anyhow!(
                "GitHub's rate limit was reached. Try again in about {minutes} {unit}, or download it from the release page."
            );
        }
        return anyhow::anyhow!(
            "GitHub's rate limit was reached. Try again later, or download it from the release page."
        );
    }
    if status == 404
        && let Some(msg) = not_found
    {
        return anyhow::anyhow!("{msg}");
    }
    anyhow::anyhow!("GitHub returned HTTP {status}")
}

/// The newest stable release: `Ok(None)` when the shared Atom feed or ETag
/// confirmed nothing changed since the cached check, `Ok(Some(None))` while
/// there is no published release, or `Ok(Some(Some(release)))` when updated.
async fn fetch(
    endpoint: &str,
    has_cached: bool,
    check: irori_core::github_releases::CheckKind,
) -> Result<Option<Option<Release>>> {
    let body = if irori_core::github_releases::is_github(endpoint) {
        match irori_core::github_releases::fetch_guarded_json(
            endpoint,
            "app-update:irori",
            has_cached,
            check,
        )
        .await
        {
            Ok(irori_core::github_releases::GuardedFetch::Unchanged) => return Ok(None),
            Ok(irori_core::github_releases::GuardedFetch::NotFound) => return Ok(Some(None)),
            Ok(irori_core::github_releases::GuardedFetch::Updated(body)) => body,
            Err(e) if irori_core::github_releases::rate_limited(&e).is_some() => return Err(e),
            Err(e) => {
                tracing::debug!("reaching GitHub ({endpoint}): {e:#}");
                if e.chain()
                    .any(|c| c.downcast_ref::<reqwest::Error>().is_some())
                {
                    return Err(anyhow::anyhow!("Could not reach GitHub"));
                }
                return Err(e);
            }
        }
    } else {
        let resp = irori_core::http::client()
            .get(endpoint)
            .header("Accept", "application/vnd.github+json")
            .send()
            .await
            .map_err(|e| {
                tracing::debug!("reaching GitHub ({endpoint}): {e:#}");
                anyhow::anyhow!("Could not reach GitHub")
            })?;
        let status = resp.status().as_u16();
        // GitHub answers 404 while a repository has no published release.
        if status == 404 {
            return Ok(Some(None));
        }
        if !resp.status().is_success() {
            let headers = resp.headers().clone();
            let body = if status == 403 {
                resp.text().await.ok()
            } else {
                None
            };
            return Err(github_http_error(
                status,
                &headers,
                body.as_deref(),
                None,
                util::unix_now(),
            ));
        }
        resp.text().await.map_err(|e| {
            tracing::debug!("reading GitHub release response: {e:#}");
            anyhow::anyhow!("Unexpected answer from GitHub")
        })?
    };
    let release: GhRelease = serde_json::from_str(&body).map_err(|e| {
        tracing::debug!("parsing GitHub release JSON: {e:#}");
        anyhow::anyhow!("Unexpected answer from GitHub")
    })?;
    Ok(Some(parse(release)))
}

fn parse(release: GhRelease) -> Option<Release> {
    if release.draft || release.prerelease {
        return None;
    }
    let version = release.tag_name.strip_prefix('v')?;
    parse_version(version)?;
    Some(Release {
        version: version.to_string(),
        url: release
            .html_url
            .filter(|u| u.starts_with("https://github.com/"))
            .unwrap_or_else(|| release_page(version)),
    })
}

/// What the last restart left of a staged update: the update is in (its
/// leftovers go), it didn't start and this version came back, or it still
/// waits for a restart.
fn settle_staged(record: &Path, current: &str) -> SelfInstall {
    let Some(staged) = std::fs::read(record)
        .ok()
        .and_then(|b| serde_json::from_slice::<crate::self_update::Staged>(&b).ok())
    else {
        return SelfInstall::Idle;
    };
    let waiting = !staged.rolled_back
        && newer(&staged.version, current)
        && staged.swaps.iter().all(|s| s.new.exists())
        && staged.installer.as_ref().is_none_or(|i| i.exists())
        && (staged.installer.is_some() || !staged.swaps.is_empty());
    if waiting {
        return SelfInstall::Ready {
            version: staged.version,
        };
    }
    crate::self_update::discard_stage(&staged.stage);
    let _ = std::fs::remove_file(record);
    if staged.rolled_back {
        SelfInstall::RolledBack {
            version: staged.version,
        }
    } else {
        SelfInstall::Idle
    }
}

/// The release page of `version`, with its notes.
pub fn release_page(version: &str) -> String {
    format!("{RELEASES_PAGE}{version}")
}

/// `X.Y.Z` only: tags with anything else (`-beta.1`, `nightly`) are never
/// offered.
fn parse_version(v: &str) -> Option<(u64, u64, u64)> {
    let mut parts = v.split('.').map(|p| {
        (!p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
            .then(|| p.parse().ok())
            .flatten()
    });
    let version = (parts.next()??, parts.next()??, parts.next()??);
    parts.next().is_none().then_some(version)
}

/// `a` is a later version than `b`. Unreadable versions never are.
fn newer(a: &str, b: &str) -> bool {
    match (parse_version(a), parse_version(b)) {
        (Some(a), Some(b)) => a > b,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use irori_depot::test_server::{Resp, TestServer};
    use serde_json::json;

    fn exe(path: &str) -> PathBuf {
        PathBuf::from(path)
    }

    #[test]
    fn install_kinds() {
        let none = Probe::default();
        let kind = |path: &str, probe: &Probe| classify(&exe(path), probe);
        assert_eq!(
            kind("/usr/bin/irori", &none),
            InstallKind::SystemPackage(None)
        );
        assert_eq!(
            kind(
                "/usr/bin/irori",
                &Probe {
                    system_pkg_manager: Some(SystemPackageManager::Pacman),
                    ..Probe::default()
                }
            ),
            InstallKind::SystemPackage(Some(SystemPackageManager::Pacman))
        );
        assert_eq!(kind("/usr/local/bin/irori", &none), InstallKind::Manual);
        assert_eq!(kind("/home/a/.local/bin/irori", &none), InstallKind::Manual);
        assert_eq!(
            kind("/nix/store/abc-irori-0.2.0/bin/irori", &none),
            InstallKind::Nix
        );
        assert_eq!(
            kind(
                "/Applications/Irori.app/Contents/MacOS/irori",
                &Probe {
                    homebrew_cask: true,
                    ..Probe::default()
                }
            ),
            InstallKind::Homebrew
        );
        assert_eq!(
            kind(r"C:\Users\a\scoop\apps\irori\current\irori.exe", &none),
            InstallKind::Scoop
        );
        let setup = |channel: &str| Probe {
            install_ini: Some(format!("[Install]\r\nChannel={channel}\r\n")),
            ..Probe::default()
        };
        let programs = r"C:\Users\a\AppData\Local\Programs\Irori\irori.exe";
        assert_eq!(kind(programs, &setup("winget")), InstallKind::Winget);
        assert_eq!(kind(programs, &setup("setup")), InstallKind::Manual);
        assert_eq!(
            kind(
                "/usr/bin/irori",
                &Probe {
                    flatpak: true,
                    ..Probe::default()
                }
            ),
            InstallKind::Flatpak
        );
        assert_eq!(
            kind(
                "/home/a/irori/target/release/irori",
                &Probe {
                    source_build: true,
                    ..Probe::default()
                }
            ),
            InstallKind::Source
        );
        // A packager's word wins over every guess.
        assert_eq!(
            kind(
                "/usr/bin/irori",
                &Probe {
                    explanation: Some("Update with your distribution.".into()),
                    flatpak: true,
                    ..Probe::default()
                }
            ),
            InstallKind::Packaged("Update with your distribution.".into())
        );
        assert!(InstallKind::Manual.checks() && InstallKind::Winget.checks());
        assert!(!InstallKind::Flatpak.checks() && !InstallKind::Source.checks());
        assert!(InstallKind::Flatpak.can_check_manually());
        assert!(
            InstallKind::SystemPackage(Some(SystemPackageManager::Pacman)).can_check_manually()
        );
        assert!(!InstallKind::Source.can_check_manually());
        assert!(!InstallKind::Nix.can_check_manually());
        assert!(!InstallKind::Packaged("custom".into()).can_check_manually());
    }

    #[test]
    fn system_package_manager_detection() {
        let no_bins = |_: &str| false;
        assert_eq!(
            detect_system_package_manager(Some("ID=arch\n"), no_bins),
            Some(SystemPackageManager::Pacman)
        );
        assert_eq!(
            detect_system_package_manager(Some("ID=cachyos\nID_LIKE=arch\n"), no_bins),
            Some(SystemPackageManager::Pacman)
        );
        assert_eq!(
            detect_system_package_manager(Some("ID=ubuntu\nID_LIKE=debian\n"), no_bins),
            Some(SystemPackageManager::Apt)
        );
        assert_eq!(
            detect_system_package_manager(Some("ID=\"fedora\"\n"), no_bins),
            Some(SystemPackageManager::Dnf)
        );
        // Image-based distros do not get a mutable package manager command,
        // including Fedora Atomic variants identified via VARIANT_ID.
        assert_eq!(
            detect_system_package_manager(Some("ID=bazzite\nID_LIKE=fedora\n"), |b| b == "dnf"),
            None
        );
        assert_eq!(
            detect_system_package_manager(Some("ID=steamos\nID_LIKE=arch\n"), |b| b == "pacman"),
            None
        );
        assert_eq!(
            detect_system_package_manager(Some("ID=fedora\nVARIANT_ID=silverblue\n"), |b| b
                == "dnf"),
            None
        );
        assert_eq!(
            detect_system_package_manager(Some("ID=fedora\nVARIANT_ID=kinoite\n"), |b| b == "dnf"),
            None
        );
        assert_eq!(
            detect_system_package_manager(Some("ID=fedora\nVARIANT_ID=sway-atomic\n"), |b| b
                == "dnf"),
            None
        );
        // Fallback to binary probe when os-release is missing or unknown.
        assert_eq!(
            detect_system_package_manager(None, |b| b == "pacman"),
            Some(SystemPackageManager::Pacman)
        );
        assert_eq!(
            detect_system_package_manager(Some("ID=unknown\n"), |b| b == "apt"),
            Some(SystemPackageManager::Apt)
        );
        assert_eq!(
            detect_system_package_manager(Some("ID=unknown\n"), |b| b == "dnf"),
            Some(SystemPackageManager::Dnf)
        );
        assert_eq!(detect_system_package_manager(None, no_bins), None);
    }

    /// Each package channel Irori publishes to is told apart, and updates go
    /// through its package manager: Irori never checks automatically for
    /// system-managed installs, while still providing copyable commands and
    /// manual checks for Flatpak and detected Linux package managers.
    #[test]
    fn package_channels_defer_to_their_package_manager() {
        let none = Probe::default();
        let kind = |path: &str, probe: &Probe| classify(&exe(path), probe);
        // Unknown distro under /usr: never auto-checks, no guessed command.
        let system = kind("/usr/bin/irori", &none);
        assert_eq!(system, InstallKind::SystemPackage(None));
        assert!(!system.checks());
        assert!(system.can_check_manually());
        assert_eq!(system.update_command(), None);
        // Detected Linux package managers: no auto-check, copyable command.
        for (pm, expected_cmd) in [
            (SystemPackageManager::Pacman, "sudo pacman -Syu irori"),
            (
                SystemPackageManager::Apt,
                "sudo apt update && sudo apt install irori",
            ),
            (SystemPackageManager::Dnf, "sudo dnf upgrade irori"),
        ] {
            let k = kind(
                "/usr/bin/irori",
                &Probe {
                    system_pkg_manager: Some(pm),
                    ..Probe::default()
                },
            );
            assert_eq!(k, InstallKind::SystemPackage(Some(pm)));
            assert!(!k.checks());
            assert!(k.can_check_manually());
            assert_eq!(k.update_command(), Some(expected_cmd));
        }
        // Flathub: Flatpak updates it; no auto-check, copyable command.
        let flathub = kind(
            "/app/bin/irori",
            &Probe {
                flatpak: true,
                ..Probe::default()
            },
        );
        assert_eq!(flathub, InstallKind::Flatpak);
        assert!(!flathub.checks());
        assert!(flathub.can_check_manually());
        assert_eq!(
            flathub.update_command(),
            Some("flatpak update app.irori.Irori")
        );
        // Scoop, in its default root and in one `SCOOP` moved elsewhere
        // (`current` is resolved to the version folder).
        let scoop_files = Probe {
            scoop_files: true,
            ..Probe::default()
        };
        for scoop in [
            kind(r"C:\Users\a\scoop\apps\irori\1.2.3\irori.exe", &none),
            kind(r"D:\Tools\apps\irori\1.2.3\irori.exe", &scoop_files),
        ] {
            assert_eq!(scoop, InstallKind::Scoop);
            assert!(scoop.checks());
            assert_eq!(scoop.update_command(), Some("scoop update irori"));
        }
        // Without Scoop's files a folder named `apps` is just a folder.
        assert_eq!(
            kind(r"D:\Tools\apps\irori\irori.exe", &none),
            InstallKind::Manual
        );
        // winget, through the setup's switch in its manifest.
        let winget = kind(
            r"C:\Users\a\AppData\Local\Programs\Irori\irori.exe",
            &Probe {
                install_ini: Some("[Install]\r\nChannel=winget\r\n".into()),
                ..Probe::default()
            },
        );
        assert_eq!(winget, InstallKind::Winget);
        assert_eq!(winget.update_command(), Some("winget upgrade Irori.Irori"));
    }

    #[test]
    fn versions_compare_numerically() {
        assert!(newer("0.10.0", "0.9.9"));
        assert!(newer("1.0.0", "0.99.0"));
        assert!(!newer("0.2.0", "0.2.0"));
        assert!(!newer("0.3.0-beta.1", "0.2.0"));
        assert!(!newer("0.3", "0.2.0"));
        assert!(!newer("0.3.0.1", "0.2.0"));
    }

    #[test]
    fn only_stable_tags_are_releases() {
        let gh = |tag: &str, prerelease: bool, draft: bool| GhRelease {
            tag_name: tag.into(),
            html_url: Some(format!(
                "https://github.com/irori-app/irori/releases/tag/{tag}"
            )),
            draft,
            prerelease,
        };
        assert_eq!(
            parse(gh("v0.3.0", false, false)),
            Some(Release {
                version: "0.3.0".into(),
                url: "https://github.com/irori-app/irori/releases/tag/v0.3.0".into(),
            })
        );
        assert_eq!(parse(gh("v0.3.0", true, false)), None);
        assert_eq!(parse(gh("v0.3.0", false, true)), None);
        assert_eq!(parse(gh("v0.3.0-rc.1", false, false)), None);
        assert_eq!(parse(gh("nightly", false, false)), None);
        // A page outside GitHub is never opened.
        let mut odd = gh("v0.3.0", false, false);
        odd.html_url = Some("https://example.com/".into());
        assert_eq!(parse(odd).unwrap().url, release_page("0.3.0"));
    }

    fn release_server(tag: &'static str) -> TestServer {
        TestServer::start(move |_| {
            Resp::json(json!({
                "tag_name": tag,
                "html_url": format!("https://github.com/irori-app/irori/releases/tag/{tag}"),
                "draft": false,
                "prerelease": false,
            }))
        })
    }

    #[tokio::test]
    async fn announces_a_newer_release_until_dismissed() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("app-update.json");
        let server = release_server("v0.3.0");
        let endpoint = server.url("/releases/latest");
        let updater = AppUpdater::with(&endpoint, file.clone(), "0.2.0", InstallKind::Manual, true);
        assert!(updater.due(util::unix_now()));
        updater
            .check(irori_core::github_releases::CheckKind::Manual)
            .await
            .unwrap();
        assert!(!updater.due(util::unix_now()));
        let status = updater.status(true);
        assert_eq!(status.available.as_ref().unwrap().version, "0.3.0");
        assert!(status.announce && !status.dismissed);
        assert!(!updater.status(false).announce, "the setting is off");

        updater.dismiss("0.3.0");
        let status = updater.status(true);
        assert!(status.dismissed && !status.announce);
        assert!(status.available.is_some(), "Settings still shows it");

        // The answer and the dismissal survive a restart, without asking
        // GitHub again within the day.
        let again = AppUpdater::with(&endpoint, file, "0.2.0", InstallKind::Manual, true);
        assert!(!again.due(util::unix_now()));
        assert!(again.status(true).dismissed);
        assert_eq!(server.requests().len(), 1);
    }

    #[tokio::test]
    async fn the_same_or_an_older_release_is_not_an_update() {
        let dir = tempfile::tempdir().unwrap();
        let server = release_server("v0.2.0");
        let updater = AppUpdater::with(
            &server.url("/releases/latest"),
            dir.path().join("app-update.json"),
            "0.2.0",
            InstallKind::Manual,
            true,
        );
        updater
            .check(irori_core::github_releases::CheckKind::Manual)
            .await
            .unwrap();
        let status = updater.status(true);
        assert!(status.available.is_none() && !status.announce);
        assert!(status.checked_at.is_some());
    }

    #[tokio::test]
    async fn no_release_and_errors() {
        let dir = tempfile::tempdir().unwrap();
        let missing = TestServer::start(|_| Resp::status(404));
        let updater = AppUpdater::with(
            &missing.url("/releases/latest"),
            dir.path().join("a.json"),
            "0.2.0",
            InstallKind::Manual,
            true,
        );
        updater
            .check(irori_core::github_releases::CheckKind::Manual)
            .await
            .unwrap();
        assert!(updater.status(true).available.is_none());

        let broken = TestServer::start(|_| Resp::status(503));
        let updater = AppUpdater::with(
            &broken.url("/releases/latest"),
            dir.path().join("b.json"),
            "0.2.0",
            InstallKind::Manual,
            true,
        );
        assert!(
            updater
                .check(irori_core::github_releases::CheckKind::Manual)
                .await
                .is_err()
        );
        assert!(
            updater.due(util::unix_now()),
            "a failed check doesn't count"
        );
    }

    #[test]
    fn notices_a_new_version_once() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("app-update.json");
        let first = AppUpdater::with(
            RELEASES_API,
            file.clone(),
            "0.2.0",
            InstallKind::Manual,
            true,
        );
        assert!(!first.status(true).updated, "nothing to compare with");
        let same = AppUpdater::with(
            RELEASES_API,
            file.clone(),
            "0.2.0",
            InstallKind::Manual,
            true,
        );
        assert!(!same.status(true).updated);
        let newer = AppUpdater::with(
            RELEASES_API,
            file.clone(),
            "0.3.0",
            InstallKind::Manual,
            true,
        );
        assert!(newer.status(true).updated);
        let after = AppUpdater::with(
            RELEASES_API,
            file.clone(),
            "0.3.0",
            InstallKind::Manual,
            true,
        );
        assert!(!after.status(true).updated);
        // A command without a window neither notes nor records a version.
        let cli = AppUpdater::with(
            RELEASES_API,
            file.clone(),
            "0.4.0",
            InstallKind::Manual,
            false,
        );
        assert!(!cli.status(true).updated);
        let app = AppUpdater::with(RELEASES_API, file, "0.4.0", InstallKind::Manual, true);
        assert!(app.status(true).updated);
    }

    #[tokio::test]
    async fn manual_check_on_flatpak_and_system_package_never_announces() {
        let dir = tempfile::tempdir().unwrap();
        let server = release_server("v1.3.0");
        let endpoint = server.url("/releases/latest");
        for (name, kind) in [
            ("flatpak.json", InstallKind::Flatpak),
            (
                "pacman.json",
                InstallKind::SystemPackage(Some(SystemPackageManager::Pacman)),
            ),
        ] {
            let updater = AppUpdater::with(&endpoint, dir.path().join(name), "1.2.0", kind, true);
            updater
                .check(irori_core::github_releases::CheckKind::Manual)
                .await
                .unwrap();
            let status = updater.status(true);
            assert_eq!(status.available.as_ref().unwrap().version, "1.3.0");
            assert!(status.has_update());
            assert!(!status.announce, "system-managed installs must not pop up");
            assert!(status.checked_at.is_some());
        }
    }

    #[tokio::test]
    async fn non_checkable_installs_ignore_stale_cached_releases() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("app-update.json");
        let server = release_server("v1.5.0");
        let manual = AppUpdater::with(
            &server.url("/releases/latest"),
            file.clone(),
            "1.2.0",
            InstallKind::Manual,
            true,
        );
        manual
            .check(irori_core::github_releases::CheckKind::Manual)
            .await
            .unwrap();
        assert!(manual.status(true).has_update());

        for kind in [
            InstallKind::Nix,
            InstallKind::Source,
            InstallKind::Packaged("Managed externally.".into()),
        ] {
            let updater = AppUpdater::with(RELEASES_API, file.clone(), "1.2.0", kind, true);
            let status = updater.status(true);
            assert!(
                status.available.is_none(),
                "non-checkable install must not surface cached releases"
            );
            assert!(!status.has_update());
            assert!(!status.announce);
        }
    }

    #[test]
    fn restart_guard_blocks_concurrent_attempts_and_resets_on_failure() {
        let dir = tempfile::tempdir().unwrap();
        let updater = AppUpdater::with(
            RELEASES_API,
            dir.path().join("app-update.json"),
            "1.2.0",
            InstallKind::Manual,
            false,
        );
        assert!(updater.try_begin_restart());
        assert!(!updater.try_begin_restart());
        updater.finish_restart_failed();
        assert!(updater.try_begin_restart());
    }

    #[tokio::test]
    async fn formats_rate_limit_errors_cleanly() {
        let manual = irori_core::github_releases::CheckKind::Manual;
        let reset = util::unix_now() + 185;
        let with_reset = TestServer::start(move |_| Resp {
            status: 403,
            headers: vec![
                ("x-ratelimit-remaining".into(), "0".into()),
                ("x-ratelimit-reset".into(), reset.to_string()),
            ],
            ..Default::default()
        });
        let err = fetch(&with_reset.url("/releases/latest"), false, manual)
            .await
            .unwrap_err();
        let text = format!("{err:#}");
        assert!(
            text.contains("GitHub's rate limit was reached") && text.contains("minutes"),
            "unexpected rate limit message: {text}"
        );
        assert!(
            !text.contains("http://") && !text.contains("https://"),
            "error must not leak raw request URL: {text}"
        );

        let without_reset = TestServer::start(|_| Resp {
            status: 403,
            headers: vec![("x-ratelimit-remaining".into(), "0".into())],
            ..Default::default()
        });
        let err = fetch(&without_reset.url("/releases/latest"), false, manual)
            .await
            .unwrap_err();
        let text = format!("{err:#}");
        assert_eq!(
            text,
            "GitHub's rate limit was reached. Try again later, or download it from the release page."
        );

        let too_many = TestServer::start(|_| Resp {
            status: 429,
            headers: vec![("retry-after".into(), "45".into())],
            ..Default::default()
        });
        let err = fetch(&too_many.url("/releases/latest"), false, manual)
            .await
            .unwrap_err();
        assert_eq!(
            format!("{err:#}"),
            "GitHub's rate limit was reached. Try again in about 1 minute, or download it from the release page."
        );

        let server_error = TestServer::start(|_| Resp::status(503));
        let err = fetch(&server_error.url("/releases/latest"), false, manual)
            .await
            .unwrap_err();
        assert_eq!(format!("{err:#}"), "GitHub returned HTTP 503");

        // 403 without rate-limit headers or body hint is reported cleanly as HTTP 403.
        let forbidden = TestServer::start(|_| Resp {
            status: 403,
            headers: vec![("x-ratelimit-remaining".into(), "50".into())],
            body: br#"{"message":"Resource not accessible"}"#.to_vec(),
            ..Default::default()
        });
        let err = fetch(&forbidden.url("/releases/latest"), false, manual)
            .await
            .unwrap_err();
        assert_eq!(format!("{err:#}"), "GitHub returned HTTP 403");

        // 403 with retry-after: 0 must still fall through to x-ratelimit-reset.
        let reset_five = util::unix_now() + 295;
        let zero_retry_with_reset = TestServer::start(move |_| Resp {
            status: 403,
            headers: vec![
                ("retry-after".into(), "0".into()),
                ("x-ratelimit-reset".into(), reset_five.to_string()),
            ],
            ..Default::default()
        });
        let err = fetch(
            &zero_retry_with_reset.url("/releases/latest"),
            false,
            manual,
        )
        .await
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("about 5 minutes"),
            "expected x-ratelimit-reset fallback when retry-after is 0, got: {err:#}"
        );

        // 403 secondary rate limit with body message and no rate-limit headers.
        let secondary = TestServer::start(|_| Resp {
            status: 403,
            body: br#"{"message":"You have exceeded a secondary rate limit."}"#.to_vec(),
            ..Default::default()
        });
        let err = fetch(&secondary.url("/releases/latest"), false, manual)
            .await
            .unwrap_err();
        assert_eq!(
            format!("{err:#}"),
            "GitHub's rate limit was reached. Try again later, or download it from the release page."
        );

        // 403 with HTTP-date Retry-After parses into minutes.
        let retry_date = (chrono::Utc::now() + chrono::Duration::seconds(185)).to_rfc2822();
        let http_date_retry = TestServer::start(move |_| Resp {
            status: 403,
            headers: vec![("retry-after".into(), retry_date.clone())],
            ..Default::default()
        });
        let err = fetch(&http_date_retry.url("/releases/latest"), false, manual)
            .await
            .unwrap_err();
        assert!(
            format!("{err:#}").contains("GitHub's rate limit was reached")
                && format!("{err:#}").contains("minutes"),
            "unexpected HTTP-date rate limit message: {err:#}"
        );

        // Invalid JSON response stays clean without leaking serde diagnostics.
        let bad_json = TestServer::start(|_| Resp {
            status: 200,
            body: b"<html>gateway error</html>".to_vec(),
            ..Default::default()
        });
        let err = fetch(&bad_json.url("/releases/latest"), false, manual)
            .await
            .unwrap_err();
        assert_eq!(format!("{err:#}"), "Unexpected answer from GitHub");
    }
}
