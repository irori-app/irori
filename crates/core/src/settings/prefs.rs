use serde::{Deserialize, Serialize};

/// How often Irori backs up on its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackupSchedule {
    Off,
    Daily,
    #[default]
    Weekly,
}

/// Scheduled backups: when, where, how many and what goes in. Store
/// sign-ins never do.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct BackupPrefs {
    pub schedule: BackupSchedule,
    /// Where backups go ("" = the `backups` folder in Irori's data folder).
    pub folder: String,
    /// How many scheduled backups to keep; older ones are removed.
    pub keep: u32,
    /// Back up before a new version of Irori first runs.
    pub before_updates: bool,
    pub settings: bool,
    pub game_settings: bool,
    pub library: bool,
    pub playtime: bool,
    /// Where store games are installed, so a restore attaches them again.
    pub installs: bool,
    /// Copy the downloaded game art too (large; it can be downloaded again).
    pub art: bool,
}

impl Default for BackupPrefs {
    fn default() -> Self {
        Self {
            schedule: BackupSchedule::Weekly,
            folder: String::new(),
            keep: 10,
            before_updates: true,
            settings: true,
            game_settings: true,
            library: true,
            playtime: true,
            installs: true,
            art: false,
        }
    }
}

/// Where the library syncs with Irori on other computers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncMode {
    #[default]
    Off,
    /// A folder something else keeps in sync (Syncthing, Dropbox, a share).
    Folder,
    /// A WebDAV server; the login is kept with `irori_secrets`.
    Webdav,
    /// A remote of the user's own rclone (`rcloneRemote`); rclone keeps the
    /// account.
    Rclone,
}

/// Library sync between computers.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct SyncPrefs {
    pub mode: SyncMode,
    pub folder: String,
    /// The WebDAV folder's address, without the login.
    pub webdav_url: String,
    /// What other computers call this one ("" = its host name).
    pub device_name: String,
    /// The rclone remote, with an optional folder (`gdrive:Games`).
    pub rclone_remote: String,
    /// The rclone program; empty finds it on `PATH`.
    pub rclone_path: String,
    /// Cloud copies of backups, Ludusavi's saves and screenshots at the
    /// sync target.
    pub copies: CopyPrefs,
    /// Exchange only playtime and sessions: this computer's favourites,
    /// categories and statuses stay here, and the others' don't change it.
    pub playtime_only: bool,
}

/// When a kind of cloud copy is sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CopyMode {
    #[default]
    Off,
    /// Only when the user asks ("Send now").
    Manual,
    /// On its own, every so often, never while a game runs.
    Automatic,
}

/// What goes to the sync target besides the library.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct CopyPrefs {
    /// Irori's scheduled backups (never the store sign-ins).
    pub backups: CopyMode,
    /// Ludusavi's backup folder.
    pub saves: CopyMode,
    /// The screenshots Irori finds (not clips).
    pub screenshots: CopyMode,
}

/// Which of Irori's games Sunshine lists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SunshineGames {
    /// Every installed game the library shows.
    #[default]
    Installed,
    /// Installed favourites only.
    Favourites,
}

/// How unlocks show while a game runs (`iroriAchievementToast`), with
/// `iroriAchievementNotices` on: a card in a corner above the game, with
/// the achievement's name and the lines chosen here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct AchievementToastPrefs {
    /// Show the card above the game. Off, or where nothing can show above
    /// it, unlocks are announced as a notice (on the desktop while Irori is
    /// in the background).
    pub over_game: bool,
    /// The game's title.
    pub show_game: bool,
    pub show_description: bool,
    /// The share of players who have it.
    pub show_rarity: bool,
    /// How many of the game's achievements are unlocked now.
    pub show_progress: bool,
    /// What it is worth, for stores whose achievements have points.
    pub show_points: bool,
}

impl Default for AchievementToastPrefs {
    fn default() -> Self {
        Self {
            over_game: true,
            show_game: false,
            show_description: true,
            show_rarity: true,
            show_progress: true,
            show_points: false,
        }
    }
}

/// The in-game overlay (`iroriOverlay`): Guide over a game Irori started
/// opens it instead of Big Picture. Off by default while it is in beta.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct OverlayPrefs {
    pub enabled: bool,
    /// Where the panel sits on the screen.
    pub side: OverlaySide,
    /// Guide + RB (R1) takes a screenshot.
    pub screenshot_chord: bool,
    /// Windows and macOS: Ctrl+Shift+S (⌃⇧S) takes a screenshot while a game
    /// Irori started runs, with or without the overlay.
    pub screenshot_key: bool,
    /// Guide + Up/Down change the system's volume.
    pub volume_chords: bool,
    /// Guide + Y (Triangle) marks the moment: a screenshot and a journal
    /// note to write later (`momentChord`).
    pub moment_chord: bool,
    /// How far one press of a volume control moves a volume, in percent
    /// (see [`Self::volume_step`]).
    pub volume_step: u32,
    /// After Guide + Up/Down the new level shows over the game for a moment.
    pub level_notice: bool,
    /// Back + Start (View + Menu) together open and close the overlay where
    /// Guide can't be read while a game has the focus (Windows' XInput).
    pub back_start: bool,
}

impl Default for OverlayPrefs {
    fn default() -> Self {
        Self {
            enabled: false,
            side: OverlaySide::Center,
            screenshot_chord: true,
            screenshot_key: true,
            volume_chords: true,
            moment_chord: true,
            volume_step: 5,
            level_notice: true,
            back_start: true,
        }
    }
}

impl OverlayPrefs {
    /// [`Self::volume_step`] within the offered range, 1 to 25 percent.
    pub fn volume_step(&self) -> i32 {
        self.volume_step.clamp(1, 25) as i32
    }
}

/// What applies a game's frame limit (`iroriFpsLimiter`). Values a newer
/// Irori writes read as [`FpsLimiter::Auto`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FpsLimiter {
    /// gamescope, else a shown MangoHud, else DXVK, else a hidden MangoHud.
    #[default]
    Auto,
    Gamescope,
    #[serde(rename = "mangohud")]
    MangoHud,
    Dxvk,
}

impl<'de> Deserialize<'de> for FpsLimiter {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(match String::deserialize(deserializer)?.as_str() {
            "gamescope" => FpsLimiter::Gamescope,
            "mangohud" => FpsLimiter::MangoHud,
            "dxvk" => FpsLimiter::Dxvk,
            _ => FpsLimiter::Auto,
        })
    }
}

/// Which build the Wine manager installs where a source publishes one for
/// x86-64-v3 processors beside the baseline one (`iroriProtonCpuLevel`).
/// Values a newer Irori writes read as [`ProtonCpuLevel::Auto`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProtonCpuLevel {
    /// The x86-64-v3 build when this processor runs it.
    #[default]
    Auto,
    /// Always the baseline x86-64 build.
    Baseline,
    /// Always the x86-64-v3 build.
    V3,
}

impl<'de> Deserialize<'de> for ProtonCpuLevel {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(match String::deserialize(deserializer)?.as_str() {
            "baseline" => ProtonCpuLevel::Baseline,
            "v3" => ProtonCpuLevel::V3,
            _ => ProtonCpuLevel::Auto,
        })
    }
}

/// Where the in-game overlay's panel sits. Values a newer Irori writes read
/// as [`OverlaySide::Center`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OverlaySide {
    #[default]
    Center,
    /// Along the left edge, the height of the screen.
    Left,
    /// Along the right edge, the height of the screen.
    Right,
}

impl<'de> Deserialize<'de> for OverlaySide {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(match String::deserialize(deserializer)?.as_str() {
            "left" => OverlaySide::Left,
            "right" => OverlaySide::Right,
            _ => OverlaySide::Center,
        })
    }
}

/// Irori's games in Sunshine (`iroriSunshine`). Sunshine's own login is
/// kept with `irori_secrets`, not here.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct SunshinePrefs {
    /// Keep Irori's games among Sunshine's applications.
    pub enabled: bool,
    pub games: SunshineGames,
    /// Add an application that opens Big Picture.
    pub big_picture: bool,
    /// Sunshine's web UI port; 0 reads it from Sunshine's configuration.
    pub port: u16,
    /// Sunshine on another computer: its web UI's address (`htpc.local`,
    /// `192.168.1.20:47990`); empty for this computer.
    pub address: String,
    /// Games in gamescope take the Moonlight client's resolution, frame rate
    /// and HDR when a stream starts them, unless the game sets its own.
    pub match_client: bool,
}

impl Default for SunshinePrefs {
    fn default() -> Self {
        Self {
            enabled: false,
            games: SunshineGames::Installed,
            big_picture: true,
            port: 0,
            address: String::new(),
            match_client: true,
        }
    }
}

/// Global defaults inherited by per-game OptiScaler setups (`iroriOptiscaler`).
/// Stored as plain strings/flags to avoid coupling `irori-core` to `irori-reshade`.
/// Irori-only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct OptiscalerDefaults {
    /// Release channel: `"stable"` or `"nightly"`.
    pub channel: String,
    /// GPU driver mode: `"mesaAuto"` or `"nvidia"`.
    pub gpu_driver: String,
    /// Proxy file name (`"dxgi.dll"`, `"version.dll"`, `"winmm.dll"`, `"OptiScaler.asi"`, ...).
    pub proxy_dll: String,
    /// Install `plugins/OptiPatcher.asi` and enable `LoadAsiPlugins=true`.
    pub enable_optipatcher: bool,
    /// Default `[Menu] ShortcutKey` Windows VK hex code (e.g. `"0x2E"`, `"0x24"`),
    /// or empty for OptiScaler's default (`auto` / Insert).
    pub shortcut_key: String,
}

impl Default for OptiscalerDefaults {
    fn default() -> Self {
        Self {
            channel: "stable".into(),
            gpu_driver: "mesaAuto".into(),
            proxy_dll: "dxgi.dll".into(),
            enable_optipatcher: false,
            shortcut_key: String::new(),
        }
    }
}

/// Default concurrency for direct-link and BitTorrent downloads
/// (`iroriWebDownloads.maxConcurrent`).
pub const DEFAULT_WEB_DOWNLOAD_CONCURRENT: u32 = 3;

/// Settings for direct HTTP/HTTPS link and opt-in BitTorrent downloads
/// (`iroriWebDownloads`). Irori-only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct WebDownloadPrefs {
    /// Where downloaded files are saved (`""` resolves to portable `Downloads`,
    /// the OS Downloads folder's `Irori` subfolder, `~/Downloads/Irori`, or
    /// `defaultInstallPath`).
    pub default_folder: String,
    /// Active concurrent web downloads (`1..=5`, default `3`).
    pub max_concurrent: u32,
    /// Extract downloaded archives automatically on completion.
    pub auto_extract: bool,
    /// Move the original archive to Trash after a verified extraction.
    pub delete_archive_after_extract: bool,
    /// Register detected portable games in the library automatically.
    pub auto_add_to_library: bool,
    /// Opt-in BitTorrent support (`.torrent` files and `magnet:` links via local `aria2c`).
    pub bittorrent: bool,
    /// Custom path to the `aria2c` executable (`""` = automatic discovery).
    pub aria2_path: String,
}

impl Default for WebDownloadPrefs {
    fn default() -> Self {
        Self {
            default_folder: String::new(),
            max_concurrent: DEFAULT_WEB_DOWNLOAD_CONCURRENT,
            auto_extract: true,
            delete_archive_after_extract: false,
            auto_add_to_library: true,
            bittorrent: false,
            aria2_path: String::new(),
        }
    }
}

impl WebDownloadPrefs {
    /// [`Self::max_concurrent`] clamped to the supported `1..=5` range.
    pub fn clamped_max_concurrent(&self) -> usize {
        self.max_concurrent.clamp(1, 5) as usize
    }

    /// Resolve the effective download folder from this setting and platform defaults.
    pub fn resolved_default_folder(&self, default_install_path: &str) -> std::path::PathBuf {
        resolve_web_download_folder_with(
            &self.default_folder,
            crate::paths::portable_root(),
            dirs::download_dir(),
            dirs::home_dir(),
            default_install_path,
        )
    }
}

/// Pure folder resolution helper for [`WebDownloadPrefs::resolved_default_folder`].
pub(crate) fn resolve_web_download_folder_with(
    configured: &str,
    portable_root: Option<std::path::PathBuf>,
    os_downloads: Option<std::path::PathBuf>,
    home_dir: Option<std::path::PathBuf>,
    default_install_path: &str,
) -> std::path::PathBuf {
    let trimmed = configured.trim();
    if !trimmed.is_empty() {
        return crate::paths::expand_home(trimmed);
    }
    if let Some(root) = portable_root {
        return root.join("Downloads");
    }
    if let Some(downloads) = os_downloads {
        return downloads.join("Irori");
    }
    if let Some(home) = home_dir {
        return home.join("Downloads").join("Irori");
    }
    let install = default_install_path.trim();
    if !install.is_empty() {
        return crate::paths::expand_home(install);
    }
    crate::paths::Paths::get().data.join("downloads")
}
