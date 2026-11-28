//! Irori's backend facade. The UI talks only to [`Backend`]; stores, the Wine
//! runtime and the filesystem sit behind it.
//!
//! Threading: the backend owns a small Tokio runtime. Async methods must run on
//! it — UI code calls [`Backend::spawn`] and awaits the returned handle from its
//! own executor (Tokio join handles are executor-agnostic). Synchronous
//! accessors (`library`, `settings`, `status`…) are lock-protected snapshots and
//! safe to call from render.

pub mod accounts;
pub mod achievements;
pub mod app_update;
pub mod art_cache;
pub mod audio;
pub mod backup;
pub mod backup_paths;
mod backup_restore;
pub mod backup_schedule;
pub mod bluetooth;
pub mod capture;
pub mod category_file;
pub mod copies;
mod discord;
pub mod displays;
mod download_power;
mod download_space;
mod downloads;
pub mod drives;
pub mod duplicates;
pub mod engine_health;
mod engines;
mod facts;
pub mod field_rules;
pub mod file_achievements;
mod files;
mod follow;
pub mod game_window;
mod games;
mod ggdeals;
pub mod gpu_hide;
pub mod gpu_preference;
pub mod hidden_lock;
mod hooks;
pub mod how_it_starts;
mod igdb;
mod inhibit;
pub mod journal;
mod library;
pub mod ludusavi;
pub mod mangohud;
pub mod media;
pub mod mods;
mod network;
pub mod news;
pub mod news_feeds;
pub mod optiscaler;
mod other_launchers;
pub mod play_status;
pub mod power;
pub mod prefixes;
mod program_icons;
mod programs;
pub mod related;
pub mod renodx;
pub mod replay;
pub mod reshade;
mod running;
pub mod save_snapshots;
pub mod screen;
pub mod screenshots;
pub mod scripts;
pub mod search;
pub mod self_update;
pub mod sessions;
pub mod settings;
mod settings_migration;
pub mod smart_categories;
pub mod status;
pub mod steam_link;
pub mod steam_overlay;
pub mod steam_reception;
mod steam_shortcuts;
mod steam_tags;
pub mod storage;
mod store_facts;
pub mod sunshine;
pub mod sync;
pub mod trash;
pub mod troubleshoot;
pub mod updates;
pub mod user_actions;
pub mod userdata;
mod vndb;
mod windows_files;

use std::collections::BTreeMap;
use std::future::Future;
use std::ops::Deref;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use irori_core::{DynStore, Event, EventBus, Notice, NoticeLevel, Runner, SessionExpired};
use parking_lot::{Mutex, RwLock};
use tokio::runtime::Runtime;
use tokio::sync::{Notify, broadcast};

pub use accounts::{MakeSlot, MultiAccountStore, SlotDirs};
pub use achievements::{
    AchievementIndex, AchievementTotals, GameAchievements, Unlock, unlock_notice,
};
pub use app_update::{AppUpdateStatus, InstallKind, Release, SelfInstall, SystemPackageManager};
pub use audio::{GameAudio, Volume, VolumeTarget};
pub use backup::{
    Backup, BackupParts, ExportFormat, InstallRecord, PathCheck, PathKind, RestoreSummary,
};
pub use backup_paths::Remap;
pub use backup_restore::remap_from_located;
pub use backup_schedule::{BackupEntry, BackupKind, BackupStatus};
pub use download_power::{DOWNLOADS_FINISHED_NOTICE, POWER_COUNTDOWN, PowerCountdown};
pub use download_space::QueueDisk;
pub use downloads::{DownloadItem, DownloadKind, DownloadQueueSnapshot, DownloadState};
pub use drives::{
    DetectedDrive, InstallDriveProbe, detect_extra_drives, drive_label, format_disk_label,
    probe_install_drives,
};
pub use engines::EngineStatus;
pub use files::{FileDownload, FileState, key_of as file_download_key};
pub use games::{
    LaunchCancelled, LaunchParams, MoveReport, PrefixChoice, RunningGame, SAVE_SYNC_TIMEOUT,
};
pub use igdb::IgdbStatus;
pub use irori_engines::rom_sources::{Plan as RomPlan, RomSource, SourceKind as RomSourceKind};
pub use irori_extras::news_feeds::{
    ArticleContent, CustomFeed, FeedArticle, FeedFetchResult, NewsFeedsConfig, PRESET_FEEDS,
    validate_feed_url,
};
pub use irori_steam_api::{NewsKind, SteamNewsItem, StoreHit as SteamHit};
pub use journal::{Journal, JournalEntry, LeftOff};
pub use library::{LibraryFilter, LibrarySnapshot, LibrarySort, StoreFilter};
pub use ludusavi::{BackupItem, GameBackups, LudusaviInfo, RestorePlan, SavesDone, SavesKeeper};
pub use mods::{FomodPrompt, InstallOutcome, ModProgress, ModsView, ScriptExtender, supports_mods};
pub use news::{GameNewsEntry, GameNewsResult, NewsFeedSnapshot, NewsScope};
pub use optiscaler::{OptiscalerInstallParams, OptiscalerStatus};
pub use play_status::{EffectiveStatus, PlayStatus, PlayStatusRecord, StatusChange, StatusHistory};
pub use power::{Permission, PowerAction, PowerSystem};
pub use renodx::{RenodxChoice, RenodxStatus};
pub use replay::{QuickLaunchShortcut, Recorder, RecorderInfo, ReplayInfo, ShortcutState};
pub use reshade::{ReshadeStatus, reshade_needs_steam_overrides, supports_reshade};
pub use screenshots::{Screenshot, ScreenshotIndex};
pub use scripts::{ScriptInstall, ScriptProgress, ScriptPrompt, ScriptRun};
pub use self_update::SelfUpdateChannel;
pub use sessions::Session;
pub use settings::SettingsService;
pub use smart_categories::{
    CategoryIndex, CategoryLook, CategoryRef, Condition, HltbTimes, Match, Rule, SmartCategory,
};
pub use status::StatusRegistry;
pub use steam_link::{STEAM_NONE, SteamLink, TitleLinkReport};
pub use steam_overlay::SteamLaunch;
pub use storage::{CleanupKind, StorageReport};
pub use store_facts::DetailsReport;
pub use sunshine::{
    StreamClient, StreamHost, SunshineInstall, SunshineLogin, SunshineReport, SunshineStatus,
};
pub use sync::{RcloneInfo, SyncConflict, SyncDevice, SyncReport, SyncStatus, WebDavLogin};
pub use updates::{
    AutoUpdateHold, COMPONENT_ROLLBACK_ARG, ComponentKind, ComponentReport, ComponentUpdate,
    PreviousBuild,
};
pub use userdata::{BulkSnapshot, UserData, UserDataService};
pub use windows_files::{can_open_windows_files_by_default, opens_windows_files};

/// Whether a store's library refresh reads only this computer while Irori
/// works offline: these stores read local files and leave their network
/// parts (Steam's account, the Xbox account) out while working offline.
fn refreshes_offline(runner: Runner) -> bool {
    matches!(
        runner,
        Runner::Steam | Runner::Launchers | Runner::Emulation | Runner::Sideload
    )
}

/// Whether the network mode settles being online without a probe: working
/// offline never is, "always online" always is; `None` probes.
fn decided_online(mode: irori_core::net::NetworkMode) -> Option<bool> {
    match mode {
        irori_core::net::NetworkMode::Offline => Some(false),
        irori_core::net::NetworkMode::Online => Some(true),
        irori_core::net::NetworkMode::Auto => None,
    }
}

/// How often connectivity is re-checked.
const CONNECTIVITY_INTERVAL: Duration = Duration::from_secs(60);

/// Whether the third-party launchers' games are due a rescan
/// (`iroriLaunchersRescan` minutes since the last, while they are listed).
fn launchers_rescan_due(app: &irori_core::AppSettings, since: Duration) -> bool {
    app.third_party_launchers
        && app.launchers_rescan > 0
        && since >= Duration::from_secs(u64::from(app.launchers_rescan) * 60)
}

/// Shared handle to the backend. Cheap to clone.
#[derive(Clone)]
pub struct Backend(Arc<Inner>);

pub struct Inner {
    rt: &'static Runtime,
    pub bus: EventBus,
    pub settings: Arc<SettingsService>,
    pub userdata: UserDataService,
    facts: facts::FactsService,
    store_facts: store_facts::StoreFactsService,
    steam_tags: steam_tags::SteamTagsService,
    program_icons: program_icons::ProgramIcons,
    steam_links: steam_link::SteamLinkService,
    hidden_lock: hidden_lock::HiddenLock,
    pub statuses: StatusRegistry,
    stores: BTreeMap<Runner, DynStore>,
    library: RwLock<Arc<LibrarySnapshot>>,
    downloads: Mutex<downloads::QueueState>,
    downloads_wake: Notify,
    files: Mutex<files::FilesState>,
    files_wake: Notify,
    running: Mutex<BTreeMap<irori_core::GameId, games::RunningGame>>,
    /// Game sessions running or still ending (saves, mods), which powering
    /// off waits for ([`power::OpenSession`]).
    open_sessions: tokio::sync::watch::Sender<usize>,
    /// What powers the computer off ([`power`]).
    power_system: RwLock<Arc<dyn power::PowerSystem>>,
    updates: RwLock<Arc<std::collections::HashSet<irori_core::GameId>>>,
    /// Stores whose last update check failed; their earlier answer stands.
    update_check_failed: RwLock<Vec<Runner>>,
    /// New versions of Wine builds, translation layers and emulators.
    components: updates::ComponentUpdates,
    online: AtomicBool,
    /// Started for tests ([`Backend::start_for_tests`]): nothing reaches the
    /// network, the desktop or the system's settings.
    isolated: bool,
    /// The queue was paused by [`Backend::pause_downloads_while_offline`].
    auto_paused: AtomicBool,
    /// The computer is kept awake for downloads after the screen was turned
    /// off (`screen.rs`).
    screen_off_hold: AtomicBool,
    /// Steam shortcut changes waiting, as last told to the Steam card.
    steam_shortcuts_shown: std::sync::atomic::AtomicUsize,
    presence: discord::Presence,
    /// The replay buffer of the session it records, and its shortcut.
    replay: replay::ReplayState,
    /// Stores whose saved login was refused, with the store's reason.
    expired_logins: RwLock<BTreeMap<Runner, String>>,
    /// The update check ran at least once this session.
    updates_checked: AtomicBool,
    /// Every store has read its state from disk.
    stores_ready: AtomicBool,
    /// New versions of Irori itself.
    app_update: app_update::AppUpdater,
    /// Every game's achievements, summed up.
    achievement_store: achievements::AchievementStore,
    /// Scheduled backups.
    backups: backup_schedule::BackupScheduler,
    /// Library sync between computers.
    sync: sync::SyncService,
    /// Irori's games in Sunshine.
    sunshine: sunshine::SunshineService,
    /// Fixes tried and exit codes, for launch troubleshooting.
    troubleshoot: troubleshoot::TroubleshootState,
    /// The user's IGDB application.
    igdb: igdb::IgdbState,
    /// The VNDB client, shared so requests stay spaced.
    vndb: vndb::VndbState,
    /// Optional personal GG.deals API key.
    ggdeals: ggdeals::GgDealsState,
}

impl Deref for Backend {
    type Target = Inner;
    fn deref(&self) -> &Inner {
        &self.0
    }
}

impl Backend {
    /// Build the backend. `make_stores` receives the event bus and a settings
    /// accessor and returns the store backends to use. Loads settings and cached
    /// state synchronously; store initialisation and the first library build
    /// run in the background (watch for [`Event::LibraryUpdated`]).
    pub fn start(
        make_stores: impl FnOnce(EventBus, irori_core::SettingsFn) -> Vec<DynStore>,
    ) -> anyhow::Result<Self> {
        Self::start_with(make_stores, true, false)
    }

    /// Like [`Backend::start`] for UI tests: the app's state, models and
    /// workers, without what would reach outside the test. No connectivity
    /// probe (the backend counts as online), no update, compatibility-data,
    /// sync or store refresh at start, no periodic tasks, no clip shortcut,
    /// Sunshine or graphics-preference watch. The first library build is the
    /// only one, so a test sees no second rebuild racing its assertions.
    pub fn start_for_tests(
        make_stores: impl FnOnce(EventBus, irori_core::SettingsFn) -> Vec<DynStore>,
    ) -> anyhow::Result<Self> {
        Self::start_with(make_stores, true, true)
    }

    /// Like [`Backend::start`] for one-shot command-line use: no startup
    /// refresh, compatibility-data update or periodic checks.
    pub fn start_headless(
        make_stores: impl FnOnce(EventBus, irori_core::SettingsFn) -> Vec<DynStore>,
    ) -> anyhow::Result<Self> {
        Self::start_with(make_stores, false, false)
    }

    fn start_with(
        make_stores: impl FnOnce(EventBus, irori_core::SettingsFn) -> Vec<DynStore>,
        interactive: bool,
        isolated: bool,
    ) -> anyhow::Result<Self> {
        // This crate's own tests never reach the network either.
        let isolated = isolated || cfg!(test);
        let rt: &'static Runtime = Box::leak(Box::new(
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(3)
                .max_blocking_threads(16)
                .thread_name("irori-io")
                .enable_all()
                .build()?,
        ));
        let _guard = rt.enter();
        let bus = EventBus::new();
        let settings = Arc::new(SettingsService::load(bus.clone()));
        if !isolated {
            settings.drive_network_policy();
        }
        let settings_fn: irori_core::SettingsFn = {
            let settings = settings.clone();
            Arc::new(move || settings.app())
        };
        let stores = make_stores(bus.clone(), settings_fn);
        let backend = Backend(Arc::new(Inner {
            rt,
            settings,
            userdata: UserDataService::load(bus.clone()),
            facts: facts::FactsService::load(),
            store_facts: store_facts::StoreFactsService::load(),
            steam_tags: steam_tags::SteamTagsService::load(),
            program_icons: program_icons::ProgramIcons::load(),
            steam_links: steam_link::SteamLinkService::load(),
            hidden_lock: hidden_lock::HiddenLock::load(),
            statuses: StatusRegistry::new(bus.clone()),
            stores: stores.into_iter().map(|s| (s.runner(), s)).collect(),
            library: RwLock::new(Arc::new(LibrarySnapshot::default())),
            downloads: Mutex::new(downloads::QueueState::load()),
            downloads_wake: Notify::new(),
            files: Mutex::new(files::FilesState::default()),
            files_wake: Notify::new(),
            running: Mutex::new(BTreeMap::new()),
            open_sessions: tokio::sync::watch::Sender::new(0),
            power_system: RwLock::new(Arc::new(power::ThisSystem)),
            updates: RwLock::new(Arc::default()),
            update_check_failed: RwLock::new(Vec::new()),
            components: updates::ComponentUpdates::load(),
            online: AtomicBool::new(true),
            isolated,
            auto_paused: AtomicBool::new(false),
            screen_off_hold: AtomicBool::new(false),
            steam_shortcuts_shown: std::sync::atomic::AtomicUsize::new(0),
            presence: discord::Presence::default(),
            replay: replay::ReplayState::default(),
            expired_logins: RwLock::new(BTreeMap::new()),
            updates_checked: AtomicBool::new(false),
            stores_ready: AtomicBool::new(false),
            app_update: app_update::AppUpdater::load(interactive),
            achievement_store: achievements::AchievementStore::default(),
            backups: backup_schedule::BackupScheduler::load(),
            sync: sync::SyncService::load(),
            sunshine: Default::default(),
            troubleshoot: Default::default(),
            igdb: Default::default(),
            vndb: Default::default(),
            ggdeals: Default::default(),
            bus,
        }));
        irori_runtime::set_hooks(Arc::new(hooks::KnownFixHooks {
            backend: backend.clone(),
        }));
        backend.spawn_background_tasks(interactive);
        Ok(backend)
    }

    /// Run a future on the backend runtime.
    pub fn spawn<F>(&self, fut: F) -> tokio::task::JoinHandle<F::Output>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        self.rt.spawn(fut)
    }

    /// Run blocking work (file scans, image decoding) on the blocking pool.
    pub fn spawn_blocking<F, R>(&self, f: F) -> tokio::task::JoinHandle<R>
    where
        F: FnOnce() -> R + Send + 'static,
        R: Send + 'static,
    {
        self.rt.spawn_blocking(f)
    }

    pub fn runtime(&self) -> &'static Runtime {
        self.rt
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.bus.subscribe()
    }

    /// Whether every store has read its state from disk and the library holds
    /// what the user really has. An [`Event::LibraryUpdated`] follows it, so
    /// the UI can wait for that one.
    pub fn stores_ready(&self) -> bool {
        self.stores_ready.load(Ordering::Relaxed)
    }

    pub fn store(&self, runner: Runner) -> Option<DynStore> {
        self.stores.get(&runner).cloned()
    }

    pub fn stores(&self) -> impl Iterator<Item = &DynStore> {
        self.stores.values()
    }

    /// Whether Irori may use the network now: the connection works and the
    /// user hasn't asked Irori to work offline (`iroriNetworkMode`).
    pub fn is_online(&self) -> bool {
        self.online.load(Ordering::Relaxed) && !self.working_offline()
    }

    /// Whether the user asked Irori to work offline.
    pub fn working_offline(&self) -> bool {
        self.settings.app().network_mode == irori_core::net::NetworkMode::Offline
    }

    /// Fails with [`irori_core::net::WorkingOffline`] while working offline.
    fn ensure_online(&self) -> anyhow::Result<()> {
        if self.working_offline() {
            return Err(irori_core::net::WorkingOffline.into());
        }
        Ok(())
    }

    /// Work offline, or follow the connection again (`iroriNetworkMode`
    /// back to automatic), from the palette and the title bar.
    pub fn set_working_offline(&self, offline: bool) {
        self.settings.update_app(|s| {
            s.network_mode = if offline {
                irori_core::net::NetworkMode::Offline
            } else {
                irori_core::net::NetworkMode::Auto
            };
        });
    }

    /// Follow a changed `iroriNetworkMode` at once: probe (or not), pause
    /// downloads going offline and take them up again coming back.
    async fn follow_network_mode(&self) {
        let was_online = self.is_online_now();
        let online = self.check_connectivity().await;
        self.pause_downloads_while_offline(was_online, online);
        if online && !was_online {
            self.downloads_wake.notify_one();
        }
    }

    /// The connection as last probed, whatever the network mode.
    fn is_online_now(&self) -> bool {
        self.online.load(Ordering::Relaxed)
    }

    fn spawn_background_tasks(&self, interactive: bool) {
        // Only the app: a command-line run may sit beside a running game.
        // Games still running from before are followed again first, so
        // their mods stay in while they play.
        if interactive {
            let b = self.clone();
            self.spawn(async move {
                b.adopt_sessions().await;
                b.recover_mods().await;
            });
        }

        // A new version of Irori backs up what the old one wrote first.
        if interactive {
            let b = self.clone();
            self.spawn(async move { b.back_up_before_update().await });
        }

        // Initialise stores from disk, then build the library.
        let b = self.clone();
        self.spawn(async move {
            // Read beside the stores: sorting and smart categories count
            // plays from the first library on.
            let plays = b.spawn_blocking(crate::sessions::load_stats);
            let inits = b.stores.values().map(|s| {
                let s = s.clone();
                async move {
                    if let Err(e) = s.init().await {
                        tracing::error!("initialising {}: {e:#}", s.runner().store_name());
                    }
                }
            });
            futures::future::join_all(inits).await;
            let _ = plays.await;
            // Before anything that may wait on the network: the first screen
            // depends on this.
            b.rebuild_library();
            b.stores_ready.store(true, Ordering::Relaxed);
            b.bus.emit(Event::LibraryUpdated(b.library().revision));
            // Items restored from the persisted queue are waiting again.
            for item in b.downloads().active {
                b.statuses.set(&item.id, irori_core::StatusKind::Queued);
            }
            if !b.isolated {
                b.check_connectivity().await;
                b.rebuild_library();
            }
            b.downloads_wake.notify_one();
            if !interactive || b.isolated {
                return;
            }
            // The clip shortcut works from the first game on.
            if cfg!(target_os = "linux") && b.settings.app().game.replay.enabled {
                let shortcut = b.clone();
                b.spawn(async move { shortcut.listen_for_clip_shortcut().await });
            }
            if irori_secrets::unreadable_logins() {
                b.bus.startup_notice(
                    Notice::new(NoticeLevel::Warning, "Your saved logins couldn’t be opened")
                        .message(
                            "Unlock your system keyring and restart Irori, or log in to your \
                             stores again.",
                        )
                        .keys(
                            "shell.notice.logins_locked",
                            Some("shell.notice.logins_locked_message"),
                            &[],
                        ),
                );
            }
            b.sync_steam_tools().await;
            b.sync_when_due().await;
            b.back_up_when_due().await;
            b.check_app_update_when_due().await;
            b.refresh_compat_data().await;
            if b.settings.app().check_for_updates_on_startup {
                b.refresh_all(false).await;
            }
            b.check_components_when_due().await;
            b.run_automatic_updates().await;
        });

        // Rebuild the aggregated library when stores or user data change, the
        // user shows or hides a store, or turns grouping copies or program
        // icons on or off.
        let b = self.clone();
        self.spawn(async move {
            let mut rx = b.subscribe();
            let mut shown = b.settings.app().ui.stores;
            // Opt-in stores show their account and games only while on.
            let switches = |a: &irori_core::AppSettings| [a.zoom_platform, a.itch_io];
            let mut on = switches(&b.settings.app());
            let mut copies = b.settings.app().ui.group_copies;
            let mut icons = b.settings.app().ui.program_icon_art;
            let mut tag_limit = b.settings.app().steam_tag_limit;
            let mut proton_to_steam = b.settings.app().download_proton_to_steam;
            let mut network_mode = b.settings.app().network_mode;
            loop {
                match rx.recv().await {
                    Ok(Event::LibraryChanged(_) | Event::GameChanged(_)) => {
                        // Coalesce bursts (a refresh touches many games).
                        tokio::time::sleep(Duration::from_millis(30)).await;
                        while rx.try_recv().is_ok() {}
                        b.rebuild_library();
                    }
                    Ok(Event::GameSettingsChanged(id)) => b.follow_steam_link_setting(&id),
                    Ok(Event::WineVersionsChanged) => {
                        let b = b.clone();
                        b.clone().spawn(async move {
                            b.forget_present_wine_updates().await;
                            // A build's registration with Steam comes and
                            // goes with it.
                            b.sync_steam_tools().await;
                        });
                    }
                    Ok(Event::SettingsChanged) => {
                        let app = b.settings.app();
                        if app.download_proton_to_steam != proton_to_steam {
                            proton_to_steam = app.download_proton_to_steam;
                            let b = b.clone();
                            b.clone().spawn(async move { b.sync_steam_tools().await });
                        }
                        if app.network_mode != network_mode {
                            network_mode = app.network_mode;
                            let b = b.clone();
                            b.clone()
                                .spawn(async move { b.follow_network_mode().await });
                        }
                        let now = switches(&app);
                        for (runner, (was, is)) in [Runner::Zoom, Runner::Itch]
                            .into_iter()
                            .zip(on.iter().zip(now))
                        {
                            if *was != is {
                                b.bus.emit(Event::AccountChanged(runner));
                            }
                        }
                        if app.ui.stores != shown
                            || now != on
                            || app.ui.group_copies != copies
                            || app.ui.program_icon_art != icons
                            || app.steam_tag_limit != tag_limit
                        {
                            shown = app.ui.stores;
                            on = now;
                            copies = app.ui.group_copies;
                            icons = app.ui.program_icon_art;
                            tag_limit = app.steam_tag_limit;
                            b.rebuild_library();
                        }
                    }
                    Ok(_) => {}
                    Err(broadcast::error::RecvError::Lagged(_)) => b.rebuild_library(),
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        });

        // A graphics card a killed session left disabled comes back
        // (`gpu_hide`).
        if interactive && !self.isolated && gpu_hide::system_devices().is_some() {
            let b = self.clone();
            self.spawn(async move { b.recover_hidden_gpus().await });
        }

        // Windows' graphics preference follows a changed or cleared setting
        // at once, not only at the game's next launch (`gpu_preference`).
        if interactive && !self.isolated && gpu_preference::system_registry().is_some() {
            let b = self.clone();
            self.spawn(async move {
                let mut rx = b.subscribe();
                b.reconcile_gpu_preferences().await;
                loop {
                    match rx.recv().await {
                        Ok(Event::SettingsChanged | Event::GameSettingsChanged(_))
                        | Err(broadcast::error::RecvError::Lagged(_)) => {
                            // Coalesce a burst (a preset applied to many games).
                            tokio::time::sleep(Duration::from_millis(500)).await;
                            while rx.try_recv().is_ok() {}
                            b.reconcile_gpu_preferences().await;
                        }
                        Ok(_) => {}
                        Err(broadcast::error::RecvError::Closed) => break,
                    }
                }
            });
        }

        // File downloads (offline installers, goodies).
        let b = self.clone();
        self.spawn(async move { b.file_worker().await });

        // Download queue worker.
        let b = self.clone();
        self.spawn(async move { b.download_worker().await });

        // Periodic update checks and connectivity.
        if interactive && !self.isolated {
            let b = self.clone();
            self.spawn(async move {
                b.load_steam_shortcut_queue().await;
                b.note_other_launchers().await;
                b.periodic_tasks().await
            });
            self.watch_for_sunshine();
        }
    }

    async fn periodic_tasks(&self) {
        let mut since_refresh = Duration::ZERO;
        let mut since_launchers = Duration::ZERO;
        loop {
            // Connectivity is cheap to probe; library refreshes follow the
            // user's interval.
            tokio::time::sleep(CONNECTIVITY_INTERVAL).await;
            since_refresh += CONNECTIVITY_INTERVAL;
            since_launchers += CONNECTIVITY_INTERVAL;
            let was_online = self.is_online_now();
            let online = self.check_connectivity().await;
            self.pause_downloads_while_offline(was_online, online);
            self.sync_when_due().await;
            self.cloud_copies_when_due().await;
            self.sync_sunshine_when_due().await;
            self.steam_shortcuts_when_due().await;
            self.back_up_when_due().await;
            self.check_app_update_when_due().await;
            self.check_components_when_due().await;
            self.trim_art_cache_when_due().await;
            let interval =
                Duration::from_secs(self.settings.app().check_updates_interval.max(5) as u64 * 60);
            let reconnected = online && !was_online;
            if online && (since_refresh >= interval || reconnected) {
                since_refresh = Duration::ZERO;
                since_launchers = Duration::ZERO;
                self.refresh_all(false).await;
                self.run_automatic_updates().await;
            } else if launchers_rescan_due(&self.settings.app(), since_launchers) {
                // The launchers' games are read from local files, so this
                // runs offline too.
                since_launchers = Duration::ZERO;
                if let Err(e) = self.refresh_store(Runner::Launchers, false).await {
                    tracing::info!("rescanning third-party launchers: {e:#}");
                }
            }
        }
    }

    /// What Irori knows about its own new versions.
    pub fn app_update(&self) -> AppUpdateStatus {
        self.app_update
            .status(self.settings.app().check_for_app_updates)
    }

    /// Ask GitHub for a new version of Irori now, however recent the last
    /// check ("Check now").
    pub async fn check_app_update(&self) -> anyhow::Result<AppUpdateStatus> {
        self.check_app_update_with_kind(irori_core::github_releases::CheckKind::Manual)
            .await
    }

    async fn check_app_update_with_kind(
        &self,
        check: irori_core::github_releases::CheckKind,
    ) -> anyhow::Result<AppUpdateStatus> {
        self.app_update.set_checking(true);
        self.bus.emit(Event::AppUpdateChanged);
        let result = self.app_update.check(check).await;
        self.app_update.set_checking(false);
        self.bus.emit(Event::AppUpdateChanged);
        result.map(|()| self.app_update())
    }

    /// Download the newer Irori, check it against the release's checksums
    /// and unpack it beside this install, to go in at the next restart
    /// ([`self_update`]). Only for installs no package manager updates.
    pub async fn install_app_update(&self) -> anyhow::Result<()> {
        use anyhow::Context as _;
        use std::sync::atomic::AtomicU8;
        let status = self.app_update();
        let Some(release) = status.available else {
            anyhow::bail!("There is no newer Irori to install");
        };
        let Some(layout) = self.app_update.layout().cloned() else {
            anyhow::bail!("This copy of Irori is updated another way");
        };
        if !self.app_update.try_begin_install() {
            return Ok(());
        }
        self.bus.emit(Event::AppUpdateChanged);
        // Only whole percents reach the UI.
        let shown = Arc::new(AtomicU8::new(u8::MAX));
        let (b, seen) = (self.clone(), shown.clone());
        let progress: irori_core::install::ProgressSink = Arc::new(move |p| {
            let Some(percent) = p.percent.map(|p| p.clamp(0., 100.) as u8) else {
                return;
            };
            if seen.swap(percent, Ordering::Relaxed) != percent {
                b.app_update.set_install(SelfInstall::Downloading {
                    percent: Some(percent),
                });
                b.bus.emit(Event::AppUpdateChanged);
            }
        });
        let staged = self_update::stage(
            &self_update::Source::default(),
            &layout,
            self.app_update.current(),
            &release.version,
            std::env::consts::ARCH,
            &progress,
            &irori_core::process::CancellationToken::new(),
        )
        .await
        .and_then(|staged| {
            irori_core::json_store::write(self.app_update.staged_record(), &staged)
                .context("Couldn't save the update")?;
            Ok(staged)
        });
        let result = match staged {
            Ok(staged) => {
                self.app_update.set_install(SelfInstall::Ready {
                    version: staged.version,
                });
                Ok(())
            }
            Err(e) => {
                tracing::warn!("installing Irori {}: {e:#}", release.version);
                self.app_update.set_install(SelfInstall::Failed {
                    error: format!("{e:#}"),
                });
                Err(e)
            }
        };
        self.bus.emit(Event::AppUpdateChanged);
        result
    }

    /// Start what puts the downloaded update in once Irori has quit; the
    /// caller quits right after. Blocking: it may copy the helper.
    pub fn restart_into_app_update(&self) -> anyhow::Result<()> {
        use anyhow::Context as _;
        if !self.app_update.try_begin_restart() {
            anyhow::bail!("Already restarting into the update");
        }
        let record = self.app_update.staged_record().to_path_buf();
        let result = (|| -> anyhow::Result<()> {
            let staged: self_update::Staged = irori_core::json_store::read(&record)?
                .context("No update is waiting for a restart")?;
            self_update::start_helper(&staged, &record)
        })();
        if result.is_err() {
            self.app_update.finish_restart_failed();
        }
        result
    }

    /// Stop announcing `version` of Irori; a later one is announced again.
    pub fn dismiss_app_update(&self, version: &str) {
        self.app_update.dismiss(version);
        self.bus.emit(Event::AppUpdateChanged);
    }

    /// The daily check, for installs the system doesn't update, while online
    /// and with the setting on.
    async fn check_app_update_when_due(&self) {
        if !self.is_online()
            || !self.settings.app().check_for_app_updates
            || !self.app_update.kind().checks()
            || !self.app_update.due(irori_core::util::unix_now())
        {
            return;
        }
        if let Err(e) = self
            .check_app_update_with_kind(irori_core::github_releases::CheckKind::Automatic)
            .await
        {
            tracing::info!("checking for a new version of Irori: {e:#}");
        }
    }

    /// Update the anticheat and known-fixes databases when their published
    /// checksums change.
    pub async fn refresh_compat_data(&self) {
        if self.working_offline() {
            return;
        }
        match irori_extras::releases::fetch().await {
            Ok(info) => {
                if let Err(e) =
                    irori_extras::anticheat::refresh(Some(&info.anticheat_files.sha_linux)).await
                {
                    tracing::warn!("anticheat data: {e:#}");
                }
                if let Err(e) =
                    irori_extras::known_fixes::refresh(Some(&info.known_fixes_sha)).await
                {
                    tracing::warn!("known fixes data: {e:#}");
                }
            }
            Err(e) => tracing::info!("release data unavailable: {e:#}"),
        }
    }

    /// Pause the queue when the connection drops and resume it when it
    /// returns, unless the user paused it themselves.
    fn pause_downloads_while_offline(&self, was_online: bool, online: bool) {
        if was_online && !online && !self.downloads().paused && self.downloads().current().is_some()
        {
            self.auto_paused.store(true, Ordering::Relaxed);
            self.pause_downloads();
            let notice = if self.working_offline() {
                irori_core::Notice::new(irori_core::NoticeLevel::Warning, "Downloads paused")
                    .message(
                        "Irori is working offline. Downloads resume when you work online again.",
                    )
                    .keys(
                        "shell.notice.downloads_paused",
                        Some("shell.notice.downloads_paused_working_offline"),
                        &[],
                    )
            } else {
                irori_core::Notice::new(irori_core::NoticeLevel::Warning, "Downloads paused")
                    .message("You're offline. Downloads resume when the connection is back.")
                    .keys(
                        "shell.notice.downloads_paused",
                        Some("shell.notice.downloads_paused_message"),
                        &[],
                    )
            };
            self.bus.notice(notice);
        } else if online && !was_online && self.auto_paused.swap(false, Ordering::Relaxed) {
            self.resume_downloads();
        }
    }

    pub async fn check_connectivity(&self) -> bool {
        let ok = match decided_online(self.settings.app().network_mode) {
            Some(ok) => ok,
            None if self.isolated => return self.is_online_now(),
            None => irori_core::http::client()
                .head("https://www.gstatic.com/generate_204")
                .timeout(Duration::from_secs(5))
                .send()
                .await
                .is_ok(),
        };
        if self.online.swap(ok, Ordering::Relaxed) != ok {
            self.bus.emit(Event::Connectivity(ok));
        }
        ok
    }

    /// Refresh every logged-in store's catalogue.
    /// Take stores' own playtime totals and last-played times (time played
    /// in their launchers) into Irori's.
    async fn sync_store_playtime(&self) {
        for store in self.stores.values().filter(|s| s.is_logged_in()) {
            match store.remote_playtime().await {
                Ok(totals) if !totals.is_empty() => {
                    let grown =
                        self.userdata
                            .merge_store_playtime(store.runner(), &store.games(), &totals);
                    if grown > 0 {
                        tracing::info!(
                            "{} playtime: {grown} games had more time recorded there",
                            store.runner().store_name()
                        );
                    }
                }
                Ok(_) => {}
                Err(e) => tracing::warn!("{} playtime: {e:#}", store.runner().store_name()),
            }
            match store.remote_last_played().await {
                Ok(times) if !times.is_empty() => {
                    self.userdata
                        .merge_store_last_played(store.runner(), &store.games(), &times);
                }
                Ok(_) => {}
                Err(e) => tracing::warn!("{} last played: {e:#}", store.runner().store_name()),
            }
        }
    }

    pub async fn refresh_all(&self, full: bool) {
        let shown = self.settings.app().ui.stores;
        let offline = self.working_offline();
        let jobs = self
            .stores
            .values()
            .filter(|s| s.is_logged_in() || s.runner() == Runner::Sideload)
            .filter(|s| shown.shows(s.runner()))
            .filter(|s| !offline || refreshes_offline(s.runner()))
            .map(|s| {
                let s = s.clone();
                let b = self.clone();
                async move {
                    let result = s.refresh(full).await;
                    if let Err(e) = &result {
                        tracing::warn!("refreshing {}: {e:#}", s.runner().store_name());
                    }
                    b.note_login(s.runner(), result.as_ref().err());
                }
            });
        futures::future::join_all(jobs).await;
        if offline {
            // Store playtime and updates are asked of the stores' servers.
            self.rebuild_library();
            return;
        }
        self.sync_store_playtime().await;
        self.rebuild_library();
        self.refresh_updates().await;
    }

    /// Games with a pending update (cached; refreshed with the library).
    pub fn updates(&self) -> Arc<std::collections::HashSet<irori_core::GameId>> {
        self.updates.read().clone()
    }

    /// Stores whose last update check failed.
    pub fn update_check_failures(&self) -> Vec<Runner> {
        self.update_check_failed.read().clone()
    }

    /// Stop offering updates for `id`, as its setting does.
    pub fn ignore_game_updates(&self, id: &irori_core::GameId) {
        self.settings
            .set_game_value(id, "ignoreGameUpdates", Some(serde_json::Value::Bool(true)));
        self.forget_update(id);
        self.bus
            .emit(Event::LibraryUpdated(self.library().revision));
    }

    /// `id` has no update to offer any more.
    fn forget_update(&self, id: &irori_core::GameId) {
        let mut set = (**self.updates.read()).clone();
        if set.remove(id) {
            *self.updates.write() = Arc::new(set);
        }
    }

    /// Ask every store which installed games have updates.
    pub async fn refresh_updates(&self) {
        let jobs = self.stores.values().filter(|s| s.is_logged_in()).map(|s| {
            let s = s.clone();
            let b = self.clone();
            async move {
                match s.updatable_games().await {
                    Ok(apps) => (
                        s.runner(),
                        Some(
                            apps.into_iter()
                                .map(|a| irori_core::GameId::new(s.runner(), a))
                                .collect::<Vec<_>>(),
                        ),
                    ),
                    Err(e) => {
                        tracing::warn!("update check for {}: {e:#}", s.runner().store_name());
                        if SessionExpired::of(&e).is_some() {
                            b.note_login(s.runner(), Some(&e));
                        }
                        (s.runner(), None)
                    }
                }
            }
        });
        let answers = futures::future::join_all(jobs).await;
        *self.update_check_failed.write() = answers
            .iter()
            .filter(|(_, apps)| apps.is_none())
            .map(|(runner, _)| *runner)
            .collect();
        let all: std::collections::HashSet<_> =
            updates::merge_game_updates(&self.updates(), answers)
                .into_iter()
                .filter(|id| !self.settings.game(id).ignore_game_updates)
                .collect();
        let new: Vec<irori_core::GameId> = {
            let old = self.updates.read();
            all.iter()
                .filter(|id| !old.contains(*id))
                .cloned()
                .collect()
        };
        *self.updates.write() = Arc::new(all);
        // The first check of a session shows as badges in the library; later
        // ones find updates released while Irori was running, which the user
        // may not see unless told. Automatic updates queue them instead.
        let first = !self.updates_checked.swap(true, Ordering::Relaxed);
        if !first && !self.settings.app().auto_update_games {
            self.notice_new_updates(&new);
        }
        self.bus
            .emit(Event::LibraryUpdated(self.library().revision));
    }

    pub async fn refresh_store(&self, runner: Runner, full: bool) -> anyhow::Result<()> {
        let store = self
            .store(runner)
            .ok_or_else(|| anyhow::anyhow!("{runner} is not available"))?;
        if !refreshes_offline(runner) {
            self.ensure_online()?;
        }
        let result = store.refresh(full).await;
        self.note_login(runner, result.as_ref().err());
        result?;
        self.rebuild_library();
        Ok(())
    }

    /// Refresh one account of a store (`slot` from [`irori_core::Store::accounts`]).
    pub async fn refresh_account(
        &self,
        runner: Runner,
        slot: &str,
        full: bool,
    ) -> anyhow::Result<()> {
        let store = self
            .store(runner)
            .ok_or_else(|| anyhow::anyhow!("{runner} is not available"))?;
        self.ensure_online()?;
        let result = store.refresh_account(slot, full).await;
        self.note_login(runner, result.as_ref().err());
        result?;
        self.rebuild_library();
        Ok(())
    }

    /// Tell the user about updates that appeared since the last check.
    fn notice_new_updates(&self, new: &[irori_core::GameId]) {
        let notice = match new {
            [] => return,
            [id] => {
                let title = self
                    .game(id)
                    .map(|g| g.title.clone())
                    .unwrap_or_else(|| id.app_name.to_string());
                Notice::new(NoticeLevel::Info, format!("Update available for {title}"))
                    .keys("shell.notice.update_available", None, &[("title", &title)])
                    .game(id.clone())
            }
            many => {
                let count = many.len().to_string();
                Notice::new(
                    NoticeLevel::Info,
                    format!("Updates available for {count} games"),
                )
                .keys("shell.notice.updates_available", None, &[("count", &count)])
            }
        };
        self.bus.notice(notice);
    }

    /// Stores whose saved login was refused and needs logging in again, with
    /// the store's reason.
    pub fn expired_logins(&self) -> BTreeMap<Runner, String> {
        self.expired_logins.read().clone()
    }

    /// Forget that a store's login was refused (the user logged in again or
    /// logged out).
    pub fn forget_expired_login(&self, runner: Runner) {
        if self.expired_logins.write().remove(&runner).is_some() {
            self.bus.emit(Event::AccountChanged(runner));
        }
    }

    /// Record how a call to a store ended. A refused login is remembered and
    /// reported once; a call that worked clears it.
    fn note_login(&self, runner: Runner, error: Option<&anyhow::Error>) {
        let Some(expired) = error.and_then(SessionExpired::of) else {
            if error.is_none() {
                self.forget_expired_login(runner);
            }
            return;
        };
        let first = self
            .expired_logins
            .write()
            .insert(runner, expired.reason.clone())
            .is_none();
        if !first {
            return;
        }
        let store = runner.store_name();
        self.bus.emit(Event::AccountChanged(runner));
        self.bus.notice(
            Notice::new(NoticeLevel::Warning, format!("Log in to {store} again"))
                .message(expired.reason.clone())
                .keys(
                    "shell.notice.login_expired",
                    Some("shell.notice.login_expired_message"),
                    &[("store", store)],
                )
                .account(runner),
        );
    }
}

#[cfg(test)]
mod tests;
