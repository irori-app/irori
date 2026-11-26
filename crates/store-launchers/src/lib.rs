//! Official third-party launchers ([`Runner::Launchers`]): Battle.net, the
//! EA app, Ubisoft Connect and the Rockstar Games Launcher, plus the games
//! of the Xbox app and the Microsoft Store on Windows.
//!
//! Irori never handles these launchers' accounts. It sets each launcher up
//! once (a prefix of its own with a Wine build the user picks, and the
//! vendor's official installer; on Windows the installer runs natively), and
//! lists the games the launcher installed by reading its local data
//! (`irori-launchers-data`). Games start through the launcher (its URI or
//! flags); the launcher installs, updates and removes them, so Irori doesn't.
//!
//! A launcher can also be adopted from a prefix the user already has, closed
//! (everything in its prefix stops), and removed from Irori, which forgets it
//! and leaves its prefix and games on disk.
//!
//! Xbox app games (PC Game Pass, Microsoft Store) need no setup: on Windows
//! the installed game packages are found by themselves
//! (`irori_launchers_data::xbox`), start through the shell, and are followed
//! by their processes. Elsewhere there are none.
//!
//! An optional Xbox (Microsoft) account (`xbox_account`) adds the
//! account's minutes played, last played times, achievements and store art
//! to those games, and lists the PC games it played that aren't installed.
//! It is the store's [`Store::login`]; the store itself needs none.
//!
//! Everything is behind `iroriThirdPartyLaunchers`: while it is off the store
//! lists nothing.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Weak};
use std::time::Duration;

use anyhow::{Context as _, Result, bail};
use async_trait::async_trait;
use irori_core::process::{self, CommandSpec};
use irori_core::{
    Achievement, ClientAction, EnvVar, Event, EventBus, ExtraInfo, GameId, GameInfo, GameSettings,
    GameWatch, InstallInfo, InstallInfoQuery, InstallPlatform, InstallRequest, InstalledInfo,
    LaunchPlan, LaunchRequest, LoginFlow, Notice, NoticeLevel, OpContext, Outcome, Paths,
    RemoveArgs, Runner, SessionExpired, SettingsFn, Store, StoreAccount, UpdateRequest, UserInfo,
    WineInstallation, WineKind, json_store, util,
};
use irori_launchers_data::xbox::{self, XboxGame};
use irori_launchers_data::{InstalledGame, Launcher, Registry, ScanReport, SignIn, rockstar};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use xbox_account::XboxAccount;

mod xbox_account;
mod xbox_friends;

/// Online states of [`XboxFriend`]s.
pub use irori_xbox_api::OnlineState as XboxOnlineState;
pub use xbox_account::{
    AccountTitle as XboxAccountTitle, CLIENT_ID_VAR as XBOX_CLIENT_ID_VAR, XBOX_CLIENT_ID,
    XboxAccountStatus,
};
pub use xbox_friends::{XboxFriend, XboxFriends};

pub use irori_launchers_data::Launcher as LauncherKind;
pub use irori_launchers_data::SignIn as LauncherSignIn;
pub use irori_launchers_data::xbox::DISPLAY_NAME as XBOX_APP;

/// Whether `app_name` is an Xbox app game's.
pub fn is_xbox_game(app_name: &str) -> bool {
    app_name
        .strip_prefix(xbox::ID)
        .is_some_and(|rest| rest.starts_with(':'))
}

const MANAGED_BY_LAUNCHER: &str =
    "This game belongs to its launcher; install, update and remove it there";

/// Steam's fixed art addresses, used for the Rockstar titles Irori knows.
const STEAM_CDN: &str = "https://cdn.cloudflare.steamstatic.com/steam/apps";
/// How often a running installer is checked for the launcher's program.
const INSTALL_POLL: Duration = Duration::from_secs(2);

/// A launcher Irori set up.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Setup {
    /// The launcher's own Wine prefix (empty on Windows).
    pub prefix: PathBuf,
    /// The Wine build it runs with (unset on Windows).
    #[serde(default)]
    pub wine: WineInstallation,
    /// RFC 3339.
    #[serde(default)]
    pub set_up_at: String,
    /// The launcher's own settings, laid over the game defaults as a
    /// game's sparse overrides are (environment variables so far).
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub settings: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct StateFile {
    setups: BTreeMap<Launcher, Setup>,
}

#[derive(Default)]
struct State {
    setups: BTreeMap<Launcher, Setup>,
    found: Vec<InstalledGame>,
    games: Vec<Arc<GameInfo>>,
    sign_in: BTreeMap<Launcher, SignIn>,
    warnings: Vec<String>,
    xbox: Vec<XboxGame>,
    xbox_app: bool,
    /// Xbox games' sizes in bytes, by package folder and version.
    sizes: HashMap<(PathBuf, Option<String>), u64>,
    /// PC games the Xbox account played.
    account: Vec<xbox_account::AccountTitle>,
}

/// What the launchers page shows for the Xbox app.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct XboxStatus {
    /// Xbox games can be listed here: Windows only.
    pub supported: bool,
    /// The Xbox app is installed.
    pub app_installed: bool,
    /// Games found.
    pub games: usize,
}

/// What the launchers page shows for one launcher.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LauncherStatus {
    pub launcher: Launcher,
    pub setup: Option<Setup>,
    /// The launcher's program exists.
    pub installed: bool,
    /// Games found for it.
    pub games: usize,
    /// Whether someone is signed in to it, as of the last scan.
    pub sign_in: SignIn,
}

pub struct LaunchersStore {
    bus: EventBus,
    settings: SettingsFn,
    state_file: PathBuf,
    downloads: PathBuf,
    /// Installers' output.
    logs: PathBuf,
    state: RwLock<State>,
    /// One setup or scan at a time.
    busy: tokio::sync::Mutex<()>,
    xbox_account: XboxAccount,
    /// The account's expiry was announced.
    expiry_told: std::sync::atomic::AtomicBool,
    me: Weak<Self>,
}

impl LaunchersStore {
    pub fn new(bus: EventBus, settings: SettingsFn) -> Arc<Self> {
        let paths = Paths::get();
        let (config, cache) = (
            paths.store_config_dir(Runner::Launchers),
            paths.store_cache_dir(Runner::Launchers),
        );
        let xbox = XboxAccount::new(&config, &cache, Default::default());
        Self::build(bus, settings, config, cache, paths.logs_dir(), xbox)
    }

    /// A store keeping its files in the given folders (tests).
    pub fn with_dirs(
        bus: EventBus,
        settings: SettingsFn,
        config_dir: PathBuf,
        cache_dir: PathBuf,
    ) -> Arc<Self> {
        let logs = cache_dir.join("logs");
        let xbox = XboxAccount::new(&config_dir, &cache_dir, Default::default());
        Self::build(bus, settings, config_dir, cache_dir, logs, xbox)
    }

    /// A store keeping its files in the given folders whose Xbox account
    /// talks to `endpoints` (tests).
    pub fn with_xbox_endpoints(
        bus: EventBus,
        settings: SettingsFn,
        config_dir: PathBuf,
        cache_dir: PathBuf,
        endpoints: irori_xbox_api::Endpoints,
    ) -> Arc<Self> {
        let logs = cache_dir.join("logs");
        let client = irori_xbox_api::Client::with_endpoints("test-client", endpoints).ok();
        let xbox = XboxAccount::with_client(&config_dir, &cache_dir, client);
        Self::build(bus, settings, config_dir, cache_dir, logs, xbox)
    }

    fn build(
        bus: EventBus,
        settings: SettingsFn,
        config_dir: PathBuf,
        cache_dir: PathBuf,
        logs: PathBuf,
        xbox_account: XboxAccount,
    ) -> Arc<Self> {
        Arc::new_cyclic(|me| Self {
            bus,
            settings,
            state_file: config_dir.join("launchers.json"),
            downloads: cache_dir.join("installers"),
            logs,
            state: RwLock::new(State::default()),
            busy: tokio::sync::Mutex::new(()),
            xbox_account,
            expiry_told: Default::default(),
            me: me.clone(),
        })
    }

    fn enabled(&self) -> bool {
        (self.settings)().third_party_launchers
    }

    /// Where a launcher's prefix goes: beside the other prefixes.
    pub fn default_prefix(&self, launcher: Launcher) -> PathBuf {
        let app = (self.settings)();
        irori_core::paths::expand_home(&app.default_wine_prefix_dir).join(launcher.display_name())
    }

    /// Every launcher with its setup and game count.
    pub fn statuses(&self) -> Vec<LauncherStatus> {
        let s = self.state.read();
        Launcher::ALL
            .into_iter()
            .map(|launcher| {
                let setup = s.setups.get(&launcher).cloned();
                LauncherStatus {
                    launcher,
                    installed: launcher_program(launcher, setup.as_ref())
                        .is_some_and(|p| p.exists()),
                    games: s.found.iter().filter(|g| g.launcher == launcher).count(),
                    sign_in: s.sign_in.get(&launcher).copied().unwrap_or_default(),
                    setup,
                }
            })
            .collect()
    }

    /// Whether Xbox games can be listed, and what the last scan found.
    pub fn xbox_status(&self) -> XboxStatus {
        let s = self.state.read();
        XboxStatus {
            supported: cfg!(windows),
            app_installed: s.xbox_app,
            games: s.xbox.len(),
        }
    }

    /// The Xbox account: whether Irori can sign in, who is signed in, and
    /// what it found.
    pub fn xbox_account_status(&self) -> XboxAccountStatus {
        self.xbox_account.status()
    }

    /// Whether an Xbox account is signed in, so its friends can be read.
    pub fn has_xbox_account(&self) -> bool {
        self.xbox_account.user().is_some()
    }

    /// The Xbox account's friends, with what they play; `None` while
    /// signed out. A list read in the last minute is reused unless `force`.
    pub async fn xbox_friends(&self, force: bool) -> Result<Option<XboxFriends>> {
        let language = (self.settings)().ui_language.clone();
        self.xbox_account.friends(force, &language).await
    }

    /// Fetch the Xbox account's titles again and show them. A refused
    /// sign-in is announced once and fails with [`SessionExpired`].
    async fn refresh_xbox(&self, full: bool) -> Result<()> {
        let language = (self.settings)().ui_language.clone();
        match self.xbox_account.refresh(full, &language).await {
            Ok(changed) => {
                if changed {
                    self.show_account_titles();
                }
                Ok(())
            }
            Err(e) => {
                if let Some(expired) = SessionExpired::of(&e)
                    && !self
                        .expiry_told
                        .swap(true, std::sync::atomic::Ordering::Relaxed)
                {
                    self.bus.emit(Event::AccountChanged(Runner::Launchers));
                    self.bus.notice(
                        Notice::new(NoticeLevel::Warning, "Sign in to Xbox again")
                            .message(expired.reason.clone())
                            .keys(
                                "shell.notice.login_expired",
                                Some("shell.notice.xbox_login_expired_message"),
                                &[("store", "Xbox")],
                            ),
                    );
                }
                Err(e)
            }
        }
    }

    /// Rebuild the library with the account's titles as last fetched.
    fn show_account_titles(&self) {
        {
            let mut s = self.state.write();
            s.account = self.xbox_account.titles();
            s.games = library(&s);
        }
        self.bus.emit(Event::LibraryChanged(Runner::Launchers));
    }

    /// Start the Xbox app (Windows), to sign in or install games there.
    pub async fn open_xbox_app(&self) -> Result<()> {
        anyhow::ensure!(cfg!(windows), "The Xbox app runs only on Windows");
        process::spawn(
            &shell_open(&format!("shell:AppsFolder\\{}", xbox::XBOX_APP_AUMID)),
            None,
        )?;
        Ok(())
    }

    /// Files Irori couldn't read during the last scan.
    pub fn warnings(&self) -> Vec<String> {
        self.state.read().warnings.clone()
    }

    async fn save(&self) -> Result<()> {
        let file = StateFile {
            setups: self.state.read().setups.clone(),
        };
        let path = self.state_file.clone();
        tokio::task::spawn_blocking(move || json_store::write(&path, &file))
            .await
            .context("saving stopped")?
            .context("Could not save the launchers")
    }

    /// Download the launcher's official installer and run it: inside a new
    /// prefix with `wine` on Linux and macOS, natively on Windows. Waits until
    /// the launcher is installed, then lists the launcher's games.
    pub async fn set_up(
        &self,
        launcher: Launcher,
        wine: Option<WineInstallation>,
        ctx: &OpContext,
    ) -> Result<()> {
        let _one = self.busy.lock().await;
        let wine = if cfg!(windows) {
            WineInstallation::default()
        } else {
            wine.filter(WineInstallation::is_set)
                .context("Choose a Wine build for the launcher")?
        };
        (ctx.step)("Downloading");
        let installer = self.downloads.join(format!("{}-setup.exe", launcher.id()));
        irori_runtime::download::download(
            launcher.installer_url(),
            &installer,
            &ctx.progress,
            &ctx.cancel,
        )
        .await
        .with_context(|| {
            format!(
                "Could not download the {} installer",
                launcher.display_name()
            )
        })?;
        let setup = Setup {
            prefix: if cfg!(windows) {
                PathBuf::new()
            } else {
                self.default_prefix(launcher)
            },
            wine,
            set_up_at: util::now_rfc3339(),
            settings: Default::default(),
        };
        self.run_installer(launcher, &setup, &installer, ctx)
            .await
            .with_context(|| format!("The {} installer failed", launcher.display_name()))?;
        self.state.write().setups.insert(launcher, setup);
        self.save().await?;
        let _ = tokio::fs::remove_file(&installer).await;
        drop(_one);
        self.rescan().await;
        Ok(())
    }

    /// Run `installer` until the launcher's program exists. The installers
    /// start the launcher when they finish, and the launcher (and its update
    /// agent) keep running with the installer's process tree and the prefix,
    /// so neither ever ends on its own: waiting for them stalls the setup
    /// until the user quits the launcher.
    async fn run_installer(
        &self,
        launcher: Launcher,
        setup: &Setup,
        installer: &Path,
        ctx: &OpContext,
    ) -> Result<()> {
        let program = launcher_program(launcher, Some(setup)).context("no launcher program")?;
        let log = self.logs.join(format!("{}-setup.log", launcher.id()));
        let _ = tokio::fs::remove_file(&log).await;
        let spec = if cfg!(windows) {
            CommandSpec::new(installer)
        } else {
            let app = (self.settings)();
            let settings = game_settings(&app.game, setup);
            let umu = if irori_runtime::runtimes::is_umu_supported(&settings, false) {
                match irori_runtime::runtimes::ensure_umu(ctx).await {
                    Ok(path) => Some(path),
                    Err(e) => {
                        tracing::warn!("{e:#}; running Proton directly");
                        None
                    }
                }
            } else {
                None
            };
            let wc = wine_context(setup, umu);
            irori_runtime::prefix::verify_prefix(&app, &settings, &wc, ctx).await?;
            // Before the installer starts the launcher for the first time,
            // so it reads them from the start.
            let drive = irori_launchers_data::drive_c(&windows_root(setup));
            match tokio::task::spawn_blocking(move || {
                irori_launchers_data::wine_config::prepare(launcher, &drive)
            })
            .await
            {
                Ok(Ok(files)) => {
                    for file in files {
                        tracing::info!("Wine-friendly settings in {}", file.display());
                    }
                }
                Ok(Err(e)) => tracing::warn!(
                    "could not write {}'s Wine settings: {e}",
                    launcher.display_name()
                ),
                Err(e) => tracing::warn!("writing launcher settings stopped: {e}"),
            }
            let mut cmd =
                irori_runtime::prefix::WineCommand::new(installer_argv(launcher, installer));
            cmd.skip_prefix_check = true;
            irori_runtime::prefix::wine_spec(
                &app,
                &settings,
                &wc,
                &cmd,
                &irori_runtime::env::HostEnv::current(),
            )
        };
        if program.exists() {
            return Ok(());
        }
        (ctx.step)("Installing");
        let mut child = process::spawn(&spec, Some(&log))?;
        let mut tick = tokio::time::interval(INSTALL_POLL);
        let mut exited = false;
        let settled = async {
            if !cfg!(windows) {
                // A setup program may hand over to a process of its own and
                // exit; the prefix tells when everything it started is done.
                let _ = irori_runtime::prefix::wait(&setup.wine, &setup.prefix).await;
            }
        };
        tokio::pin!(settled);
        loop {
            tokio::select! {
                _ = tick.tick() => {
                    if program.exists() {
                        tracing::info!("{} installed in {}", launcher.display_name(), setup.prefix.display());
                        return Ok(());
                    }
                }
                _ = child.wait(), if !exited => exited = true,
                _ = &mut settled, if exited => {
                    if program.exists() {
                        return Ok(());
                    }
                    bail!(
                        "The installer closed before {} was installed. Its output is in {}",
                        launcher.display_name(),
                        log.display()
                    );
                }
                _ = ctx.cancel.cancelled() => {
                    process::terminate(&mut child).await;
                    if !cfg!(windows) {
                        let _ = irori_runtime::prefix::kill(&setup.wine, &setup.prefix).await;
                    }
                    bail!("The installer was stopped");
                }
            }
        }
    }

    /// The launchers set up in a prefix of their own, whose Wine settings
    /// Irori keeps (none on Windows). Reads no files.
    pub fn wine_launchers(&self) -> Vec<Launcher> {
        self.state.read().setups.keys().copied().collect()
    }

    /// The launchers Irori set up, or whose games it found (on Windows the
    /// launchers need no setup). Reads no files.
    pub fn known_launchers(&self) -> Vec<Launcher> {
        let s = self.state.read();
        Launcher::ALL
            .into_iter()
            .filter(|l| s.setups.contains_key(l) || s.found.iter().any(|g| g.launcher == *l))
            .collect()
    }

    /// The launcher's program, when it is installed (its icon makes the
    /// launcher's shortcuts').
    pub fn program(&self, launcher: Launcher) -> Option<PathBuf> {
        let setup = self.state.read().setups.get(&launcher).cloned();
        launcher_program(launcher, setup.as_ref())
    }

    /// Start the launcher itself (to sign in or install games there).
    pub async fn open(&self, launcher: Launcher) -> Result<()> {
        let setup = self.state.read().setups.get(&launcher).cloned();
        let program = launcher_program(launcher, setup.as_ref())
            .filter(|p| p.exists())
            .with_context(|| format!("{} isn't set up yet", launcher.display_name()))?;
        let mut spec = self.command(launcher, setup.as_ref(), &program, Vec::new())?;
        self.apply_hud_with_paths(&mut spec, Paths::get(), None)
            .await?;
        process::spawn(&spec, None)?;
        Ok(())
    }

    /// Use a prefix the user already has, with the launcher installed in it,
    /// run with `wine`. Nothing in the prefix changes.
    pub async fn adopt(
        &self,
        launcher: Launcher,
        prefix: PathBuf,
        wine: Option<WineInstallation>,
    ) -> Result<()> {
        anyhow::ensure!(
            !cfg!(windows),
            "On Windows, Irori finds the launcher by itself"
        );
        let wine = wine
            .filter(WineInstallation::is_set)
            .context("Choose a Wine build for the launcher")?;
        let _one = self.busy.lock().await;
        let prefix = tokio::task::spawn_blocking(move || adoptable_prefix(launcher, &prefix))
            .await
            .context("checking the prefix stopped")??;
        self.state.write().setups.insert(
            launcher,
            Setup {
                prefix,
                wine,
                set_up_at: util::now_rfc3339(),
                settings: Default::default(),
            },
        );
        self.save().await?;
        drop(_one);
        self.rescan().await;
        Ok(())
    }

    /// Stop the launcher, and with it everything running in its prefix
    /// (its games too). On Windows, the processes started from its folder.
    pub async fn close(&self, launcher: Launcher) -> Result<()> {
        let setup = self.state.read().setups.get(&launcher).cloned();
        if cfg!(windows) {
            let folder = launcher_program(launcher, setup.as_ref())
                .and_then(|p| p.parent().map(Path::to_path_buf))
                .context("unknown launcher folder")?;
            tokio::task::spawn_blocking(move || stop_processes_in(&folder))
                .await
                .context("closing stopped")?;
        } else {
            let setup =
                setup.with_context(|| format!("{} isn't set up yet", launcher.display_name()))?;
            irori_runtime::prefix::kill(&setup.wine, &setup.prefix)
                .await
                .with_context(|| format!("Could not close {}", launcher.display_name()))?;
        }
        self.rescan().await;
        Ok(())
    }

    /// A set-up launcher's Wine build and its own environment variables.
    pub fn wine_settings(&self, launcher: Launcher) -> Option<(WineInstallation, Vec<EnvVar>)> {
        let s = self.state.read();
        let setup = s.setups.get(&launcher)?;
        let variables = setup
            .settings
            .get(VARIABLES)
            .and_then(|v| serde_json::from_value::<Vec<EnvVar>>(v.clone()).ok())
            .unwrap_or_default();
        Some((setup.wine.clone(), variables))
    }

    /// Change a set-up launcher's Wine build and environment variables; the
    /// launcher and its games run with them from their next start.
    pub async fn set_wine_settings(
        &self,
        launcher: Launcher,
        wine: WineInstallation,
        variables: Vec<EnvVar>,
    ) -> Result<()> {
        anyhow::ensure!(wine.is_set(), "Choose a Wine build for the launcher");
        {
            let mut s = self.state.write();
            let setup = s
                .setups
                .get_mut(&launcher)
                .with_context(|| format!("{} isn't set up yet", launcher.display_name()))?;
            setup.wine = wine;
            if variables.is_empty() {
                setup.settings.remove(VARIABLES);
            } else {
                setup
                    .settings
                    .insert(VARIABLES.into(), serde_json::to_value(&variables)?);
            }
        }
        self.save().await
    }

    /// Forget a launcher's setup. Its prefix and games stay on disk.
    pub async fn forget(&self, launcher: Launcher) -> Result<()> {
        self.state.write().setups.remove(&launcher);
        self.save().await?;
        self.rescan().await;
        Ok(())
    }

    /// The command that runs `program` of `launcher` with `args`.
    fn command(
        &self,
        launcher: Launcher,
        setup: Option<&Setup>,
        program: &Path,
        args: Vec<String>,
    ) -> Result<CommandSpec> {
        if cfg!(windows) {
            return Ok(CommandSpec::new(program).args(args));
        }
        let setup =
            setup.with_context(|| format!("{} isn't set up yet", launcher.display_name()))?;
        let app = (self.settings)();
        let settings = game_settings(&app.game, setup);
        let umu = irori_runtime::runtimes::is_umu_supported(&settings, true)
            .then(irori_runtime::runtimes::umu_path);
        let wc = wine_context(setup, umu);
        irori_runtime::prefix::check_layout(&wc)?;
        let mut argv = vec![program.display().to_string()];
        argv.extend(args);
        if launcher == Launcher::BattleNet
            && !argv[1..]
                .iter()
                .any(|arg| arg == BATTLENET_IN_PROCESS_GPU_FLAG)
        {
            argv.push(BATTLENET_IN_PROCESS_GPU_FLAG.to_string());
        }
        let mut cmd = irori_runtime::prefix::WineCommand::new(argv);
        cmd.skip_prefix_check = true;
        cmd.cwd = program.parent().map(Path::to_path_buf);
        let mut spec = irori_runtime::prefix::wine_spec(
            &app,
            &settings,
            &wc,
            &cmd,
            &irori_runtime::env::HostEnv::current(),
        );
        if launcher == Launcher::BattleNet {
            merge_dll_override(&mut spec, "locationapi", "d");
        }
        Ok(spec)
    }

    /// Write any managed HUD config under `paths` and apply HUD environment
    /// variables to `spec` for either a specific game or the global profile.
    async fn apply_hud_with_paths(
        &self,
        spec: &mut CommandSpec,
        paths: &Paths,
        game: Option<(&str, &GameSettings)>,
    ) -> Result<()> {
        if cfg!(windows) {
            return Ok(());
        }
        let app = (self.settings)();
        let default_settings = GameSettings::default();
        let (game_id, settings) = match game {
            Some((app_name, settings)) => {
                (Some(GameId::new(Runner::Launchers, app_name)), settings)
            }
            None => (None, &default_settings),
        };
        irori_runtime::apply_hud_to_command_with_paths(
            spec,
            paths,
            &app,
            settings,
            game_id.as_ref(),
            InstallPlatform::Windows,
        )
        .await
    }

    /// Read every set-up launcher's local data again.
    async fn rescan(&self) {
        let setups = self.state.read().setups.clone();
        let report = tokio::task::spawn_blocking(move || Scan::run(&setups))
            .await
            .unwrap_or_default();
        let missing_sizes = {
            let mut s = self.state.write();
            s.found = report.games;
            s.sign_in = report.sign_in;
            s.warnings = report.warnings;
            s.xbox = report.xbox;
            s.xbox_app = report.xbox_app;
            s.games = library(&s);
            s.xbox
                .iter()
                .map(|g| (g.install_dir.clone(), g.version.clone()))
                .filter(|key| !s.sizes.contains_key(key))
                .collect::<Vec<_>>()
        };
        self.bus.emit(Event::LibraryChanged(Runner::Launchers));
        if !missing_sizes.is_empty() {
            self.measure(missing_sizes);
        }
    }

    /// Add up the sizes of Xbox games' folders in the background, since a
    /// large game takes a while, then show them.
    fn measure(&self, folders: Vec<(PathBuf, Option<String>)>) {
        let Ok(rt) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let me = self.me.clone();
        rt.spawn(async move {
            let sizes = tokio::task::spawn_blocking(move || {
                folders
                    .into_iter()
                    .map(|(dir, version)| {
                        let size = xbox::game_size(&dir);
                        ((dir, version), size)
                    })
                    .collect::<Vec<_>>()
            })
            .await
            .unwrap_or_default();
            let Some(store) = me.upgrade() else { return };
            {
                let mut s = store.state.write();
                s.sizes.extend(sizes);
                s.games = library(&s);
            }
            store.bus.emit(Event::LibraryChanged(Runner::Launchers));
        });
    }

    fn xbox_game(&self, app_name: &str) -> Option<XboxGame> {
        if !is_xbox_game(app_name) {
            return None;
        }
        self.state
            .read()
            .xbox
            .iter()
            .find(|g| g.app_name() == app_name)
            .cloned()
    }
}

/// Every game the library shows, from what the last scan found and the
/// Xbox account's titles.
fn library(s: &State) -> Vec<Arc<GameInfo>> {
    let title = |pfn: &str| s.account.iter().find(|t| t.pfn.eq_ignore_ascii_case(pfn));
    let launchers = s.found.iter().map(game_info);
    let xbox = s.xbox.iter().map(|g| {
        let size = s
            .sizes
            .get(&(g.install_dir.clone(), g.version.clone()))
            .copied();
        let info = xbox_game_info(g, size);
        match title(&g.family_name) {
            Some(t) => with_account_title(info, t),
            None => info,
        }
    });
    let played = s
        .account
        .iter()
        .filter(|t| {
            !s.xbox
                .iter()
                .any(|g| g.family_name.eq_ignore_ascii_case(&t.pfn))
        })
        .map(account_game_info);
    unique(launchers.chain(xbox).chain(played).map(Arc::new).collect())
}

/// The games with one entry per app name, the first kept: two entries
/// with one id would share every per-game state, and the UI keys its
/// elements by it.
fn unique(games: Vec<Arc<GameInfo>>) -> Vec<Arc<GameInfo>> {
    let mut seen = std::collections::HashSet::new();
    let before = games.len();
    let games: Vec<_> = games
        .into_iter()
        .filter(|g| seen.insert(g.app_name.clone()))
        .collect();
    if games.len() != before {
        tracing::warn!(
            "{} launcher games listed twice; showing each once",
            before - games.len()
        );
    }
    games
}

/// An installed Xbox game with what the account knows: the store's cover
/// and hero art over the package's small tiles, and its details where the
/// package has none.
fn with_account_title(info: GameInfo, t: &xbox_account::AccountTitle) -> GameInfo {
    GameInfo {
        art_cover: t.cover.clone().or(info.art_cover),
        art_square: t.square.clone().or(info.art_square),
        art_background: t.background.clone().or(info.art_background),
        art_logo: t.logo.clone().or(info.art_logo),
        developer: info.developer.or_else(|| t.developer.clone()),
        publisher: info.publisher.or_else(|| t.publisher.clone()),
        description: info.description.or_else(|| t.description.clone()),
        store_url: info.store_url.or_else(|| t.store_web_url()),
        release_date: info.release_date.or_else(|| t.release_date.clone()),
        genres: if info.genres.is_empty() {
            t.genres.clone()
        } else {
            info.genres
        },
        ..info
    }
    .finalize()
}

/// A PC game the Xbox account played that isn't installed here: listed,
/// never installed by Irori (the Microsoft Store page installs it).
fn account_game_info(t: &xbox_account::AccountTitle) -> GameInfo {
    GameInfo {
        runner: Some(Runner::Launchers),
        app_name: t.app_name(),
        title: t.name.clone(),
        developer: t.developer.clone(),
        publisher: t.publisher.clone(),
        description: t.description.clone(),
        store_url: t.store_web_url(),
        art_cover: t.cover.clone(),
        art_square: t.square.clone(),
        art_background: t.background.clone(),
        art_logo: t.logo.clone(),
        is_installed: false,
        installable: false,
        third_party_managed_app: Some(xbox::DISPLAY_NAME.to_string()),
        platforms: vec![InstallPlatform::Windows],
        release_date: t.release_date.clone(),
        genres: t.genres.clone(),
        ..Default::default()
    }
    .finalize()
}

/// The command that hands `target` (a link, or a `shell:` address) to the
/// Windows shell, as the Start menu and Run box do. `ShellExec_RunDLL`
/// returns at once and with success, so the session follows the game, not
/// this command.
fn shell_open(target: &str) -> CommandSpec {
    CommandSpec::new("rundll32.exe")
        .arg("shell32.dll,ShellExec_RunDLL")
        .arg(target)
}

/// Follows an Xbox game by the processes running from its package folder:
/// the shell starts it and hands it to Windows.
struct XboxWatch {
    folder: String,
}

impl XboxWatch {
    fn processes(&self) -> sysinfo::System {
        use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};
        let mut sys = System::new();
        sys.refresh_processes_specifics(
            ProcessesToUpdate::All,
            true,
            ProcessRefreshKind::nothing()
                .with_cmd(UpdateKind::Always)
                .with_exe(UpdateKind::Always),
        );
        sys
    }

    fn runs_here(&self, p: &sysinfo::Process) -> bool {
        let cmd: Vec<String> = p
            .cmd()
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        process::runs_from(&self.folder, p.exe(), &cmd)
    }
}

impl GameWatch for XboxWatch {
    fn running(&self) -> bool {
        self.processes()
            .processes()
            .values()
            .any(|p| self.runs_here(p))
    }

    fn stop(&self, force: bool) {
        for p in self.processes().processes().values() {
            if self.runs_here(p) {
                if force {
                    p.kill();
                } else {
                    // Not supported on Windows; the forced stop that follows
                    // ends the game there.
                    let _ = p.kill_with(sysinfo::Signal::Term);
                }
            }
        }
    }
}

/// Launch settings for a launcher's prefix and Wine build.
fn game_settings(defaults: &GameSettings, setup: &Setup) -> GameSettings {
    let own = irori_core::GameSettingsOverrides(setup.settings.clone()).apply(defaults);
    GameSettings {
        wine_version: setup.wine.clone(),
        wine_prefix: setup.prefix.display().to_string(),
        ..own
    }
}

/// The settings key of the launcher's environment variables (Heroic's).
const VARIABLES: &str = "enviromentOptions";

/// Keeps Battle.net's CEF GPU process inside the main process under Wine/Proton
/// so hardware-accelerated UI rendering stays responsive without blank windows.
const BATTLENET_IN_PROCESS_GPU_FLAG: &str = "--in-process-gpu";

/// Forces `battlenet-setup.exe` to install where [`Launcher::exe_path`]
/// expects `Battle.net.exe` inside the Wine prefix.
const BATTLENET_INSTALL_PATH_FLAG: &str = r"--installpath=C:\Program Files (x86)\Battle.net";

/// Arguments passed to Wine/Proton when running a launcher's Windows installer.
fn installer_argv(launcher: Launcher, installer: &Path) -> Vec<String> {
    let mut argv = vec![installer.display().to_string()];
    if launcher == Launcher::BattleNet {
        argv.push(BATTLENET_INSTALL_PATH_FLAG.to_string());
    }
    argv
}

/// Merge `dll=mode` into `spec.env`'s `WINEDLLOVERRIDES` entry (preserving any
/// existing overrides and key casing without duplicating `dll`).
fn merge_dll_override(spec: &mut CommandSpec, dll: &str, mode: &str) {
    let key = spec
        .env
        .keys()
        .find(|k| k.eq_ignore_ascii_case("WINEDLLOVERRIDES"))
        .cloned()
        .unwrap_or_else(|| "WINEDLLOVERRIDES".to_string());
    let merged = irori_runtime::env::add_dll_overrides(
        spec.env.get(&key).map(String::as_str),
        &[(dll, mode)],
    );
    spec.env.insert(key, merged);
}

/// The Wine layer a launcher's prefix runs with. Proton goes through `umu`
/// when there is one, as games do: run directly, outside the Steam Linux
/// Runtime, its 32-bit parts need the system's 32-bit loader, which a
/// Flatpak without its i386 extension doesn't have, and the installer can't
/// start. A prefix Proton made directly keeps working, since umu uses its
/// `pfx` folder.
fn wine_context(setup: &Setup, umu: Option<PathBuf>) -> irori_runtime::WineContext {
    irori_runtime::WineContext::new(
        setup.wine.clone(),
        setup.prefix.clone(),
        umu.is_some(),
        umu.unwrap_or_default(),
    )
}

/// Where the setup's `drive_c` and registry are: what [`wine_root`] finds on
/// disk, and before the prefix exists, `<prefix>/pfx` for Proton, which
/// keeps them there.
fn windows_root(setup: &Setup) -> PathBuf {
    let found = wine_root(&setup.prefix);
    if found == setup.prefix && setup.wine.kind == WineKind::Proton {
        setup.prefix.join("pfx")
    } else {
        found
    }
}

/// The launcher's program on this computer.
fn launcher_program(launcher: Launcher, setup: Option<&Setup>) -> Option<PathBuf> {
    if cfg!(windows) {
        return Some(PathBuf::from(launcher.windows_exe_path()));
    }
    setup.map(|s| launcher.exe_in_prefix(&windows_root(s)))
}

/// What a scan of the launchers' data found.
#[derive(Default)]
struct Scan {
    games: Vec<InstalledGame>,
    sign_in: BTreeMap<Launcher, SignIn>,
    warnings: Vec<String>,
    xbox: Vec<XboxGame>,
    xbox_app: bool,
}

impl Scan {
    /// Read the launchers' data. Blocking.
    fn run(setups: &BTreeMap<Launcher, Setup>) -> Scan {
        let mut report = ScanReport::default();
        let mut sign_in = BTreeMap::new();
        let mut xbox = xbox::XboxScan::default();
        if cfg!(windows) {
            xbox = xbox::scan_windows();
            report.warnings.append(&mut xbox.warnings);
            let registry = windows_registry();
            let documents: Vec<PathBuf> = std::env::var_os("USERPROFILE")
                .map(|home| PathBuf::from(home).join("Documents"))
                .into_iter()
                .collect();
            for launcher in Launcher::ALL {
                report.merge(launcher.scan_drive(Path::new("C:\\"), &registry));
                sign_in.insert(launcher, launcher.sign_in(&documents));
            }
        } else {
            for (launcher, setup) in setups {
                let root = wine_root(&setup.prefix);
                report.merge(launcher.scan_prefix(&root));
                let documents =
                    irori_launchers_data::user_documents(&irori_launchers_data::drive_c(&root));
                sign_in.insert(*launcher, launcher.sign_in(&documents));
            }
        }
        Scan {
            games: report.games,
            sign_in,
            warnings: report.warnings,
            xbox: xbox.games,
            xbox_app: xbox.xbox_app,
        }
    }
}

/// The folder of a prefix that holds `drive_c` and the registry: `pfx`
/// inside a Proton prefix.
fn wine_root(prefix: &Path) -> PathBuf {
    let proton = prefix.join("pfx");
    if proton.join("drive_c").is_dir() {
        proton
    } else {
        prefix.to_path_buf()
    }
}

/// `prefix` when the launcher is installed in it, else an error saying what
/// is wrong. A Proton prefix may be given by its `pfx` folder or the folder
/// holding it. Blocking.
fn adoptable_prefix(launcher: Launcher, prefix: &Path) -> Result<PathBuf> {
    let prefix = if prefix.file_name().is_some_and(|n| n == "pfx")
        && prefix.parent().is_some_and(|p| p.join("pfx").is_dir())
    {
        prefix.parent().map(Path::to_path_buf).unwrap_or_default()
    } else {
        prefix.to_path_buf()
    };
    let drive = irori_launchers_data::drive_c(&wine_root(&prefix));
    anyhow::ensure!(
        drive.is_dir(),
        "{} isn't a Wine prefix: it has no drive_c folder",
        prefix.display()
    );
    let program = drive.join(launcher.exe_path());
    anyhow::ensure!(
        program.is_file(),
        "{} isn't installed in {}",
        launcher.display_name(),
        prefix.display()
    );
    Ok(prefix)
}

/// Stop every process started from `folder` (Windows). Blocking.
fn stop_processes_in(folder: &Path) {
    use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};
    let folder = folder.to_string_lossy();
    let mut sys = System::new();
    sys.refresh_processes_specifics(
        ProcessesToUpdate::All,
        true,
        ProcessRefreshKind::nothing().with_exe(UpdateKind::Always),
    );
    for process in sys.processes().values() {
        if process::runs_from(&folder, process.exe(), &[]) {
            process.kill();
        }
    }
}

/// The machine-wide registry keys launchers write, exported with `reg`
/// (Windows only; empty elsewhere or when `reg` fails).
fn windows_registry() -> Registry {
    #[cfg(windows)]
    {
        let out = std::env::temp_dir().join("irori-launchers.reg");
        let mut text = String::new();
        for key in [
            r"HKLM\SOFTWARE\WOW6432Node\Ubisoft",
            r"HKLM\SOFTWARE\WOW6432Node\Rockstar Games",
            r"HKLM\SOFTWARE\Rockstar Games",
        ] {
            // A short system tool on the blocking scan thread, not a game or
            // a long-running tool, so `irori_core::process` isn't needed.
            let mut reg = std::process::Command::new("reg");
            let ok = irori_core::process::hide_console(&mut reg)
                .args(["export", key])
                .arg(&out)
                .arg("/y")
                .status()
                .is_ok_and(|s| s.success());
            if ok && let Ok(bytes) = std::fs::read(&out) {
                text.push_str(&decode_reg(&bytes));
                text.push('\n');
            }
        }
        let _ = std::fs::remove_file(&out);
        Registry::parse(&text)
    }
    #[cfg(not(windows))]
    {
        Registry::default()
    }
}

/// `reg export` writes UTF-16 with a byte-order mark.
#[cfg(windows)]
fn decode_reg(bytes: &[u8]) -> String {
    if bytes.starts_with(&[0xff, 0xfe]) {
        let (pairs, _) = bytes[2..].as_chunks::<2>();
        let units: Vec<u16> = pairs.iter().map(|c| u16::from_le_bytes(*c)).collect();
        String::from_utf16_lossy(&units)
    } else {
        String::from_utf8_lossy(bytes).into_owned()
    }
}

/// What the library shows for a launcher's game.
fn game_info(g: &InstalledGame) -> GameInfo {
    let known = (g.launcher == Launcher::Rockstar)
        .then(|| rockstar::title_by_id(&g.id))
        .flatten();
    let art = |file: &str| known.map(|t| format!("{STEAM_CDN}/{}/{file}", t.steam_app_id));
    GameInfo {
        runner: Some(Runner::Launchers),
        app_name: g.app_name(),
        title: g.title.clone(),
        publisher: Some(g.launcher.display_name().to_string()),
        developer: known.map(|_| "Rockstar Games".to_string()),
        art_cover: art("library_600x900_2x.jpg"),
        art_square: art("header.jpg"),
        art_background: art("library_hero.jpg"),
        art_logo: art("logo.png"),
        is_installed: true,
        install: Some(InstalledInfo {
            install_path: g.install_dir.display().to_string(),
            executable: g.executable.clone().unwrap_or_default(),
            platform: InstallPlatform::Windows,
            ..Default::default()
        }),
        installable: false,
        third_party_managed_app: Some(g.launcher.display_name().to_string()),
        is_ea_managed: g.launcher == Launcher::EaApp,
        is_ubisoft_managed: g.launcher == Launcher::Ubisoft,
        platforms: vec![InstallPlatform::Windows],
        folder_name: g
            .install_dir
            .to_string_lossy()
            .rsplit(['\\', '/'])
            .find(|s| !s.is_empty())
            .map(str::to_string),
        ..Default::default()
    }
    .finalize()
}

/// What the library shows for an Xbox game. Its art is the package's own
/// images; a cover from SteamGridDB can replace it as for any game.
fn xbox_game_info(g: &XboxGame, size: Option<u64>) -> GameInfo {
    let file = |p: &Option<PathBuf>| p.as_ref().map(|p| format!("file://{}", p.display()));
    GameInfo {
        runner: Some(Runner::Launchers),
        app_name: g.app_name(),
        title: g.title.clone(),
        publisher: g.publisher.clone(),
        description: g.description.clone(),
        store_url: g.store_web_url(),
        art_square: file(&g.art.square),
        art_background: file(&g.art.wide),
        art_icon: file(&g.art.icon),
        is_installed: true,
        install: Some(InstalledInfo {
            install_path: g.install_dir.display().to_string(),
            executable: g.executable.clone().unwrap_or_default(),
            install_size: size.map(util::format_bytes).unwrap_or_default(),
            install_size_bytes: size.unwrap_or_default(),
            version: g.version.clone().unwrap_or_default(),
            platform: InstallPlatform::Windows,
            ..Default::default()
        }),
        installable: false,
        third_party_managed_app: Some(xbox::DISPLAY_NAME.to_string()),
        platforms: vec![InstallPlatform::Windows],
        version: g.version.clone(),
        folder_name: xbox::game_folder(&g.install_dir)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned()),
        ..Default::default()
    }
    .finalize()
}

#[async_trait]
impl Store for LaunchersStore {
    fn runner(&self) -> Runner {
        Runner::Launchers
    }

    async fn init(&self) -> Result<()> {
        let path = self.state_file.clone();
        let file: StateFile =
            tokio::task::spawn_blocking(move || json_store::read_or_default(&path))
                .await
                .context("loading the launchers stopped")?;
        self.state.write().setups = file.setups;
        self.xbox_account.load().await;
        self.state.write().account = self.xbox_account.titles();
        self.rescan().await;
        Ok(())
    }

    fn user(&self) -> Option<UserInfo> {
        Some(UserInfo {
            account_id: "local".into(),
            display_name: "Local".into(),
        })
    }

    /// The Xbox account's sign-in (Microsoft's page); none without a
    /// Microsoft app to sign in with.
    fn login_flow(&self) -> LoginFlow {
        self.xbox_account.login_flow()
    }

    /// Sign in to Xbox with what the sign-in page gave, then read the
    /// account's titles.
    async fn login(&self, input: &str) -> Result<UserInfo> {
        let user = self.xbox_account.login(input).await?;
        self.expiry_told
            .store(false, std::sync::atomic::Ordering::Relaxed);
        self.bus.emit(Event::AccountChanged(Runner::Launchers));
        if let Err(e) = self.refresh_xbox(true).await {
            tracing::warn!("Xbox titles: {e:#}");
        }
        self.show_account_titles();
        Ok(user)
    }

    /// Sign out of Xbox; the account's games leave the library.
    async fn logout(&self) -> Result<()> {
        self.xbox_account.logout().await?;
        self.bus.emit(Event::AccountChanged(Runner::Launchers));
        self.show_account_titles();
        Ok(())
    }

    /// The Xbox account, when signed in.
    fn accounts(&self) -> Vec<StoreAccount> {
        let expired = self.xbox_account.expired();
        self.xbox_account
            .user()
            .map(|u| StoreAccount {
                slot: xbox_account::SLOT.into(),
                account_id: u.account_id,
                display_name: u.display_name,
                signed_in: true,
                games: self.xbox_account.titles().len(),
                expired,
                ..Default::default()
            })
            .into_iter()
            .collect()
    }

    /// Read the launchers' data again, then the Xbox account's titles. The
    /// local games show even when the Xbox network can't be reached; a
    /// refused Xbox sign-in is announced by the store itself.
    async fn refresh(&self, full: bool) -> Result<()> {
        self.rescan().await;
        if irori_core::net::working_offline() {
            return Ok(());
        }
        if let Err(e) = self.refresh_xbox(full).await {
            tracing::warn!("Xbox titles: {e:#}");
        }
        Ok(())
    }

    async fn achievements(&self, app_name: &str, language: &str) -> Result<Vec<Achievement>> {
        if !is_xbox_game(app_name) {
            return Ok(Vec::new());
        }
        self.xbox_account.achievements(app_name, language).await
    }

    async fn forget_achievements(&self, app_name: &str) {
        self.xbox_account.forget_achievements(app_name).await
    }

    /// Minutes played on the Xbox account, by app name.
    async fn remote_playtime(&self) -> Result<HashMap<String, u64>> {
        Ok(self.xbox_account.playtime())
    }

    /// When the Xbox account last played each game.
    async fn remote_last_played(&self) -> Result<HashMap<String, String>> {
        Ok(self.xbox_account.last_played())
    }

    fn games(&self) -> Vec<Arc<GameInfo>> {
        if !self.enabled() {
            return Vec::new();
        }
        let games = self.state.read().games.clone();
        debug_assert!(
            {
                let mut names: Vec<&str> = games.iter().map(|g| g.app_name.as_str()).collect();
                names.sort_unstable();
                names.windows(2).all(|w| w[0] != w[1])
            },
            "every game is listed once"
        );
        games
    }

    fn game(&self, app_name: &str) -> Option<Arc<GameInfo>> {
        self.games().into_iter().find(|g| g.app_name == app_name)
    }

    async fn extra_info(&self, _app_name: &str) -> Result<ExtraInfo> {
        Ok(ExtraInfo::default())
    }

    async fn install_info(
        &self,
        _app_name: &str,
        _platform: InstallPlatform,
        _query: &InstallInfoQuery,
    ) -> Result<InstallInfo> {
        bail!(MANAGED_BY_LAUNCHER)
    }

    async fn updatable_games(&self) -> Result<Vec<String>> {
        Ok(Vec::new())
    }

    async fn install(&self, _: &str, _: &InstallRequest, _: OpContext) -> Result<Outcome> {
        Ok(Outcome::Error(MANAGED_BY_LAUNCHER.into()))
    }

    async fn update(&self, _: &str, _: &UpdateRequest, _: OpContext) -> Result<Outcome> {
        Ok(Outcome::Error(MANAGED_BY_LAUNCHER.into()))
    }

    async fn repair(&self, _: &str, _: OpContext) -> Result<Outcome> {
        Ok(Outcome::Error(MANAGED_BY_LAUNCHER.into()))
    }

    async fn uninstall(&self, _: &str, _: RemoveArgs) -> Result<()> {
        bail!(MANAGED_BY_LAUNCHER)
    }

    async fn import(&self, _: &str, _: &Path, _: InstallPlatform) -> Result<()> {
        bail!(MANAGED_BY_LAUNCHER)
    }

    async fn set_install_path(&self, _: &str, _: &Path) -> Result<()> {
        bail!(MANAGED_BY_LAUNCHER)
    }

    async fn is_available(&self, app_name: &str) -> bool {
        self.game(app_name).is_some_and(|g| g.is_installed)
    }

    /// Xbox games: show the game's page in the Microsoft Store, where the
    /// Xbox app's install, update and remove commands are too. The other
    /// launchers keep their games to themselves.
    ///
    /// A game the Xbox account played that isn't installed: its Microsoft
    /// Store page installs it, so Install shows that too (Windows only).
    async fn client_action(&self, app_name: &str, action: ClientAction) -> Result<()> {
        if let Some(game) = self.xbox_game(app_name) {
            anyhow::ensure!(
                action == ClientAction::Show,
                "Install, update and remove Xbox games in the Xbox app"
            );
            process::spawn(&shell_open(&game.store_link()), None)?;
            return Ok(());
        }
        let link = self
            .xbox_account
            .store_link(app_name)
            .filter(|_| is_xbox_game(app_name))
            .context("This game's launcher has no page to show")?;
        anyhow::ensure!(
            matches!(action, ClientAction::Show | ClientAction::Install),
            "Install, update and remove Xbox games in the Xbox app"
        );
        anyhow::ensure!(
            cfg!(windows),
            "Xbox app games install and play only on Windows; the game's store page is in its menu"
        );
        process::spawn(&shell_open(&link), None)?;
        Ok(())
    }

    /// Start the game through its launcher. The session follows the game's
    /// own processes (see the services layer), not the launcher's.
    async fn launch(
        &self,
        app_name: &str,
        request: &LaunchRequest,
        _ctx: OpContext,
    ) -> Result<LaunchPlan> {
        if let Some(game) = self.xbox_game(app_name) {
            // Packaged games take no arguments from the shell.
            return Ok(LaunchPlan {
                command: shell_open(&game.launch_uri()),
                track_wineserver: false,
                watch: Some(Arc::new(XboxWatch {
                    folder: game.install_dir.display().to_string(),
                })),
            });
        }
        let game = self
            .state
            .read()
            .found
            .iter()
            .find(|g| g.app_name() == app_name)
            .cloned()
            .with_context(|| {
                if is_xbox_game(app_name) {
                    "Install this game in the Xbox app first"
                } else {
                    "This game is no longer listed by its launcher"
                }
            })?;
        let setup = self.state.read().setups.get(&game.launcher).cloned();
        let program = launcher_program(game.launcher, setup.as_ref())
            .filter(|p| p.exists())
            .with_context(|| {
                format!(
                    "{} isn't installed; set it up again on the Stores page",
                    game.launcher.display_name()
                )
            })?;
        let mut args = game.launch.launcher_args();
        args.extend(request.extra_args.iter().cloned());
        let mut command = self.command(game.launcher, setup.as_ref(), &program, args)?;
        self.apply_hud_with_paths(
            &mut command,
            Paths::get(),
            Some((app_name, &request.settings)),
        )
        .await?;
        Ok(LaunchPlan {
            command,
            track_wineserver: false,
            watch: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use irori_core::AppSettings;
    use irori_launchers_data::LaunchSpec;

    #[test]
    fn a_launchers_own_variables_lay_over_the_defaults() {
        let var = |k: &str, v: &str| EnvVar {
            key: k.into(),
            value: v.into(),
        };
        let defaults = GameSettings {
            environment_options: vec![var("A", "1"), var("B", "1")],
            ..Default::default()
        };
        let mut setup = Setup {
            prefix: "/p".into(),
            wine: WineInstallation {
                bin: "/w/bin/wine".into(),
                ..Default::default()
            },
            set_up_at: String::new(),
            settings: Default::default(),
        };
        let plain = game_settings(&defaults, &setup);
        assert_eq!(plain.environment_options, defaults.environment_options);
        setup.settings.insert(
            VARIABLES.into(),
            serde_json::to_value([var("B", "2"), var("C", "3")]).unwrap(),
        );
        let own = game_settings(&defaults, &setup);
        assert_eq!(own.wine_version.bin, "/w/bin/wine");
        assert_eq!(own.wine_prefix, "/p");
        let mut vars: Vec<_> = own
            .environment_options
            .iter()
            .map(|v| (v.key.as_str(), v.value.as_str()))
            .collect();
        vars.sort();
        assert_eq!(vars, [("A", "1"), ("B", "2"), ("C", "3")]);
    }

    fn store(dir: &Path, enabled: bool) -> Arc<LaunchersStore> {
        let app = Arc::new(AppSettings {
            third_party_launchers: enabled,
            default_wine_prefix_dir: dir.join("prefixes").display().to_string(),
            ..Default::default()
        });
        LaunchersStore::with_dirs(
            EventBus::new(),
            Arc::new(move || app.clone()),
            dir.join("config"),
            dir.join("cache"),
        )
    }

    #[test]
    fn games_show_their_launcher_and_folder() {
        let g = InstalledGame {
            launcher: Launcher::BattleNet,
            id: "fenris".into(),
            title: "Diablo IV".into(),
            install_dir: PathBuf::from("C:\\Program Files (x86)\\Diablo IV"),
            executable: None,
            launch: LaunchSpec::LauncherArgs(vec!["--exec=launch Fen".into()]),
        };
        let info = game_info(&g);
        assert_eq!(info.app_name, "battlenet:fenris");
        assert_eq!(info.third_party_managed_app.as_deref(), Some("Battle.net"));
        assert_eq!(info.folder_name.as_deref(), Some("Diablo IV"));
        assert_eq!(info.runner(), Runner::Launchers);
        assert!(!info.installable);
    }

    #[tokio::test]
    async fn lists_nothing_until_turned_on_and_set_up() {
        let dir = tempfile::tempdir().unwrap();
        let off = store(dir.path(), false);
        off.init().await.unwrap();
        assert!(off.games().is_empty());
        let on = store(dir.path(), true);
        on.init().await.unwrap();
        assert!(on.games().is_empty());
        let statuses = on.statuses();
        assert_eq!(statuses.len(), 4);
        assert!(statuses.iter().all(|s| s.setup.is_none() && s.games == 0));
        assert!(
            on.default_prefix(Launcher::EaApp)
                .ends_with("prefixes/EA app")
        );
        assert!(
            on.launch(
                "battlenet:x",
                &dummy_request(dir.path()),
                OpContext::detached()
            )
            .await
            .is_err()
        );
    }

    #[test]
    fn rockstar_titles_get_steam_art() {
        let g = InstalledGame {
            launcher: Launcher::Rockstar,
            id: "rdr2".into(),
            title: "Red Dead Redemption 2".into(),
            install_dir: PathBuf::from(r"C:\Program Files\Rockstar Games\Red Dead Redemption 2"),
            executable: Some("RDR2.exe".into()),
            launch: LaunchSpec::LauncherArgs(Vec::new()),
        };
        let info = game_info(&g);
        assert_eq!(
            info.art_cover.as_deref(),
            Some(
                "https://cdn.cloudflare.steamstatic.com/steam/apps/1174180/library_600x900_2x.jpg"
            )
        );
        assert!(
            info.art_background
                .unwrap()
                .ends_with("/1174180/library_hero.jpg")
        );
        assert!(info.art_logo.unwrap().ends_with("/1174180/logo.png"));
        assert_eq!(info.developer.as_deref(), Some("Rockstar Games"));

        let unknown = game_info(&InstalledGame {
            id: "some-new-game".into(),
            ..g
        });
        assert_eq!(unknown.art_cover, None);
        assert_eq!(unknown.developer, None);
    }

    #[cfg(not(windows))]
    #[tokio::test]
    async fn adopts_a_prefix_that_has_the_launcher() {
        let dir = tempfile::tempdir().unwrap();
        let store = store(dir.path(), true);
        store.init().await.unwrap();
        let wine = WineInstallation {
            bin: "/usr/bin/wine".into(),
            name: "Wine".into(),
            ..Default::default()
        };

        // A Proton prefix, chosen by its `pfx` folder.
        let prefix = dir.path().join("old/Rockstar");
        let drive = prefix.join("pfx/drive_c");
        std::fs::create_dir_all(&drive).unwrap();
        let err = store
            .adopt(Launcher::Rockstar, prefix.join("pfx"), Some(wine.clone()))
            .await
            .unwrap_err();
        assert!(format!("{err:#}").contains("isn't installed in"), "{err:#}");
        assert!(store.statuses()[3].setup.is_none());

        let program = drive.join(Launcher::Rockstar.exe_path());
        std::fs::create_dir_all(program.parent().unwrap()).unwrap();
        std::fs::write(&program, b"MZ").unwrap();
        let game = drive.join("Program Files/Rockstar Games/Grand Theft Auto V");
        std::fs::create_dir_all(&game).unwrap();
        std::fs::write(
            prefix.join("pfx/system.reg"),
            "WINE REGISTRY Version 2\n\n[Software\\\\Wow6432Node\\\\Rockstar Games\\\\Grand Theft Auto V] 1\n\
             \"InstallFolder\"=\"C:\\\\Program Files\\\\Rockstar Games\\\\Grand Theft Auto V\"\n",
        )
        .unwrap();
        let profile = drive.join("users/steamuser/Documents/Rockstar Games/Launcher");
        std::fs::create_dir_all(&profile).unwrap();
        std::fs::write(
            profile.join("launcher.log"),
            "[t] [DISPLAY] [Main ] [launcher] Presence Event - Signed In\n",
        )
        .unwrap();

        assert!(
            store
                .adopt(Launcher::Rockstar, prefix.join("pfx"), None)
                .await
                .is_err(),
            "a Wine build is needed"
        );
        store
            .adopt(Launcher::Rockstar, prefix.join("pfx"), Some(wine))
            .await
            .unwrap();
        let status = store.statuses()[3].clone();
        assert_eq!(status.launcher, Launcher::Rockstar);
        assert_eq!(status.setup.as_ref().unwrap().prefix, prefix);
        assert!(status.installed);
        assert_eq!(status.games, 1);
        assert_eq!(status.sign_in, SignIn::SignedIn);
        assert_eq!(
            store.games()[0].title,
            "Grand Theft Auto V Legacy",
            "{:?}",
            store.warnings()
        );

        // Removing it from Irori leaves the prefix alone.
        store.forget(Launcher::Rockstar).await.unwrap();
        assert!(store.games().is_empty());
        assert!(program.is_file() && game.is_dir());
    }

    #[test]
    fn proton_runs_through_umu_when_there_is_one() {
        let setup = Setup {
            prefix: PathBuf::from("/prefixes/Battle.net"),
            wine: WineInstallation {
                bin: "/proton/GE-Proton10-25/proton".into(),
                name: "GE-Proton10-25".into(),
                kind: WineKind::Proton,
                ..Default::default()
            },
            set_up_at: String::new(),
            settings: Default::default(),
        };
        let wc = wine_context(&setup, Some(PathBuf::from("/umu/umu-run")));
        assert!(wc.uses_umu);
        assert_eq!(wc.runner_bin(), Path::new("/umu/umu-run"));
        let direct = wine_context(&setup, None);
        assert!(!direct.uses_umu);
        assert_eq!(
            direct.runner_bin(),
            Path::new("/proton/GE-Proton10-25/proton")
        );
        // Both find the launcher where Proton keeps the Windows files.
        assert_eq!(windows_root(&setup), Path::new("/prefixes/Battle.net/pfx"));
    }

    #[tokio::test]
    async fn finds_launchers_set_up_with_proton_in_pfx() {
        let dir = tempfile::tempdir().unwrap();
        let prefix = dir.path().join("prefixes/Battle.net");
        let exe = Launcher::BattleNet.exe_in_prefix(&prefix.join("pfx"));
        std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
        std::fs::write(&exe, b"").unwrap();
        let file = StateFile {
            setups: BTreeMap::from([(
                Launcher::BattleNet,
                Setup {
                    prefix,
                    wine: WineInstallation {
                        bin: "/proton".into(),
                        kind: WineKind::Proton,
                        ..Default::default()
                    },
                    set_up_at: String::new(),
                    settings: Default::default(),
                },
            )]),
        };
        json_store::write(&dir.path().join("config/launchers.json"), &file).unwrap();
        let store = store(dir.path(), true);
        store.init().await.unwrap();
        let status = store
            .statuses()
            .into_iter()
            .find(|s| s.launcher == Launcher::BattleNet)
            .unwrap();
        assert_eq!(status.installed, !cfg!(windows));
    }

    /// A Wine that makes the prefix, then acts like an installer which starts
    /// the launcher it installed and keeps running.
    #[cfg(unix)]
    fn fake_wine(dir: &Path, installs: bool) -> WineInstallation {
        use std::os::unix::fs::PermissionsExt;
        let script = |name: &str, body: &str| {
            let path = dir.join(name);
            std::fs::write(&path, format!("#!/bin/sh\n{body}")).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
            path.display().to_string()
        };
        let install = if installs {
            r#"mkdir -p "$WINEPREFIX/drive_c/Program Files (x86)/Battle.net"
touch "$WINEPREFIX/drive_c/Program Files (x86)/Battle.net/Battle.net.exe"
echo $$ >> "$WINEPREFIX/../running"
exec sleep 30"#
        } else {
            "echo setup failed >&2\nexit 3"
        };
        WineInstallation {
            bin: script(
                "wine",
                &format!(
                    r#"if [ "$1" = wineboot ]; then
  mkdir -p "$WINEPREFIX/drive_c" "$WINEPREFIX/dosdevices"
  touch "$WINEPREFIX/system.reg" "$WINEPREFIX/user.reg" "$WINEPREFIX/userdef.reg"
  exit 0
fi
{install}
"#
                ),
            ),
            name: "Fake Wine".into(),
            kind: WineKind::Wine,
            wineserver: Some(script("wineserver", "exit 0")),
            ..Default::default()
        }
    }

    #[cfg(unix)]
    fn setup_with(dir: &Path, installs: bool) -> (Arc<LaunchersStore>, Setup, PathBuf) {
        let store = store(dir, true);
        let setup = Setup {
            prefix: store.default_prefix(Launcher::BattleNet),
            wine: fake_wine(dir, installs),
            set_up_at: String::new(),
            settings: Default::default(),
        };
        let installer = dir.join("setup.exe");
        std::fs::write(&installer, b"").unwrap();
        (store, setup, installer)
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn setup_ends_when_the_launcher_is_installed_though_it_keeps_running() {
        let dir = tempfile::tempdir().unwrap();
        let (store, setup, installer) = setup_with(dir.path(), true);
        let done = tokio::time::timeout(
            Duration::from_secs(20),
            store.run_installer(
                Launcher::BattleNet,
                &setup,
                &installer,
                &OpContext::detached(),
            ),
        )
        .await;
        let running = std::fs::read_to_string(setup.prefix.parent().unwrap().join("running"));
        for pid in running.iter().flat_map(|s| s.lines()) {
            let _ = std::process::Command::new("kill").arg(pid).status();
        }
        done.expect("setup waited for the launcher to exit")
            .unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn setup_fails_when_the_installer_exits_without_installing() {
        let dir = tempfile::tempdir().unwrap();
        let (store, setup, installer) = setup_with(dir.path(), false);
        let e = store
            .run_installer(
                Launcher::BattleNet,
                &setup,
                &installer,
                &OpContext::detached(),
            )
            .await
            .unwrap_err();
        assert!(format!("{e:#}").contains("closed before Battle.net was installed"));
        let log = std::fs::read_to_string(dir.path().join("cache/logs/battlenet-setup.log"));
        assert!(log.unwrap().contains("setup failed"));
    }

    fn sky_racer(dir: &Path) -> XboxGame {
        XboxGame {
            family_name: "Studio.SkyRacer_abcdefghjkmnp".into(),
            title: "Sky Racer".into(),
            publisher: Some("Example Studio".into()),
            description: None,
            version: Some("1.6.0.0".into()),
            app_id: "Game".into(),
            executable: Some("SkyRacer.exe".into()),
            install_dir: dir.join("XboxGames/Sky Racer/Content"),
            store_id: Some("9NZTKMP36L8J".into()),
            art: xbox::PackageArt {
                square: Some(dir.join("XboxGames/Sky Racer/Content/Logo480.png")),
                wide: None,
                icon: None,
            },
        }
    }

    #[test]
    fn xbox_games_show_their_package() {
        let dir = Path::new("/d");
        let info = xbox_game_info(&sky_racer(dir), Some(3 * 1024 * 1024 * 1024));
        assert_eq!(info.app_name, "xbox:Studio.SkyRacer_abcdefghjkmnp");
        assert_eq!(info.runner(), Runner::Launchers);
        assert_eq!(info.third_party_managed_app.as_deref(), Some("Xbox app"));
        assert_eq!(info.folder_name.as_deref(), Some("Sky Racer"));
        let logo = format!(
            "file://{}",
            dir.join("XboxGames/Sky Racer/Content/Logo480.png")
                .display()
        );
        assert_eq!(info.cover_art(), Some(logo.as_str()));
        assert_eq!(
            info.store_url.as_deref(),
            Some("https://apps.microsoft.com/detail/9NZTKMP36L8J")
        );
        let install = info.install.as_ref().unwrap();
        assert_eq!(install.install_size_bytes, 3 * 1024 * 1024 * 1024);
        assert!(!install.install_size.is_empty());
        assert_eq!(install.version, "1.6.0.0");
        assert!(!info.installable);
        assert!(is_xbox_game(&info.app_name));
        assert!(!is_xbox_game("xboxish:x") && !is_xbox_game("battlenet:wow"));
    }

    #[tokio::test]
    async fn xbox_games_start_through_the_shell_and_are_followed() {
        let dir = tempfile::tempdir().unwrap();
        let launchers = store(dir.path(), true);
        launchers.init().await.unwrap();
        let status = launchers.xbox_status();
        assert_eq!(status.supported, cfg!(windows));
        assert_eq!(status.games, 0);
        {
            let mut s = launchers.state.write();
            s.xbox = vec![sky_racer(dir.path())];
            s.games = library(&s);
        }
        assert_eq!(launchers.games().len(), 1);
        assert_eq!(launchers.xbox_status().games, 1);

        let app = "xbox:Studio.SkyRacer_abcdefghjkmnp";
        let plan = launchers
            .launch(app, &dummy_request(dir.path()), OpContext::detached())
            .await
            .unwrap();
        assert_eq!(plan.command.program, Path::new("rundll32.exe"));
        assert_eq!(
            plan.command.args,
            [
                "shell32.dll,ShellExec_RunDLL",
                r"shell:AppsFolder\Studio.SkyRacer_abcdefghjkmnp!Game"
            ]
        );
        let watch = plan
            .watch
            .expect("the session follows the game's processes");
        assert!(!watch.running(), "nothing runs from the game's folder");

        let refused = launchers
            .client_action(app, ClientAction::Uninstall)
            .await
            .unwrap_err();
        assert!(format!("{refused:#}").contains("Xbox app"));
        assert!(
            launchers
                .client_action("battlenet:wow", ClientAction::Show)
                .await
                .is_err()
        );

        // Turning the launchers off hides Xbox games too.
        let off = store(dir.path(), false);
        off.state.write().xbox = vec![sky_racer(dir.path())];
        assert!(off.games().is_empty());
    }

    fn dummy_request(dir: &Path) -> LaunchRequest {
        LaunchRequest {
            settings: GameSettings::default(),
            environment: Default::default(),
            option: None,
            extra_args: Vec::new(),
            offline: false,
            skip_version_check: false,
            log_file: dir.join("log"),
        }
    }

    #[tokio::test]
    async fn the_xbox_account_adds_art_and_played_games() {
        let dir = tempfile::tempdir().unwrap();
        let launchers = store(dir.path(), true);
        launchers.init().await.unwrap();
        // Signed out: no account entry, nothing more listed.
        assert!(launchers.accounts().is_empty());
        let title = |pfn: &str, name: &str| xbox_account::AccountTitle {
            title_id: "17".into(),
            pfn: pfn.into(),
            name: name.into(),
            cover: Some("https://img/poster".into()),
            background: Some("https://img/hero".into()),
            developer: Some("Studio".into()),
            product_id: Some("9NFLY".into()),
            ..Default::default()
        };
        {
            let mut s = launchers.state.write();
            s.xbox = vec![sky_racer(dir.path())];
            s.account = vec![
                // Matched by family name, whatever its letter case.
                title("studio.skyracer_abcdefghjkmnp", "Sky Racer"),
                title("Studio.Glider_abcdefghjkmnp", "Glider"),
                // Listed twice: still one game, one id.
                title("Studio.Glider_abcdefghjkmnp", "Glider (old)"),
            ];
            s.games = library(&s);
        }
        let games = launchers.games();
        assert_eq!(games.len(), 2);
        let installed = launchers
            .game("xbox:Studio.SkyRacer_abcdefghjkmnp")
            .unwrap();
        assert!(installed.is_installed);
        assert_eq!(installed.art_cover.as_deref(), Some("https://img/poster"));
        assert_eq!(
            installed.art_background.as_deref(),
            Some("https://img/hero")
        );
        // The package's own details stay.
        assert_eq!(installed.publisher.as_deref(), Some("Example Studio"));
        assert_eq!(installed.developer.as_deref(), Some("Studio"));

        let played = launchers.game("xbox:Studio.Glider_abcdefghjkmnp").unwrap();
        assert!(!played.is_installed && !played.installable);
        assert_eq!(played.third_party_managed_app.as_deref(), Some("Xbox app"));
        assert_eq!(
            played.store_url.as_deref(),
            Some("https://apps.microsoft.com/detail/9NFLY")
        );
        assert!(!launchers.is_available(&played.app_name).await);
        let refused = launchers
            .launch(
                &played.app_name,
                &dummy_request(dir.path()),
                OpContext::detached(),
            )
            .await
            .unwrap_err();
        assert!(format!("{refused:#}").contains("Xbox app"));
        // Without an account to know the title, there is no page to show.
        // (The account is set in the state only.)
        assert!(
            launchers
                .client_action(&played.app_name, ClientAction::Show)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn signing_in_to_xbox_lists_the_accounts_games_and_playtime() {
        let dir = tempfile::tempdir().unwrap();
        let server = xbox_account::tests::server(true);
        let app = Arc::new(AppSettings {
            third_party_launchers: true,
            ..Default::default()
        });
        let launchers = LaunchersStore::with_xbox_endpoints(
            EventBus::new(),
            Arc::new(move || app.clone()),
            dir.path().join("config"),
            dir.path().join("cache"),
            irori_xbox_api::Endpoints::all_at(&server.base()),
        );
        launchers.init().await.unwrap();
        assert!(launchers.xbox_account_status().available);
        let LoginFlow::Browser { capture, .. } = launchers.login_flow() else {
            panic!("the Xbox sign-in");
        };
        assert!(capture.contains("oauth20_desktop"));
        let user = launchers
            .login("https://login.live.com/oauth20_desktop.srf?code=M.C1.code")
            .await
            .unwrap();
        assert_eq!(user.display_name, "Pilot");
        // The store's own login stays local; the account is listed beside it.
        assert!(launchers.is_logged_in());
        let accounts = launchers.accounts();
        assert_eq!(accounts.len(), 1);
        assert_eq!((accounts[0].slot.as_str(), accounts[0].games), ("xbox", 1));

        let app_name = format!("xbox:{}", xbox_account::tests::PFN);
        let game = launchers.game(&app_name).expect("the played game");
        assert!(!game.is_installed);
        assert_eq!(
            launchers.remote_playtime().await.unwrap(),
            HashMap::from([(app_name.clone(), 754)])
        );
        assert!(
            launchers
                .remote_last_played()
                .await
                .unwrap()
                .contains_key(&app_name)
        );
        assert_eq!(
            launchers.achievements(&app_name, "en").await.unwrap().len(),
            1
        );

        launchers.logout().await.unwrap();
        assert!(launchers.game(&app_name).is_none());
        assert!(launchers.accounts().is_empty());
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn apply_hud_respects_global_custom_disabled_and_off_modes() {
        use irori_core::{GameHudMode, GameHudSettings, GlobalHudMode, GlobalHudSettings};

        let dir = tempfile::tempdir().unwrap();
        let make_store = |mode: GlobalHudMode| {
            let mut app = AppSettings {
                third_party_launchers: true,
                default_wine_prefix_dir: dir.path().join("prefixes").display().to_string(),
                ..Default::default()
            };
            app.hud = GlobalHudSettings {
                mode,
                ..Default::default()
            };
            let app = Arc::new(app);
            LaunchersStore::with_dirs(
                EventBus::new(),
                Arc::new(move || app.clone()),
                dir.path().join("config"),
                dir.path().join("cache"),
            )
        };
        let setup = Setup {
            prefix: dir.path().join("prefixes/Battle.net"),
            wine: fake_wine(dir.path(), false),
            set_up_at: String::new(),
            settings: Default::default(),
        };
        let exe = Launcher::BattleNet.exe_in_prefix(&windows_root(&setup));
        std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
        std::fs::write(&exe, b"").unwrap();
        let paths = Paths::under(dir.path());

        // Global Off + InheritGlobal injects nothing and writes no config file.
        let off_store = make_store(GlobalHudMode::Off);
        let mut off_spec = off_store
            .command(Launcher::BattleNet, Some(&setup), &exe, Vec::new())
            .unwrap();
        off_store
            .apply_hud_with_paths(
                &mut off_spec,
                &paths,
                Some(("battlenet:fenris", &GameSettings::default())),
            )
            .await
            .unwrap();
        assert!(!off_spec.env.contains_key("MANGOHUD"));
        assert!(!off_spec.env.contains_key("DISABLE_MANGOHUD"));
        assert!(!off_spec.env.contains_key("MANGOHUD_CONFIGFILE"));
        assert!(!paths.mangohud_config_file("global").exists());

        // Global On + InheritGlobal (and direct `open` with None) uses global.conf.
        let on_store = make_store(GlobalHudMode::IroriProfile);
        let mut open_spec = on_store
            .command(Launcher::BattleNet, Some(&setup), &exe, Vec::new())
            .unwrap();
        on_store
            .apply_hud_with_paths(&mut open_spec, &paths, None)
            .await
            .unwrap();
        let expected_global = paths.mangohud_config_file("global");
        assert!(expected_global.is_file());
        assert_eq!(open_spec.env.get("MANGOHUD").map(String::as_str), Some("1"));
        assert_eq!(
            open_spec.env.get("MANGOHUD_CONFIGFILE").map(PathBuf::from),
            Some(expected_global)
        );
        assert!(open_spec.env_remove.iter().any(|k| k == "MANGOHUD_CONFIG"));
        assert!(open_spec.env_remove.iter().any(|k| k == "DISABLE_MANGOHUD"));

        // Per-game Custom writes sanitized launchers_battlenet_fenris.conf.
        let mut custom_spec = on_store
            .command(Launcher::BattleNet, Some(&setup), &exe, Vec::new())
            .unwrap();
        let custom_settings = GameSettings {
            game_hud: GameHudSettings {
                mode: GameHudMode::Custom,
                ..Default::default()
            },
            ..Default::default()
        };
        on_store
            .apply_hud_with_paths(
                &mut custom_spec,
                &paths,
                Some(("battlenet:fenris", &custom_settings)),
            )
            .await
            .unwrap();
        let expected_custom = paths.mangohud_config_file("launchers_battlenet:fenris");
        assert_eq!(
            expected_custom.file_name().and_then(|n| n.to_str()),
            Some("launchers_battlenet_fenris.conf")
        );
        assert!(expected_custom.is_file());
        assert_eq!(
            custom_spec
                .env
                .get("MANGOHUD_CONFIGFILE")
                .map(PathBuf::from),
            Some(expected_custom)
        );

        // Per-game Disabled forces MangoHud off even when global HUD is enabled.
        let mut disabled_spec = on_store
            .command(Launcher::BattleNet, Some(&setup), &exe, Vec::new())
            .unwrap();
        let disabled_settings = GameSettings {
            game_hud: GameHudSettings {
                mode: GameHudMode::Disabled,
                ..Default::default()
            },
            ..Default::default()
        };
        on_store
            .apply_hud_with_paths(
                &mut disabled_spec,
                &paths,
                Some(("battlenet:fenris", &disabled_settings)),
            )
            .await
            .unwrap();
        assert_eq!(
            disabled_spec
                .env
                .get("DISABLE_MANGOHUD")
                .map(String::as_str),
            Some("1")
        );
        assert_eq!(
            disabled_spec.env.get("MANGOHUD").map(String::as_str),
            Some("0")
        );
        assert!(!disabled_spec.env.contains_key("MANGOHUD_CONFIGFILE"));
        assert!(
            disabled_spec
                .env_remove
                .iter()
                .any(|k| k == "MANGOHUD_CONFIGFILE")
        );
        assert!(
            disabled_spec
                .env_remove
                .iter()
                .any(|k| k == "MANGOHUD_CONFIG")
        );
    }

    #[cfg(unix)]
    #[test]
    fn battlenet_command_adds_gpu_flag_and_locationapi_override() {
        let dir = tempfile::tempdir().unwrap();
        let store = store(dir.path(), true);
        let setup = Setup {
            prefix: dir.path().join("prefixes/Battle.net"),
            wine: fake_wine(dir.path(), false),
            set_up_at: String::new(),
            settings: Default::default(),
        };
        let exe = Launcher::BattleNet.exe_in_prefix(&windows_root(&setup));
        std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
        std::fs::write(&exe, b"").unwrap();

        let spec = store
            .command(
                Launcher::BattleNet,
                Some(&setup),
                &exe,
                vec!["--exec=launch Pro".into()],
            )
            .unwrap();
        let args: Vec<String> = spec
            .args
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            args.iter()
                .filter(|a| a.as_str() == "--in-process-gpu")
                .count(),
            1,
            "expected --in-process-gpu once: {args:?}"
        );
        let overrides = spec.env.get("WINEDLLOVERRIDES").expect("WINEDLLOVERRIDES");
        assert!(
            overrides.split(';').any(|entry| entry == "locationapi=d"),
            "expected locationapi=d in {overrides}"
        );

        // Passing --in-process-gpu explicitly must not duplicate it.
        let dedup_spec = store
            .command(
                Launcher::BattleNet,
                Some(&setup),
                &exe,
                vec!["--in-process-gpu".into(), "--exec=launch Pro".into()],
            )
            .unwrap();
        assert_eq!(
            dedup_spec
                .args
                .iter()
                .filter(|a| a.to_string_lossy() == "--in-process-gpu")
                .count(),
            1
        );
    }

    #[cfg(unix)]
    #[test]
    fn battlenet_command_merges_custom_winedlloverrides() {
        let dir = tempfile::tempdir().unwrap();
        let store = store(dir.path(), true);
        let mut proton = fake_wine(dir.path(), false);
        proton.kind = WineKind::Proton;
        let mut settings = serde_json::Map::new();
        settings.insert("disableUMU".into(), serde_json::json!(true));
        settings.insert(
            VARIABLES.into(),
            serde_json::json!([
                { "key": "WINEDLLOVERRIDES", "value": "d3d11=n,b" }
            ]),
        );
        let setup = Setup {
            prefix: dir.path().join("prefixes/Battle.net"),
            wine: proton.clone(),
            set_up_at: String::new(),
            settings,
        };
        let exe = Launcher::BattleNet.exe_in_prefix(&windows_root(&setup));
        std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
        std::fs::write(&exe, b"").unwrap();

        let spec = store
            .command(Launcher::BattleNet, Some(&setup), &exe, Vec::new())
            .unwrap();
        assert_eq!(
            spec.env.get("WINEDLLOVERRIDES").map(String::as_str),
            Some("d3d11=n,b;locationapi=d")
        );

        // Existing locationapi=d is not duplicated.
        let mut dup_settings = serde_json::Map::new();
        dup_settings.insert("disableUMU".into(), serde_json::json!(true));
        dup_settings.insert(
            VARIABLES.into(),
            serde_json::json!([
                { "key": "WINEDLLOVERRIDES", "value": "d3d11=n,b;locationapi=d" }
            ]),
        );
        let dup_setup = Setup {
            prefix: dir.path().join("prefixes/Battle.net"),
            wine: proton,
            set_up_at: String::new(),
            settings: dup_settings,
        };
        let dup_spec = store
            .command(Launcher::BattleNet, Some(&dup_setup), &exe, Vec::new())
            .unwrap();
        assert_eq!(
            dup_spec.env.get("WINEDLLOVERRIDES").map(String::as_str),
            Some("d3d11=n,b;locationapi=d")
        );

        // Plain Wine keeps its Linux winemenubuilder.exe=d override alongside
        // custom entries and locationapi=d (macOS Wine does not add
        // winemenubuilder.exe=d).
        let mut wine_settings = serde_json::Map::new();
        wine_settings.insert(
            VARIABLES.into(),
            serde_json::json!([
                { "key": "WINEDLLOVERRIDES", "value": "d3d11=n,b" }
            ]),
        );
        let wine_setup = Setup {
            prefix: dir.path().join("prefixes/Battle.net-wine"),
            wine: fake_wine(dir.path(), false),
            set_up_at: String::new(),
            settings: wine_settings,
        };
        let wine_exe = Launcher::BattleNet.exe_in_prefix(&windows_root(&wine_setup));
        std::fs::create_dir_all(wine_exe.parent().unwrap()).unwrap();
        std::fs::write(&wine_exe, b"").unwrap();
        let wine_spec = store
            .command(
                Launcher::BattleNet,
                Some(&wine_setup),
                &wine_exe,
                Vec::new(),
            )
            .unwrap();
        let expected_wine = if cfg!(target_os = "linux") {
            "d3d11=n,b;winemenubuilder.exe=d;locationapi=d"
        } else {
            "d3d11=n,b;locationapi=d"
        };
        assert_eq!(
            wine_spec.env.get("WINEDLLOVERRIDES").map(String::as_str),
            Some(expected_wine)
        );

        // Lowercase variable name and `.dll` suffix are preserved/deduplicated.
        let mut lower_settings = serde_json::Map::new();
        lower_settings.insert("disableUMU".into(), serde_json::json!(true));
        lower_settings.insert(
            VARIABLES.into(),
            serde_json::json!([
                { "key": "winedlloverrides", "value": "d3d11=n,b;LocationAPI.dll=n" }
            ]),
        );
        let lower_setup = Setup {
            prefix: dir.path().join("prefixes/Battle.net"),
            wine: WineInstallation {
                kind: WineKind::Proton,
                ..fake_wine(dir.path(), false)
            },
            set_up_at: String::new(),
            settings: lower_settings,
        };
        let lower_spec = store
            .command(Launcher::BattleNet, Some(&lower_setup), &exe, Vec::new())
            .unwrap();
        assert_eq!(
            lower_spec.env.get("winedlloverrides").map(String::as_str),
            Some("d3d11=n,b;locationapi=d")
        );
        assert!(!lower_spec.env.contains_key("WINEDLLOVERRIDES"));
    }

    #[cfg(unix)]
    #[test]
    fn other_launchers_do_not_receive_battlenet_flags_or_overrides() {
        let dir = tempfile::tempdir().unwrap();
        let store = store(dir.path(), true);
        let wine = fake_wine(dir.path(), false);
        for launcher in [Launcher::Ubisoft, Launcher::EaApp, Launcher::Rockstar] {
            let setup = Setup {
                prefix: dir.path().join(format!("prefixes/{}", launcher.id())),
                wine: wine.clone(),
                set_up_at: String::new(),
                settings: Default::default(),
            };
            let exe = launcher.exe_in_prefix(&windows_root(&setup));
            std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
            std::fs::write(&exe, b"").unwrap();

            let spec = store
                .command(launcher, Some(&setup), &exe, Vec::new())
                .unwrap();
            assert!(
                !spec
                    .args
                    .iter()
                    .any(|a| a.to_string_lossy() == "--in-process-gpu"),
                "{launcher:?} unexpectedly received --in-process-gpu"
            );
            let overrides = spec
                .env
                .get("WINEDLLOVERRIDES")
                .map(String::as_str)
                .unwrap_or_default();
            assert!(
                !overrides.to_ascii_lowercase().contains("locationapi"),
                "{launcher:?} unexpectedly received locationapi override: {overrides}"
            );
        }
    }

    #[test]
    fn installer_argv_sets_installpath_only_for_battlenet() {
        let installer = Path::new("/tmp/setup.exe");
        assert_eq!(
            installer_argv(Launcher::BattleNet, installer),
            vec![
                "/tmp/setup.exe".to_string(),
                r"--installpath=C:\Program Files (x86)\Battle.net".to_string(),
            ]
        );
        for other in [Launcher::Ubisoft, Launcher::EaApp, Launcher::Rockstar] {
            assert_eq!(
                installer_argv(other, installer),
                vec!["/tmp/setup.exe".to_string()]
            );
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn battlenet_run_installer_passes_installpath_argument() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let store = store(dir.path(), true);
        let args_log = dir.path().join("installer-args.txt");
        let wine_path = dir.path().join("wine");
        let wineserver_path = dir.path().join("wineserver");
        std::fs::write(
            &wine_path,
            format!(
                r#"#!/bin/sh
if [ "$1" = wineboot ]; then
  mkdir -p "$WINEPREFIX/drive_c/users/steamuser" "$WINEPREFIX/dosdevices"
  touch "$WINEPREFIX/system.reg" "$WINEPREFIX/user.reg" "$WINEPREFIX/userdef.reg"
  exit 0
fi
printf '%s\n' "$@" > "{}"
mkdir -p "$WINEPREFIX/drive_c/Program Files (x86)/Battle.net"
touch "$WINEPREFIX/drive_c/Program Files (x86)/Battle.net/Battle.net.exe"
exit 0
"#,
                args_log.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&wine_path, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::write(&wineserver_path, "#!/bin/sh\nexit 0\n").unwrap();
        std::fs::set_permissions(&wineserver_path, std::fs::Permissions::from_mode(0o755)).unwrap();

        let setup = Setup {
            prefix: store.default_prefix(Launcher::BattleNet),
            wine: WineInstallation {
                bin: wine_path.display().to_string(),
                name: "Fake Wine".into(),
                kind: WineKind::Wine,
                wineserver: Some(wineserver_path.display().to_string()),
                ..Default::default()
            },
            set_up_at: String::new(),
            settings: Default::default(),
        };
        let installer = dir.path().join("battlenet-setup.exe");
        std::fs::write(&installer, b"").unwrap();

        store
            .run_installer(
                Launcher::BattleNet,
                &setup,
                &installer,
                &OpContext::detached(),
            )
            .await
            .unwrap();

        let recorded = std::fs::read_to_string(&args_log).unwrap();
        let lines: Vec<&str> = recorded.lines().collect();
        assert!(
            lines.contains(&r"--installpath=C:\Program Files (x86)\Battle.net"),
            "expected --installpath in recorded args: {lines:?}"
        );

        let config_path = setup
            .prefix
            .join("drive_c/users/steamuser/AppData/Roaming/Battle.net/Battle.net.config");
        let cfg: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&config_path).unwrap()).unwrap();
        assert_eq!(cfg["Client"]["HardwareAcceleration"], "true");
        assert_eq!(cfg["Client"]["GameLaunchWindowBehavior"], "2");
        assert_eq!(cfg["Client"]["DefaultStartupScreen"], "1");
        assert_eq!(cfg["Client"]["SingleInstance"], "true");
        assert_eq!(cfg["Client"]["GameSearch"]["BackgroundSearch"], "true");
        assert_eq!(cfg["Client"]["Install"]["CreateDesktopShortcut"], "false");
    }
}
