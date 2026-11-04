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
    ClientAction, Event, EventBus, ExtraInfo, GameInfo, GameSettings, GameWatch, InstallInfo,
    InstallInfoQuery, InstallPlatform, InstallRequest, InstalledInfo, LaunchPlan, LaunchRequest,
    LoginFlow, OpContext, Outcome, Paths, RemoveArgs, Runner, SettingsFn, Store, UpdateRequest,
    UserInfo, WineInstallation, WineKind, json_store, util,
};
use irori_launchers_data::xbox::{self, XboxGame};
use irori_launchers_data::{InstalledGame, Launcher, Registry, ScanReport, SignIn, rockstar};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};

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
    me: Weak<Self>,
}

impl LaunchersStore {
    pub fn new(bus: EventBus, settings: SettingsFn) -> Arc<Self> {
        let paths = Paths::get();
        Self::build(
            bus,
            settings,
            paths.store_config_dir(Runner::Launchers),
            paths.store_cache_dir(Runner::Launchers),
            paths.logs_dir(),
        )
    }

    /// A store keeping its files in the given folders (tests).
    pub fn with_dirs(
        bus: EventBus,
        settings: SettingsFn,
        config_dir: PathBuf,
        cache_dir: PathBuf,
    ) -> Arc<Self> {
        let logs = cache_dir.join("logs");
        Self::build(bus, settings, config_dir, cache_dir, logs)
    }

    fn build(
        bus: EventBus,
        settings: SettingsFn,
        config_dir: PathBuf,
        cache_dir: PathBuf,
        logs: PathBuf,
    ) -> Arc<Self> {
        Arc::new_cyclic(|me| Self {
            bus,
            settings,
            state_file: config_dir.join("launchers.json"),
            downloads: cache_dir.join("installers"),
            logs,
            state: RwLock::new(State::default()),
            busy: tokio::sync::Mutex::new(()),
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
            let settings = game_settings(&app.game, &setup.wine, &setup.prefix);
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
            let mut cmd =
                irori_runtime::prefix::WineCommand::new([installer.display().to_string()]);
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

    /// Start the launcher itself (to sign in or install games there).
    pub async fn open(&self, launcher: Launcher) -> Result<()> {
        let setup = self.state.read().setups.get(&launcher).cloned();
        let program = launcher_program(launcher, setup.as_ref())
            .filter(|p| p.exists())
            .with_context(|| format!("{} isn't set up yet", launcher.display_name()))?;
        let spec = self.command(launcher, setup.as_ref(), &program, Vec::new())?;
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
        let settings = game_settings(&app.game, &setup.wine, &setup.prefix);
        let umu = irori_runtime::runtimes::is_umu_supported(&settings, true)
            .then(irori_runtime::runtimes::umu_path);
        let wc = wine_context(setup, umu);
        let mut argv = vec![program.display().to_string()];
        argv.extend(args);
        let mut cmd = irori_runtime::prefix::WineCommand::new(argv);
        cmd.skip_prefix_check = true;
        cmd.cwd = program.parent().map(Path::to_path_buf);
        Ok(irori_runtime::prefix::wine_spec(
            &app,
            &settings,
            &wc,
            &cmd,
            &irori_runtime::env::HostEnv::current(),
        ))
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
                        let size = xbox::folder_size(&dir);
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

/// Every game the library shows, from what the last scan found.
fn library(s: &State) -> Vec<Arc<GameInfo>> {
    let launchers = s.found.iter().map(game_info);
    let xbox = s.xbox.iter().map(|g| {
        let size = s
            .sizes
            .get(&(g.install_dir.clone(), g.version.clone()))
            .copied();
        xbox_game_info(g, size)
    });
    launchers.chain(xbox).map(Arc::new).collect()
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
fn game_settings(defaults: &GameSettings, wine: &WineInstallation, prefix: &Path) -> GameSettings {
    GameSettings {
        wine_version: wine.clone(),
        wine_prefix: prefix.display().to_string(),
        ..defaults.clone()
    }
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
            let ok = std::process::Command::new("reg")
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
        folder_name: g
            .install_dir
            .parent()
            .filter(|_| g.install_dir.ends_with(xbox::CONTENT))
            .unwrap_or(&g.install_dir)
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
        self.rescan().await;
        Ok(())
    }

    fn user(&self) -> Option<UserInfo> {
        Some(UserInfo {
            account_id: "local".into(),
            display_name: "Local".into(),
        })
    }

    fn login_flow(&self) -> LoginFlow {
        LoginFlow::None
    }

    async fn login(&self, _input: &str) -> Result<UserInfo> {
        self.user().context("no account")
    }

    async fn logout(&self) -> Result<()> {
        Ok(())
    }

    async fn refresh(&self, _full: bool) -> Result<()> {
        self.rescan().await;
        Ok(())
    }

    fn games(&self) -> Vec<Arc<GameInfo>> {
        if !self.enabled() {
            return Vec::new();
        }
        self.state.read().games.clone()
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
        self.game(app_name).is_some()
    }

    /// Xbox games: show the game's page in the Microsoft Store, where the
    /// Xbox app's install, update and remove commands are too. The other
    /// launchers keep their games to themselves.
    async fn client_action(&self, app_name: &str, action: ClientAction) -> Result<()> {
        let game = self
            .xbox_game(app_name)
            .context("This game's launcher has no page to show")?;
        anyhow::ensure!(
            action == ClientAction::Show,
            "Install, update and remove Xbox games in the Xbox app"
        );
        process::spawn(&shell_open(&game.store_link()), None)?;
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
            .context("This game is no longer listed by its launcher")?;
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
        let command = self.command(game.launcher, setup.as_ref(), &program, args)?;
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
        assert_eq!(
            info.cover_art(),
            Some("file:///d/XboxGames/Sky Racer/Content/Logo480.png")
        );
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
}
