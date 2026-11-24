#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]
//! Irori: a fast, native game launcher for Linux.

mod actions;
mod app_update;
mod autostart;
mod cli;
mod crash;
mod export;
mod headless;
mod import;
mod instance;
mod launcher_sync;
mod lutris;
#[cfg(not(target_os = "linux"))]
mod menu_theme;
mod pages;
mod palette;
mod placement;
mod playing;
mod playnite;
mod quick_launch;
mod routing;
mod shell;
mod sidebar;
mod stores;
mod strings;
mod title_bar;
#[cfg(target_os = "linux")]
mod tray;
#[cfg(not(target_os = "linux"))]
#[path = "tray_native.rs"]
mod tray;
mod tray_hide;
mod waiting;

use std::sync::Arc;

use anyhow::Result;
use futures::StreamExt as _;
use futures::channel::mpsc;
use gpui_kit::component::WindowExt as _;
use gpui_kit::component::notification::Notification;
use gpui_kit::component::{Root, TitleBar};
use gpui_kit::{
    AnyWindowHandle, App, AppContext as _, Focusable as _, QuitMode, WindowDecorations,
    WindowHandle, WindowOptions, px, size,
};
use irori_core::{GameId, Paths};
use irori_services::Backend;
use irori_ui::i18n::{t, t_args};
use irori_ui::{Route, models::models, nav};
use parking_lot::Mutex;

use crate::cli::Cli;
use crate::shell::AppShell;

/// Requests from outside the GPUI thread (tray, second instances).
#[derive(Debug, Clone)]
pub enum ShellCommand {
    Show,
    Quit,
    Launch(GameId),
    BigPicture,
    Args(Vec<String>),
    /// Arguments of an invocation that waits for the session it asks for.
    Wait(Vec<String>, Arc<instance::Waiter>),
    /// The program waiting for Big Picture went away.
    LeaveBigPicture,
    /// Save a clip of the replay buffer.
    SaveClip,
    /// Open the quick launcher, or close it (its global shortcut).
    QuickLaunch,
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    // Started by the login dialog to show a store's sign-in page.
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    if args.first().map(String::as_str) == Some(irori_sign_in::SIGN_IN_FLAG) {
        std::process::exit(i32::from(irori_sign_in::main(args.into_iter().skip(1))));
    }
    // Started as administrator to hide a graphics card from a game
    // (`irori_services::gpu_hide`): it does that and nothing else.
    #[cfg(target_os = "windows")]
    if args.first().map(String::as_str) == Some(irori_services::gpu_hide::HELPER_FLAG) {
        std::process::exit(irori_services::gpu_hide::helper_main(
            args.into_iter().skip(1),
        ));
    }
    // Started by "Restart to update" to put the downloaded update in once
    // the Irori that started it has quit (`irori_services::self_update`).
    if args.first().map(String::as_str) == Some(irori_services::self_update::HELPER_FLAG) {
        std::process::exit(irori_services::self_update::helper_main(
            args.into_iter().skip(1),
        ));
    }
    irori_secrets::use_os_keyring();
    // A renamed Mac account leaves the old home's paths in saved files.
    irori_core::home_rename::enable();
    let args = match std::env::current_dir() {
        Ok(cwd) => cli::absolute_file_args(args, &cwd),
        Err(_) => args,
    };
    let cli = Cli::parse(args.clone());
    if cli.help {
        println!("{}\n\n{}", cli::HELP, headless::HELP);
        return Ok(());
    }
    // A bare word is a command; never fall through to opening the window
    // for a mistyped one.
    if let Some(first) = args
        .first()
        .filter(|a| !a.starts_with('-') && !cli::is_link_arg(a) && !cli::is_windows_file_arg(a))
        && !headless::COMMANDS.contains(&first.as_str())
    {
        eprintln!("irori: unknown command '{first}'\n\n{}", headless::HELP);
        std::process::exit(2);
    }
    if args
        .first()
        .is_some_and(|a| headless::COMMANDS.contains(&a.as_str()))
    {
        init_logging(false);
        let code = headless::run(&args)?;
        std::process::exit(code);
    }
    if cli.version {
        println!("irori {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    if cli.wait {
        std::process::exit(waiting::run(&args));
    }
    // A key binding in the user's compositor: only a running Irori has a
    // replay buffer or a game to take a picture of.
    if cli.capture() {
        if instance::forward(&args)? {
            return Ok(());
        }
        eprintln!("irori: Irori isn't running, so there is nothing to capture");
        std::process::exit(1);
    }

    let Some(listener) = instance::acquire(&args)? else {
        // Forwarded to the running instance.
        return Ok(());
    };
    init_logging(true);
    crash::install();
    tracing::info!("Irori {} starting", env!("CARGO_PKG_VERSION"));
    limit_malloc_arenas();

    let backend = Backend::start(stores::build)?;
    crash::notice_last_crash(&backend.bus);
    let (tx, rx) = mpsc::unbounded::<ShellCommand>();
    {
        let tx = tx.clone();
        instance::serve(listener, move |args, waiter| {
            let command = match waiter {
                Some(waiter) => ShellCommand::Wait(args, Arc::new(waiter)),
                None => ShellCommand::Args(args),
            };
            let _ = tx.unbounded_send(command);
        })?;
    }

    let application = gpui_kit::application().with_assets(gpui_kit::assets::AllAssets);
    let url_tx = tx.clone();
    application.on_open_urls(move |urls| {
        let _ = url_tx.unbounded_send(ShellCommand::Args(urls));
    });
    let reopen_tx = tx.clone();
    application.on_reopen(move |_| {
        let _ = reopen_tx.unbounded_send(ShellCommand::Show);
    });
    application.run(move |cx| {
        gpui_kit::init(cx);
        irori_ui::init(backend.clone(), cx);
        irori_ui::i18n::register(strings::CATALOGS);
        irori_ui::hooks::install(stores::hooks(), cx);
        irori_ui::art_picker::install(irori_library::open_art_picker_dialog, cx);
        irori_bigpicture::init(cx);
        irori_library::init(cx);
        irori_manage::init(cx);
        irori_settings::init(cx);
        irori_game::init(cx);
        irori_game::install_hooks(stores::game_hooks(), cx);
        let epic = stores::EPIC.get().map(|epic| epic.main());
        irori_settings::install_store_handles(epic.clone(), stores::STEAM.get().cloned(), cx);
        irori_game::install_store_handles(epic, stores::GOG.get().cloned(), cx);
        if let Some(steam) = stores::STEAM.get() {
            irori_manage::install_steam_store(steam.clone(), cx);
            irori_manage::install_steam_library_store(steam.clone(), cx);
        }
        #[cfg(target_os = "linux")]
        if let Some(xbox) = stores::XBOX.get() {
            irori_manage::install_xbox_store(xbox.clone(), cx);
        }
        if let Some(sideload) = stores::SIDELOAD.get() {
            irori_library::install_sideload_store(sideload.clone(), cx);
            if let Some(launchers) = stores::LAUNCHERS.get() {
                irori_manage::install_launchers_store(launchers.clone(), cx);
            }
            if let Some(emulation) = stores::EMULATION.get() {
                irori_library::install_emulation_store(emulation.clone(), cx);
                irori_settings::install_emulation_store(emulation.clone(), cx);
            }
        }
        actions::bind_keys(cx);
        cx.set_quit_mode(QuitMode::Explicit);
        let prefs = backend.settings.app().ui.clone();
        irori_ui::theme::apply(&prefs, None, cx);

        launcher_sync::start(backend.clone());
        let app = Arc::new(Mutex::new(AppHandles::default()));
        start_tray(&backend, tx.clone(), app.clone(), cx);
        tray_hide::install(app.clone(), cx);
        {
            let (show, window, launch) = (app.clone(), app.clone(), app.clone());
            quick_launch::install(
                quick_launch::ShellAccess {
                    show: std::rc::Rc::new(move |cx| open_main_window(&show, cx)),
                    window: std::rc::Rc::new(move || window.lock().window.map(Into::into)),
                    launch: std::rc::Rc::new(move |id, cx| launch_from_outside(id, &launch, cx)),
                },
                cx,
            );
            let tx = tx.clone();
            quick_launch::follow_shortcut(
                move || {
                    let _ = tx.unbounded_send(ShellCommand::QuickLaunch);
                },
                cx,
            );
        }
        {
            // The in-game overlay's "Back to Irori".
            let app = app.clone();
            irori_bigpicture::overlay::install_show_irori(move |cx| open_main_window(&app, cx), cx);
        }
        {
            let app = app.clone();
            irori_ui::system_notify::install(
                move |route, cx| {
                    open_main_window(&app, cx);
                    if let Some(route) = route {
                        nav::navigate(route, cx);
                    }
                },
                cx,
            );
        }

        // Pre-warm Linux HUD hardware detection in the background so opening
        // Performance HUD settings shows real GPU/CPU names on the first frame.
        #[cfg(target_os = "linux")]
        backend.spawn(async {
            let _ = tokio::task::spawn_blocking(
                irori_runtime::hud::hardware_linux::detect_linux_hud_env,
            )
            .await;
        });

        // Steam's Game Mode has no tray to come back from. A game launched
        // with no window asked for none: the game is what shows, and
        // starting Irori again brings the window.
        // The quick launcher alone opens no main window either, while a
        // tray brings it back.
        let can_hide = !irori_core::platform::in_steam_gamepad_ui()
            && ((cfg!(target_os = "linux") && !backend.settings.app().no_tray_icon)
                || app.lock().tray.is_some());
        let start_hidden = cli.launch_without_window()
            || (can_hide
                && (((cli.minimized || backend.settings.app().start_in_tray)
                    && !cli.has_request())
                    || cli.quick_launch_only()
                    || cli.open_launcher_only()));
        if !start_hidden {
            open_main_window(&app, cx);
            let app_settings = backend.settings.app();
            let game_mode = app_settings.ui.big_picture_in_game_mode
                && irori_core::platform::in_steam_gamepad_ui();
            if app_settings.start_in_console_mode || game_mode {
                nav::navigate(Route::BigPicture, cx);
            }
        }
        handle_cli(&cli, &app, cx);
        listen(rx, tx.clone(), app.clone(), cx);
        install_quit_on_close(app, cx);
    });
    instance::release();
    Ok(())
}

/// glibc gives every thread that allocates concurrently its own arena (up to
/// 8 × cores) and rarely returns their memory; image decoding on the worker
/// threads would otherwise leave tens of MiB stranded in each.
fn limit_malloc_arenas() {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    // SAFETY: mallopt only adjusts allocator tunables; called before other threads allocate.
    unsafe {
        libc::mallopt(libc::M_ARENA_MAX, 2);
    }
}

#[derive(Default)]
struct AppHandles {
    window: Option<WindowHandle<Root>>,
    tray: Option<tray::TrayHandle>,
    /// The window closed to the tray while these games play.
    hidden: Option<tray_hide::Hidden>,
}

/// Log to stderr and the launcher log file. Headless commands (`gui = false`)
/// default to warnings only so their output stays readable.
fn init_logging(gui: bool) {
    use tracing_subscriber::prelude::*;
    let default = if gui { "info" } else { "warn" };
    let filter = log_filter(default);
    restrict_logs_dir();
    let file = gui
        .then(|| {
            let path = Paths::get().launcher_log_file();
            let _ = std::fs::create_dir_all(path.parent().unwrap_or(&path));
            // Keep the previous session's log for bug reports.
            let _ = std::fs::rename(&path, path.with_extension("log.old"));
            std::fs::File::create(&path).ok()
        })
        .flatten();
    let registry = tracing_subscriber::registry()
        .with(filter)
        .with(tracing_subscriber::fmt::layer().with_writer(std::io::stderr));
    match file {
        Some(f) => registry
            .with(
                tracing_subscriber::fmt::layer()
                    .with_ansi(false)
                    .with_writer(std::sync::Mutex::new(f)),
            )
            .init(),
        None => registry.init(),
    }
}

fn log_filter(default: &str) -> tracing_subscriber::EnvFilter {
    tracing_subscriber::EnvFilter::try_from_env("IRORI_LOG")
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(default))
        // comet logs network errors with their address, and the address of a
        // token refresh holds the GOG refresh token. Logs get shared.
        .add_directive("comet=off".parse().expect("valid directive"))
}

/// Logs can hold paths and account names: only the user may enter the folder.
fn restrict_logs_dir() {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = Paths::get().logs_dir();
        let _ = std::fs::create_dir_all(&dir);
        if let Err(e) = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)) {
            eprintln!("restricting {}: {e}", dir.display());
        }
    }
}

fn open_main_window(app: &Arc<Mutex<AppHandles>>, cx: &mut App) {
    #[cfg(target_os = "macos")]
    cx.activate(true);
    // Opened for any reason, the window is no longer waiting in the tray
    // for a game to end.
    app.lock().hidden = None;
    let existing = app.lock().window;
    if let Some(handle) = existing
        && handle
            .update(cx, |_, window, cx| {
                // Windows activates by simulating an Alt press, which an
                // active window takes as a press of its own: it waits in menu
                // mode and stops painting until the next input.
                if !window.is_window_active() {
                    window.activate_window();
                    irori_ui::state::run_infallible(cx, irori_services::power::activate_window())
                        .detach();
                }
            })
            .is_ok()
    {
        return;
    }
    let prefs = models(cx).settings.read(cx).app.ui.clone();
    let route = shell::initial_route(cx);
    let (bounds, display_id) = placement::open_bounds(&prefs, cx);
    let options = WindowOptions {
        window_bounds: Some(bounds),
        display_id,
        window_min_size: Some(size(px(800.), px(540.))),
        app_id: Some("irori".into()),
        // Irori draws its own title bar; ask Wayland compositors not to add one.
        window_decorations: Some(WindowDecorations::Client),
        // The backdrop from the first frame, so a translucent window never
        // flashes opaque. The shell keeps it in step from here on.
        window_background: irori_ui::glass::window_background(shell::initial_glass(&route, cx)),
        ..TitleBar::window_options()
    };
    nav::navigator(cx).update(cx, |n, cx| n.replace(route, cx));
    match gpui_kit::open_window(options, cx, |window, cx| {
        let shell = cx.new(|cx| AppShell::new(window, cx));
        let focus = shell.read(cx).focus_handle(cx);
        focus.focus(window, cx);
        shell
    }) {
        Ok((handle, _)) => {
            app.lock().window = handle.downcast::<Root>();
        }
        Err(e) => tracing::error!("opening window: {e:#}"),
    }
}

fn handle_cli(cli: &Cli, app: &Arc<Mutex<AppHandles>>, cx: &mut App) {
    if cli.quick_launch {
        quick_launch::toggle(cx);
    }
    if cli.save_clip {
        irori_ui::backend(cx).save_clip();
    }
    if cli.screenshot {
        let backend = irori_ui::backend(cx);
        backend.take_screenshot(backend.running_games().into_iter().next());
    }
    if let Some(id) = &cli.open {
        open_main_window(app, cx);
        nav::navigate(Route::Game(id.clone()), cx);
    }
    if let Some(route) = &cli.route {
        open_main_window(app, cx);
        nav::navigate(route.clone(), cx);
    }
    if cli.big_picture {
        open_main_window(app, cx);
        nav::navigate(Route::BigPicture, cx);
    }
    if let Some(slug) = &cli.lutris {
        open_main_window(app, cx);
        let window = app.lock().window;
        if let Some(handle) = window {
            let slug = slug.clone();
            // The dialog lives on the window's root, which `update` holds:
            // open it once this update is over.
            let _ = handle.update(cx, |_, window, cx| {
                window.defer(cx, move |window, cx| {
                    irori_library::open_lutris_installer(Some(slug), window, cx)
                })
            });
        }
    }
    if let Some(link) = &cli.nxm {
        open_main_window(app, cx);
        let window = app.lock().window;
        if let Some(handle) = window {
            let link = link.clone();
            let _ = handle.update(cx, |_, window, cx| {
                window.defer(cx, move |window, cx| {
                    irori_game::open_nxm_dialog(&link, window, cx)
                })
            });
        }
    }
    if let Some(link) = &cli.thunderstore {
        open_main_window(app, cx);
        let window = app.lock().window;
        if let Some(handle) = window {
            let link = link.clone();
            let _ = handle.update(cx, |_, window, cx| {
                window.defer(cx, move |window, cx| {
                    irori_game::open_thunderstore_link_dialog(&link, window, cx)
                })
            });
        }
    }
    if let Some(file) = &cli.windows_file {
        open_main_window(app, cx);
        let window = app.lock().window;
        if let Some(handle) = window {
            let file = file.clone();
            let _ = handle.update(cx, |_, window, cx| {
                window.defer(cx, move |window, cx| {
                    irori_game::open_windows_file_dialog(file, window, cx)
                })
            });
        }
    }
    if let Some(uri) = &cli.windows_uri {
        open_main_window(app, cx);
        let window = app.lock().window;
        if let Some(handle) = window {
            let uri = uri.clone();
            let _ = handle.update(cx, |_, window, cx| {
                window.defer(cx, move |window, cx| {
                    // Any web page can open a link: one opening links in a
                    // loop must not pile up pickers, or cover a question the
                    // user is already answering.
                    if !window.has_active_dialog(cx) {
                        irori_game::open_windows_uri_dialog(uri, window, cx)
                    }
                })
            });
        }
    }
    if let Some(id) = &cli.open_launcher {
        open_launcher(id, cx);
    }
    if let Some(id) = &cli.launch {
        if cli.launch_from_link {
            confirm_link_launch(id.clone(), app, cx);
        } else {
            launch_from_outside(id.clone(), app, cx);
        }
    }
}

/// Open a third-party launcher Irori set up (its shortcut, `--open-launcher`,
/// `irori://launcher/<id>`). It only starts a program the user set up, so a
/// link needs no question; a failure becomes a notice.
fn open_launcher(id: &str, cx: &mut App) {
    let backend = irori_ui::backend(cx);
    let Some(launcher) = irori_launchers::LauncherKind::from_id(id) else {
        tracing::warn!("no launcher is called {id}");
        return;
    };
    let Some(store) = stores::LAUNCHERS.get().cloned() else {
        return;
    };
    let b = backend.clone();
    backend.spawn(async move {
        if let Err(e) = store.open(launcher).await {
            let name = launcher.display_name();
            b.bus.notice(
                irori_core::Notice::new(
                    irori_core::NoticeLevel::Error,
                    format!("{name} didn't open"),
                )
                .message(format!("{e:#}"))
                .keys(
                    "shell.notice.launcher_open_failed",
                    None,
                    &[("launcher", name)],
                ),
            );
        }
    });
}

/// A link can come from any web page or program, and playing runs the game's
/// before-launch script: an installed game starts only when the user agrees.
fn confirm_link_launch(id: GameId, app: &Arc<Mutex<AppHandles>>, cx: &mut App) {
    let Some(game) = irori_ui::backend(cx).game(&id).filter(|g| g.is_installed) else {
        launch_from_outside(id, app, cx);
        return;
    };
    open_main_window(app, cx);
    let window = app.lock().window;
    let Some(handle) = window else {
        return;
    };
    let app = app.clone();
    let heading = t_args("shell.link_launch.title", &[("title", &game.title)]);
    let _ = handle.update(cx, |_, window, cx| {
        window.defer(cx, move |window, cx| {
            // A page opening a link in a loop must not pile up prompts, or
            // cover a question the user is already answering.
            if window.has_active_dialog(cx) {
                return;
            }
            window.open_alert_dialog(cx, move |alert, _, _| {
                let (id, app) = (id.clone(), app.clone());
                alert
                    .title(heading.clone())
                    .description(t("shell.link_launch.body"))
                    .confirm()
                    .ok_text(t("shell.link_launch.confirm"))
                    .on_ok(move |_, _, cx| {
                        // This runs inside the window's update, which
                        // `launch_from_outside` needs for itself.
                        let (id, app) = (id.clone(), app.clone());
                        cx.defer(move |cx| launch_from_outside(id, &app, cx));
                        true
                    })
            });
        })
    });
}

/// Launch requested by a URL, shortcut or the tray.
fn launch_from_outside(id: GameId, app: &Arc<Mutex<AppHandles>>, cx: &mut App) {
    // Steam shortcuts may name a game that isn't installed yet: offer the
    // install instead of failing to launch.
    if irori_ui::backend(cx)
        .game(&id)
        .is_some_and(|g| !g.is_installed && g.installable)
    {
        offer_install(id, app, cx);
        return;
    }
    let hide = irori_ui::backend(cx)
        .settings
        .app()
        .hide_window_on_protocol_launch;
    let window = app.lock().window;
    match window {
        Some(handle) if !hide => {
            let _ = handle.update(cx, |_, window, cx| shell::launch_game(id, window, cx));
        }
        _ => {
            let b = irori_ui::backend(cx);
            let launch_id = id.clone();
            let task = irori_ui::run(
                cx,
                async move { b.launch(&launch_id, None, Vec::new()).await },
            );
            let app = app.clone();
            cx.spawn(async move |cx| {
                if let Err(e) = task.await {
                    tracing::error!("launch failed: {e:#}");
                    // Nothing else would tell the user: no window shows.
                    cx.update(|cx| show_launch_error(id, e, &app, cx));
                }
            })
            .detach();
        }
    }
}

/// Bring the window up to explain a launch that failed while it was hidden,
/// with the same fixes the game page offers.
fn show_launch_error(id: GameId, e: anyhow::Error, app: &Arc<Mutex<AppHandles>>, cx: &mut App) {
    open_main_window(app, cx);
    let window = app.lock().window;
    let Some(handle) = window else {
        return;
    };
    let _ = handle.update(cx, |_, window, cx| {
        window.defer(cx, move |window, cx| {
            if !irori_ui::save_conflict::resolve_launch_error(
                &id,
                Default::default(),
                &e,
                window,
                cx,
            ) {
                window.push_notification(
                    Notification::error(format!("{e:#}")).title(t("shell.launch_failed")),
                    cx,
                );
            }
        })
    });
}

/// Show a game that isn't installed with its install options: its commands
/// in Big Picture, else its page with the install sheet.
fn offer_install(id: GameId, app: &Arc<Mutex<AppHandles>>, cx: &mut App) {
    open_main_window(app, cx);
    if nav::navigator(cx).read(cx).current() == &Route::BigPicture {
        irori_bigpicture::show_game(id, cx);
        return;
    }
    nav::navigate(Route::Game(id.clone()), cx);
    let window = app.lock().window;
    if let Some(handle) = window {
        let _ = handle.update(cx, |_, window, cx| {
            irori_game::open_install_sheet(id, window, cx)
        });
    }
}

fn listen(
    mut rx: mpsc::UnboundedReceiver<ShellCommand>,
    tx: mpsc::UnboundedSender<ShellCommand>,
    app: Arc<Mutex<AppHandles>>,
    cx: &mut App,
) {
    cx.spawn(async move |cx| {
        while let Some(cmd) = rx.next().await {
            let app = app.clone();
            cx.update(|cx| match cmd {
                ShellCommand::Show => open_main_window(&app, cx),
                ShellCommand::Quit => cx.quit(),
                ShellCommand::Launch(id) => launch_from_outside(id, &app, cx),
                ShellCommand::BigPicture => {
                    open_main_window(&app, cx);
                    nav::navigate(Route::BigPicture, cx);
                }
                ShellCommand::SaveClip => irori_ui::backend(cx).save_clip(),
                ShellCommand::QuickLaunch => quick_launch::toggle(cx),
                ShellCommand::Wait(args, waiter) => {
                    waiting::serve(&Cli::parse(args), waiter, &app, tx.clone(), cx)
                }
                ShellCommand::LeaveBigPicture => {
                    if nav::navigator(cx).read(cx).current() == &Route::BigPicture {
                        irori_bigpicture::exit_big_picture(cx);
                    }
                }
                ShellCommand::Args(args) => {
                    let cli = Cli::parse(args);
                    if !cli.has_request() {
                        open_main_window(&app, cx);
                    }
                    handle_cli(&cli, &app, cx);
                }
            });
        }
    })
    .detach();
}

#[cfg(target_os = "linux")]
fn start_tray(
    backend: &Backend,
    tx: mpsc::UnboundedSender<ShellCommand>,
    app: Arc<Mutex<AppHandles>>,
    cx: &mut App,
) {
    let settings = backend.settings.app();
    if settings.no_tray_icon {
        return;
    }
    let dark = settings.dark_tray_icon;
    let task = irori_ui::state::run_infallible(cx, async move { tray::start(tx, dark).await });
    cx.spawn(async move |cx| {
        let Some(Some(handle)) = task.await else {
            cx.update(|cx| open_main_window(&app, cx));
            return;
        };
        app.lock().tray = Some(handle.clone());
        cx.update(|cx| {
            watch_replay_for_tray(handle.clone(), cx);
            watch_recent_for_tray(handle, cx);
        });
    })
    .detach();
}

/// Offer "Save clip" in the tray while the replay buffer records. It starts
/// and stops with the "playing" status, so the status model says when.
#[cfg(target_os = "linux")]
fn watch_replay_for_tray(handle: tray::TrayHandle, cx: &mut App) {
    let statuses = models(cx).statuses.clone();
    let last = std::rc::Rc::new(std::cell::Cell::new(false));
    cx.observe(&statuses, move |_, cx| {
        let b = irori_ui::backend(cx);
        let replay = b.replay().is_some();
        if last.replace(replay) != replay {
            let h = handle.clone();
            b.spawn(async move { tray::set_replay(&h, replay).await });
        }
    })
    .detach();
}

#[cfg(not(target_os = "linux"))]
fn start_tray(
    backend: &Backend,
    tx: mpsc::UnboundedSender<ShellCommand>,
    app: Arc<Mutex<AppHandles>>,
    cx: &mut App,
) {
    let settings = backend.settings.app();
    if settings.no_tray_icon {
        return;
    }
    if let Some(handle) = tray::start(tx, settings.dark_tray_icon) {
        app.lock().tray = Some(handle.clone());
        watch_recent_for_tray(handle, cx);
        follow_theme_in_menus(cx);
    }
}

/// Draw the tray's menu in Irori's light or dark, as its window is
/// (Windows; macOS menus follow the system on their own).
#[cfg(not(target_os = "linux"))]
fn follow_theme_in_menus(cx: &mut App) {
    use gpui_kit::component::{ActiveTheme as _, Theme};
    let last = std::rc::Rc::new(std::cell::Cell::new(None));
    let apply = move |cx: &mut App| {
        let dark = cx.theme().is_dark();
        if last.replace(Some(dark)) != Some(dark) {
            menu_theme::follow(dark);
        }
    };
    apply(cx);
    cx.observe_global::<Theme>(apply).detach();
}

/// Keep the tray's "Recently played" menu in step with the library model.
fn watch_recent_for_tray(handle: tray::TrayHandle, cx: &mut App) {
    let library = models(cx).library.clone();
    let last: std::rc::Rc<std::cell::RefCell<Vec<GameId>>> = Default::default();
    let push = move |cx: &mut App| {
        let b = irori_ui::backend(cx);
        let recent: Vec<(GameId, String)> = b
            .recent_games(true)
            .iter()
            .map(|g| (g.id(), g.title.clone()))
            .collect();
        let ids: Vec<GameId> = recent.iter().map(|(id, _)| id.clone()).collect();
        if *last.borrow() == ids {
            return;
        }
        *last.borrow_mut() = ids;
        let h = handle.clone();
        #[cfg(target_os = "linux")]
        b.spawn(async move { tray::set_recent(&h, recent).await });
        #[cfg(not(target_os = "linux"))]
        tray::set_recent(&h, recent);
    };
    push(cx);
    cx.observe(&library, move |_, cx| push(cx)).detach();
}

/// Quit when the main window closes, unless the user keeps Irori in the tray.
/// Other windows (the in-game overlay) come and go without either.
fn install_quit_on_close(app: Arc<Mutex<AppHandles>>, cx: &mut App) {
    cx.on_window_closed(move |cx, closed| {
        // The quick launcher comes and goes; Irori stays for its games.
        if quick_launch::is_launcher(closed, cx) {
            return;
        }
        let main = app
            .lock()
            .window
            .map(|w| AnyWindowHandle::from(w).window_id());
        if main.is_some_and(|id| id != closed) {
            return;
        }
        let keep = {
            let handles = app.lock();
            handles.tray.is_some() && irori_ui::backend(cx).settings.app().exit_to_tray
        };
        app.lock().window = None;
        if !keep && cx.windows().is_empty() {
            cx.quit();
        }
    })
    .detach();
}
