//! Public domain types and traits for direct-link and BitTorrent web downloads.

use std::fmt;
use std::path::{Path, PathBuf};

use irori_core::{GameId, InstallPlatform};
use serde::{Deserialize, Serialize};

/// Unique identifier for a direct-link or BitTorrent download item.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub struct WebDownloadId(pub String);

impl WebDownloadId {
    /// Generate a fresh unique download identifier.
    pub fn new() -> Self {
        Self(uuid::Uuid::new_v4().simple().to_string())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for WebDownloadId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<String> for WebDownloadId {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for WebDownloadId {
    fn from(value: &str) -> Self {
        Self(value.to_string())
    }
}

/// Transport kind of a web download.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WebDownloadKind {
    #[default]
    DirectUrl,
    TorrentFile,
    Magnet,
}

impl WebDownloadKind {
    pub fn is_bittorrent(self) -> bool {
        matches!(self, Self::TorrentFile | Self::Magnet)
    }
}

/// Lifecycle state of a web download.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WebDownloadState {
    #[default]
    Queued,
    FetchingMetadata,
    Running,
    Extracting,
    Installing,
    Paused,
    Done,
    Failed,
    Canceled,
}

impl WebDownloadState {
    /// Whether this item belongs in the active list (not finished/history).
    pub fn is_active(self) -> bool {
        matches!(
            self,
            Self::Queued
                | Self::FetchingMetadata
                | Self::Running
                | Self::Extracting
                | Self::Installing
                | Self::Paused
        )
    }

    /// Whether work is currently executing (network transfer, extraction, or setup).
    pub fn is_busy(self) -> bool {
        matches!(
            self,
            Self::FetchingMetadata | Self::Running | Self::Extracting | Self::Installing
        )
    }

    /// Whether the item is currently transferring over the network.
    pub fn is_network_active(self) -> bool {
        matches!(self, Self::FetchingMetadata | Self::Running)
    }

    /// Whether the item is in a terminal state.
    pub fn is_finished(self) -> bool {
        matches!(self, Self::Done | Self::Failed | Self::Canceled)
    }
}

/// Primary post-download action offered on a completed web download.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PostAction {
    #[default]
    None,
    /// Portable or installed game added to the library, ready to launch in one click.
    Play(GameId),
    /// Detected silent-capable setup program (Inno Setup, NSIS, or MSI).
    InstallSetup { installer: PathBuf, kind: String },
    /// Non-game file or archive requiring manual handling.
    ShowInFolder,
}

/// One direct-link or BitTorrent download entry.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct WebDownload {
    pub id: WebDownloadId,
    pub title: String,
    pub kind: WebDownloadKind,
    /// Original URL, magnet URI, or `.torrent` filename.
    pub source: String,
    /// Persisted copy of the `.torrent` file under `state/web-downloads/<id>.torrent`
    /// so Flatpak portal paths survive restarts.
    pub stored_torrent: Option<PathBuf>,
    /// 1-based aria2 `select-file` indices when a subset of files was picked.
    pub selected_files: Vec<usize>,
    /// Target directory where the downloaded file(s) are saved.
    pub destination: PathBuf,
    /// Downloaded file or folder path once resolved.
    pub output_path: Option<PathBuf>,
    /// Extracted or installed game directory in `default_install_path`, if any.
    pub installed_dir: Option<PathBuf>,
    /// Extract downloaded archives automatically.
    pub auto_extract: bool,
    /// Move original archive to Trash after a verified extraction.
    pub delete_archive_after_extract: bool,
    /// Auto-add detected portable games to the library.
    pub auto_add_to_library: bool,
    pub state: WebDownloadState,
    pub post_action: PostAction,
    /// Non-fatal informational note (e.g. `"Requires manual extraction"`).
    pub status_note: Option<String>,
    pub error: Option<String>,
    /// Unix timestamps (seconds).
    pub added_at: u64,
    pub started_at: Option<u64>,
    pub finished_at: Option<u64>,
    /// Expected total bytes (`0` = unknown).
    pub bytes_total: u64,
    /// Bytes downloaded so far.
    pub bytes_done: u64,
    /// Live network speed in bytes/sec (not persisted).
    #[serde(skip)]
    pub download_speed: u64,
    /// Live ETA in seconds (not persisted).
    #[serde(skip)]
    pub eta_secs: Option<u64>,
    /// Active BitTorrent peer count (not persisted).
    #[serde(skip)]
    pub peers: Option<u32>,
    /// Active BitTorrent seeder count (not persisted).
    #[serde(skip)]
    pub seeders: Option<u32>,
    /// Whether the user explicitly paused this item (never auto-resumed on reconnect).
    pub user_paused: bool,
    /// Whether `title` was explicitly provided by the caller and must not be overwritten.
    pub custom_title: bool,
    /// Active aria2 GID while running (not persisted).
    #[serde(skip)]
    pub aria2_gid: Option<String>,
}

impl WebDownload {
    /// Percentage completed in `0.0..=100.0`, or `None` when total size is unknown.
    pub fn progress_percent(&self) -> Option<f32> {
        (self.bytes_total > 0).then(|| {
            ((self.bytes_done as f64 / self.bytes_total as f64) * 100.0).clamp(0.0, 100.0) as f32
        })
    }

    /// Remaining bytes still to write (`0` when size is unknown or complete).
    pub fn remaining_bytes(&self) -> u64 {
        self.bytes_total.saturating_sub(self.bytes_done)
    }

    /// Best path to reveal in the file manager.
    pub fn reveal_path(&self) -> PathBuf {
        self.installed_dir
            .clone()
            .or_else(|| self.output_path.clone())
            .unwrap_or_else(|| self.destination.clone())
    }
}

/// Request to enqueue a new direct-link or BitTorrent download.
#[derive(Debug, Clone, Default)]
pub struct WebDownloadRequest {
    /// `http(s)://...`, `magnet:...`, or local path to a `.torrent` file.
    pub source: String,
    /// Optional display title override.
    pub title: Option<String>,
    /// Optional destination folder (`None` uses the resolved default folder).
    pub destination: Option<PathBuf>,
    /// Optional 1-based aria2 `select-file` indices (empty = download all files).
    pub selected_files: Vec<usize>,
    /// Optional raw `.torrent` bytes already fetched during a preview step.
    pub torrent_bytes: Option<Vec<u8>>,
    /// Per-download post-processing overrides (`None` inherits from `WebDownloadPrefs`).
    pub auto_extract: Option<bool>,
    pub delete_archive_after_extract: Option<bool>,
    pub auto_add_to_library: Option<bool>,
    /// Optional pre-known total byte size (e.g. from [`TorrentPreview`]).
    pub total_bytes: u64,
}

/// Read-only snapshot of the web download manager for the UI.
#[derive(Debug, Clone, Default)]
pub struct WebDownloadSnapshot {
    /// Queued, running, fetching metadata, extracting, installing, or paused items.
    pub active: Vec<WebDownload>,
    /// Completed, failed, or canceled items, most recent first.
    pub finished: Vec<WebDownload>,
    /// Whether the web download queue is globally paused.
    pub paused: bool,
    /// Aggregate live download speed across active web downloads (bytes/sec).
    pub total_speed: u64,
}

/// One user-facing file inside a torrent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TorrentFileEntry {
    /// Exact 1-based aria2 `select-file` index (preserving `.pad` offsets).
    pub index: usize,
    /// Normalized `/`-separated relative path inside the torrent.
    pub path: String,
    /// File size in bytes.
    pub length: u64,
}

/// Metadata preview of a `.torrent` file or resolved `magnet:` link.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TorrentPreview {
    pub name: String,
    /// Lowercase 40-character hex SHA-1 BitTorrent v1 info hash.
    pub info_hash: String,
    pub total_bytes: u64,
    /// User-facing files (`.pad` alignment entries omitted; original 1-based indices kept).
    pub files: Vec<TorrentFileEntry>,
    /// Raw `.torrent` payload captured during preview (e.g. BEP-9 magnet metadata),
    /// allowing enqueue to skip a second metadata fetch.
    #[serde(skip)]
    pub torrent_bytes: Option<Vec<u8>>,
}

/// Classified user input for the unified `+ Add download` input bar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DownloadSource {
    DirectUrl(url::Url),
    RemoteTorrentUrl(url::Url),
    LocalTorrent(PathBuf),
    Magnet {
        uri: String,
        display_name: Option<String>,
        info_hash: String,
    },
}

/// Availability of the external `aria2c` binary for opt-in BitTorrent downloads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Aria2Status {
    Ready {
        path: PathBuf,
        on_host: bool,
    },
    Missing {
        can_auto_install: bool,
        install_hint: String,
    },
}

/// A playable game discovered after extracting or installing a web download.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SideloadCandidate {
    pub title: String,
    pub executable: PathBuf,
    pub install_folder: Option<PathBuf>,
    pub platform: InstallPlatform,
    pub launcher_args: String,
    pub cover_art: Option<String>,
    pub background_art: Option<String>,
}

/// Bridge injected by the app crate (`crates/irori/src/stores.rs`) so
/// `irori-services` can scan extracted games and register sideloaded entries
/// without depending on `irori-sideload`.
#[irori_core::async_trait]
pub trait SideloadBridge: Send + Sync + 'static {
    /// Scan an extracted folder (or standalone executable) for one playable game.
    async fn scan_extracted(&self, root: &Path) -> Option<SideloadCandidate>;

    /// Register `candidate` in the sideload store, returning the existing [`GameId`]
    /// if the same executable or install folder is already registered.
    async fn register_game(&self, candidate: SideloadCandidate) -> anyhow::Result<GameId>;
}
