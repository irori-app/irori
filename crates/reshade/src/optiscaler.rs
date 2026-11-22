//! OptiScaler per game: official GitHub release catalog with ETag/TTL
//! caching and mandatory SHA-256 verification, streaming archive unpacking,
//! surgical INI patching, and journaled installs through [`crate::journal`].
//!
//! Nothing is bundled. Stable releases come from `optiscaler/OptiScaler`,
//! nightly builds from `optiscaler/OptiScaler-nightly`, FakeNVAPI from
//! `optiscaler/fakenvapi` (when [`GpuDriverMode::MesaAuto`] is chosen), and
//! OptiPatcher from `optiscaler/OptiPatcher` (when enabled). Every download is
//! verified against the release asset's SHA-256 digest before extraction,
//! unpacked streaming to disk without holding the archive in RAM, and written
//! into the game folder through [`GraphicsMod::Optiscaler`]'s journal.

use std::fs::File;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context as _, Result, bail};
use irori_core::archive_guard::{ensure_no_symlinks, relative_entry_path};
use irori_core::install::ProgressSink;
use irori_core::json_store;
use irori_core::process::CancellationToken;
use once_cell::sync::Lazy;
use regex::Regex;
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::compat::CompatList;
use crate::detect::{self, Detection};
use crate::install::Step;
use crate::journal::{self, Existing, Manifest, ModSource, StagedFile, sha256_file_with_size};
use crate::layer::GraphicsMod;
use crate::pe::{self, Machine};
use crate::{Api, ReshadeError};

/// Official GitHub API endpoint for the latest stable OptiScaler release.
pub const STABLE_RELEASE_URL: &str =
    "https://api.github.com/repos/optiscaler/OptiScaler/releases/latest";
/// Official GitHub API endpoint for recent nightly OptiScaler releases
/// (pre-releases are included when listing `/releases`).
pub const NIGHTLY_RELEASES_URL: &str =
    "https://api.github.com/repos/optiscaler/OptiScaler-nightly/releases?per_page=5";
/// Official GitHub API endpoint for the latest FakeNVAPI release.
pub const FAKENVAPI_RELEASE_URL: &str =
    "https://api.github.com/repos/optiscaler/fakenvapi/releases/latest";
/// Official GitHub API endpoint for the latest OptiPatcher release.
pub const OPTIPATCHER_RELEASE_URL: &str =
    "https://api.github.com/repos/optiscaler/OptiPatcher/releases/latest";

/// Minimum TTL before revalidating a cached GitHub release response (15 minutes).
pub const CATALOG_TTL: Duration = Duration::from_secs(15 * 60);

/// Relative install path for OptiPatcher inside the game directory.
pub const OPTIPATCHER_REL_PATH: &str = "plugins/OptiPatcher.asi";

/// Bounds on extracted archive contents to prevent decompression bombs.
const MAX_ARCHIVE_FILES: usize = 2048;
const MAX_ARCHIVE_BYTES: u64 = 512 << 20;
const MAX_INI_BYTES: u64 = 2 << 20;

static SHA256_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^[0-9a-f]{64}$").expect("valid sha256 regex"));

/// OptiScaler release channel.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OptiscalerChannel {
    #[default]
    Stable,
    Nightly,
}

impl OptiscalerChannel {
    /// Stable identifier used in cache names and manifest sources.
    pub fn id(self) -> &'static str {
        match self {
            OptiscalerChannel::Stable => "stable",
            OptiscalerChannel::Nightly => "nightly",
        }
    }

    /// Display label; brand/channel name.
    pub fn label(self) -> &'static str {
        match self {
            OptiscalerChannel::Stable => "Stable",
            OptiscalerChannel::Nightly => "Nightly",
        }
    }
}

/// GPU driver mode controlling FakeNVAPI deployment and launch environment.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum GpuDriverMode {
    /// Non-NVIDIA or auto mode: installs FakeNVAPI (`nvapi64.dll` and
    /// `fakenvapi.ini`) and sets `UseFakenvapi=true` in `OptiScaler.ini`.
    #[default]
    MesaAuto,
    /// Proprietary NVIDIA driver: skips FakeNVAPI so native NVAPI is used
    /// (`UseFakenvapi=false` in `OptiScaler.ini`).
    Nvidia,
}

/// Target file name OptiScaler is installed as in the game directory.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProxyDll {
    #[default]
    Dxgi,
    Version,
    Dbghelp,
    D3d12,
    Wininet,
    Winhttp,
    Winmm,
    #[serde(rename = "optiscaler.asi")]
    OptiScalerAsi,
}

impl ProxyDll {
    /// All supported proxy file names in UI presentation order.
    pub const ALL: &'static [ProxyDll] = &[
        ProxyDll::Dxgi,
        ProxyDll::Version,
        ProxyDll::Dbghelp,
        ProxyDll::D3d12,
        ProxyDll::Wininet,
        ProxyDll::Winhttp,
        ProxyDll::Winmm,
        ProxyDll::OptiScalerAsi,
    ];

    /// File name written into the game folder.
    pub fn file_name(self) -> &'static str {
        match self {
            ProxyDll::Dxgi => "dxgi.dll",
            ProxyDll::Version => "version.dll",
            ProxyDll::Dbghelp => "dbghelp.dll",
            ProxyDll::D3d12 => "d3d12.dll",
            ProxyDll::Wininet => "wininet.dll",
            ProxyDll::Winhttp => "winhttp.dll",
            ProxyDll::Winmm => "winmm.dll",
            ProxyDll::OptiScalerAsi => "OptiScaler.asi",
        }
    }

    /// DLL stem for `WINEDLLOVERRIDES` (`<stem>=n,b`), or `None` for
    /// `OptiScaler.asi` which is loaded by an ASI loader rather than Wine's
    /// DLL loader.
    pub fn override_stem(self) -> Option<&'static str> {
        match self {
            ProxyDll::Dxgi => Some("dxgi"),
            ProxyDll::Version => Some("version"),
            ProxyDll::Dbghelp => Some("dbghelp"),
            ProxyDll::D3d12 => Some("d3d12"),
            ProxyDll::Wininet => Some("wininet"),
            ProxyDll::Winhttp => Some("winhttp"),
            ProxyDll::Winmm => Some("winmm"),
            ProxyDll::OptiScalerAsi => None,
        }
    }

    /// Look up a `ProxyDll` by its file name (ASCII case-insensitive).
    pub fn from_file_name(name: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|p| p.file_name().eq_ignore_ascii_case(name))
    }
}

/// Per-game OptiScaler configuration persisted in the install manifest.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct OptiscalerConfig {
    pub channel: OptiscalerChannel,
    pub gpu_driver: GpuDriverMode,
    pub proxy_dll: ProxyDll,
    pub enable_optipatcher: bool,
    /// Per-game `[Menu] ShortcutKey` override (`None` = leave at default / user's INI).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shortcut_key: Option<String>,
}

/// Release API endpoints; use [`OptiscalerEndpoints::official`] outside tests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OptiscalerEndpoints {
    pub stable: String,
    pub nightly: String,
    pub fakenvapi: String,
    pub optipatcher: String,
    /// Allowed download URL prefixes (ASCII case-insensitive).
    pub allowed_download_prefixes: Vec<String>,
}

impl OptiscalerEndpoints {
    /// Official GitHub API endpoints and GitHub download host allowlist.
    pub fn official() -> Self {
        Self {
            stable: STABLE_RELEASE_URL.into(),
            nightly: NIGHTLY_RELEASES_URL.into(),
            fakenvapi: FAKENVAPI_RELEASE_URL.into(),
            optipatcher: OPTIPATCHER_RELEASE_URL.into(),
            allowed_download_prefixes: vec![
                "https://github.com/".into(),
                "https://objects.githubusercontent.com/".into(),
            ],
        }
    }

    fn url_allowed(&self, url: &str) -> bool {
        let lower = url.to_ascii_lowercase();
        self.allowed_download_prefixes
            .iter()
            .any(|prefix| lower.starts_with(&prefix.to_ascii_lowercase()))
    }
}

/// A verified release asset from one of the official OptiScaler repositories.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OptiscalerRelease {
    /// Release tag name (for example `v0.7.7`).
    pub version: String,
    /// Asset file name (for example `OptiScaler_0.7.7.7z`).
    pub asset_name: String,
    /// Direct download URL validated against the endpoint allowlist.
    pub download_url: String,
    /// Mandatory lowercase hex SHA-256 digest from GitHub release metadata.
    pub sha256: String,
    /// Asset size in bytes, if reported by GitHub.
    pub size: u64,
    /// Publication or update date (`YYYY-MM-DD`) when reported.
    pub published_at: String,
}

/// Catalog of official OptiScaler releases and companion components.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OptiscalerCatalog {
    pub stable: Option<OptiscalerRelease>,
    pub nightly: Option<OptiscalerRelease>,
    pub fakenvapi: Option<OptiscalerRelease>,
    pub optipatcher: Option<OptiscalerRelease>,
}

impl OptiscalerCatalog {
    /// Return the OptiScaler release for `channel`, if available.
    pub fn release_for(&self, channel: OptiscalerChannel) -> Option<&OptiscalerRelease> {
        match channel {
            OptiscalerChannel::Stable => self.stable.as_ref(),
            OptiscalerChannel::Nightly => self.nightly.as_ref(),
        }
    }
}

/// Everything needed to install OptiScaler into a game folder.
#[derive(Debug, Clone)]
pub struct OptiscalerInstallRequest {
    /// Root folder of the installed game.
    pub game_root: PathBuf,
    /// Configured executable from the store or game settings, if known.
    pub configured_exe: Option<PathBuf>,
    /// Explicit executable chosen by the user, overriding detection.
    pub chosen_exe: Option<PathBuf>,
    /// OptiScaler options (channel, GPU driver mode, proxy DLL, OptiPatcher).
    pub config: OptiscalerConfig,
    /// Where this game's OptiScaler manifest is written
    /// ([`GraphicsMod::journal_path`]).
    pub journal: PathBuf,
    /// OptiScaler cache folder ([`GraphicsMod::cache_dir`]).
    pub cache: PathBuf,
    /// Release endpoints ([`OptiscalerEndpoints::official`] outside tests).
    pub endpoints: OptiscalerEndpoints,
    /// Existing complete installation in the same directory when updating.
    /// Allows identical files to be skipped without rewriting and preserves
    /// user-edited INI settings.
    pub previous: Option<Manifest>,
}

/// Result of a completed OptiScaler installation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OptiscalerInstallResult {
    pub manifest: Manifest,
    pub release: OptiscalerRelease,
    pub fakenvapi: Option<OptiscalerRelease>,
    pub optipatcher: Option<OptiscalerRelease>,
}

/// Compare an installed version tag against the latest release tag for `channel`.
pub fn is_newer_release(channel: OptiscalerChannel, installed: &str, latest: &str) -> bool {
    let norm = |s: &str| {
        s.trim()
            .strip_prefix(['v', 'V'])
            .unwrap_or(s.trim())
            .to_string()
    };
    let a = norm(installed);
    let b = norm(latest);
    if a.is_empty() || b.is_empty() || a.eq_ignore_ascii_case(&b) {
        return false;
    }
    match channel {
        OptiscalerChannel::Nightly => true,
        OptiscalerChannel::Stable => match (parse_semver_nums(&a), parse_semver_nums(&b)) {
            (Some((an, apre)), Some((bn, bpre))) => {
                bn > an || (bn == an && apre && !bpre) || (bn == an && apre && bpre && b != a)
            }
            _ => true,
        },
    }
}

fn parse_semver_nums(s: &str) -> Option<(Vec<u32>, bool)> {
    let (core, pre) = match s.split_once('-') {
        Some((c, _)) => (c, true),
        None => (s, false),
    };
    let parts: Option<Vec<u32>> = core.split('.').map(|p| p.parse::<u32>().ok()).collect();
    let parts = parts?;
    (!parts.is_empty()).then_some((parts, pre))
}

#[derive(Debug, Clone, Copy)]
enum AssetKind {
    Stable,
    Nightly,
    FakeNvapi,
    OptiPatcher,
}

impl AssetKind {
    fn label(self) -> &'static str {
        match self {
            AssetKind::Stable => "OptiScaler (Stable)",
            AssetKind::Nightly => "OptiScaler (Nightly)",
            AssetKind::FakeNvapi => "FakeNVAPI",
            AssetKind::OptiPatcher => "OptiPatcher",
        }
    }

    fn cache_name(self) -> &'static str {
        match self {
            AssetKind::Stable => "stable.json",
            AssetKind::Nightly => "nightly.json",
            AssetKind::FakeNvapi => "fakenvapi.json",
            AssetKind::OptiPatcher => "optipatcher.json",
        }
    }

    /// Match priority for an asset file name; higher is preferred, `None` means skip.
    fn priority(self, name: &str) -> Option<u8> {
        // Asset name must be a single plain file name with no separators.
        if name.contains('/') || name.contains('\\') {
            return None;
        }
        if relative_entry_path(Path::new(name)).as_deref() != Some(Path::new(name)) {
            return None;
        }
        let lower = name.to_ascii_lowercase();
        match self {
            AssetKind::Stable => {
                (lower.starts_with("optiscaler_") && lower.ends_with(".7z")).then_some(2)
            }
            AssetKind::Nightly => {
                if !lower.ends_with(".7z") {
                    return None;
                }
                if lower.starts_with("optiscaler") {
                    Some(2)
                } else {
                    Some(1)
                }
            }
            AssetKind::FakeNvapi => {
                if !(lower.ends_with(".7z") || lower.ends_with(".zip")) {
                    return None;
                }
                if lower.contains("fakenvapi") {
                    Some(2)
                } else {
                    Some(1)
                }
            }
            AssetKind::OptiPatcher => {
                if !lower.ends_with(".asi") {
                    return None;
                }
                if lower.contains("optipatcher") {
                    Some(2)
                } else {
                    Some(1)
                }
            }
        }
    }
}

fn str_at<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    v.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

fn parse_sha256_digest(asset: &Value) -> Option<String> {
    str_at(asset, "digest")
        .and_then(|d| d.strip_prefix("sha256:"))
        .map(str::to_ascii_lowercase)
        .filter(|d| SHA256_RE.is_match(d))
}

fn parse_single_release(
    release: &Value,
    kind: AssetKind,
    endpoints: &OptiscalerEndpoints,
) -> Option<OptiscalerRelease> {
    if release.get("draft").and_then(Value::as_bool) == Some(true) {
        return None;
    }
    let version = str_at(release, "tag_name")
        .or_else(|| str_at(release, "name"))?
        .to_string();
    let release_date = str_at(release, "published_at")
        .or_else(|| str_at(release, "created_at"))
        .and_then(|s| s.get(..10))
        .unwrap_or_default();
    let assets = release.get("assets")?.as_array()?;

    let mut best: Option<(u8, OptiscalerRelease)> = None;
    for asset in assets {
        let Some(name) = str_at(asset, "name") else {
            continue;
        };
        let Some(prio) = kind.priority(name) else {
            continue;
        };
        let Some(url) = str_at(asset, "browser_download_url") else {
            continue;
        };
        if !endpoints.url_allowed(url) {
            continue;
        }
        // Mandatory SHA-256 digest verification: refuse assets without a valid digest.
        let Some(sha256) = parse_sha256_digest(asset) else {
            continue;
        };
        let size = asset.get("size").and_then(Value::as_u64).unwrap_or(0);
        let published_at = str_at(asset, "updated_at")
            .and_then(|s| s.get(..10))
            .unwrap_or(release_date)
            .to_string();
        let candidate = OptiscalerRelease {
            version: version.clone(),
            asset_name: name.to_string(),
            download_url: url.to_string(),
            sha256,
            size,
            published_at,
        };
        if best.as_ref().is_none_or(|(p, _)| prio > *p) {
            best = Some((prio, candidate));
        }
    }
    best.map(|(_, rel)| rel)
}

/// Parse a GitHub release response (either a single release object from
/// `/releases/latest` or an array from `/releases`).
fn parse_release_response(
    body: &str,
    kind: AssetKind,
    endpoints: &OptiscalerEndpoints,
) -> Result<OptiscalerRelease> {
    let value: Value = serde_json::from_str(body)
        .with_context(|| format!("{} release response is not valid JSON", kind.label()))?;
    let parsed = match &value {
        Value::Array(items) => items
            .iter()
            .find_map(|item| parse_single_release(item, kind, endpoints)),
        Value::Object(_) => parse_single_release(&value, kind, endpoints),
        _ => None,
    };
    parsed.with_context(|| {
        format!(
            "No verified {} release asset with a SHA-256 digest was found",
            kind.label()
        )
    })
}

/// Parse a GitHub release JSON body for the requested OptiScaler channel.
pub fn parse_channel_release(
    channel: OptiscalerChannel,
    body: &str,
    endpoints: &OptiscalerEndpoints,
) -> Result<OptiscalerRelease> {
    let kind = match channel {
        OptiscalerChannel::Stable => AssetKind::Stable,
        OptiscalerChannel::Nightly => AssetKind::Nightly,
    };
    parse_release_response(body, kind, endpoints)
}

/// Parse a GitHub release JSON body for FakeNVAPI.
pub fn parse_fakenvapi_release(
    body: &str,
    endpoints: &OptiscalerEndpoints,
) -> Result<OptiscalerRelease> {
    parse_release_response(body, AssetKind::FakeNvapi, endpoints)
}

/// Parse a GitHub release JSON body for OptiPatcher.
pub fn parse_optipatcher_release(
    body: &str,
    endpoints: &OptiscalerEndpoints,
) -> Result<OptiscalerRelease> {
    parse_release_response(body, AssetKind::OptiPatcher, endpoints)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CachedResponse {
    fetched_at_unix: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    etag: Option<String>,
    body: String,
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Fetch a GitHub API JSON endpoint with ETag and minimum 15-minute TTL caching.
///
/// - Within `ttl` (unless `force_refresh` is true), returns the cached body
///   with zero network traffic.
/// - When expired or forced, sends `If-None-Match` with the saved ETag; a
///   `304 Not Modified` refreshes the timestamp and reuses the cached body.
/// - On network or HTTP error, falls back to the stale cached body when one
///   is available.
async fn fetch_cached_github_json(
    what: &str,
    url: &str,
    cache_path: PathBuf,
    ttl: Duration,
    force_refresh: bool,
) -> Result<String> {
    let read_path = cache_path.clone();
    let cached: Option<CachedResponse> = tokio::task::spawn_blocking(move || {
        json_store::read::<CachedResponse>(&read_path)
            .ok()
            .flatten()
    })
    .await
    .unwrap_or(None);

    let now = unix_now();
    if !force_refresh
        && let Some(ref entry) = cached
        && now.saturating_sub(entry.fetched_at_unix) < ttl.as_secs()
    {
        return Ok(entry.body.clone());
    }

    let mut req = irori_core::http::client()
        .get(url)
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28");
    if let Some(etag) = cached.as_ref().and_then(|c| c.etag.as_deref()) {
        req = req.header("If-None-Match", etag);
    }

    let fetched = async {
        let resp = req
            .send()
            .await
            .with_context(|| format!("Could not reach GitHub for {what}"))?;
        let status = resp.status();
        let new_etag = resp
            .headers()
            .get(reqwest::header::ETAG)
            .and_then(|v| v.to_str().ok())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(String::from);

        if status == StatusCode::NOT_MODIFIED
            && let Some(mut entry) = cached.clone()
        {
            entry.fetched_at_unix = now;
            if new_etag.is_some() {
                entry.etag = new_etag;
            }
            return Ok(entry);
        }

        if !status.is_success() {
            bail!("GitHub returned HTTP {} for {what}", status.as_u16());
        }

        let body = resp
            .text()
            .await
            .with_context(|| format!("Could not read GitHub response for {what}"))?;
        // Validate JSON syntax before overwriting cache.
        serde_json::from_str::<Value>(&body)
            .with_context(|| format!("GitHub returned invalid JSON for {what}"))?;

        Ok(CachedResponse {
            fetched_at_unix: now,
            etag: new_etag,
            body,
        })
    }
    .await;

    match fetched {
        Ok(entry) => {
            let body = entry.body.clone();
            let save_path = cache_path;
            let what_owned = what.to_string();
            let _ = tokio::task::spawn_blocking(move || {
                if let Err(e) = json_store::write(&save_path, &entry) {
                    tracing::warn!("caching {what_owned} metadata: {e:#}");
                }
            })
            .await;
            Ok(body)
        }
        Err(e) => {
            if let Some(entry) = cached {
                tracing::warn!("using cached {what} release metadata: {e:#}");
                Ok(entry.body)
            } else {
                Err(e)
            }
        }
    }
}

async fn fetch_release_with_ttl(
    kind: AssetKind,
    url: &str,
    endpoints: &OptiscalerEndpoints,
    cache: &Path,
    ttl: Duration,
    force_refresh: bool,
) -> Result<OptiscalerRelease> {
    let cache_path = cache.join("catalog").join(kind.cache_name());
    let body = fetch_cached_github_json(kind.label(), url, cache_path, ttl, force_refresh).await?;
    parse_release_response(&body, kind, endpoints)
}

/// Fetch the latest OptiScaler release for `channel` (using ETag and 15-minute TTL cache).
pub async fn fetch_channel_release(
    channel: OptiscalerChannel,
    endpoints: &OptiscalerEndpoints,
    cache: &Path,
    force_refresh: bool,
) -> Result<OptiscalerRelease> {
    let (kind, url) = match channel {
        OptiscalerChannel::Stable => (AssetKind::Stable, endpoints.stable.as_str()),
        OptiscalerChannel::Nightly => (AssetKind::Nightly, endpoints.nightly.as_str()),
    };
    fetch_release_with_ttl(kind, url, endpoints, cache, CATALOG_TTL, force_refresh).await
}

/// Fetch the latest FakeNVAPI release (using ETag and 15-minute TTL cache).
pub async fn fetch_fakenvapi_release(
    endpoints: &OptiscalerEndpoints,
    cache: &Path,
    force_refresh: bool,
) -> Result<OptiscalerRelease> {
    fetch_release_with_ttl(
        AssetKind::FakeNvapi,
        &endpoints.fakenvapi,
        endpoints,
        cache,
        CATALOG_TTL,
        force_refresh,
    )
    .await
}

/// Fetch the latest OptiPatcher release (using ETag and 15-minute TTL cache).
pub async fn fetch_optipatcher_release(
    endpoints: &OptiscalerEndpoints,
    cache: &Path,
    force_refresh: bool,
) -> Result<OptiscalerRelease> {
    fetch_release_with_ttl(
        AssetKind::OptiPatcher,
        &endpoints.optipatcher,
        endpoints,
        cache,
        CATALOG_TTL,
        force_refresh,
    )
    .await
}

/// Fetch all four official release entries concurrently using the ETag/TTL cache.
/// Individual companion failures are logged and left as `None`; fails only if
/// neither Stable nor Nightly OptiScaler release can be resolved.
pub async fn fetch_catalog(
    endpoints: &OptiscalerEndpoints,
    cache: &Path,
    force_refresh: bool,
) -> Result<OptiscalerCatalog> {
    let (stable, nightly, fakenvapi, optipatcher) = tokio::join!(
        fetch_channel_release(OptiscalerChannel::Stable, endpoints, cache, force_refresh),
        fetch_channel_release(OptiscalerChannel::Nightly, endpoints, cache, force_refresh),
        fetch_fakenvapi_release(endpoints, cache, force_refresh),
        fetch_optipatcher_release(endpoints, cache, force_refresh),
    );
    let stable_err = stable.as_ref().err().map(|e| format!("{e:#}"));
    let stable = stable.ok();
    let nightly = match nightly {
        Ok(r) => Some(r),
        Err(e) => {
            tracing::info!("OptiScaler nightly catalog: {e:#}");
            None
        }
    };
    let fakenvapi = match fakenvapi {
        Ok(r) => Some(r),
        Err(e) => {
            tracing::info!("FakeNVAPI catalog: {e:#}");
            None
        }
    };
    let optipatcher = match optipatcher {
        Ok(r) => Some(r),
        Err(e) => {
            tracing::info!("OptiPatcher catalog: {e:#}");
            None
        }
    };
    if stable.is_none() && nightly.is_none() {
        bail!(
            "Could not load OptiScaler release catalog: {}",
            stable_err.unwrap_or_else(|| "no releases available".into())
        );
    }
    Ok(OptiscalerCatalog {
        stable,
        nightly,
        fakenvapi,
        optipatcher,
    })
}

/// Download `release` streaming to disk into `cache`, verifying its SHA-256
/// digest with a 64 KiB buffered reader without allocating the asset in RAM.
pub async fn download_verified_asset(
    release: &OptiscalerRelease,
    cache: &Path,
    progress: &ProgressSink,
    cancel: &CancellationToken,
) -> Result<PathBuf> {
    if !SHA256_RE.is_match(&release.sha256) {
        bail!("Release {} has no valid SHA-256 digest", release.asset_name);
    }
    let safe_name = irori_core::paths::sanitize(&release.asset_name);
    if safe_name.is_empty() {
        bail!("Release names an invalid asset file");
    }
    let assets_dir = cache.join("assets");
    let cached_path = assets_dir.join(format!("{}-{safe_name}", &release.sha256[..16]));

    // Reuse cached file when its SHA-256 still matches.
    let check_path = cached_path.clone();
    let expected = release.sha256.clone();
    let hit = tokio::task::spawn_blocking(move || {
        check_path.is_file()
            && sha256_file_with_size(&check_path)
                .map(|(h, _)| h == expected)
                .unwrap_or(false)
    })
    .await
    .unwrap_or(false);
    if hit {
        return Ok(cached_path);
    }

    static NEXT: AtomicU64 = AtomicU64::new(0);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let temp_path = cache
        .join("downloads")
        .join(format!("{}-{n}-{safe_name}", std::process::id()));

    irori_runtime::download::download(&release.download_url, &temp_path, progress, cancel)
        .await
        .with_context(|| format!("Could not download {}", release.asset_name))?;

    let verify_path = temp_path.clone();
    let actual = tokio::task::spawn_blocking(move || sha256_file_with_size(&verify_path))
        .await
        .context("SHA-256 verification task failed")??
        .0;

    if actual != release.sha256 {
        let _ = tokio::fs::remove_file(&temp_path).await;
        bail!(
            "SHA-256 mismatch for {}: expected {}, got {}",
            release.asset_name,
            release.sha256,
            actual
        );
    }

    tokio::fs::create_dir_all(&assets_dir)
        .await
        .with_context(|| format!("Could not create {}", assets_dir.display()))?;
    if let Err(rename_err) = tokio::fs::rename(&temp_path, &cached_path).await {
        // Fallback if cache/downloads and cache/assets ever span filesystems.
        let src = temp_path.clone();
        let dst = cached_path.clone();
        tokio::task::spawn_blocking(move || std::fs::copy(&src, &dst))
            .await
            .context("Copying verified asset into cache failed")?
            .with_context(|| format!("Could not save cached asset ({rename_err})"))?;
        let _ = tokio::fs::remove_file(&temp_path).await;
    }

    Ok(cached_path)
}

/// Check for Windows drive prefixes like `C:` even when running on Unix.
fn has_windows_drive_prefix(normalized: &str) -> bool {
    let bytes = normalized.as_bytes();
    bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':'
}

fn validated_archive_rel(raw_name: &str) -> Result<Option<PathBuf>> {
    let normalized = raw_name.replace('\\', "/");
    let trimmed = normalized.trim_matches('/');
    if trimmed.is_empty() || trimmed == "." {
        return Ok(None);
    }
    if normalized.starts_with('/') || has_windows_drive_prefix(&normalized) {
        bail!("unsafe path in archive: {raw_name}");
    }
    let rel = relative_entry_path(Path::new(&normalized))
        .with_context(|| format!("unsafe path in archive: {raw_name}"))?;
    Ok(Some(rel))
}

/// Stream-extract a `.7z` archive into `dest` using
/// `sevenz_rust2::decompress_file_with_extract_fn`, validating every entry with
/// `irori_core::archive_guard::relative_entry_path` and `ensure_no_symlinks`.
/// Blocking.
pub fn extract_7z(archive: &Path, dest: &Path, cancel: &CancellationToken) -> Result<()> {
    std::fs::create_dir_all(dest)
        .with_context(|| format!("Could not create {}", dest.display()))?;
    let mut file_count = 0usize;
    let mut total_bytes = 0u64;
    let mut failure: Option<anyhow::Error> = None;

    let res = sevenz_rust2::decompress_file_with_extract_fn(archive, dest, |entry, reader, _| {
        if cancel.is_cancelled() {
            failure = Some(anyhow::Error::new(irori_runtime::Cancelled));
            return Err(sevenz_rust2::Error::Other("cancelled".into()));
        }
        let rel = match validated_archive_rel(entry.name()) {
            Ok(Some(r)) => r,
            Ok(None) => return Ok(true),
            Err(e) => {
                let msg = format!("{e:#}");
                failure = Some(e);
                return Err(sevenz_rust2::Error::Other(msg.into()));
            }
        };
        if let Err(e) = ensure_no_symlinks(dest, &rel) {
            let msg = format!("{e:#}");
            failure = Some(e);
            return Err(sevenz_rust2::Error::Other(msg.into()));
        }
        let target = dest.join(&rel);
        let io_err =
            |e: std::io::Error| sevenz_rust2::Error::Io(e, target.display().to_string().into());
        if entry.is_directory() {
            std::fs::create_dir_all(&target).map_err(io_err)?;
            return Ok(true);
        }
        file_count += 1;
        if file_count > MAX_ARCHIVE_FILES {
            let e = anyhow::anyhow!("The 7-Zip archive contains too many files");
            failure = Some(e);
            return Err(sevenz_rust2::Error::Other("too many files".into()));
        }
        if let Some(parent) = rel.parent()
            && !parent.as_os_str().is_empty()
        {
            if let Err(e) = ensure_no_symlinks(dest, parent) {
                let msg = format!("{e:#}");
                failure = Some(e);
                return Err(sevenz_rust2::Error::Other(msg.into()));
            }
            std::fs::create_dir_all(dest.join(parent)).map_err(io_err)?;
        }
        let remaining = MAX_ARCHIVE_BYTES.saturating_sub(total_bytes);
        let mut out = File::create(&target).map_err(io_err)?;
        let mut limited = reader.take(remaining.saturating_add(1));
        let written = std::io::copy(&mut limited, &mut out).map_err(io_err)?;
        total_bytes = total_bytes.saturating_add(written);
        if total_bytes > MAX_ARCHIVE_BYTES {
            let e = anyhow::anyhow!("The 7-Zip archive exceeds the extraction size limit");
            failure = Some(e);
            return Err(sevenz_rust2::Error::Other("archive too large".into()));
        }
        Ok(true)
    });

    if let Some(e) = failure {
        return Err(e);
    }
    res.map_err(|e| {
        if cancel.is_cancelled() {
            anyhow::Error::new(irori_runtime::Cancelled)
        } else {
            anyhow::anyhow!("Could not unpack 7-Zip archive: {e}")
        }
    })
}

/// Stream-extract a `.zip` archive from disk into `dest`, validating every
/// entry with `irori_core::archive_guard::relative_entry_path` and
/// `ensure_no_symlinks`. Blocking.
pub fn extract_zip(archive: &Path, dest: &Path, cancel: &CancellationToken) -> Result<()> {
    std::fs::create_dir_all(dest)
        .with_context(|| format!("Could not create {}", dest.display()))?;
    let file =
        File::open(archive).with_context(|| format!("Could not open {}", archive.display()))?;
    let mut zip = zip::ZipArchive::new(file).context("The downloaded zip archive is damaged")?;
    let mut file_count = 0usize;
    let mut total_bytes = 0u64;

    for i in 0..zip.len() {
        if cancel.is_cancelled() {
            bail!(irori_runtime::Cancelled);
        }
        let mut entry = zip.by_index(i)?;
        let raw_name = entry
            .name()
            .context("The zip archive contains an unreadable entry name")?
            .into_owned();
        if entry.is_symlink() {
            bail!("unsafe link in zip archive: {raw_name}");
        }
        let Some(rel) = validated_archive_rel(&raw_name)? else {
            continue;
        };
        ensure_no_symlinks(dest, &rel)?;
        let target = dest.join(&rel);
        if entry.is_dir() {
            std::fs::create_dir_all(&target)?;
            continue;
        }
        file_count += 1;
        if file_count > MAX_ARCHIVE_FILES {
            bail!("The zip archive contains too many files");
        }
        if let Some(parent) = rel.parent()
            && !parent.as_os_str().is_empty()
        {
            ensure_no_symlinks(dest, parent)?;
            std::fs::create_dir_all(dest.join(parent))?;
        }
        let remaining = MAX_ARCHIVE_BYTES.saturating_sub(total_bytes);
        let mut out = File::create(&target)
            .with_context(|| format!("Could not write {}", target.display()))?;
        let mut limited = (&mut entry).take(remaining.saturating_add(1));
        let written = std::io::copy(&mut limited, &mut out)
            .with_context(|| format!("Could not unpack {}", rel.display()))?;
        total_bytes = total_bytes.saturating_add(written);
        if total_bytes > MAX_ARCHIVE_BYTES {
            bail!("The zip archive exceeds the extraction size limit");
        }
    }
    Ok(())
}

fn extract_archive(archive: &Path, dest: &Path, cancel: &CancellationToken) -> Result<()> {
    let ext = archive
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase);
    match ext.as_deref() {
        Some("7z") => extract_7z(archive, dest, cancel),
        Some("zip") => extract_zip(archive, dest, cancel),
        _ => bail!("Unsupported archive format: {}", archive.display()),
    }
}

/// Surgically patch INI key/value pairs in `input` while preserving comments,
/// section order, indentation, spacing around `=`, inline comments, line
/// endings, and unknown keys.
///
/// Each entry in `updates` is `(section, key, value)`. Section and key matching
/// is ASCII case-insensitive. Missing keys are inserted at the end of their
/// section; missing sections are appended at the end of the file.
pub fn patch_ini(input: &str, updates: &[(&str, &str, &str)]) -> String {
    if updates.is_empty() {
        return input.to_string();
    }

    let (has_bom, body) = match input.strip_prefix('\u{feff}') {
        Some(rest) => (true, rest),
        None => (false, input),
    };
    let eol = if body.contains("\r\n") { "\r\n" } else { "\n" };

    // Track which updates have already been applied.
    let mut applied = vec![false; updates.len()];
    let mut out_lines: Vec<String> = Vec::new();
    let mut current_section: Option<String> = None;

    // Flush any unapplied keys belonging to `section` before leaving it,
    // inserting them before trailing blank lines so section spacing stays clean.
    let flush_section = |section: &str, out: &mut Vec<String>, applied: &mut [bool]| {
        let mut trailing_blanks = 0usize;
        while out
            .len()
            .checked_sub(trailing_blanks + 1)
            .and_then(|idx| out.get(idx))
            .is_some_and(|l| l.trim().is_empty())
        {
            trailing_blanks += 1;
        }
        let insert_at = out.len() - trailing_blanks;
        let mut to_insert = Vec::new();
        for (idx, &(sec, key, val)) in updates.iter().enumerate() {
            if !applied[idx] && sec.eq_ignore_ascii_case(section) {
                to_insert.push(format!("{key}={val}{eol}"));
                applied[idx] = true;
            }
        }
        if !to_insert.is_empty() {
            // Ensure the line preceding insertion ends with a newline.
            if insert_at > 0 && !out[insert_at - 1].ends_with('\n') {
                out[insert_at - 1].push_str(eol);
            }
            out.splice(insert_at..insert_at, to_insert);
        }
    };

    for raw_line in body.split_inclusive('\n') {
        let (line_no_eol, line_eol) = if let Some(s) = raw_line.strip_suffix("\r\n") {
            (s, "\r\n")
        } else if let Some(s) = raw_line.strip_suffix('\n') {
            (s, "\n")
        } else {
            (raw_line, "")
        };

        let trimmed = line_no_eol.trim_start();
        if trimmed.starts_with(';') || trimmed.starts_with('#') || trimmed.is_empty() {
            out_lines.push(raw_line.to_string());
            continue;
        }

        if let Some(sec_name) = parse_section_header(trimmed) {
            if let Some(prev) = current_section.as_deref() {
                flush_section(prev, &mut out_lines, &mut applied);
            }
            current_section = Some(sec_name.to_string());
            out_lines.push(raw_line.to_string());
            continue;
        }

        if let Some(sec) = current_section.as_deref()
            && let Some(eq_idx) = line_no_eol.find('=')
        {
            let lhs = &line_no_eol[..eq_idx];
            let key_trimmed = lhs.trim();
            if let Some((idx, &(_, _, new_val))) =
                updates.iter().enumerate().find(|(_, (s, k, _))| {
                    s.eq_ignore_ascii_case(sec) && k.eq_ignore_ascii_case(key_trimmed)
                })
            {
                let rhs = &line_no_eol[eq_idx + 1..];
                let patched = replace_ini_value(lhs, rhs, new_val);
                applied[idx] = true;
                out_lines.push(format!("{patched}{line_eol}"));
                continue;
            }
        }

        out_lines.push(raw_line.to_string());
    }

    if let Some(prev) = current_section.as_deref() {
        flush_section(prev, &mut out_lines, &mut applied);
    }

    // Append any remaining updates whose sections did not exist in the input.
    let mut missing_sections: Vec<&str> = Vec::new();
    for (idx, &(sec, _, _)) in updates.iter().enumerate() {
        if !applied[idx] && !missing_sections.iter().any(|s| s.eq_ignore_ascii_case(sec)) {
            missing_sections.push(sec);
        }
    }

    for sec in missing_sections {
        if !out_lines.is_empty() {
            if let Some(last) = out_lines.last_mut()
                && !last.ends_with('\n')
            {
                last.push_str(eol);
            }
            if out_lines.last().is_some_and(|l| !l.trim().is_empty()) {
                out_lines.push(eol.to_string());
            }
        }
        out_lines.push(format!("[{sec}]{eol}"));
        for (idx, &(s, key, val)) in updates.iter().enumerate() {
            if !applied[idx] && s.eq_ignore_ascii_case(sec) {
                out_lines.push(format!("{key}={val}{eol}"));
                applied[idx] = true;
            }
        }
    }

    let mut result = String::with_capacity(input.len() + 64);
    if has_bom {
        result.push('\u{feff}');
    }
    for line in out_lines {
        result.push_str(&line);
    }
    result
}

fn parse_section_header(trimmed_start: &str) -> Option<&str> {
    let rest = trimmed_start.strip_prefix('[')?;
    let end = rest.find(']')?;
    let after = rest[end + 1..].trim();
    if after.is_empty() || after.starts_with(';') || after.starts_with('#') {
        let name = rest[..end].trim();
        (!name.is_empty()).then_some(name)
    } else {
        None
    }
}

/// Replace the value portion of `lhs=rhs` while preserving whitespace around
/// `=` and any trailing inline comment (`;...` or `#...`).
fn replace_ini_value(lhs: &str, rhs: &str, new_val: &str) -> String {
    let leading_ws_len = rhs.len() - rhs.trim_start().len();
    let leading_ws = &rhs[..leading_ws_len];
    let after_ws = &rhs[leading_ws_len..];

    let comment_start = find_inline_comment(after_ws);
    match comment_start {
        Some(pos) => {
            let before_comment = &after_ws[..pos];
            let trailing_ws_len = before_comment.len() - before_comment.trim_end().len();
            let sep_before_comment = &before_comment[before_comment.len() - trailing_ws_len..];
            let comment = &after_ws[pos..];
            format!("{lhs}={leading_ws}{new_val}{sep_before_comment}{comment}")
        }
        None => format!("{lhs}={leading_ws}{new_val}"),
    }
}

fn find_inline_comment(s: &str) -> Option<usize> {
    let mut in_quotes = false;
    for (idx, ch) in s.char_indices() {
        match ch {
            '"' => in_quotes = !in_quotes,
            ';' | '#' if !in_quotes => return Some(idx),
            _ => {}
        }
    }
    None
}

/// Surgically patch `OptiScaler.ini` for `config`:
/// - `[Plugins]` `Path=plugins` (when OptiPatcher is enabled) and `LoadAsiPlugins=true|false`
/// - `[fakenvapi]` `UseFakenvapi=true|false`
/// - `[Menu]` `ShortcutKey` when configured in `config.shortcut_key`
pub fn patch_optiscaler_ini(input: &str, config: &OptiscalerConfig) -> String {
    let use_fakenvapi = match config.gpu_driver {
        GpuDriverMode::MesaAuto => "true",
        GpuDriverMode::Nvidia => "false",
    };
    let mut updates: Vec<(&str, &str, &str)> = Vec::with_capacity(4);
    if config.enable_optipatcher {
        updates.push(("Plugins", "Path", "plugins"));
        updates.push(("Plugins", "LoadAsiPlugins", "true"));
    } else {
        updates.push(("Plugins", "LoadAsiPlugins", "false"));
    }
    updates.push(("fakenvapi", "UseFakenvapi", use_fakenvapi));

    let normalized_shortcut = config
        .shortcut_key
        .as_deref()
        .and_then(crate::optiscaler_keys::normalize_shortcut_key);
    if let Some(ref key) = normalized_shortcut {
        updates.push(("Menu", "ShortcutKey", key.as_str()));
    }

    patch_ini(input, &updates)
}

/// Surgically patch `fakenvapi.ini` with any requested `(section, key, value)`
/// overrides. Passing an empty slice preserves the file verbatim.
pub fn patch_fakenvapi_ini(input: &str, overrides: &[(&str, &str, &str)]) -> String {
    patch_ini(input, overrides)
}

/// Detect the 64-bit game executable and install folder for OptiScaler.
/// Blocking.
pub fn detect_target(
    root: &Path,
    configured: Option<&Path>,
    chosen: Option<&Path>,
) -> Result<Detection> {
    let found = detect::detect(root, configured, chosen, &CompatList::default())?;
    if found.machine != Machine::X64 {
        bail!(ReshadeError::UnsupportedArch);
    }
    Ok(found)
}

/// RAII temporary directory inside the cache that removes itself on drop.
struct StagingDir(PathBuf);

impl StagingDir {
    fn new(cache: &Path) -> Result<Self> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let path = cache
            .join("staging")
            .join(format!("optiscaler-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&path)
            .with_context(|| format!("Could not create {}", path.display()))?;
        Ok(Self(path))
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for StagingDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// If `dir` contains only a single subdirectory (common GitHub release archive
/// wrapper), return that subdirectory; otherwise return `dir`.
fn unwrap_single_root(dir: &Path) -> PathBuf {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return dir.to_path_buf();
    };
    let items: Vec<_> = entries.flatten().collect();
    if items.len() == 1 && items[0].file_type().is_ok_and(|t| t.is_dir()) {
        items[0].path()
    } else {
        dir.to_path_buf()
    }
}

/// Walk `root` and collect `(relative_path, full_path)` for every regular file,
/// sorted deterministically by relative path.
fn collect_files(root: &Path) -> Result<Vec<(PathBuf, PathBuf)>> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir)
            .with_context(|| format!("Could not read {}", dir.display()))?
            .flatten()
        {
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            let path = entry.path();
            if kind.is_dir() {
                stack.push(path);
            } else if kind.is_file()
                && let Ok(rel) = path.strip_prefix(root)
            {
                out.push((rel.to_path_buf(), path));
            }
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

fn is_setup_script(path: &Path) -> bool {
    let is_script_ext = path.extension().and_then(|e| e.to_str()).is_some_and(|e| {
        e.eq_ignore_ascii_case("bat")
            || e.eq_ignore_ascii_case("sh")
            || e.eq_ignore_ascii_case("cmd")
            || e.eq_ignore_ascii_case("ps1")
    });
    let is_extract_marker = path
        .file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.starts_with("!!"));
    is_script_ext || is_extract_marker
}

fn read_small_text(path: &Path) -> Result<String> {
    let meta = std::fs::metadata(path)?;
    if meta.len() > MAX_INI_BYTES {
        bail!("{} is too large to be an INI file", path.display());
    }
    let bytes = std::fs::read(path)?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

fn verify_x64_pe(path: &Path, label: &str) -> Result<()> {
    let info =
        pe::read_file(path).with_context(|| format!("{label} is not a valid Windows binary"))?;
    if info.machine != Machine::X64 {
        bail!("{label} is not built for 64-bit Windows");
    }
    Ok(())
}

/// Install OptiScaler (and optionally FakeNVAPI and OptiPatcher) into the game
/// folder through [`GraphicsMod::Optiscaler`]'s journal.
pub async fn install(
    req: OptiscalerInstallRequest,
    step: Arc<dyn Fn(Step) + Send + Sync>,
    progress: ProgressSink,
    cancel: CancellationToken,
) -> Result<OptiscalerInstallResult> {
    step(Step::Checking);
    let found = {
        let root = req.game_root.clone();
        let configured = req.configured_exe.clone();
        let chosen = req.chosen_exe.clone();
        tokio::task::spawn_blocking(move || {
            detect_target(&root, configured.as_deref(), chosen.as_deref())
        })
        .await??
    };

    if !found.dir.is_dir() {
        bail!(
            "The folder OptiScaler goes to, {}, does not exist",
            found.dir.display()
        );
    }

    let previous_in_dir = req
        .previous
        .filter(|prev| prev.complete && prev.dir == found.dir);

    // Fail fast before downloading if the target proxy file is already taken
    // by an untracked mod (updating our own existing proxy file is allowed).
    let proxy_name = req.config.proxy_dll.file_name();
    let replaces_own_proxy = previous_in_dir
        .as_ref()
        .is_some_and(|prev| prev.dll.eq_ignore_ascii_case(proxy_name));
    if !replaces_own_proxy {
        let proxy_path = found.dir.join(proxy_name);
        let proxy_disabled = found
            .dir
            .join(format!("{proxy_name}{}", journal::DISABLED_SUFFIX));
        if tokio::fs::symlink_metadata(&proxy_path).await.is_ok()
            || tokio::fs::symlink_metadata(&proxy_disabled).await.is_ok()
        {
            bail!(ReshadeError::DllTaken {
                name: proxy_name.to_string(),
            });
        }
    }

    if cancel.is_cancelled() {
        bail!(irori_runtime::Cancelled);
    }

    step(Step::DownloadingOptiscaler);
    let release =
        fetch_channel_release(req.config.channel, &req.endpoints, &req.cache, false).await?;
    let optiscaler_archive =
        download_verified_asset(&release, &req.cache, &progress, &cancel).await?;

    let (fakenvapi_rel, fakenvapi_archive) = match req.config.gpu_driver {
        GpuDriverMode::MesaAuto => {
            step(Step::DownloadingFakenvapi);
            let rel = fetch_fakenvapi_release(&req.endpoints, &req.cache, false).await?;
            let archive = download_verified_asset(&rel, &req.cache, &progress, &cancel).await?;
            (Some(rel), Some(archive))
        }
        GpuDriverMode::Nvidia => (None, None),
    };

    let (optipatcher_rel, optipatcher_file) = if req.config.enable_optipatcher {
        step(Step::DownloadingOptipatcher);
        let rel = fetch_optipatcher_release(&req.endpoints, &req.cache, false).await?;
        let file = download_verified_asset(&rel, &req.cache, &progress, &cancel).await?;
        (Some(rel), Some(file))
    } else {
        (None, None)
    };

    if cancel.is_cancelled() {
        bail!(irori_runtime::Cancelled);
    }

    step(Step::Writing);
    let version = release.version.clone();
    let config = req.config.clone();
    let journal_path = req.journal.clone();
    let cache = req.cache.clone();
    let cancel_bg = cancel.clone();
    let mut keep_assets = vec![optiscaler_archive.clone()];
    if let Some(p) = &fakenvapi_archive {
        keep_assets.push(p.clone());
    }
    if let Some(p) = &optipatcher_file {
        keep_assets.push(p.clone());
    }

    let pruned_fakenvapi = fakenvapi_rel.is_some();
    let pruned_optipatcher = optipatcher_rel.is_some();
    let manifest = tokio::task::spawn_blocking(move || -> Result<Manifest> {
        let staging = StagingDir::new(&cache)?;
        let opti_stage = staging.path().join("optiscaler");
        extract_7z(&optiscaler_archive, &opti_stage, &cancel_bg)?;
        let opti_root = unwrap_single_root(&opti_stage);
        let opti_files = collect_files(&opti_root)?;

        let mut optiscaler_dll: Option<PathBuf> = None;
        let mut optiscaler_ini: Option<PathBuf> = None;
        let mut bundled_fakenvapi_dll: Option<(String, PathBuf)> = None;
        let mut bundled_fakenvapi_ini: Option<PathBuf> = None;
        let mut extra_files: Vec<(PathBuf, PathBuf)> = Vec::new();

        for (rel, full) in opti_files {
            let file_name = rel.file_name().and_then(|n| n.to_str()).unwrap_or_default();
            if rel.components().count() == 1 && file_name.eq_ignore_ascii_case("optiscaler.dll") {
                optiscaler_dll = Some(full);
            } else if rel.components().count() == 1
                && file_name.eq_ignore_ascii_case("optiscaler.ini")
            {
                optiscaler_ini = Some(full);
            } else if rel.components().count() == 1
                && (file_name.eq_ignore_ascii_case("nvapi64.dll")
                    || file_name.eq_ignore_ascii_case("fakenvapi.dll"))
            {
                bundled_fakenvapi_dll = Some((file_name.to_ascii_lowercase(), full));
            } else if rel.components().count() == 1
                && file_name.eq_ignore_ascii_case("fakenvapi.ini")
            {
                bundled_fakenvapi_ini = Some(full);
            } else if is_setup_script(&rel) {
                continue;
            } else {
                extra_files.push((rel, full));
            }
        }

        let dll_src =
            optiscaler_dll.context("The OptiScaler archive does not contain OptiScaler.dll")?;
        verify_x64_pe(&dll_src, "OptiScaler.dll")?;

        // Stage the archive's OptiScaler.ini template so unmodified installs
        // receive the new template on update, while user-modified or
        // pre-existing OptiScaler.ini files on disk are kept untouched by
        // update_staged / apply_staged and only surgically patched for
        // Irori-managed keys ([Plugins] and [fakenvapi]).
        let existing_ini_path = found.dir.join("OptiScaler.ini");
        let existing_ini_was_modified_or_untracked = if existing_ini_path.is_file() {
            match &previous_in_dir {
                Some(prev) => match prev
                    .files
                    .iter()
                    .find(|r| r.path == Path::new("OptiScaler.ini"))
                {
                    Some(rec) => {
                        let unmodified = std::fs::symlink_metadata(&existing_ini_path)
                            .ok()
                            .is_some_and(|m| m.is_file() && m.len() == rec.size)
                            && journal::sha256_file_with_size(&existing_ini_path)
                                .is_ok_and(|(h, _)| h == rec.sha256);
                        !unmodified
                    }
                    None => true,
                },
                None => true,
            }
        } else {
            false
        };
        let template_ini = match optiscaler_ini {
            Some(path) => read_small_text(&path)?,
            None => String::new(),
        };
        let patched_template_ini = patch_optiscaler_ini(&template_ini, &config);

        let mut staged = vec![
            StagedFile::from_file(config.proxy_dll.file_name(), dll_src, Existing::Refuse),
            StagedFile::from_bytes(
                "OptiScaler.ini",
                patched_template_ini.into_bytes(),
                Existing::Keep,
            ),
        ];

        for (rel, full) in extra_files {
            staged.push(StagedFile::from_file(rel, full, Existing::Keep));
        }

        let mut installed_nvapi64 = false;
        if let Some(fake_archive) = fakenvapi_archive {
            let fake_stage = staging.path().join("fakenvapi");
            extract_archive(&fake_archive, &fake_stage, &cancel_bg)?;
            let fake_files = collect_files(&fake_stage)?;

            let picked = fake_files
                .iter()
                .find_map(|(rel, full)| {
                    let name = rel.file_name()?.to_str()?;
                    if name.eq_ignore_ascii_case("fakenvapi.dll")
                        || name.eq_ignore_ascii_case("nvapi64.dll")
                    {
                        Some((name.to_ascii_lowercase(), full.clone()))
                    } else {
                        None
                    }
                })
                .or(bundled_fakenvapi_dll);
            let (fake_name, fake_src) = picked
                .context("The FakeNVAPI archive does not contain fakenvapi.dll or nvapi64.dll")?;
            verify_x64_pe(&fake_src, &fake_name)?;
            installed_nvapi64 = fake_name.eq_ignore_ascii_case("nvapi64.dll");
            staged.push(StagedFile::from_file(fake_name, fake_src, Existing::Keep));

            let fake_ini_raw = fake_files
                .iter()
                .find(|(rel, _)| {
                    rel.file_name()
                        .and_then(|n| n.to_str())
                        .is_some_and(|n| n.eq_ignore_ascii_case("fakenvapi.ini"))
                })
                .map(|(_, full)| read_small_text(full))
                .or_else(|| bundled_fakenvapi_ini.as_deref().map(read_small_text))
                .transpose()?
                .unwrap_or_else(|| "[fakenvapi]\r\n".to_string());
            let patched_fake_ini = patch_fakenvapi_ini(&fake_ini_raw, &[]);
            staged.push(StagedFile::from_bytes(
                "fakenvapi.ini",
                patched_fake_ini.into_bytes(),
                Existing::Keep,
            ));
        }

        if let Some(asi_src) = optipatcher_file {
            verify_x64_pe(&asi_src, "OptiPatcher.asi")?;
            staged.push(StagedFile::from_file(
                OPTIPATCHER_REL_PATH,
                asi_src,
                Existing::Keep,
            ));
        }

        let mut overrides = Vec::new();
        if let Some(stem) = config.proxy_dll.override_stem() {
            overrides.push(stem.to_string());
        }
        if installed_nvapi64 {
            overrides.push("nvapi64".to_string());
        }

        let patch_config = config.clone();
        let base = Manifest {
            kind: GraphicsMod::Optiscaler,
            version,
            exe: found.exe,
            machine: found.machine,
            api: found.api.unwrap_or(Api::Dxgi),
            dll: config.proxy_dll.file_name().to_string(),
            dir: found.dir.clone(),
            enabled: true,
            complete: false,
            overrides,
            files: Vec::new(),
            created_dirs: Vec::new(),
            addon: false,
            source: Some(ModSource {
                id: config.channel.id().to_string(),
                title: format!("OptiScaler ({})", config.channel.label()),
            }),
            optiscaler: Some(config),
        };

        let result_manifest = if let Some(prev) = &previous_in_dir {
            journal::update_staged(&journal_path, prev, base, staged, &|| {
                cancel_bg.is_cancelled()
            })?
        } else {
            journal::apply_staged(&journal_path, base, staged, &|| cancel_bg.is_cancelled())?
        };

        // If OptiScaler.ini was user-modified or pre-existing (kept untouched
        // by update_staged / apply_staged) and Irori-managed keys changed,
        // write only the surgically patched text atomically without claiming
        // ownership in manifest.files.
        if existing_ini_was_modified_or_untracked && existing_ini_path.is_file() {
            let current_text = read_small_text(&existing_ini_path)?;
            let surgically_patched = patch_optiscaler_ini(&current_text, &patch_config);
            if surgically_patched != current_text {
                json_store::write_bytes(&existing_ini_path, surgically_patched.as_bytes())
                    .with_context(|| format!("Could not save {}", existing_ini_path.display()))?;
            }
        }

        prune_cache_assets(
            &cache,
            &journal_path,
            &keep_assets,
            pruned_fakenvapi,
            pruned_optipatcher,
        );
        Ok(result_manifest)
    })
    .await
    .context("The OptiScaler install stopped")??;

    Ok(OptiscalerInstallResult {
        manifest,
        release,
        fakenvapi: fakenvapi_rel,
        optipatcher: optipatcher_rel,
    })
}

/// Reconfigure an existing OptiScaler installation in-place without updating
/// the OptiScaler binary version or re-extracting untouched archives.
///
/// - Changing `proxy_dll` performs a single atomic rename with zero disk copy
///   and zero hashing.
/// - Toggling `gpu_driver` or `enable_optipatcher` fetches/adds or removes only
///   the affected companion files and surgically patches `OptiScaler.ini`.
/// - Changing `shortcut_key` patches `[Menu] ShortcutKey` in `OptiScaler.ini`
///   in-place with zero network requests.
#[allow(clippy::too_many_arguments)]
pub async fn reconfigure(
    journal_path: PathBuf,
    mut manifest: Manifest,
    new_config: OptiscalerConfig,
    cache: PathBuf,
    endpoints: OptiscalerEndpoints,
    step: Arc<dyn Fn(Step) + Send + Sync>,
    progress: ProgressSink,
    cancel: CancellationToken,
) -> Result<Manifest> {
    manifest.checked()?;
    let old_config = manifest.optiscaler.clone().unwrap_or_default();

    let need_fakenvapi = old_config.gpu_driver != GpuDriverMode::MesaAuto
        && new_config.gpu_driver == GpuDriverMode::MesaAuto;
    let need_optipatcher = !old_config.enable_optipatcher && new_config.enable_optipatcher;

    let fakenvapi_archive = if need_fakenvapi {
        if cancel.is_cancelled() {
            bail!(irori_runtime::Cancelled);
        }
        step(Step::DownloadingFakenvapi);
        let rel = fetch_fakenvapi_release(&endpoints, &cache, false).await?;
        Some(download_verified_asset(&rel, &cache, &progress, &cancel).await?)
    } else {
        None
    };

    let optipatcher_file = if need_optipatcher {
        if cancel.is_cancelled() {
            bail!(irori_runtime::Cancelled);
        }
        step(Step::DownloadingOptipatcher);
        let rel = fetch_optipatcher_release(&endpoints, &cache, false).await?;
        Some(download_verified_asset(&rel, &cache, &progress, &cancel).await?)
    } else {
        None
    };

    if cancel.is_cancelled() {
        bail!(irori_runtime::Cancelled);
    }

    step(Step::Writing);
    let cancel_bg = cancel.clone();
    tokio::task::spawn_blocking(move || -> Result<Manifest> {
        let dir = manifest.dir.clone();
        if !dir.is_dir() {
            bail!("The game folder {} does not exist", dir.display());
        }

        // 1. Atomic proxy rename when ProxyDll changes (zero copy, zero re-hash).
        let new_dll = new_config.proxy_dll.file_name();
        let renamed_proxy: Option<(PathBuf, PathBuf, String)> =
            if !manifest.dll.eq_ignore_ascii_case(new_dll) {
                let from = manifest.dll_path();
                let mut next = manifest.clone();
                next.dll = new_dll.to_string();
                let to = next.dll_path();

                let new_active = dir.join(new_dll);
                let new_disabled = dir.join(format!("{new_dll}{}", journal::DISABLED_SUFFIX));
                if std::fs::symlink_metadata(&new_active).is_ok()
                    || std::fs::symlink_metadata(&new_disabled).is_ok()
                {
                    bail!(ReshadeError::DllTaken {
                        name: new_dll.to_string(),
                    });
                }
                if std::fs::symlink_metadata(&from).is_err() {
                    bail!(
                        "OptiScaler's {} is no longer in the game folder",
                        manifest.dll
                    );
                }
                std::fs::rename(&from, &to)
                    .with_context(|| format!("Could not rename {}", from.display()))?;
                Some((from, to, manifest.dll.clone()))
            } else {
                None
            };

        if let Some((ref from, ref to, ref old_dll)) = renamed_proxy {
            let old_rel = PathBuf::from(old_dll);
            for rec in &mut manifest.files {
                if rec.path == old_rel {
                    rec.path = PathBuf::from(new_dll);
                }
            }
            manifest.dll = new_dll.to_string();

            let mut updated_overrides = Vec::new();
            if let Some(stem) = new_config.proxy_dll.override_stem() {
                updated_overrides.push(stem.to_string());
            }
            let old_stem = ProxyDll::from_file_name(old_dll).and_then(|p| p.override_stem());
            for ov in &manifest.overrides {
                if Some(ov.as_str()) != old_stem
                    && !ov.eq_ignore_ascii_case("OptiScaler.asi")
                    && !updated_overrides.iter().any(|x| x.eq_ignore_ascii_case(ov))
                {
                    updated_overrides.push(ov.clone());
                }
            }
            manifest.overrides = updated_overrides;

            // Roll back the rename if subsequent companion work fails before save.
            if let Err(err) = apply_reconfigure_companions(
                &dir,
                &cache,
                &mut manifest,
                &old_config,
                &new_config,
                fakenvapi_archive.as_deref(),
                optipatcher_file.as_deref(),
                &cancel_bg,
            )
            .and_then(|()| {
                manifest.optiscaler = Some(new_config.clone());
                journal::save_manifest(&journal_path, &manifest)
            }) {
                let _ = std::fs::rename(to, from);
                return Err(err);
            }
        } else {
            apply_reconfigure_companions(
                &dir,
                &cache,
                &mut manifest,
                &old_config,
                &new_config,
                fakenvapi_archive.as_deref(),
                optipatcher_file.as_deref(),
                &cancel_bg,
            )?;
            manifest.optiscaler = Some(new_config);
            journal::save_manifest(&journal_path, &manifest)?;
        }

        Ok(manifest)
    })
    .await
    .context("The OptiScaler reconfiguration stopped")?
}

#[allow(clippy::too_many_arguments)]
fn apply_reconfigure_companions(
    dir: &Path,
    cache: &Path,
    manifest: &mut Manifest,
    old_config: &OptiscalerConfig,
    new_config: &OptiscalerConfig,
    fakenvapi_archive: Option<&Path>,
    optipatcher_file: Option<&Path>,
    cancel: &CancellationToken,
) -> Result<()> {
    // 2. FakeNVAPI companion updates.
    if old_config.gpu_driver != new_config.gpu_driver {
        match new_config.gpu_driver {
            GpuDriverMode::Nvidia => {
                for rel_str in ["fakenvapi.dll", "nvapi64.dll", "fakenvapi.ini"] {
                    remove_unmodified_tracked(dir, manifest, Path::new(rel_str));
                }
                manifest
                    .overrides
                    .retain(|o| !o.eq_ignore_ascii_case("nvapi64"));
            }
            GpuDriverMode::MesaAuto => {
                if let Some(fake_archive) = fakenvapi_archive {
                    let staging = StagingDir::new(cache)?;
                    let fake_stage = staging.path().join("fakenvapi");
                    extract_archive(fake_archive, &fake_stage, cancel)?;
                    let fake_files = collect_files(&fake_stage)?;

                    let picked = fake_files
                        .iter()
                        .find_map(|(rel, full)| {
                            let name = rel.file_name()?.to_str()?;
                            if name.eq_ignore_ascii_case("fakenvapi.dll")
                                || name.eq_ignore_ascii_case("nvapi64.dll")
                            {
                                Some((name.to_ascii_lowercase(), full.clone()))
                            } else {
                                None
                            }
                        })
                        .context(
                            "The FakeNVAPI archive does not contain fakenvapi.dll or nvapi64.dll",
                        )?;
                    let (fake_name, fake_src) = picked;
                    verify_x64_pe(&fake_src, &fake_name)?;
                    let fake_rel = PathBuf::from(&fake_name);
                    let fake_target = dir.join(&fake_rel);
                    if std::fs::symlink_metadata(&fake_target).is_err() {
                        ensure_no_symlinks(dir, &fake_rel)?;
                        let (sha256, size) = journal::sha256_file_with_size(&fake_src)?;
                        journal::copy_file_atomic_pub(&fake_src, &fake_target)?;
                        manifest.files.push(journal::FileRecord {
                            path: fake_rel,
                            sha256,
                            size,
                        });
                        if fake_name.eq_ignore_ascii_case("nvapi64.dll")
                            && !manifest
                                .overrides
                                .iter()
                                .any(|o| o.eq_ignore_ascii_case("nvapi64"))
                        {
                            manifest.overrides.push("nvapi64".to_string());
                        }
                    }

                    let ini_rel = PathBuf::from("fakenvapi.ini");
                    let ini_target = dir.join(&ini_rel);
                    if std::fs::symlink_metadata(&ini_target).is_err() {
                        let fake_ini_raw = fake_files
                            .iter()
                            .find(|(rel, _)| {
                                rel.file_name()
                                    .and_then(|n| n.to_str())
                                    .is_some_and(|n| n.eq_ignore_ascii_case("fakenvapi.ini"))
                            })
                            .map(|(_, full)| read_small_text(full))
                            .transpose()?
                            .unwrap_or_else(|| "[fakenvapi]\r\n".to_string());
                        let patched_fake_ini = patch_fakenvapi_ini(&fake_ini_raw, &[]);
                        let bytes = patched_fake_ini.into_bytes();
                        let sha256 = journal::sha256_bytes(&bytes);
                        let size = bytes.len() as u64;
                        json_store::write_bytes(&ini_target, &bytes)?;
                        manifest.files.push(journal::FileRecord {
                            path: ini_rel,
                            sha256,
                            size,
                        });
                    }
                }
            }
        }
    }

    // 3. OptiPatcher toggle.
    if old_config.enable_optipatcher != new_config.enable_optipatcher {
        let rel = PathBuf::from(OPTIPATCHER_REL_PATH);
        if new_config.enable_optipatcher {
            if let Some(asi_src) = optipatcher_file {
                verify_x64_pe(asi_src, "OptiPatcher.asi")?;
                let target = dir.join(&rel);
                if std::fs::symlink_metadata(&target).is_err() {
                    ensure_no_symlinks(dir, &rel)?;
                    if let Some(parent) = rel.parent()
                        && !parent.as_os_str().is_empty()
                    {
                        let parent_full = dir.join(parent);
                        if std::fs::symlink_metadata(&parent_full).is_err() {
                            std::fs::create_dir_all(&parent_full)?;
                            if !manifest.created_dirs.contains(&parent.to_path_buf()) {
                                manifest.created_dirs.push(parent.to_path_buf());
                            }
                        }
                    }
                    let (sha256, size) = journal::sha256_file_with_size(asi_src)?;
                    journal::copy_file_atomic_pub(asi_src, &target)?;
                    manifest.files.push(journal::FileRecord {
                        path: rel,
                        sha256,
                        size,
                    });
                }
            }
        } else {
            remove_unmodified_tracked(dir, manifest, &rel);
            manifest.created_dirs.retain(|d| {
                let still_used = manifest.files.iter().any(|r| r.path.starts_with(d));
                if !still_used {
                    let _ = std::fs::remove_dir(dir.join(d));
                }
                still_used
            });
        }
    }

    // 4. Surgical OptiScaler.ini patch when FakeNVAPI, OptiPatcher, or ShortcutKey changed.
    if old_config.gpu_driver != new_config.gpu_driver
        || old_config.enable_optipatcher != new_config.enable_optipatcher
        || old_config.shortcut_key != new_config.shortcut_key
    {
        let reset_shortcut = old_config.shortcut_key.is_some() && new_config.shortcut_key.is_none();
        patch_tracked_ini_in_place(dir, manifest, Path::new("OptiScaler.ini"), |raw| {
            let mut cfg = new_config.clone();
            if reset_shortcut {
                cfg.shortcut_key = Some(crate::optiscaler_keys::SHORTCUT_AUTO.to_string());
            }
            patch_optiscaler_ini(raw, &cfg)
        })?;
    }

    Ok(())
}

fn patch_tracked_ini_in_place(
    dir: &Path,
    manifest: &mut Manifest,
    ini_rel: &Path,
    patch_fn: impl FnOnce(&str) -> String,
) -> Result<()> {
    let ini_path = dir.join(ini_rel);
    if !ini_path.is_file() {
        return Ok(());
    }
    let raw = read_small_text(&ini_path)?;
    let patched = patch_fn(&raw);
    if patched == raw {
        return Ok(());
    }
    let tracked_unmodified = manifest
        .files
        .iter()
        .position(|r| r.path == ini_rel)
        .filter(|&idx| {
            let rec = &manifest.files[idx];
            std::fs::symlink_metadata(&ini_path)
                .ok()
                .is_some_and(|m| m.is_file() && m.len() == rec.size)
                && journal::sha256_file_with_size(&ini_path).is_ok_and(|(h, _)| h == rec.sha256)
        });
    let bytes = patched.into_bytes();
    json_store::write_bytes(&ini_path, &bytes)
        .with_context(|| format!("Could not save {}", ini_path.display()))?;
    if let Some(idx) = tracked_unmodified {
        manifest.files[idx].sha256 = journal::sha256_bytes(&bytes);
        manifest.files[idx].size = bytes.len() as u64;
    }
    Ok(())
}

fn remove_unmodified_tracked(dir: &Path, manifest: &mut Manifest, rel: &Path) {
    let Some(idx) = manifest.files.iter().position(|r| r.path == rel) else {
        return;
    };
    let rec = manifest.files[idx].clone();
    let target = dir.join(rel);
    if let Ok(meta) = std::fs::symlink_metadata(&target)
        && meta.is_file()
        && meta.len() == rec.size
        && journal::sha256_file_with_size(&target).is_ok_and(|(h, _)| h == rec.sha256)
        && std::fs::remove_file(&target).is_ok()
    {
        manifest.files.remove(idx);
    }
}

/// Remove older cached archives under `<cache>/assets/` for updated component
/// buckets once no installed OptiScaler manifest still references them.
pub fn prune_cache_assets(
    cache: &Path,
    journal_path: &Path,
    keep_paths: &[PathBuf],
    pruned_fakenvapi: bool,
    pruned_optipatcher: bool,
) {
    let assets_dir = cache.join("assets");
    let Ok(entries) = std::fs::read_dir(&assets_dir) else {
        return;
    };
    let in_use_versions = collect_installed_versions(journal_path);

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() || keep_paths.iter().any(|k| k == &path) {
            continue;
        }
        let lower = entry.file_name().to_string_lossy().to_ascii_lowercase();
        let is_optiscaler = lower.contains("optiscaler");
        let is_fakenvapi = lower.contains("fakenvapi");
        let is_optipatcher = lower.contains("optipatcher");

        let should_prune = (is_optiscaler && !version_in_use(&lower, &in_use_versions))
            || (pruned_fakenvapi && is_fakenvapi)
            || (pruned_optipatcher && is_optipatcher);

        if should_prune {
            let _ = std::fs::remove_file(&path);
        }
    }
}

fn collect_installed_versions(journal_path: &Path) -> Vec<String> {
    let mut versions = Vec::new();
    let Some(runner_dir) = journal_path.parent() else {
        return versions;
    };
    let Some(root) = runner_dir.parent() else {
        return versions;
    };
    // Only inspect runner subdirectories when journal lives in `<data>/reshade/optiscaler/<runner>/<app>.json`.
    let Ok(runners) = std::fs::read_dir(root) else {
        return versions;
    };
    for r in runners.flatten() {
        if !r.file_type().is_ok_and(|t| t.is_dir()) {
            continue;
        }
        let Ok(files) = std::fs::read_dir(r.path()) else {
            continue;
        };
        for f in files.flatten() {
            if f.path().extension().and_then(|e| e.to_str()) == Some("json")
                && let Ok(Some(m)) = journal::load(&f.path())
                && m.kind == GraphicsMod::Optiscaler
            {
                versions.push(m.version);
            }
        }
    }
    versions
}

fn version_in_use(lower_asset_name: &str, versions: &[String]) -> bool {
    versions.iter().any(|v| {
        let trimmed = v.trim();
        let core = trimmed
            .strip_prefix(['v', 'V'])
            .or_else(|| trimmed.strip_prefix("nightly-"))
            .unwrap_or(trimmed)
            .to_ascii_lowercase();
        let compact = core.replace('-', "");
        (!core.is_empty() && lower_asset_name.contains(&core))
            || (!compact.is_empty() && lower_asset_name.contains(&compact))
    })
}

/// Surgically add or remove `"<dll>"="native,builtin"` entries in the
/// `[Software\\Wine\\DllOverrides]` section of a Wine/Proton prefix `user.reg`.
///
/// Returns `Ok(true)` if the file was modified, `Ok(false)` if `user_reg` does
/// not exist yet or already matched the requested state.
pub fn sync_wine_user_reg_overrides(
    user_reg: &Path,
    add: &[&str],
    remove: &[&str],
) -> Result<bool> {
    if !user_reg.is_file() || (add.is_empty() && remove.is_empty()) {
        return Ok(false);
    }
    let raw_bytes = std::fs::read(user_reg)
        .with_context(|| format!("Could not read {}", user_reg.display()))?;
    let raw = String::from_utf8_lossy(&raw_bytes);
    let eol = if raw.contains("\r\n") { "\r\n" } else { "\n" };

    let add_clean: Vec<String> = add
        .iter()
        .map(|s| {
            s.trim()
                .strip_suffix(".dll")
                .or_else(|| s.trim().strip_suffix(".DLL"))
                .unwrap_or(s.trim())
                .to_ascii_lowercase()
        })
        .filter(|s| !s.is_empty() && !s.ends_with(".asi"))
        .collect();
    let remove_clean: Vec<String> = remove
        .iter()
        .map(|s| {
            s.trim()
                .strip_suffix(".dll")
                .or_else(|| s.trim().strip_suffix(".DLL"))
                .unwrap_or(s.trim())
                .to_ascii_lowercase()
        })
        .filter(|s| !s.is_empty() && !add_clean.iter().any(|a| a == s))
        .collect();

    if add_clean.is_empty() && remove_clean.is_empty() {
        return Ok(false);
    }

    let mut out_lines: Vec<String> = Vec::new();
    let mut in_overrides = false;
    let mut found_section = false;
    let mut applied_add = vec![false; add_clean.len()];

    let flush_adds = |out: &mut Vec<String>, applied: &mut [bool]| {
        let mut trailing_blanks = 0usize;
        while out
            .len()
            .checked_sub(trailing_blanks + 1)
            .and_then(|idx| out.get(idx))
            .is_some_and(|l| l.trim().is_empty())
        {
            trailing_blanks += 1;
        }
        let insert_at = out.len() - trailing_blanks;
        let mut to_insert = Vec::new();
        for (idx, name) in add_clean.iter().enumerate() {
            if !applied[idx] {
                to_insert.push(format!("\"{name}\"=\"native,builtin\"{eol}"));
                applied[idx] = true;
            }
        }
        if !to_insert.is_empty() {
            if insert_at > 0 && !out[insert_at - 1].ends_with('\n') {
                out[insert_at - 1].push_str(eol);
            }
            out.splice(insert_at..insert_at, to_insert);
        }
    };

    for raw_line in raw.split_inclusive('\n') {
        let (line_no_eol, line_eol) = if let Some(s) = raw_line.strip_suffix("\r\n") {
            (s, "\r\n")
        } else if let Some(s) = raw_line.strip_suffix('\n') {
            (s, "\n")
        } else {
            (raw_line, "")
        };
        let trimmed = line_no_eol.trim_start();
        if trimmed.starts_with('[')
            && let Some(end) = trimmed.find(']')
        {
            if in_overrides {
                flush_adds(&mut out_lines, &mut applied_add);
                in_overrides = false;
            }
            let sec = &trimmed[1..end];
            if sec.eq_ignore_ascii_case(r"Software\\Wine\\DllOverrides") {
                in_overrides = true;
                found_section = true;
            }
            out_lines.push(raw_line.to_string());
            continue;
        }

        if in_overrides
            && let Some(rest) = trimmed.strip_prefix('"')
            && let Some(end_q) = rest.find('"')
        {
            let key = &rest[..end_q];
            if remove_clean.iter().any(|r| r.eq_ignore_ascii_case(key)) {
                continue;
            }
            if let Some(idx) = add_clean.iter().position(|a| a.eq_ignore_ascii_case(key)) {
                applied_add[idx] = true;
                out_lines.push(format!(
                    "\"{}\"=\"native,builtin\"{line_eol}",
                    add_clean[idx]
                ));
                continue;
            }
        }
        out_lines.push(raw_line.to_string());
    }

    if in_overrides {
        flush_adds(&mut out_lines, &mut applied_add);
    }

    if !found_section && applied_add.iter().any(|a| !*a) {
        if let Some(last) = out_lines.last_mut()
            && !last.ends_with('\n')
        {
            last.push_str(eol);
        }
        if out_lines.last().is_some_and(|l| !l.trim().is_empty()) {
            out_lines.push(eol.to_string());
        }
        out_lines.push(format!("[Software\\\\Wine\\\\DllOverrides]{eol}"));
        for (idx, name) in add_clean.iter().enumerate() {
            if !applied_add[idx] {
                out_lines.push(format!("\"{name}\"=\"native,builtin\"{eol}"));
                applied_add[idx] = true;
            }
        }
    }

    let mut result = String::with_capacity(raw.len() + 64);
    for line in out_lines {
        result.push_str(&line);
    }
    if result.as_bytes() == raw_bytes {
        return Ok(false);
    }
    json_store::write_bytes(user_reg, result.as_bytes())
        .with_context(|| format!("Could not save {}", user_reg.display()))?;
    Ok(true)
}

#[cfg(test)]
#[path = "optiscaler_tests.rs"]
mod tests;
