//! Running Windows games on Linux: Wine/Proton discovery and downloads, prefix
//! management, DXVK/VKD3D, umu and the Steam runtime, and assembly of the
//! launch environment (env vars and wrapper chain) handed to stores.
//!
//! The top-level functions are the contract used by `irori-services`; the
//! modules expose the building blocks (Wine manager, tools, system info) for
//! the settings UI.

pub mod discovery;
pub mod download;
pub mod env;
pub mod frame_limit;
pub mod graphics;
pub mod hooks;
pub mod hud;
pub mod locations;
pub mod mac_driver;
pub mod manager;
pub mod mangohud;
pub mod metalfx;
pub mod prefix;
pub mod protontricks;
pub mod release_checks;
pub mod rosetta;
pub mod runtimes;
pub mod sandbox;
pub mod steam_tools;
pub mod sysinfo;
pub mod tools;
pub mod umu;
pub mod vdf;
pub mod windows_file;
pub mod windows_uri;
pub mod without_wine;
pub mod wrappers;
pub mod xbox;

use std::collections::HashMap;
use std::hash::{Hash as _, Hasher as _};
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context as _, Result, anyhow, bail};
use irori_core::process::{CommandSpec, ExecResult};
use irori_core::{
    AppSettings, GameId, GameInfo, GameSettings, InstallPlatform, LaunchEnvironment, OpContext,
    WineInstallation, WineKind, WineLaunch,
};
use once_cell::sync::Lazy;
use parking_lot::Mutex;

pub use discovery::{DiscoveredWine, WineSource, recommended};
pub use download::{Cancelled, is_cancelled};
pub use hooks::{LaunchHooks, set_hooks};
pub use manager::{Release, ReleaseKind, available_releases};
pub use prefix::WineContext;
pub use tools::DxTool;

use env::HostEnv;
use prefix::WineCommand;
use runtimes::RuntimeName;
use wrappers::WrapperParts;

/// The game being launched or operated on.
#[derive(Debug, Clone, Copy)]
pub struct Target<'a> {
    pub id: &'a GameId,
    pub game: &'a GameInfo,
    /// Platform of the installed build.
    pub platform: InstallPlatform,
}

impl Target<'_> {
    /// Identifier used for `SteamGameId` (`heroic-<folder>`).
    fn folder_id(&self) -> String {
        self.game
            .folder_name
            .clone()
            .filter(|f| !f.is_empty())
            .unwrap_or_else(|| "0".into())
    }
}

/// Prefix and build of recent launches, keyed by a fingerprint of the
/// settings they used, so [`kill_prefix`]/[`wait_prefix`] (which only get the
/// settings) can find a default per-game prefix.
static ACTIVE: Lazy<Mutex<HashMap<u64, (WineInstallation, PathBuf)>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

fn fingerprint(settings: &GameSettings) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    serde_json::to_string(settings)
        .unwrap_or_default()
        .hash(&mut h);
    h.finish()
}

fn note(ctx: &OpContext, msg: &str) {
    tracing::warn!("{msg}");
    (ctx.step)(msg);
}

/// Pick the Wine/Proton build for a game: its own setting when valid, else
/// the global default, else the recommended discovered build, else (when
/// `allow_download`) a freshly downloaded Proton. Heroic asks the user in a
/// dialog at this point; Irori falls back and says so in the status line.
pub async fn resolve_wine(
    app: &AppSettings,
    settings: &GameSettings,
    ctx: &OpContext,
    allow_download: bool,
) -> Result<WineInstallation> {
    anyhow::ensure!(!cfg!(windows), "Windows games run natively on Windows");
    let wanted = &settings.wine_version;
    if discovery::is_usable(wanted) {
        return Ok(wanted.clone());
    }
    let fallback_note = |found: &WineInstallation| {
        if wanted.is_set() {
            note(
                ctx,
                &format!(
                    "Wine version {} is not valid, using {} instead",
                    wanted.name, found.name
                ),
            );
        }
    };
    if discovery::is_usable(&app.game.wine_version) {
        fallback_note(&app.game.wine_version);
        return Ok(app.game.wine_version.clone());
    }
    let found: Vec<WineInstallation> = discovery::discover(app)
        .await
        .into_iter()
        .map(|d| d.installation)
        .collect();
    if let Some(best) = recommended(&found) {
        fallback_note(best);
        return Ok(best.clone());
    }
    if allow_download {
        note(ctx, "No Wine or Proton found; downloading Proton");
        return manager::download_default_wine(app, ctx)
            .await
            .context("No Wine or Proton version is available and none could be downloaded");
    }
    bail!("No Wine or Proton version was found. Download one in the Wine manager or install Wine.")
}

/// Resolve the build, prefix and umu use for a game. umu-run is downloaded
/// when Proton needs it; if that fails, Proton runs directly.
pub async fn wine_context(
    app: &AppSettings,
    settings: &GameSettings,
    game: &GameInfo,
    ctx: &OpContext,
    allow_download: bool,
) -> Result<WineContext> {
    let wine = resolve_wine(app, settings, ctx, allow_download).await?;
    // These macOS Wine distributions execute Intel Windows code through
    // Rosetta. Nothing is installed silently: the UI offers it.
    rosetta::ensure(&wine.name).await?;
    let prefix = if wine.kind == WineKind::Crossover {
        let bottle = settings.wine_crossover_bottle.trim();
        anyhow::ensure!(
            !bottle.is_empty() && !bottle.contains(['/', '\\']) && bottle != "." && bottle != "..",
            "Choose a valid CrossOver bottle name"
        );
        let bottles = locations::crossover_bottles_dir(app);
        let prefix = bottles.join(bottle);
        anyhow::ensure!(
            prefix.join("cxbottle.conf").is_file(),
            "CrossOver bottle {bottle} was not found in {}. Create it in CrossOver first, or set CrossOver's bottles folder in Settings.",
            bottles.display()
        );
        prefix
    } else {
        prefix_path(app, settings, game)
    };
    let effective = with_wine(settings, &wine);
    let mut uses_umu = runtimes::is_umu_supported(&effective, false);
    let mut umu = PathBuf::new();
    if uses_umu {
        match runtimes::ensure_umu(ctx).await {
            Ok(p) => umu = p,
            Err(e) => {
                note(ctx, &format!("{e:#}; running Proton directly"));
                uses_umu = false;
            }
        }
    }
    Ok(WineContext::new(wine, prefix, uses_umu, umu))
}

fn with_wine(settings: &GameSettings, wine: &WineInstallation) -> GameSettings {
    GameSettings {
        wine_version: wine.clone(),
        ..settings.clone()
    }
}

/// Cached result of the gamescope CLI probe per binary.
static GAMESCOPE_NEW_CLI: Lazy<Mutex<HashMap<PathBuf, bool>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

/// Whether this gamescope uses the 3.12+ CLI (`-F, --filter` in `--help`).
async fn gamescope_new_cli(bin: &Path) -> bool {
    if let Some(v) = GAMESCOPE_NEW_CLI.lock().get(bin) {
        return *v;
    }
    let spec = CommandSpec::new(bin).arg("--help");
    let new = discovery::run_with_timeout(&spec, Duration::from_secs(10))
        .await
        .is_some_and(|r| r.stderr.contains("-F, --filter") || r.stdout.contains("-F, --filter"));
    GAMESCOPE_NEW_CLI.lock().insert(bin.to_path_buf(), new);
    new
}

fn find_on_path(name: &str) -> Option<String> {
    which::which(name)
        .ok()
        .map(|p| p.to_string_lossy().into_owned())
}

/// How MangoHud takes part in a launch: the in-game stats ([`hud`]) load
/// it, and the frame limiter ([`frame_limit`]) may use it, hidden then.
#[derive(Debug, Clone, Copy, Default)]
pub struct MangoUse {
    /// MangoHud loads for the session.
    pub loads: bool,
    /// The in-game stats show it.
    pub shown: bool,
    /// Only the frame limit loads it: Irori starts it itself.
    pub limit_only: bool,
}

/// The launch's frame limit and limiter ([`frame_limit`]), the settings
/// the rest of the launch reads (only that limiter switched on), and how
/// MangoHud takes part: when the in-game stats show it, or when it limits
/// (hidden then). The stats switched off for the game leave it out as a
/// limiter. Steam Deck's Game Mode has Steam's own MangoHud and gamescope.
fn frame_limits(
    settings: &GameSettings,
    platform: InstallPlatform,
    deck_game_mode: bool,
    hud_plan: &hud::launch_env::HudLaunchPlan,
) -> (GameSettings, Option<frame_limit::FrameLimit>, MangoUse) {
    let linux = cfg!(target_os = "linux") && !deck_game_mode;
    let shown = hud_plan.env_set.get("MANGOHUD").is_some_and(|v| v == "1");
    let disabled = hud_plan.env_set.contains_key("DISABLE_MANGOHUD");
    let cx = frame_limit::LimitContext {
        linux: cfg!(target_os = "linux"),
        gamescope: linux && wrappers::gamescope_enabled(&settings.gamescope),
        mangohud: linux && !disabled && (shown || find_on_path("mangohud").is_some()),
        wine: platform.needs_wine(),
    };
    let limit = frame_limit::resolve(settings, cx);
    let limits = linux && limit.is_some_and(|l| l.limiter == frame_limit::Limiter::MangoHud);
    let mango = MangoUse {
        loads: linux && (shown || limits),
        shown: linux && shown,
        limit_only: limits && !shown,
    };
    (frame_limit::apply(settings, limit), limit, mango)
}

/// MangoHud's file or variable for the session ([`mangohud::prepare`]),
/// over the in-game stats' configuration when they set one: hidden unless
/// it shows (and isn't started hidden), with the limit when it is the
/// launch's limiter.
async fn mangohud_env(
    id: &irori_core::GameId,
    settings: &GameSettings,
    limit: Option<frame_limit::FrameLimit>,
    vars: &mut env::EnvMap,
    host: &HostEnv,
    shown: bool,
) {
    let ours = limit.filter(|l| l.limiter == frame_limit::Limiter::MangoHud);
    let limits = limit.is_none() || ours.is_some();
    let hidden = settings.mangohud_hidden || !shown;
    let (id, mut moved, host) = (id.clone(), vars.clone(), host.clone());
    // A small file read and written: off the async workers all the same.
    let done = tokio::task::spawn_blocking(move || {
        mangohud::prepare(&id, &mut moved, &host, hidden, ours.map(|l| l.fps), limits);
        moved
    })
    .await;
    match done {
        Ok(back) => *vars = back,
        Err(e) => tracing::warn!("setting MangoHud up: {e}"),
    }
}

/// Resolve GameMode, MAKO, gamescope and the MangoHud wrapper (Heroic's
/// `prepareLaunch` steps 7–9): the in-game stats' OpenGL wrapper, or
/// `mangohud --dlsym` when only the frame limit loads MangoHud. A missing
/// `gamemoderun`, `gamescope` or (for the limit) `mangohud` fails the
/// launch; Vulkan-layer MangoHud and gamescope `--mangoapp` never require
/// the `mangohud` wrapper binary.
async fn wrapper_parts(
    settings: &GameSettings,
    hud_plan: &hud::launch_env::HudLaunchPlan,
    limit_only: bool,
) -> Result<WrapperParts> {
    if !cfg!(target_os = "linux") {
        return Ok(WrapperParts::default());
    }
    let deck_game_mode = locations::is_steam_deck_game_mode();
    let mut parts = WrapperParts::default();
    if let Some(wrapper) = &hud_plan.opengl_wrapper {
        parts.mangohud = wrapper.clone();
    } else if limit_only {
        let flatpak_hint = if locations::is_flatpak() {
            ". Make sure to install the MangoHud Flatpak extension matching Irori's runtime and restart Irori."
        } else {
            ""
        };
        let bin = find_on_path("mangohud").ok_or_else(|| {
            anyhow!("Mangohud is enabled, but `mangohud` executable could not be found on $PATH{flatpak_hint}")
        })?;
        parts.mangohud = vec![bin, "--dlsym".into()];
    }
    if settings.use_game_mode {
        let bin = find_on_path("gamemoderun").ok_or_else(|| {
            anyhow!("GameMode is enabled, but `gamemoderun` executable could not be found on $PATH")
        })?;
        parts.gamemode = Some(bin);
    }
    if settings.lsfg.active_layer() == Some(irori_core::FrameGenLayer::Mako) {
        let bin = graphics::find_mako(&HostEnv::current(), &locations::home()).ok_or_else(|| {
            anyhow!(
                "MAKO frame generation is enabled, but neither `mako-launch` nor `mako-run` could be found on $PATH or in ~/.local/bin. Install MAKO Renderer, or choose lsfg-vk."
            )
        })?;
        parts.frame_generation = vec![bin.to_string_lossy().into_owned()];
    }
    if wrappers::gamescope_enabled(&settings.gamescope) && !deck_game_mode {
        let bin = which::which("gamescope").map_err(|_| {
            let hint = if locations::is_flatpak() {
                ". Make sure to install the gamescope Flatpak extension matching Irori's runtime and restart Irori."
            } else {
                ""
            };
            anyhow!("Gamescope is enabled, but `gamescope` executable could not be found on $PATH{hint}")
        })?;
        let new_cli = gamescope_new_cli(&bin).await;
        let mut cmd = vec![bin.to_string_lossy().into_owned()];
        cmd.extend(wrappers::gamescope_args(
            &settings.gamescope,
            hud_plan.use_mangoapp || limit_only,
            new_cli,
        ));
        parts.gamescope = cmd;
    }
    Ok(parts)
}

/// Resolve the effective HUD configuration for an optional `game_id`, write any
/// Irori-managed `MangoHud.conf` under `paths` (unless `write` is off, for a
/// preview: the file is only named then), and return the launch mutations.
pub async fn prepare_hud_plan_for_paths(
    paths: &irori_core::Paths,
    app: &AppSettings,
    settings: &GameSettings,
    game_id: Option<&GameId>,
    platform: InstallPlatform,
    gamescope_enabled: bool,
    write: bool,
) -> Result<hud::launch_env::HudLaunchPlan> {
    let backend = irori_core::HudBackend::current();
    let resolved = irori_core::hud::resolve_effective_hud(
        &app.hud,
        &settings.game_hud,
        settings.show_mangohud,
        settings.show_fps,
        backend,
    );
    let deck_game_mode = cfg!(target_os = "linux") && locations::is_steam_deck_game_mode();
    let gamescope_enabled = gamescope_enabled && !deck_game_mode;
    let is_native_linux_game = cfg!(target_os = "linux") && platform == InstallPlatform::Linux;

    let mut config_file: Option<PathBuf> = None;
    let mut mangohud_dir: Option<PathBuf> = None;
    let mut mangohud_bin_path: Option<PathBuf> = None;

    if backend == irori_core::HudBackend::MangoHud
        && !deck_game_mode
        && matches!(resolved, irori_core::ResolvedHud::MangoHudProfile(_))
    {
        let linux_env = tokio::task::spawn_blocking(hud::hardware_linux::detect_linux_hud_env)
            .await
            .unwrap_or_else(|_| hud::hardware_linux::detect_linux_hud_env());
        mangohud_bin_path = linux_env.mangohud_bin_path.clone();

        if let irori_core::ResolvedHud::MangoHudProfile(ref profile) = resolved {
            let dir = paths.mangohud_dir();
            let conf_name = if settings.game_hud.mode == irori_core::GameHudMode::Custom
                && let Some(id) = game_id
            {
                format!("{}_{}", id.runner.id(), id.app_name)
            } else {
                "global".to_string()
            };
            let file = paths.mangohud_config_file(&conf_name);
            let ctx = hud::mangohud_conf::ConfRenderContext {
                detected_gpu_label: hud::hardware_linux::default_gpu_short_label(
                    &profile.gpu_selection,
                    &linux_env.gpus,
                ),
                detected_cpu_label: linux_env.cpu_label.clone(),
                gpu_vendor: hud::hardware_linux::default_gpu_vendor(
                    &profile.gpu_selection,
                    &linux_env.gpus,
                ),
                cpu_vendor: linux_env.cpu_vendor,
                pci_dev: hud::hardware_linux::resolve_pci_dev(
                    &profile.gpu_selection,
                    &linux_env.gpus,
                ),
                theme_colors: app.hud.cached_theme_colors.clone(),
            };
            let rendered = hud::mangohud_conf::render_mangohud_conf(profile, &ctx);
            if write {
                hud::write_config_live(&file, &rendered).await?;
            }
            config_file = Some(file);
            mangohud_dir = Some(dir);
        }
    }

    Ok(hud::launch_env::build_hud_launch_plan(
        &hud::launch_env::HudLaunchInput {
            backend,
            resolved: &resolved,
            config_file: config_file.as_deref(),
            mangohud_dir: mangohud_dir.as_deref(),
            mangohud_bin_path: mangohud_bin_path.as_deref(),
            is_native_linux_game,
            gamescope_enabled,
            deck_game_mode,
        },
    ))
}

/// Write any managed HUD configuration under `paths` and apply the resulting
/// environment variables to `spec`.
pub async fn apply_hud_to_command_with_paths(
    spec: &mut CommandSpec,
    paths: &irori_core::Paths,
    app: &AppSettings,
    settings: &GameSettings,
    game_id: Option<&GameId>,
    platform: InstallPlatform,
) -> Result<()> {
    let plan =
        prepare_hud_plan_for_paths(paths, app, settings, game_id, platform, false, true).await?;
    hud::launch_env::apply_hud_plan_to_spec(spec, &plan);
    Ok(())
}

/// Write any managed HUD configuration under the active [`Paths`] and apply
/// the resulting environment variables to `spec`.
pub async fn apply_hud_to_command(
    spec: &mut CommandSpec,
    app: &AppSettings,
    settings: &GameSettings,
    game_id: Option<&GameId>,
    platform: InstallPlatform,
) -> Result<()> {
    apply_hud_to_command_with_paths(
        spec,
        irori_core::Paths::get(),
        app,
        settings,
        game_id,
        platform,
    )
    .await
}

/// Resolve the effective Performance HUD for `target`, write any Irori-managed
/// `MangoHud.conf` atomically (only when `write`), and return the launch
/// mutations.
async fn prepare_hud_plan(
    app: &AppSettings,
    settings: &GameSettings,
    target: Target<'_>,
    write: bool,
) -> Result<hud::launch_env::HudLaunchPlan> {
    let deck_game_mode = cfg!(target_os = "linux") && locations::is_steam_deck_game_mode();
    let gamescope_enabled = cfg!(target_os = "linux")
        && wrappers::gamescope_enabled(&settings.gamescope)
        && !deck_game_mode;
    prepare_hud_plan_for_paths(
        irori_core::Paths::get(),
        app,
        settings,
        Some(target.id),
        target.platform,
        gamescope_enabled,
        write,
    )
    .await
}

/// Apply HUD environment removals and overrides on top of assembled launch vars.
fn apply_hud_env(vars: &mut env::EnvMap, hud_plan: &hud::launch_env::HudLaunchPlan) {
    for key in &hud_plan.env_remove {
        vars.remove(key);
    }
    env::layer(vars, hud_plan.env_set.clone());
}

/// [`graphics::graphics_env`] for a launch, probing the host on a blocking
/// thread when the settings need it. Linux only.
async fn graphics_env(app: &AppSettings, settings: &GameSettings, host: &HostEnv) -> env::EnvMap {
    if !cfg!(target_os = "linux") {
        return env::EnvMap::new();
    }
    let (app, settings, host) = (app.clone(), settings.clone(), host.clone());
    tokio::task::spawn_blocking(move || {
        let found = graphics::HostGraphics::probe_for(&app, &settings, &host);
        graphics::graphics_env(&settings, &found)
    })
    .await
    .unwrap_or_default()
}

/// What a launch's variables are made from, once the system has been
/// asked: everything [`compose_env`] needs and nothing it looks up itself.
/// [`prepare_launch`] gathers it while setting the game up; [`preview_launch`]
/// gathers it without changing anything, so both show one result.
#[derive(Debug, Clone)]
pub struct LaunchFacts {
    pub host: HostEnv,
    /// The game's settings after the frame limit was resolved: only the
    /// limiter that applies is switched on ([`frame_limit`]).
    pub settings: GameSettings,
    pub limit: Option<frame_limit::FrameLimit>,
    /// How MangoHud takes part: when the in-game stats show it, or when it
    /// limits.
    pub mango: MangoUse,
    /// The in-game stats' variables and wrapper ([`hud`]).
    pub hud_plan: hud::launch_env::HudLaunchPlan,
    pub parts: WrapperParts,
    /// The GPU and frame generation variables ([`graphics::graphics_env`]).
    pub graphics: env::EnvMap,
    /// Known fixes' variables for the game.
    pub fixes: env::EnvMap,
    /// The Steam runtime's `PROTONPATH`, for a native game on it.
    pub runtime: Option<String>,
    /// The Wine layer of a Windows game.
    pub wine: Option<WineContext>,
    /// The umu database's `GAMEID`, for a game under umu.
    pub umu_game_id: Option<String>,
    /// A Windows build whose program is one of this system's starts
    /// without Wine, as a native build does.
    pub without_wine: bool,
    /// `APPIMAGE_EXTRACT_AND_RUN` for an AppImage program FUSE can't mount
    /// here, unless the game's own variables set it.
    pub appimage: Option<(String, String)>,
    /// DLSS through MetalFX on macOS: the build's switch and DLL
    /// preferences.
    pub metalfx: metalfx::Applied,
}

/// The variables a launch starts with, MangoHud's and the sandbox's aside,
/// in [`prepare_launch`]'s precedence. Reads no more than `facts`, so the
/// launch and its preview can't differ.
pub fn compose_env(app: &AppSettings, target: Target<'_>, facts: &LaunchFacts) -> env::EnvMap {
    let mut vars = env::wrapper_env(target.id);
    let install_path = target.game.install_path();
    let host = &facts.host;
    match &facts.wine {
        None => {
            env::layer(
                &mut vars,
                env::generic_env(
                    &facts.settings,
                    install_path.as_deref(),
                    facts.runtime.as_deref(),
                    host,
                ),
            );
            env::layer(&mut vars, facts.graphics.clone());
            env::layer(&mut vars, facts.fixes.clone());
            // The game's own variables decide when they set it.
            if let Some((key, value)) = &facts.appimage
                && !vars.contains_key(key)
            {
                vars.insert(key.clone(), value.clone());
            }
        }
        Some(wc) => {
            let effective = with_wine(&facts.settings, &wc.wine);
            env::layer(
                &mut vars,
                env::generic_env(&effective, install_path.as_deref(), None, host),
            );
            env::layer(&mut vars, facts.graphics.clone());
            env::layer(&mut vars, facts.fixes.clone());
            env::layer(
                &mut vars,
                prefix::wine_env_for(app, &effective, wc, &target.folder_id(), host),
            );
            env::layer(&mut vars, env::game_debug_env(&effective, host));
            env::apply_dll_overrides(&mut vars, &effective.dll_overrides);
            facts.metalfx.add_to(&mut vars);
            if wc.uses_umu
                && let Some(id) = &facts.umu_game_id
            {
                vars.insert("GAMEID".into(), id.clone());
            }
        }
    }
    // The in-game stats' variables win over the game's own, which may set
    // MangoHud's for stats switched off.
    apply_hud_env(&mut vars, &facts.hud_plan);
    vars
}

/// How the facts are gathered: for a launch, setting the game up on the
/// way; for a preview, only looking.
#[derive(Clone, Copy)]
enum Gather<'a> {
    Launch(&'a OpContext),
    Preview,
}

/// Gather what [`compose_env`] needs for `target`. For a launch this also
/// creates or upgrades the prefix, installs DXVK/VKD3D/NVAPI and anticheat
/// runtimes when enabled and downloads umu or the Steam runtime; for a
/// preview nothing is created, downloaded or written.
async fn gather_facts(
    app: &AppSettings,
    settings: &GameSettings,
    target: Target<'_>,
    how: Gather<'_>,
) -> Result<LaunchFacts> {
    let host = HostEnv::current();
    // A Windows build whose program is one of this system's (a native
    // replacement) starts without Wine, as a native build does.
    let without_wine = target.platform.needs_wine()
        && match how {
            Gather::Launch(_) => without_wine::runs_without_wine(settings, target.game).await?,
            Gather::Preview => without_wine::decides(settings, target.game).await?,
        };
    let target = if without_wine {
        Target {
            platform: irori_core::HostPlatform::current().native(),
            ..target
        }
    } else {
        target
    };
    let write = matches!(how, Gather::Launch(_));
    let hud_plan = prepare_hud_plan(app, settings, target, write).await?;
    let deck = cfg!(target_os = "linux") && locations::is_steam_deck_game_mode();
    let (adjusted, limit, mango) = frame_limits(settings, target.platform, deck, &hud_plan);
    let settings = &adjusted;
    let mut parts = wrapper_parts(settings, &hud_plan, mango.limit_only).await?;
    let fixes = if cfg!(target_os = "linux") {
        hooks::hooks().known_fixes_env(target.id).await
    } else {
        Default::default()
    };

    if !target.platform.needs_wine() {
        let runtime = settings
            .steam_runtime
            .as_deref()
            .filter(|r| cfg!(target_os = "linux") && !r.trim().is_empty())
            .map(runtimes::steam_runtime_protonpath);
        if runtime.is_some() {
            let umu = match how {
                Gather::Launch(ctx) => runtimes::ensure_umu(ctx)
                    .await
                    .context("The Steam runtime needs umu-run")?,
                Gather::Preview => runtimes::umu_path(),
            };
            parts.steam_runtime = vec![umu.to_string_lossy().into_owned()];
        }
        let program = without_wine::launch_program(settings, target.game);
        let appimage = if program.as_deref().is_some_and(|p| {
            irori_core::program_kind::is_appimage(
                &p.file_name().unwrap_or_default().to_string_lossy(),
            )
        }) {
            let fuse = tokio::task::spawn_blocking(without_wine::fuse_usable)
                .await
                .unwrap_or(false);
            without_wine::appimage_env(program.as_deref(), fuse)
        } else {
            None
        };
        return Ok(LaunchFacts {
            graphics: graphics_env(app, settings, &host).await,
            host,
            settings: adjusted.clone(),
            limit,
            mango,
            hud_plan,
            parts,
            fixes,
            runtime,
            wine: None,
            umu_game_id: None,
            without_wine,
            appimage,
            metalfx: metalfx::Applied::default(),
        });
    }

    let (wc, metalfx) = match how {
        Gather::Launch(ctx) => {
            let wc = wine_context(app, settings, target.game, ctx, true).await?;
            let metalfx = set_up_prefix(app, settings, target, &wc, ctx).await?;
            (wc, metalfx)
        }
        Gather::Preview => {
            let wc = preview_wine_context(app, settings, target.game).await?;
            let metalfx = if cfg!(target_os = "macos") {
                let (wine, enabled) = (wc.wine.clone(), settings.metal_fx_dlss);
                tokio::task::spawn_blocking(move || metalfx::planned(&wine, enabled))
                    .await
                    .unwrap_or_default()
            } else {
                metalfx::Applied::default()
            };
            (wc, metalfx)
        }
    };
    let effective = with_wine(settings, &wc.wine);
    let umu_game_id = if wc.uses_umu {
        umu::resolve(target.id, &target.game.title, &effective)
            .await
            .map(|found| found.id)
    } else {
        None
    };
    Ok(LaunchFacts {
        graphics: graphics_env(app, &effective, &host).await,
        host,
        settings: adjusted.clone(),
        limit,
        mango,
        hud_plan,
        parts,
        fixes,
        runtime: None,
        wine: Some(wc),
        umu_game_id,
        without_wine: false,
        appimage: None,
        metalfx,
    })
}

/// A launch's work in the prefix before the game starts: verify or create
/// it, Retina mode and DLSS through MetalFX on macOS, record the game,
/// DXVK/VKD3D/NVAPI and the anticheat runtimes. What can't be done is said
/// in the status line and doesn't stop the launch, a cancel aside. Returns
/// what MetalFX leaves the variables to do.
async fn set_up_prefix(
    app: &AppSettings,
    settings: &GameSettings,
    target: Target<'_>,
    wc: &WineContext,
    ctx: &OpContext,
) -> Result<metalfx::Applied> {
    let effective = with_wine(settings, &wc.wine);
    ACTIVE
        .lock()
        .insert(fingerprint(settings), (wc.wine.clone(), wc.prefix.clone()));

    prefix::verify_prefix(app, &effective, wc, ctx).await?;
    if cfg!(target_os = "macos")
        && let Err(e) =
            mac_driver::apply_retina_mode(app, &effective, wc, settings.retina_mode, ctx).await
    {
        if is_cancelled(&e) {
            return Err(e);
        }
        note(ctx, &format!("Could not change Retina mode: {e:#}"));
    }
    let metalfx = if cfg!(target_os = "macos") {
        match metalfx::apply(app, &effective, wc, settings.metal_fx_dlss, ctx).await {
            Ok(applied) => {
                if let Some(why) = applied.unavailable {
                    note(ctx, why);
                }
                applied
            }
            Err(e) if is_cancelled(&e) => return Err(e),
            Err(e) => {
                note(
                    ctx,
                    &format!("Could not set up DLSS through MetalFX: {e:#}"),
                );
                metalfx::Applied::default()
            }
        }
    } else {
        metalfx::Applied::default()
    };
    match prefix::register_game(&wc.prefix, &target.id.app_name).await {
        Ok(true) => {
            tracing::info!("Created/Updated Wineprefix at {}", wc.prefix.display());
            if let Err(e) = hooks::hooks()
                .prefix_initialized(target.id, target.game, &wc.prefix, ctx)
                .await
            {
                tracing::warn!("prefix setup for {}: {e:#}", target.id);
            }
        }
        Ok(false) => {}
        Err(e) => tracing::warn!("{e:#}"),
    }

    if cfg!(target_os = "linux") && wc.wine.kind == WineKind::Wine {
        let mut wanted = Vec::new();
        if settings.auto_install_dxvk {
            wanted.push(DxTool::Dxvk);
        }
        if settings.auto_install_dxvk_nvapi {
            wanted.push(DxTool::DxvkNvapi);
        }
        if settings.auto_install_vkd3d {
            wanted.push(DxTool::Vkd3d);
        }
        if let Err(e) = tools::install_tools(app, &effective, wc, &wanted, ctx).await {
            if is_cancelled(&e) {
                return Err(e);
            }
            note(ctx, &format!("Could not install DXVK/VKD3D: {e:#}"));
        }
    }

    for (enabled, runtime) in [
        (settings.eac_runtime, RuntimeName::EacRuntime),
        (settings.battl_eye_runtime, RuntimeName::BattlEyeRuntime),
    ] {
        if !cfg!(target_os = "linux") || !enabled {
            continue;
        }
        if let Err(e) = runtimes::ensure(runtime, ctx).await {
            if is_cancelled(&e) {
                return Err(e);
            }
            note(
                ctx,
                &format!("Could not download the {}: {e:#}", runtime.label()),
            );
        }
    }
    Ok(metalfx)
}

/// The Wine layer a launch would use, found without downloading a build or
/// umu and without asking for Rosetta.
async fn preview_wine_context(
    app: &AppSettings,
    settings: &GameSettings,
    game: &GameInfo,
) -> Result<WineContext> {
    let wine = resolve_wine(app, settings, &OpContext::detached(), false).await?;
    let prefix = if wine.kind == WineKind::Crossover {
        locations::crossover_bottles_dir(app).join(settings.wine_crossover_bottle.trim())
    } else {
        prefix_path(app, settings, game)
    };
    let effective = with_wine(settings, &wine);
    let umu = runtimes::umu_path();
    let uses_umu = runtimes::is_umu_supported(&effective, false) && umu.is_file();
    Ok(WineContext::new(wine, prefix, uses_umu, umu))
}

/// Prepare everything needed to start `target`: create/upgrade the prefix,
/// auto-install DXVK/VKD3D/NVAPI and anticheat runtimes when enabled, download
/// umu/Steam runtime when needed, and compute env vars and wrappers.
///
/// Environment precedence (lowest first): game identity (`HEROIC_APP_*`,
/// `STORE`, `GAMEID=umu-0`), generic variables and the user's variables,
/// the GPU and frame generation options (minus what the user set),
/// known-fixes variables, Wine/Proton variables, then the umu database
/// `GAMEID` ([`compose_env`]), then MangoHud's. Wrappers, outermost first:
/// gamescope, user wrappers, MangoHud (without gamescope), gamemoderun,
/// and umu-run for native games on the Steam runtime. For Windows games
/// umu-run is the Wine layer's program ([`WineLaunch::bin`]), never a
/// wrapper.
pub async fn prepare_launch(
    app: &AppSettings,
    settings: &GameSettings,
    target: Target<'_>,
    ctx: &OpContext,
) -> Result<LaunchEnvironment> {
    anyhow::ensure!(
        irori_core::HostPlatform::current().supports(target.platform),
        "{} builds cannot run on this operating system. Install a compatible build instead.",
        target.platform.label()
    );
    if target.platform == InstallPlatform::Browser {
        return Ok(LaunchEnvironment {
            env: env::wrapper_env(target.id),
            ..Default::default()
        });
    }
    let mut facts = gather_facts(app, settings, target, Gather::Launch(ctx)).await?;
    let target = facts.target(target);
    let mut vars = compose_env(app, target, &facts);
    let effective = match &facts.wine {
        Some(wc) => with_wine(&facts.settings, &wc.wine),
        None => facts.settings.clone(),
    };
    if facts.mango.loads {
        let shown = facts.mango.shown;
        mangohud_env(
            target.id,
            &effective,
            facts.limit,
            &mut vars,
            &facts.host,
            shown,
        )
        .await;
    }
    facts.parts.sandbox = sandbox_wrapper(
        app,
        &facts.settings,
        target,
        facts.wine.as_ref(),
        &facts.parts,
        &vars,
        &facts.hud_plan.sandbox_ro_binds,
    )
    .await?;
    Ok(LaunchEnvironment {
        env: vars,
        env_remove: facts.hud_plan.env_remove.clone(),
        wrappers: wrappers::setup_wrappers(&facts.settings, &facts.parts),
        wine: facts.wine.as_ref().map(wine_launch),
        without_wine: facts.without_wine,
    })
}

/// The Wine layer a store starts a Windows game with.
fn wine_launch(wc: &WineContext) -> WineLaunch {
    WineLaunch {
        bin: wc.runner_bin(),
        build_bin: PathBuf::from(&wc.wine.bin),
        kind: wc.wine.kind,
        prefix: wc.prefix.clone(),
        uses_umu: wc.uses_umu,
        proton_verb: "waitforexitandrun",
    }
}

impl LaunchFacts {
    /// `target` as the launch runs it: a Windows build started without
    /// Wine runs as this system's own.
    fn target<'a>(&self, target: Target<'a>) -> Target<'a> {
        if self.without_wine {
            Target {
                platform: irori_core::HostPlatform::current().native(),
                ..target
            }
        } else {
            target
        }
    }
}

/// How a launch would start, worked out without starting anything.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LaunchPreview {
    /// Every variable the game gets from Irori, by name.
    pub env: Vec<(String, String)>,
    /// The wrappers, outermost first.
    pub wrappers: Vec<Vec<String>>,
    /// The Wine layer's program and verb, for a Windows game.
    pub runner: Vec<String>,
    /// The Wine prefix, for a Windows game.
    pub prefix: Option<PathBuf>,
    /// The game runs in a bubblewrap sandbox, whose options are worked
    /// out when it starts.
    pub sandboxed: bool,
    /// A Windows build starts its native program without Wine.
    pub without_wine: bool,
}

/// What [`prepare_launch`] would hand the store for `target`, from the same
/// facts and [`compose_env`], without creating the prefix, downloading or
/// writing anything. MangoHud's session file is named, not written; the
/// sandbox stands as `bwrap …`.
pub async fn preview_launch(
    app: &AppSettings,
    settings: &GameSettings,
    target: Target<'_>,
) -> Result<LaunchPreview> {
    anyhow::ensure!(
        irori_core::HostPlatform::current().supports(target.platform),
        "{} builds cannot run on this operating system. Install a compatible build instead.",
        target.platform.label()
    );
    if target.platform == InstallPlatform::Browser {
        return Ok(LaunchPreview {
            env: env::wrapper_env(target.id).into_iter().collect(),
            ..Default::default()
        });
    }
    let mut facts = gather_facts(app, settings, target, Gather::Preview).await?;
    let target = facts.target(target);
    let mut vars = compose_env(app, target, &facts);
    let effective = match &facts.wine {
        Some(wc) => with_wine(&facts.settings, &wc.wine),
        None => facts.settings.clone(),
    };
    if facts.mango.loads {
        let ours = facts
            .limit
            .filter(|l| l.limiter == frame_limit::Limiter::MangoHud);
        let limits = facts.limit.is_none() || ours.is_some();
        let hidden = effective.mangohud_hidden || !facts.mango.shown;
        let (id, seen, host) = (target.id.clone(), vars.clone(), facts.host.clone());
        let planned = tokio::task::spawn_blocking(move || {
            mangohud::plan(&id, &seen, &host, hidden, ours.map(|l| l.fps), limits)
        })
        .await?;
        vars.insert(planned.var.0, planned.var.1);
    }
    let sandboxed = facts.settings.sandbox.is_enabled();
    if sandboxed {
        facts.parts.sandbox = vec!["bwrap".into(), "…".into()];
    }
    Ok(LaunchPreview {
        env: vars.into_iter().collect(),
        wrappers: wrappers::setup_wrappers(&facts.settings, &facts.parts),
        runner: facts
            .wine
            .as_ref()
            .map(|wc| wine_launch(wc).exec_prefix())
            .unwrap_or_default(),
        prefix: facts.wine.map(|wc| wc.prefix),
        sandboxed,
        without_wine: facts.without_wine,
    })
}

/// The sandbox wrapper for a launch of a game set to run sandboxed; empty
/// otherwise. Collects the folders the game needs (its install folder and
/// prefix writable; the Wine or Proton build, umu and the anticheat runtimes
/// read-only) and refuses when bubblewrap can't run.
async fn sandbox_wrapper(
    app: &AppSettings,
    settings: &GameSettings,
    target: Target<'_>,
    wc: Option<&WineContext>,
    parts: &WrapperParts,
    vars: &env::EnvMap,
    hud_ro_binds: &[PathBuf],
) -> Result<Vec<String>> {
    if !settings.sandbox.is_enabled() {
        return Ok(Vec::new());
    }
    anyhow::ensure!(
        target.id.runner != irori_core::Runner::Emulation,
        "Emulated games can't run in a sandbox yet. Turn the sandbox off in the game's settings."
    );
    let mut game_dirs: Vec<PathBuf> = target.game.install_path().into_iter().collect();
    let exe = Path::new(settings.target_exe.trim());
    if exe.is_absolute() {
        game_dirs.extend(exe.parent().map(Path::to_path_buf));
    }
    let mut tool_dirs: Vec<PathBuf> = hud_ro_binds.to_vec();
    let mut overlay_dirs: Vec<PathBuf> = Vec::new();
    let uses_umu = !parts.steam_runtime.is_empty() || wc.is_some_and(|w| w.uses_umu);
    let kind = match wc {
        None if uses_umu => sandbox::SandboxKind::PressureVessel,
        None => sandbox::SandboxKind::Native,
        Some(wc) => {
            let (kind, tools) = sandbox_wine_parts(app, wc).await?;
            game_dirs.push(wc.prefix.clone());
            tool_dirs.extend(tools);
            kind
        }
    };
    if uses_umu {
        let umu = parts
            .steam_runtime
            .first()
            .map(PathBuf::from)
            .or_else(|| wc.map(|w| w.umu.clone()))
            .unwrap_or_else(runtimes::umu_path);
        tool_dirs.extend(umu.parent().map(Path::to_path_buf));
        overlay_dirs.extend(umu_data_dir());
    }
    for key in [
        "PROTONPATH",
        "PROTON_EAC_RUNTIME",
        "PROTON_BATTLEYE_RUNTIME",
    ] {
        if let Some(p) = vars.get(key).map(PathBuf::from).filter(|p| p.is_absolute()) {
            tool_dirs.push(p);
        }
    }
    // MangoHud's file for the session, which it reads and watches.
    if let Some(dir) = vars
        .get("MANGOHUD_CONFIGFILE")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .and_then(|p| p.parent().map(Path::to_path_buf))
    {
        tool_dirs.push(dir);
    }
    let plan = sandbox::SandboxPlan {
        settings: settings.sandbox,
        kind,
        game_dirs,
        tool_dirs,
        overlay_dirs,
        home: sandbox::sandbox_home(target.id),
        gamescope: !parts.gamescope.is_empty(),
    };
    sandbox::wrapper(target.id, plan).await
}

/// How a Wine context runs in the sandbox and the read-only folders its
/// build needs. Creates the prefix folder, which umu would otherwise create
/// in the sandbox's empty root rather than on disk.
pub async fn sandbox_wine_parts(
    app: &AppSettings,
    wc: &WineContext,
) -> Result<(sandbox::SandboxKind, Vec<PathBuf>)> {
    tokio::fs::create_dir_all(&wc.prefix)
        .await
        .with_context(|| format!("Could not create {}", wc.prefix.display()))?;
    let bin = Path::new(&wc.wine.bin);
    let mut tools: Vec<PathBuf> = Vec::new();
    if wc.wine.kind == WineKind::Proton {
        tools.extend(bin.parent().map(Path::to_path_buf));
        // Proton copies Steam's client libraries from here; the rest of
        // Steam's folder (its logins among it) stays hidden.
        tools.push(locations::steam_client_install_path(app).join("legacycompat"));
    } else {
        tools.extend(bin.parent().and_then(Path::parent).map(Path::to_path_buf));
    }
    let kind = if wc.uses_umu {
        tools.extend(wc.umu.parent().map(Path::to_path_buf));
        sandbox::SandboxKind::PressureVessel
    } else if wc.wine.kind == WineKind::Proton {
        sandbox::SandboxKind::Proton
    } else {
        sandbox::SandboxKind::Wine {
            wineserver: prefix::server_for(&wc.wine, &wc.prefix).0,
        }
    };
    Ok((kind, tools))
}

/// Where umu keeps the Steam runtime it runs Proton in.
pub fn umu_data_dir() -> Option<PathBuf> {
    dirs::data_dir().map(|d| d.join("umu"))
}

/// Run a Windows command inside the game's prefix (winecfg, regedit, an exe).
/// When `wait` is false, returns as soon as the process started.
pub async fn run_in_prefix(
    app: &AppSettings,
    settings: &GameSettings,
    target: Target<'_>,
    argv: Vec<String>,
    wait: bool,
    ctx: OpContext,
) -> Result<ExecResult> {
    if argv.is_empty() {
        bail!("nothing to run");
    }
    let wc = wine_context(app, settings, target.game, &ctx, false).await?;
    let effective = with_wine(settings, &wc.wine);
    let mut cmd = WineCommand::new(argv);
    cmd.env = env::wrapper_env(target.id);
    cmd.install_path = target.game.install_path();
    cmd.game_id = target.folder_id();
    if let Some(file) = cmd.argv.iter().find_map(|arg| {
        let p = Path::new(arg);
        (p.is_absolute() && p.is_file()).then_some(p)
    }) {
        cmd.cwd = file.parent().map(Path::to_path_buf);
    }
    if wait {
        cmd.wait = true;
        cmd.verb = "waitforexitandrun";
        // Point Proton's game drive (`S:`) at the prefix's `drive_c` while an
        // installer runs so installers that default to `S:\Games\<Title>` (such
        // as FitGirl repacks) install into `<prefix>/drive_c/Games/<Title>`
        // instead of root-owned `/home` on Btrfs `/home` subvolumes.
        let drive_c = wc.wine_prefix_dir().join("drive_c");
        let dc = drive_c.to_string_lossy().into_owned();
        cmd.install_path = Some(drive_c);
        cmd.env.insert("STEAM_COMPAT_LIBRARY_PATHS".into(), dc);
    } else {
        cmd.detach = true;
    }
    prefix::run_wine(app, &effective, &wc, cmd, &ctx).await
}

/// Open a link of another program's scheme in the game's prefix with
/// Wine's `start` ([`windows_uri::wine_command`]), so the Windows program
/// that registered the scheme there takes it. Returns once it has started.
pub async fn open_uri_in_prefix(
    app: &AppSettings,
    settings: &GameSettings,
    target: Target<'_>,
    uri: &str,
    ctx: OpContext,
) -> Result<ExecResult> {
    if !windows_uri::is_forwarded_uri(uri) {
        bail!("Irori doesn't open this kind of link in a Wine prefix");
    }
    let wc = wine_context(app, settings, target.game, &ctx, false).await?;
    let effective = with_wine(settings, &wc.wine);
    let mut cmd = windows_uri::wine_command(&wc, uri);
    let mut env = env::wrapper_env(target.id);
    env.append(&mut cmd.env);
    cmd.env = env;
    cmd.install_path = target.game.install_path();
    cmd.game_id = target.folder_id();
    cmd.detach = true;
    prefix::run_wine(app, &effective, &wc, cmd, &ctx).await
}

/// Settings-only lookup of the build and prefix of a game (explicit prefix
/// setting, else the prefix its last launch used).
fn resolve_for_settings(settings: &GameSettings) -> Option<(WineInstallation, PathBuf)> {
    let recorded = ACTIVE.lock().get(&fingerprint(settings)).cloned();
    let prefix =
        if settings.wine_prefix.is_empty() || settings.wine_version.kind == WineKind::Crossover {
            recorded.as_ref().map(|(_, p)| p.clone())?
        } else {
            irori_core::paths::expand_home(&settings.wine_prefix)
        };
    let wine = if discovery::is_usable(&settings.wine_version) {
        settings.wine_version.clone()
    } else {
        recorded.map(|(w, _)| w)?
    };
    Some((wine, prefix))
}

/// Kill every process in the prefix (`wineserver -k`).
pub async fn kill_prefix(settings: &GameSettings) -> Result<()> {
    match resolve_for_settings(settings) {
        Some((wine, prefix)) => prefix::kill(&wine, &prefix).await,
        None => {
            tracing::debug!("kill_prefix: prefix unknown for these settings");
            Ok(())
        }
    }
}

/// Kill every process in a game's prefix, resolving the default prefix from
/// the game (preferred over [`kill_prefix`]).
pub async fn kill_game_prefix(
    app: &AppSettings,
    settings: &GameSettings,
    game: &GameInfo,
) -> Result<()> {
    let prefix = prefix_path(app, settings, game);
    let wine = if discovery::is_usable(&settings.wine_version) {
        settings.wine_version.clone()
    } else {
        app.game.wine_version.clone()
    };
    prefix::kill(&wine, &prefix).await
}

/// Default prefix path for a game when its settings name none: the prefix
/// another launcher installed it in, else one of its own under the prefixes
/// folder.
pub fn default_prefix(app: &AppSettings, game: &GameInfo) -> PathBuf {
    if let Some(prefix) = game
        .install
        .as_ref()
        .and_then(|i| i.store_prefix.as_deref())
        .filter(|p| !p.trim().is_empty())
    {
        return irori_core::paths::expand_home(prefix.trim());
    }
    let base = if app.default_wine_prefix_dir.is_empty() {
        PathBuf::from(&app.default_install_path).join("Prefixes")
    } else {
        PathBuf::from(&app.default_wine_prefix_dir)
    };
    base.join(irori_core::paths::sanitize(&game.title))
}

/// The folder holding every game's own prefix (`<folder>/<title>`): the
/// prefixes folder of the settings, else `Prefixes` in the install folder.
pub fn prefixes_root(app: &AppSettings) -> PathBuf {
    let dir = app.default_wine_prefix_dir.trim();
    if dir.is_empty() {
        irori_core::paths::expand_home(&app.default_install_path).join("Prefixes")
    } else {
        irori_core::paths::expand_home(dir)
    }
}

/// The prefixes directly inside `root` (folders holding an initialised
/// prefix, flat or Proton's `pfx/`), sorted by name. Reads the folder; call
/// off the UI thread.
pub fn prefixes_in(root: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut found: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir() && is_prefix(p))
        .collect();
    found.sort_by_key(|p| p.file_name().map(|n| n.to_string_lossy().to_lowercase()));
    found
}

/// Every Wine/Proton build usable on this system (system Wine, downloaded
/// builds, Steam Proton, Lutris/Bottles builds, custom paths).
pub async fn wine_installations(app: &AppSettings) -> Vec<WineInstallation> {
    discovery::discover(app)
        .await
        .into_iter()
        .map(|d| d.installation)
        .collect()
}

/// Every usable build, as [`wine_installations`], each with its source when
/// another build has the same name, for pickers to tell them apart.
pub async fn wine_installations_with_sources(
    app: &AppSettings,
) -> Vec<(WineInstallation, Option<discovery::WineSource>)> {
    discovery::sources_where_shared(discovery::discover(app).await)
}

/// Run winetricks for the game's prefix: GUI when `verbs` is empty.
pub async fn winetricks(
    app: &AppSettings,
    settings: &GameSettings,
    target: Target<'_>,
    verbs: Vec<String>,
    ctx: OpContext,
) -> Result<ExecResult> {
    let wc = wine_context(app, settings, target.game, &ctx, false).await?;
    let effective = with_wine(settings, &wc.wine);
    prefix::verify_prefix(app, &effective, &wc, &ctx).await?;
    tools::run_winetricks(app, &effective, &wc, &target.id.app_name, &verbs, &ctx).await
}

/// Whether a path holds a valid initialised prefix.
pub fn is_prefix(path: &Path) -> bool {
    path.join("system.reg").is_file() || path.join("pfx/system.reg").is_file()
}

/// Find the Wine or Proton prefix that encloses `path` (typically a Windows
/// executable inside `drive_c`), if any. Recognises flat Wine/umu prefixes,
/// Proton compatdata directories (`pfx/drive_c` with Proton metadata files),
/// and umu's `pfx -> .` symlink.
pub fn enclosing_wine_prefix(path: &Path) -> Option<PathBuf> {
    for drive_c in path.ancestors() {
        if !drive_c
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.eq_ignore_ascii_case("drive_c"))
        {
            continue;
        }
        let Some(parent) = drive_c.parent() else {
            continue;
        };
        if parent.file_name().and_then(|n| n.to_str()) == Some("pfx")
            && let Some(grandparent) = parent.parent()
        {
            if prefix::prefix_layout(grandparent) == (prefix::PrefixLayout::Flat { pfx_link: true })
            {
                return Some(grandparent.to_path_buf());
            }
            let has_proton_meta = ["version", "pfx.lock", "config_info"]
                .iter()
                .any(|f| grandparent.join(f).is_file());
            if parent.join("system.reg").is_file() && has_proton_meta {
                return Some(grandparent.to_path_buf());
            }
        }
        if parent.join("system.reg").is_file() {
            return Some(parent.to_path_buf());
        }
    }
    None
}

/// Wait until every process in the game's prefix has exited (`wineserver -w`).
/// Used to keep tracking games whose launcher process exits early.
pub async fn wait_prefix(settings: &GameSettings) -> Result<()> {
    match resolve_for_settings(settings) {
        Some((wine, prefix)) => prefix::wait(&wine, &prefix).await,
        None => Ok(()),
    }
}

/// Resolve the prefix path a game uses (settings value, or the default).
pub fn prefix_path(app: &AppSettings, settings: &GameSettings, game: &GameInfo) -> PathBuf {
    let wine = if settings.wine_version.is_set() {
        &settings.wine_version
    } else {
        &app.game.wine_version
    };
    if cfg!(target_os = "macos") && wine.kind == WineKind::Crossover {
        let bottle = settings.wine_crossover_bottle.trim();
        if !bottle.is_empty() && !bottle.contains(['/', '\\']) && bottle != "." && bottle != ".." {
            return locations::crossover_bottles_dir(app).join(bottle);
        }
    }
    if settings.wine_prefix.is_empty() {
        default_prefix(app, game)
    } else {
        irori_core::paths::expand_home(&settings.wine_prefix)
    }
}

/// The prefix to suggest when installing: the game's saved prefix while it
/// still exists, else the one the defaults give it. A prefix deleted since
/// (by hand, or a cleanup) would otherwise be offered again.
pub fn install_prefix(app: &AppSettings, settings: &GameSettings, game: &GameInfo) -> PathBuf {
    let saved = prefix_path(app, settings, game);
    if settings.wine_prefix.is_empty() || saved.exists() {
        return saved;
    }
    let defaults = GameSettings {
        wine_version: settings.wine_version.clone(),
        wine_crossover_bottle: settings.wine_crossover_bottle.clone(),
        wine_prefix: app.game.wine_prefix.clone(),
        ..GameSettings::default()
    };
    prefix_path(app, &defaults, game)
}

/// Whether `prefix` is shared by several games: the root that holds every
/// game's own prefix, the prefix set for all games in the defaults, or the
/// prefix of another launcher (the linked Epic Games Launcher's, the one
/// `game` was installed in). Deleting it with one game would take the others'
/// saves, or the launcher, with it.
pub fn prefix_is_shared(app: &AppSettings, game: &GameInfo, prefix: &Path) -> bool {
    let same = |setting: &str| {
        !setting.trim().is_empty() && irori_core::paths::expand_home(setting.trim()) == prefix
    };
    let store_prefix = game
        .install
        .as_ref()
        .and_then(|i| i.store_prefix.as_deref())
        .unwrap_or_default();
    same(&app.default_wine_prefix_dir)
        || same(&app.game.wine_prefix)
        || same(&app.egs_linked_path)
        || same(store_prefix)
}

/// Install (`enabled`) or remove a DLL tool in the game's prefix (the DXVK /
/// VKD3D / NVAPI toggles). Only Wine prefixes are affected; Proton ships its
/// own.
pub async fn set_dx_tool(
    app: &AppSettings,
    settings: &GameSettings,
    target: Target<'_>,
    tool: DxTool,
    enabled: bool,
    ctx: OpContext,
) -> Result<()> {
    anyhow::ensure!(
        cfg!(target_os = "linux"),
        "DXVK management is available on Linux. macOS runtimes manage their own graphics libraries."
    );
    let wc = wine_context(app, settings, target.game, &ctx, false).await?;
    let effective = with_wine(settings, &wc.wine);
    prefix::verify_prefix(app, &effective, &wc, &ctx).await?;
    if enabled {
        tools::install_tools(app, &effective, &wc, &[tool], &ctx).await
    } else {
        tools::remove_tool(app, &effective, &wc, tool, &ctx).await
    }
}

/// Open `winecfg` for the game's prefix.
pub async fn winecfg(
    app: &AppSettings,
    settings: &GameSettings,
    target: Target<'_>,
    ctx: OpContext,
) -> Result<ExecResult> {
    run_in_prefix(app, settings, target, vec!["winecfg".into()], false, ctx).await
}

/// Open the registry editor for the game's prefix.
pub async fn regedit(
    app: &AppSettings,
    settings: &GameSettings,
    target: Target<'_>,
    ctx: OpContext,
) -> Result<ExecResult> {
    run_in_prefix(app, settings, target, vec!["regedit".into()], false, ctx).await
}

/// Start a Windows executable in the game's prefix (from its own folder).
pub async fn run_exe(
    app: &AppSettings,
    settings: &GameSettings,
    target: Target<'_>,
    exe: &Path,
    ctx: OpContext,
) -> Result<ExecResult> {
    run_in_prefix(
        app,
        settings,
        target,
        vec![exe.to_string_lossy().into_owned()],
        false,
        ctx,
    )
    .await
}

/// Open the game's prefix folder in the file manager.
pub fn open_prefix_folder(
    app: &AppSettings,
    settings: &GameSettings,
    game: &GameInfo,
) -> Result<()> {
    let prefix = prefix_path(app, settings, game);
    let wc = WineContext::new(settings.wine_version.clone(), prefix, false, PathBuf::new());
    tools::open_prefix_folder(&wc)
}

#[cfg(test)]
mod tests {
    use super::*;
    use irori_core::Runner;

    fn game() -> GameInfo {
        GameInfo {
            app_name: "app".into(),
            title: "My: Game".into(),
            install: Some(irori_core::InstalledInfo {
                install_path: "/games/my".into(),
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    #[test]
    fn knows_shared_prefixes() {
        let root = irori_core::paths::expand_home("~/Games/Prefixes");
        let mut app = AppSettings {
            default_wine_prefix_dir: "~/Games/Prefixes".into(),
            ..Default::default()
        };
        assert!(prefix_is_shared(&app, &game(), &root));
        assert!(!prefix_is_shared(&app, &game(), &root.join("My Game")));
        app.game.wine_prefix = "/data/shared".into();
        assert!(prefix_is_shared(&app, &game(), Path::new("/data/shared")));
        app.game.wine_prefix.clear();
        assert!(
            !prefix_is_shared(&app, &game(), Path::new("")),
            "no setting, no match"
        );
    }

    #[test]
    fn prefixes() {
        let app = AppSettings {
            default_wine_prefix_dir: "/p".into(),
            ..Default::default()
        };
        let mut gs = GameSettings::default();
        assert_eq!(
            prefix_path(&app, &gs, &game()),
            PathBuf::from("/p/My_ Game")
        );
        gs.wine_prefix = "/x/y".into();
        assert_eq!(prefix_path(&app, &gs, &game()), PathBuf::from("/x/y"));
    }

    /// Heroic #6001: a game imported from the Epic Games Launcher's prefix
    /// plays there unless the user chose a prefix, and that prefix (EGL's
    /// own) is never deleted with the game.
    #[test]
    fn games_from_another_launcher_use_its_prefix() {
        let mut app = AppSettings {
            default_wine_prefix_dir: "/p".into(),
            ..Default::default()
        };
        let mut imported = game();
        if let Some(install) = imported.install.as_mut() {
            install.store_prefix = Some("/egl".into());
        }
        let mut gs = GameSettings::default();
        assert_eq!(default_prefix(&app, &imported), PathBuf::from("/egl"));
        assert_eq!(prefix_path(&app, &gs, &imported), PathBuf::from("/egl"));
        assert!(prefix_is_shared(&app, &imported, Path::new("/egl")));
        assert!(!prefix_is_shared(&app, &game(), Path::new("/egl")));
        app.egs_linked_path = " /egl ".into();
        assert!(prefix_is_shared(&app, &game(), Path::new("/egl")));
        gs.wine_prefix = "/mine".into();
        assert_eq!(prefix_path(&app, &gs, &imported), PathBuf::from("/mine"));
    }

    #[test]
    fn lists_the_prefixes_in_the_prefixes_folder() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("zeta")).unwrap();
        std::fs::write(root.join("zeta/system.reg"), "").unwrap();
        std::fs::create_dir_all(root.join("Alpha/pfx")).unwrap();
        std::fs::write(root.join("Alpha/pfx/system.reg"), "").unwrap();
        // Not prefixes: an empty folder and a file.
        std::fs::create_dir_all(root.join("empty")).unwrap();
        std::fs::write(root.join("notes.txt"), "").unwrap();
        assert_eq!(
            prefixes_in(root),
            vec![root.join("Alpha"), root.join("zeta")]
        );
        assert!(prefixes_in(&root.join("missing")).is_empty());

        let app = AppSettings {
            default_wine_prefix_dir: " /p ".into(),
            ..Default::default()
        };
        assert_eq!(prefixes_root(&app), PathBuf::from("/p"));
        let app = AppSettings {
            default_install_path: "/games".into(),
            default_wine_prefix_dir: String::new(),
            ..Default::default()
        };
        assert_eq!(prefixes_root(&app), PathBuf::from("/games/Prefixes"));
    }

    #[test]
    fn enclosing_wine_prefix_detects_flat_proton_and_umu_prefixes() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();

        // Flat Wine prefix.
        let wine = root.join("wine");
        let wine_exe = wine.join("drive_c/Program Files/App/app.exe");
        std::fs::create_dir_all(wine_exe.parent().unwrap()).unwrap();
        std::fs::write(wine.join("system.reg"), "").unwrap();
        std::fs::write(&wine_exe, b"MZ").unwrap();
        assert_eq!(enclosing_wine_prefix(&wine_exe), Some(wine.clone()));

        // Direct Proton prefix (`<dir>/pfx/drive_c/...` with Proton metadata).
        let proton = root.join("proton");
        let proton_exe = proton.join("pfx/drive_c/Program Files/App/app.exe");
        std::fs::create_dir_all(proton_exe.parent().unwrap()).unwrap();
        std::fs::write(proton.join("pfx/system.reg"), "").unwrap();
        std::fs::write(proton.join("version"), "9.0").unwrap();
        std::fs::write(&proton_exe, b"MZ").unwrap();
        assert_eq!(enclosing_wine_prefix(&proton_exe), Some(proton.clone()));

        // Flat Wine prefix literally named `pfx` (no Proton metadata in grandparent).
        let literal_pfx = root.join("holder/pfx");
        let literal_exe = literal_pfx.join("drive_c/game.exe");
        std::fs::create_dir_all(literal_exe.parent().unwrap()).unwrap();
        std::fs::write(literal_pfx.join("system.reg"), "").unwrap();
        std::fs::write(&literal_exe, b"MZ").unwrap();
        assert_eq!(
            enclosing_wine_prefix(&literal_exe),
            Some(literal_pfx.clone())
        );

        // Plain folder containing `drive_c` without `system.reg` is not a prefix.
        let not_prefix = root.join("plain");
        let plain_exe = not_prefix.join("drive_c/game.exe");
        std::fs::create_dir_all(plain_exe.parent().unwrap()).unwrap();
        std::fs::write(&plain_exe, b"MZ").unwrap();
        assert_eq!(enclosing_wine_prefix(&plain_exe), None);

        // umu symlink (`<dir>/pfx -> <dir>`).
        #[cfg(unix)]
        {
            let umu = root.join("umu");
            std::fs::create_dir_all(umu.join("drive_c")).unwrap();
            std::fs::write(umu.join("system.reg"), "").unwrap();
            std::fs::write(umu.join("drive_c/game.exe"), b"MZ").unwrap();
            std::os::unix::fs::symlink(&umu, umu.join("pfx")).unwrap();
            assert_eq!(
                enclosing_wine_prefix(&umu.join("pfx/drive_c/game.exe")),
                Some(umu.clone())
            );
        }
    }

    #[test]
    fn a_deleted_prefix_is_not_suggested_again() {
        let dir = tempfile::tempdir().unwrap();
        let app = AppSettings {
            default_wine_prefix_dir: dir.path().display().to_string(),
            ..Default::default()
        };
        let mut gs = GameSettings {
            wine_prefix: dir.path().join("old").display().to_string(),
            ..Default::default()
        };
        assert_eq!(
            install_prefix(&app, &gs, &game()),
            dir.path().join("My_ Game")
        );
        std::fs::create_dir(dir.path().join("old")).unwrap();
        assert_eq!(install_prefix(&app, &gs, &game()), dir.path().join("old"));
        gs.wine_prefix.clear();
        assert_eq!(
            install_prefix(&app, &gs, &game()),
            dir.path().join("My_ Game")
        );
    }

    #[test]
    fn crossover_bottles_can_live_elsewhere() {
        let mut app = AppSettings::default();
        assert!(
            locations::crossover_bottles_dir(&app)
                .ends_with("Library/Application Support/CrossOver/Bottles")
        );
        app.crossover_bottles_path = " /Volumes/Games/Bottles ".into();
        assert_eq!(
            locations::crossover_bottles_dir(&app),
            PathBuf::from("/Volumes/Games/Bottles")
        );
        if cfg!(target_os = "macos") {
            let gs = GameSettings {
                wine_version: WineInstallation {
                    bin: "/Applications/CrossOver.app/wine".into(),
                    name: "CrossOver".into(),
                    kind: WineKind::Crossover,
                    ..Default::default()
                },
                wine_crossover_bottle: "Steam".into(),
                ..Default::default()
            };
            assert_eq!(
                prefix_path(&app, &gs, &game()),
                PathBuf::from("/Volumes/Games/Bottles/Steam")
            );
        }
    }

    #[tokio::test]
    async fn native_launch_environment() {
        let id = GameId::new(Runner::Gog, "app");
        let g = game();
        let gs = GameSettings {
            wrapper_options: vec![irori_core::Wrapper {
                exe: "env".into(),
                args: "-u X".into(),
            }],
            environment_options: vec![irori_core::EnvVar {
                key: "A".into(),
                value: "1".into(),
            }],
            ..Default::default()
        };
        let target = Target {
            id: &id,
            game: &g,
            platform: irori_core::HostPlatform::current().native(),
        };
        let le = prepare_launch(&AppSettings::default(), &gs, target, &OpContext::detached())
            .await
            .unwrap();
        assert!(le.wine.is_none());
        assert_eq!(le.wrappers, vec![vec!["env", "-u", "X"]]);
        assert_eq!(le.env["A"], "1");
        assert_eq!(le.env["HEROIC_APP_RUNNER"], "gog");
        if cfg!(target_os = "linux") {
            assert_eq!(le.env["STEAM_COMPAT_INSTALL_PATH"], "/games/my");
        }
        assert!(!le.env.contains_key("WINEPREFIX"));
    }

    #[tokio::test]
    async fn the_preview_shows_what_the_launch_gets() {
        let id = GameId::new(Runner::Gog, "app");
        let g = game();
        let gs = GameSettings {
            wrapper_options: vec![irori_core::Wrapper {
                exe: "env".into(),
                args: "-u X".into(),
            }],
            environment_options: vec![irori_core::EnvVar {
                key: "TOKEN".into(),
                value: "s3cret".into(),
            }],
            ..Default::default()
        };
        let target = Target {
            id: &id,
            game: &g,
            platform: irori_core::HostPlatform::current().native(),
        };
        let app = AppSettings::default();
        let launch = prepare_launch(&app, &gs, target, &OpContext::detached())
            .await
            .unwrap();
        let preview = preview_launch(&app, &gs, target).await.unwrap();
        let launch_env: Vec<(String, String)> = launch.env.into_iter().collect();
        assert_eq!(preview.env, launch_env);
        assert_eq!(preview.wrappers, launch.wrappers);
        assert!(preview.runner.is_empty() && preview.prefix.is_none());
        assert!(!preview.sandboxed);
    }

    // On Windows every build starts without Wine; the script is a Unix one.
    #[cfg(unix)]
    #[tokio::test]
    async fn the_preview_of_a_build_started_without_wine_matches_its_launch() {
        // A Windows build whose program is a native script starts it
        // directly: no Wine layer in either.
        let dir = std::env::temp_dir().join(format!("irori-no-wine-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("start.sh"), b"#!/bin/sh\nexec true\n").unwrap();
        let id = GameId::new(Runner::Sideload, "native");
        let g = GameInfo {
            app_name: "native".into(),
            title: "Native".into(),
            install: Some(irori_core::InstalledInfo {
                install_path: dir.to_string_lossy().into_owned(),
                executable: "start.sh".into(),
                ..Default::default()
            }),
            ..Default::default()
        };
        let target = Target {
            id: &id,
            game: &g,
            platform: InstallPlatform::Windows,
        };
        let app = AppSettings::default();
        let gs = GameSettings::default();
        let launch = prepare_launch(&app, &gs, target, &OpContext::detached())
            .await
            .unwrap();
        let preview = preview_launch(&app, &gs, target).await.unwrap();
        assert!(launch.without_wine && launch.wine.is_none());
        assert!(preview.without_wine && preview.runner.is_empty());
        assert_eq!(preview.env, launch.env.into_iter().collect::<Vec<_>>());
        assert_eq!(preview.wrappers, launch.wrappers);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn metalfx_is_planned_as_it_is_applied() {
        let wine = WineInstallation {
            bin: "/nowhere/bin/wine".into(),
            ..Default::default()
        };
        assert_eq!(metalfx::planned(&wine, false), metalfx::Applied::default());
        let asked = metalfx::planned(&wine, true);
        assert!(asked.switch.is_none() && asked.unavailable.is_some());
    }

    fn facts(wine: Option<WineContext>) -> LaunchFacts {
        LaunchFacts {
            host: HostEnv::from_pairs([("HOME", "/home/me")]),
            settings: GameSettings {
                environment_options: vec![
                    irori_core::EnvVar {
                        key: "SHARED".into(),
                        value: "user".into(),
                    },
                    irori_core::EnvVar {
                        key: "WINEDLLOVERRIDES".into(),
                        value: "dinput8=n,b".into(),
                    },
                ],
                ..Default::default()
            },
            limit: None,
            mango: MangoUse::default(),
            hud_plan: Default::default(),
            parts: WrapperParts::default(),
            graphics: [("SHARED".to_string(), "graphics".to_string())].into(),
            fixes: [
                ("SHARED".to_string(), "fix".to_string()),
                ("FIXED".to_string(), "1".to_string()),
            ]
            .into(),
            runtime: None,
            wine,
            umu_game_id: Some("umu-123".into()),
            without_wine: false,
            appimage: None,
            metalfx: metalfx::Applied::default(),
        }
    }

    #[test]
    fn composes_variables_in_the_launch_s_order() {
        let id = GameId::new(Runner::Gog, "app");
        let g = game();
        let target = Target {
            id: &id,
            game: &g,
            platform: InstallPlatform::Linux,
        };
        let app = AppSettings::default();
        // Fixes come after the graphics options and the user's variables.
        let native = compose_env(&app, target, &facts(None));
        assert_eq!(native["SHARED"], "fix");
        assert_eq!(native["FIXED"], "1");
        assert_eq!(native["HEROIC_APP_RUNNER"], "gog");
        assert!(!native.contains_key("WINEPREFIX"));

        let wine = |uses_umu| {
            WineContext::new(
                WineInstallation {
                    bin: "/w/proton".into(),
                    name: "Proton".into(),
                    kind: WineKind::Proton,
                    ..Default::default()
                },
                PathBuf::from("/prefixes/app"),
                uses_umu,
                PathBuf::from("/usr/bin/umu-run"),
            )
        };
        let windows = Target {
            platform: InstallPlatform::Windows,
            ..target
        };
        let direct = compose_env(&app, windows, &facts(Some(wine(false))));
        assert_eq!(direct["FIXED"], "1");
        // The database's GAMEID only goes to umu.
        assert_ne!(direct.get("GAMEID").map(String::as_str), Some("umu-123"));
        let umu = compose_env(&app, windows, &facts(Some(wine(true))));
        assert_eq!(umu["GAMEID"], "umu-123");

        // DLSS through MetalFX: the build's switch and its DLLs after the
        // user's own overrides.
        let mut mac = facts(Some(wine(false)));
        mac.metalfx = metalfx::Applied {
            switch: Some("D3DM_ENABLE_METALFX"),
            unavailable: None,
        };
        let with_metalfx = compose_env(&app, windows, &mac);
        assert_eq!(with_metalfx["D3DM_ENABLE_METALFX"], "1");
        let overrides = &with_metalfx["WINEDLLOVERRIDES"];
        assert!(overrides.contains("nvngx=n") && overrides.contains("dinput8=n,b"));

        // An AppImage FUSE can't mount unpacks itself, unless the game's
        // variables say otherwise.
        let mut appimage = facts(None);
        appimage.appimage = Some(("APPIMAGE_EXTRACT_AND_RUN".into(), "1".into()));
        assert_eq!(
            compose_env(&app, target, &appimage)["APPIMAGE_EXTRACT_AND_RUN"],
            "1"
        );
        appimage
            .settings
            .environment_options
            .push(irori_core::EnvVar {
                key: "APPIMAGE_EXTRACT_AND_RUN".into(),
                value: "0".into(),
            });
        assert_eq!(
            compose_env(&app, target, &appimage)["APPIMAGE_EXTRACT_AND_RUN"],
            "0"
        );
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn missing_gamemode_fails() {
        if which::which("gamemoderun").is_ok() {
            return;
        }
        let gs = GameSettings {
            use_game_mode: true,
            ..Default::default()
        };
        let err = wrapper_parts(&gs, &hud::launch_env::HudLaunchPlan::default(), false)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("gamemoderun"));
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn missing_gamescope_fails() {
        if which::which("gamescope").is_ok() {
            return;
        }
        let gs = GameSettings {
            gamescope: irori_core::GamescopeSettings {
                enabled: Some(true),
                ..Default::default()
            },
            ..Default::default()
        };
        let err = wrapper_parts(&gs, &hud::launch_env::HudLaunchPlan::default(), false)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("gamescope"));
    }

    #[tokio::test]
    async fn native_launch_applies_disabled_hud_env_and_removals() {
        let id = GameId::new(Runner::Gog, "app");
        let g = game();
        let mut gs = GameSettings::default();
        gs.game_hud.mode = irori_core::GameHudMode::Disabled;
        gs.environment_options.push(irori_core::EnvVar {
            key: "MANGOHUD_CONFIG".into(),
            value: "fps".into(),
        });
        let target = Target {
            id: &id,
            game: &g,
            platform: irori_core::HostPlatform::current().native(),
        };
        let le = prepare_launch(&AppSettings::default(), &gs, target, &OpContext::detached())
            .await
            .unwrap();
        if cfg!(target_os = "linux") {
            assert_eq!(le.env["DISABLE_MANGOHUD"], "1");
            assert_eq!(le.env["MANGOHUD"], "0");
            assert!(!le.env.contains_key("MANGOHUD_CONFIG"));
            assert!(le.env_remove.contains(&"MANGOHUD_CONFIG".into()));
            assert!(le.env_remove.contains(&"MANGOHUD_CONFIGFILE".into()));
        } else if cfg!(target_os = "macos") {
            assert_eq!(le.env["MTL_HUD_ENABLED"], "0");
            assert!(le.env_remove.contains(&"MTL_HUD_ALIGNMENT".into()));
        }
    }

    #[test]
    fn settings_lookup_uses_recorded_prefix() {
        let gs = GameSettings {
            launcher_args: "unique-for-test".into(),
            ..Default::default()
        };
        assert!(resolve_for_settings(&gs).is_none());
        let wine = WineInstallation {
            bin: "/w/bin/wine".into(),
            ..Default::default()
        };
        ACTIVE
            .lock()
            .insert(fingerprint(&gs), (wine.clone(), "/pfx/a".into()));
        let (w, p) = resolve_for_settings(&gs).unwrap();
        assert_eq!(w, wine);
        assert_eq!(p, PathBuf::from("/pfx/a"));
    }
}
