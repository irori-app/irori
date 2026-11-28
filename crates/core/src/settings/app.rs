use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::Runner;

use super::{
    AchievementToastPrefs, BackupPrefs, DEFAULT_ART_CACHE_LIMIT, DEFAULT_STEAM_TAG_LIMIT,
    GameSettings, OverlayPrefs, ProtonCpuLevel, SunshinePrefs, SyncPrefs, UiPrefs,
};

/// Layout of the Stores → Deals view (`iroriDealsLayout`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DealsLayout {
    #[default]
    Grid,
    List,
}

/// What the library shows above the grid.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LibraryTopSection {
    Disabled,
    #[default]
    RecentlyPlayed,
    RecentlyPlayedInstalled,
    Favourites,
}

/// Snapshots of each game's saves kept by default (`iroriSaveSnapshots`).
pub const DEFAULT_SAVE_SNAPSHOTS: u32 = 5;

/// Other launchers Irori keeps in sync with (`iroriLauncherImports`): read
/// again at start and every quarter of an hour, and only the games not seen
/// before are imported. Their own files are only read.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct LauncherImports {
    pub lutris: bool,
    pub bottles: bool,
    pub faugus: bool,
}

impl LauncherImports {
    pub fn any(&self) -> bool {
        self.lutris || self.bottles || self.faugus
    }
}

/// Application-wide settings. Heroic stores these next to the default game
/// settings in one object; `game` is flattened to keep that shape.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct AppSettings {
    #[serde(flatten)]
    pub game: GameSettings,
    pub add_desktop_shortcuts: bool,
    pub add_start_menu_shortcuts: bool,
    pub add_steam_shortcuts: bool,
    pub auto_update_games: bool,
    pub check_for_updates_on_startup: bool,
    /// Minutes between background update checks.
    pub check_updates_interval: u32,
    pub custom_wine_paths: Vec<String>,
    pub dark_tray_icon: bool,
    /// Use the desktop icon theme's generic games icon (`applications-games`
    /// or `applications-games-symbolic`) in the Linux system tray instead of
    /// Irori's logo. Irori-only.
    #[serde(rename = "iroriGenericTrayIcon")]
    pub generic_tray_icon: bool,
    pub default_install_path: String,
    /// Install each store's games in a folder of the store's own inside
    /// `defaultInstallPath` (`Games/Irori/GOG/<game>`). Irori-only.
    #[serde(rename = "iroriInstallPerStore")]
    pub install_per_store: bool,
    /// More folders games install into beside `defaultInstallPath`, each
    /// with a name (another drive, an SD card). Irori-only.
    #[serde(rename = "iroriLibraryFolders")]
    pub library_folders: Vec<LibraryFolder>,
    pub default_steam_path: String,
    pub default_wine_prefix_dir: String,
    pub disable_controller: bool,
    /// How controller input behaves (`iroriController`). Irori-only.
    #[serde(rename = "iroriController")]
    pub controller: crate::controller_prefs::ControllerPrefs,
    /// Big Picture's sounds and idle screen (`iroriBigPicture`). Irori-only.
    #[serde(rename = "iroriBigPicture")]
    pub big_picture: crate::big_picture_prefs::BigPicturePrefs,
    pub disable_playtime_sync: bool,
    pub disable_logs: bool,
    pub disable_animations: bool,
    #[serde(rename = "discordRPC")]
    pub discord_rpc: bool,
    /// A Discord application ID of the user's own for Rich Presence,
    /// replacing [`DISCORD_APPLICATION_ID`]. Empty uses Irori's.
    pub discord_client_id: String,
    #[serde(rename = "disableGOGPresence")]
    pub disable_gog_presence: bool,
    pub download_no_https: bool,
    pub download_proton_to_steam: bool,
    /// Proton-CachyOS and Proton-Wineland builds for x86-64-v3 processors.
    /// Linux only; Irori-only.
    #[serde(rename = "iroriProtonCpuLevel")]
    pub proton_cpu_level: ProtonCpuLevel,
    pub egs_linked_path: String,
    pub exit_to_tray: bool,
    pub gamepad_repeat_delay: u32,
    pub gamepad_initial_repeat_delay: u32,
    pub no_tray_icon: bool,
    pub hide_changelogs_on_startup: bool,
    pub hide_window_on_protocol_launch: bool,
    pub library_top_section: LibraryTopSection,
    pub max_recent_games: u32,
    /// Download workers (0 = store default).
    pub max_workers: u32,
    /// Download bandwidth limit in KiB/s (0 = unlimited).
    pub max_download_speed: u32,
    pub minimize_on_launch: bool,
    pub start_in_console_mode: bool,
    pub start_in_tray: bool,
    /// Start Irori when the user logs in. Irori-only.
    #[serde(rename = "iroriStartWithSystem")]
    pub start_with_system: bool,
    /// When started with the system, start in the tray. Irori-only.
    #[serde(rename = "iroriStartWithSystemMinimized")]
    pub start_with_system_minimized: bool,
    pub allow_installation_broken_anticheat: bool,
    pub show_valve_proton: bool,
    pub steam_grid_db_api_key: String,
    pub comet_support: bool,
    pub zoom_platform: bool,
    /// The itch.io store (off until turned on). Irori-only.
    #[serde(rename = "iroriItchIo", alias = "emberItchIo")]
    pub itch_io: bool,
    /// The Xbox store: Microsoft Store and PC Game Pass games, Linux only
    /// (off until turned on). Irori-only.
    #[serde(rename = "iroriXbox")]
    pub xbox: bool,
    /// The Microsoft Store market (two-letter region) the Xbox store's
    /// library, catalog and licences use; empty follows the interface
    /// language's region, else `US`. Irori-only.
    #[serde(rename = "iroriXboxMarket")]
    pub xbox_market: String,
    /// UI language code ("en", "de", …).
    #[serde(rename = "uiLanguage")]
    pub ui_language: String,
    #[serde(rename = "iroriUi", alias = "emberUi")]
    pub ui: UiPrefs,
    /// Where offline installers and goodies are saved (each game in its own
    /// folder inside).
    #[serde(rename = "iroriExtrasPath", alias = "emberExtrasPath")]
    pub extras_path: String,
    /// macOS: where CrossOver keeps its bottles, when moved from
    /// `~/Library/Application Support/CrossOver/Bottles` ("" = there).
    #[serde(
        rename = "iroriCrossoverBottlesPath",
        alias = "emberCrossoverBottlesPath"
    )]
    pub crossover_bottles_path: String,
    /// Where the Wine manager installs the Wine and Proton builds it
    /// downloads, in `wine` and `proton` folders inside ("" = Irori's tools
    /// folder). Builds already installed stay where they are. Irori-only.
    #[serde(rename = "iroriCompatToolsPath")]
    pub compat_tools_path: String,
    /// The default emulator of each system (`psx` → `duckstation`); systems
    /// not listed use the first installed recommended emulator.
    #[serde(rename = "iroriSystemEngines", alias = "emberSystemEngines")]
    pub system_engines: BTreeMap<String, String>,
    /// Folders scanned for console and retro games.
    #[serde(rename = "iroriRomFolders", alias = "emberRomFolders")]
    pub rom_folders: Vec<String>,
    /// Where the user keeps BIOS files ("" = not chosen).
    #[serde(rename = "iroriBiosFolder", alias = "emberBiosFolder")]
    pub bios_folder: String,
    /// List games of Battle.net, EA app, Ubisoft Connect and Rockstar
    /// (off until turned on). Irori-only.
    #[serde(
        rename = "iroriThirdPartyLaunchers",
        alias = "emberThirdPartyLaunchers"
    )]
    pub third_party_launchers: bool,
    /// Minutes between rescans of the third-party launchers' games, so
    /// games installed in a launcher show up without a refresh; 0 rescans
    /// only on a refresh. Irori-only.
    #[serde(rename = "iroriLaunchersRescan")]
    pub launchers_rescan: u32,
    /// Install new builds of components (DXVK, Wine, emulators) as soon as
    /// the daily check finds them. Irori-only.
    #[serde(rename = "iroriAutoUpdateComponents")]
    pub auto_update_components: bool,
    /// Hour automatic updates may start, `0`..`23`; empty for any. Irori-only.
    #[serde(rename = "iroriAutoUpdateFrom")]
    pub auto_update_from: String,
    /// Hour automatic updates stop starting, `0`..`23`; empty for any.
    /// Earlier than `auto_update_from` means past midnight. Irori-only.
    #[serde(rename = "iroriAutoUpdateUntil")]
    pub auto_update_until: String,
    /// Hold automatic updates on a metered connection. Irori-only.
    #[serde(rename = "iroriAutoUpdateUnmetered")]
    pub auto_update_unmetered: bool,
    /// Look for new versions of Irori once a day. Irori-only; about Irori
    /// itself, where `checkForUpdatesOnStartup` is about games.
    #[serde(rename = "iroriCheckForAppUpdates")]
    pub check_for_app_updates: bool,
    /// Show deals from GOG and other stores on the Stores page; off, they
    /// are neither shown nor asked for. Irori-only.
    #[serde(rename = "iroriDeals")]
    pub deals: bool,
    /// Preferred country code (`""` = Auto from system locale) for regional
    /// store deals (`iroriDealsRegion`). Irori-only.
    #[serde(rename = "iroriDealsRegion")]
    pub deals_region: String,
    /// Grid or compact list layout on the Stores Deals tab
    /// (`iroriDealsLayout`). Irori-only.
    #[serde(rename = "iroriDealsLayout")]
    pub deals_layout: DealsLayout,
    /// Hide base games already owned in the library on the Stores Deals tab
    /// (`iroriDealsHideOwned`). Irori-only.
    #[serde(rename = "iroriDealsHideOwned")]
    pub deals_hide_owned: bool,
    /// Whether Irori probes the connection, takes it as working, or works
    /// offline and never reaches the internet. Irori-only.
    #[serde(rename = "iroriNetworkMode")]
    pub network_mode: crate::net::NetworkMode,
    /// Seconds a network request may take (10–600), for slow or unreliable
    /// connections; 0 keeps Irori's own timeouts. Irori-only.
    #[serde(rename = "iroriNetworkTimeout")]
    pub network_timeout: u32,
    /// Snapshots of a game's saves kept from before cloud syncs that could
    /// replace them (0–50); 0 takes none. Irori-only.
    #[serde(rename = "iroriSaveSnapshots")]
    pub save_snapshots: u32,
    /// Open a game's log when it starts. Irori-only.
    #[serde(rename = "iroriShowLogOnLaunch")]
    pub show_log_on_launch: bool,
    /// Announce the achievements a session unlocked once the game exits.
    /// Irori-only.
    #[serde(rename = "iroriAchievementNotices")]
    pub achievement_notices: bool,
    /// How unlocks show while the game runs: over the game, and with which
    /// lines. Irori-only.
    #[serde(rename = "iroriAchievementToast")]
    pub achievement_toast: AchievementToastPrefs,
    /// Read the achievements that Steam API emulators (Goldberg, gbe_fork,
    /// CODEX-style) keep in a game's own files, for games Irori starts from
    /// their own executable. Irori-only.
    #[serde(rename = "iroriFileAchievements")]
    pub file_achievements: bool,
    /// Fill what store pages leave out (descriptions, genres, release
    /// dates, screenshots) from IGDB, and offer its art in the art picker.
    /// Needs the user's own Twitch application, kept with `irori_secrets`;
    /// off by default. Irori-only.
    #[serde(rename = "iroriIgdb")]
    pub igdb: bool,
    /// Fill what store pages and IGDB leave out of visual novels
    /// (descriptions, release dates, developers, covers, screenshots) from
    /// VNDB, and offer its covers in the art picker. VNDB's public API
    /// needs no account; off by default. Irori-only.
    #[serde(rename = "iroriVndb")]
    pub vndb: bool,
    /// Box art for the ROM library from the LaunchBox Games Database: its
    /// game list (about 110 MB) is downloaded, and again once a week while
    /// the ROM library has games. Off by default; the art picker's and the
    /// palette's download turn it on. Irori-only.
    #[serde(rename = "iroriLaunchBox", default)]
    pub launchbox: bool,
    /// Link games of other stores to the Steam app of their exact title
    /// (edition words aside, and a release year apart at most), for
    /// Steam's tags, ProtonDB and screenshots, when no source names their
    /// Steam app. Off by default. Irori-only.
    #[serde(rename = "iroriSteamTitleMatch")]
    pub steam_title_match: bool,
    /// Steam tags each game keeps, strongest first (5–20); tags far fewer
    /// players gave than the strongest are left out too. Irori-only.
    #[serde(rename = "iroriSteamTagLimit")]
    pub steam_tag_limit: u32,
    /// The user's own launch presets. Irori-only.
    #[serde(rename = "iroriLaunchPresets")]
    pub launch_presets: Vec<crate::launch_extras::LaunchPreset>,
    /// The Ludusavi program to run for save backups; empty finds it on
    /// `PATH`, in the usual install folders or on Flathub. Irori-only.
    #[serde(rename = "iroriLudusaviPath")]
    pub ludusavi_path: String,
    /// Where Ludusavi keeps the backups Irori makes; empty for Ludusavi's
    /// own backup folder. Irori-only.
    #[serde(rename = "iroriLudusaviBackupPath")]
    pub ludusavi_backup_path: String,
    /// Full backups Ludusavi keeps of each game when Irori backs it up
    /// (1–255); 0 keeps Ludusavi's own setting. Irori-only.
    #[serde(rename = "iroriLudusaviFullLimit")]
    pub ludusavi_full_limit: u32,
    /// Differential backups Ludusavi keeps per full one ("0"–"255"); empty
    /// keeps Ludusavi's own setting. Irori-only.
    #[serde(rename = "iroriLudusaviDifferentialLimit")]
    pub ludusavi_differential_limit: String,
    /// Largest size of the art cache on disk in MiB (downloaded art and the
    /// thumbnails cut from it); the least recently used art goes first.
    /// 0 = no cap. Irori-only.
    #[serde(rename = "iroriArtCacheLimit")]
    pub art_cache_limit: u32,
    /// Other launchers whose new games Irori keeps importing. Irori-only.
    #[serde(rename = "iroriLauncherImports")]
    pub launcher_imports: LauncherImports,
    /// Shortcuts of a Windows program take the program's own icon, before
    /// the store's art. Irori-only.
    #[serde(rename = "iroriShortcutIconFromExe")]
    pub shortcut_icon_from_exe: bool,
    /// Scheduled backups. Irori-only; stays on this computer (backups and
    /// restores leave it alone).
    #[serde(rename = "iroriBackups")]
    pub backups: BackupPrefs,
    /// Library sync between computers. Irori-only; stays on this computer.
    #[serde(rename = "iroriSync")]
    pub sync: SyncPrefs,
    /// Irori's games in Sunshine, the game-streaming host. Irori-only; stays
    /// on this computer.
    #[serde(rename = "iroriSunshine")]
    pub sunshine: SunshinePrefs,
    /// The in-game overlay on the controller's Guide button. Irori-only;
    /// Linux only for now.
    #[serde(rename = "iroriOverlay")]
    pub overlay: OverlayPrefs,
    /// Global Performance HUD settings (MangoHud on Linux, Metal HUD on macOS).
    /// Irori-only.
    #[serde(rename = "iroriHud", default)]
    pub hud: crate::hud::GlobalHudSettings,
}

impl Default for AppSettings {
    fn default() -> Self {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"));
        let games = home.join("Games").join("Irori");
        let extras = dirs::download_dir()
            .unwrap_or_else(|| home.join("Downloads"))
            .join("Irori");
        Self {
            extras_path: extras.to_string_lossy().into_owned(),
            crossover_bottles_path: String::new(),
            compat_tools_path: String::new(),
            system_engines: BTreeMap::new(),
            rom_folders: Vec::new(),
            bios_folder: String::new(),
            third_party_launchers: false,
            launchers_rescan: 5,
            auto_update_components: false,
            auto_update_from: String::new(),
            auto_update_until: String::new(),
            auto_update_unmetered: false,
            check_for_app_updates: true,
            deals: true,
            deals_region: String::new(),
            deals_layout: DealsLayout::Grid,
            deals_hide_owned: true,
            network_mode: crate::net::NetworkMode::Auto,
            network_timeout: 0,
            save_snapshots: DEFAULT_SAVE_SNAPSHOTS,
            show_log_on_launch: false,
            achievement_notices: false,
            achievement_toast: AchievementToastPrefs::default(),
            file_achievements: false,
            igdb: false,
            vndb: false,
            launchbox: false,
            steam_title_match: false,
            steam_tag_limit: DEFAULT_STEAM_TAG_LIMIT,
            launch_presets: Vec::new(),
            ludusavi_path: String::new(),
            ludusavi_backup_path: String::new(),
            art_cache_limit: DEFAULT_ART_CACHE_LIMIT,
            ludusavi_full_limit: 0,
            ludusavi_differential_limit: String::new(),
            backups: BackupPrefs::default(),
            sync: SyncPrefs::default(),
            sunshine: SunshinePrefs::default(),
            overlay: OverlayPrefs::default(),
            hud: crate::hud::GlobalHudSettings::default(),
            game: GameSettings::default(),
            add_desktop_shortcuts: false,
            add_start_menu_shortcuts: false,
            add_steam_shortcuts: false,
            launcher_imports: LauncherImports::default(),
            shortcut_icon_from_exe: false,
            auto_update_games: false,
            check_for_updates_on_startup: true,
            check_updates_interval: 10,
            custom_wine_paths: Vec::new(),
            dark_tray_icon: false,
            generic_tray_icon: false,
            default_install_path: games.to_string_lossy().into_owned(),
            install_per_store: false,
            library_folders: Vec::new(),
            default_steam_path: crate::platform::steam_dir().to_string_lossy().into_owned(),
            default_wine_prefix_dir: games.join("Prefixes").to_string_lossy().into_owned(),
            disable_controller: false,
            controller: Default::default(),
            big_picture: Default::default(),
            disable_playtime_sync: false,
            disable_logs: false,
            disable_animations: false,
            discord_rpc: false,
            discord_client_id: String::new(),
            disable_gog_presence: false,
            download_no_https: false,
            download_proton_to_steam: false,
            proton_cpu_level: ProtonCpuLevel::Auto,
            egs_linked_path: String::new(),
            exit_to_tray: false,
            gamepad_repeat_delay: 50,
            gamepad_initial_repeat_delay: 300,
            no_tray_icon: false,
            hide_changelogs_on_startup: false,
            hide_window_on_protocol_launch: false,
            library_top_section: LibraryTopSection::RecentlyPlayed,
            max_recent_games: 8,
            max_workers: 0,
            max_download_speed: 0,
            minimize_on_launch: false,
            start_in_console_mode: false,
            start_in_tray: false,
            start_with_system: false,
            start_with_system_minimized: true,
            allow_installation_broken_anticheat: false,
            show_valve_proton: false,
            steam_grid_db_api_key: String::new(),
            comet_support: true,
            zoom_platform: true,
            itch_io: false,
            xbox: false,
            xbox_market: String::new(),
            ui_language: "en".into(),
            ui: UiPrefs::default(),
        }
    }
}

/// Which stores Irori shows. A hidden store keeps its sign-in and installed
/// games on disk; its games, account card and store page are left out.
/// ZOOM Platform follows `zoomPlatform`, itch.io follows `iroriItchIo`, and
/// apps the user added are always shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ShownStores {
    pub epic: bool,
    pub gog: bool,
    pub amazon: bool,
    pub steam: bool,
}

impl Default for ShownStores {
    fn default() -> Self {
        Self {
            epic: true,
            gog: true,
            amazon: true,
            steam: true,
        }
    }
}

impl ShownStores {
    /// Whether the store's games and pages are shown.
    pub fn shows(&self, runner: Runner) -> bool {
        match runner {
            Runner::Legendary => self.epic,
            Runner::Gog => self.gog,
            Runner::Nile => self.amazon,
            Runner::Steam => self.steam,
            Runner::Zoom
            | Runner::Itch
            | Runner::Xbox
            | Runner::Sideload
            | Runner::Emulation
            | Runner::Launchers => true,
        }
    }

    /// Show or hide a store. ZOOM Platform, itch.io and added apps have no
    /// switch here, so they are left as they are.
    pub fn set(&mut self, runner: Runner, shown: bool) {
        match runner {
            Runner::Legendary => self.epic = shown,
            Runner::Gog => self.gog = shown,
            Runner::Nile => self.amazon = shown,
            Runner::Steam => self.steam = shown,
            Runner::Zoom
            | Runner::Itch
            | Runner::Xbox
            | Runner::Sideload
            | Runner::Emulation
            | Runner::Launchers => {}
        }
    }

    /// Whether the store can be hidden with [`ShownStores::set`].
    pub fn can_hide(runner: Runner) -> bool {
        matches!(
            runner,
            Runner::Legendary | Runner::Gog | Runner::Nile | Runner::Steam
        )
    }
}

/// Irori's Discord application (client) ID, which Rich Presence shows as
/// "Irori". It is public, not a secret: the project owner created the
/// Discord application "Irori" at discord.com/developers/applications and
/// copied its Application ID from General Information. Rich Presence's
/// small image is that application's art asset named `irori` (Rich
/// Presence → Art Assets). Left empty, Rich Presence stays off unless the
/// user enters an application ID of their own (`discordClientId`).
pub const DISCORD_APPLICATION_ID: &str = "1568789394926870640";

impl AppSettings {
    /// The Discord application Rich Presence speaks for: the user's own, else
    /// Irori's. `None` while Rich Presence is off or there is no ID at all.
    pub fn discord_application(&self) -> Option<String> {
        if !self.discord_rpc {
            return None;
        }
        let own = self.discord_client_id.trim();
        let id = if own.is_empty() {
            DISCORD_APPLICATION_ID
        } else {
            own
        };
        (!id.is_empty()).then(|| id.to_string())
    }

    /// The folder new installs of a store's games are offered, as the user
    /// wrote it (`~` unexpanded): `defaultInstallPath`, and with
    /// `iroriInstallPerStore` the store's own folder inside it, named as the
    /// store is (`Epic Games`, `GOG`). Stores whose games Irori doesn't
    /// install, and added games, keep `defaultInstallPath`.
    pub fn install_folder(&self, runner: Runner) -> String {
        self.install_folder_in(&self.default_install_path, runner)
    }

    /// The folder new installs of a store's games go to inside the library
    /// folder `base`: `base` itself, or with `iroriInstallPerStore` the
    /// store's own folder inside it, as [`Self::install_folder`] does for the
    /// default one.
    pub fn install_folder_in(&self, base: &str, runner: Runner) -> String {
        if !self.install_per_store || base.trim().is_empty() || !installs_per_store(runner) {
            return base.to_string();
        }
        let separator = if base.contains('\\') && !base.contains('/') {
            '\\'
        } else {
            '/'
        };
        let base = base.trim_end_matches(['/', '\\']);
        format!("{base}{separator}{}", runner.store_name())
    }

    /// Every library folder: the default install folder first (unnamed),
    /// then the user's named ones with a folder, each folder once.
    pub fn library_folders(&self) -> Vec<LibraryFolder> {
        let mut out = vec![LibraryFolder {
            name: String::new(),
            path: self.default_install_path.clone(),
        }];
        for folder in &self.library_folders {
            if folder.path.trim().is_empty()
                || out.iter().any(|f| same_folder(&f.path, &folder.path))
            {
                continue;
            }
            let mut folder = folder.clone();
            if folder.name.trim().is_empty() {
                // An unnamed folder would read as the default one.
                folder.name = folder_name(&folder.path);
            }
            out.push(folder);
        }
        out
    }
}

/// A named folder games can be installed into (`iroriLibraryFolders`)
/// beside the default install folder: another drive, an SD card.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct LibraryFolder {
    /// What the user calls it; empty only for the default install folder.
    pub name: String,
    /// As the user chose it (`~` unexpanded).
    pub path: String,
}

impl LibraryFolder {
    /// Whether this is the default install folder (`defaultInstallPath`).
    pub fn is_default(&self) -> bool {
        self.name.is_empty()
    }
}

/// Whether two folders as the user wrote them are one folder.
pub fn same_folder(a: &str, b: &str) -> bool {
    let normal = |p: &str| {
        crate::paths::expand_home(p.trim())
            .components()
            .collect::<PathBuf>()
    };
    normal(a) == normal(b)
}

/// A folder's own name (`SD card` for `/run/media/me/SD card`), for a
/// library folder the user gave no name.
pub fn folder_name(path: &str) -> String {
    let path = path.trim().trim_end_matches(['/', '\\']);
    path.rsplit(['/', '\\'])
        .next()
        .filter(|n| !n.is_empty())
        .unwrap_or(path)
        .to_string()
}

/// The library folder holding `install` (an installed game's folder): the
/// one whose folder is the longest start of it.
pub fn library_folder_of(folders: &[LibraryFolder], install: &std::path::Path) -> Option<usize> {
    folders
        .iter()
        .enumerate()
        .filter(|(_, f)| !f.path.trim().is_empty())
        .map(|(i, f)| (i, crate::paths::expand_home(f.path.trim())))
        .filter(|(_, root)| install.starts_with(root))
        .max_by_key(|(_, root)| root.components().count())
        .map(|(i, _)| i)
}

/// Whether a store's games get a folder of the store's own with
/// `iroriInstallPerStore`: the stores Irori downloads games from.
fn installs_per_store(runner: Runner) -> bool {
    matches!(
        runner,
        Runner::Legendary | Runner::Gog | Runner::Nile | Runner::Zoom | Runner::Itch | Runner::Xbox
    )
}
