//! The quick launcher: the command palette in a small window of its own,
//! opened from anywhere (`irori --quick-launch`, `irori://quick-launch`, or
//! the desktop's shortcut) without bringing up the main window. Enter plays
//! a game, or goes back to it while it runs; Shift+Enter shows it in Irori;
//! → lists its actions. It closes on Escape, once something ran, and when
//! another window takes the focus. Asked for again while it is open, it
//! closes.
//!
//! Its window is the kind each system shows above everything and gives the
//! keyboard to: a layer surface with the keyboard on Wayland compositors
//! that have layer-shell, else a pop-up window (macOS: a panel that becomes
//! the key window over any Space, Windows: a topmost tool window that
//! takes the foreground), and an ordinary window on X11, where pop-ups get
//! no keyboard.

use std::rc::Rc;

use gpui_kit::component::WindowExt as _;
use gpui_kit::component::notification::Notification;
use gpui_kit::{
    AnyWindowHandle, App, AppContext as _, Bounds, Global, Pixels, WindowBackgroundAppearance,
    WindowBounds, WindowKind, WindowOptions, point, px, size,
};
use irori_core::GameId;
use irori_ui::i18n::t;

use crate::palette::{Host, PaletteTarget, PaletteView, run_target};

/// The launcher's largest size in logical pixels (the window exists before
/// any rem does); room for the list and its preview.
const WIDTH: f32 = 860.;
const HEIGHT: f32 = 500.;

/// Plays a game as a shortcut would.
type Launch = Rc<dyn Fn(GameId, &mut App)>;

/// What the launcher needs of the shell, which owns the main window.
#[derive(Clone)]
pub(crate) struct ShellAccess {
    /// Open (or bring up) the main window.
    pub(crate) show: Rc<dyn Fn(&mut App)>,
    /// The main window, while it is open.
    pub(crate) window: Rc<dyn Fn() -> Option<AnyWindowHandle>>,
    /// Play a game as a shortcut would (failures bring the window up).
    pub(crate) launch: Launch,
}

struct Launcher {
    shell: ShellAccess,
    window: Option<AnyWindowHandle>,
}

impl Global for Launcher {}

/// Let the launcher reach the shell. Call once at startup.
pub(crate) fn install(shell: ShellAccess, cx: &mut App) {
    cx.set_global(Launcher {
        shell,
        window: None,
    });
}

/// Listen for the launcher's global shortcut while `quickLaunchShortcut` is
/// on, from startup and as soon as it is turned on; `open` runs on each
/// press, from whichever thread the desktop's answer arrives on. Linux asks
/// the desktop through the replay buffer's shortcut session (portal, X11
/// grab, or compositor lines; `irori_services::replay::shortcut`); macOS
/// and Windows register Ctrl+Alt+K themselves (`native`).
pub(crate) fn follow_shortcut(open: impl Fn() + Send + Sync + 'static, cx: &mut App) {
    let b = irori_ui::backend(cx);
    b.on_quick_launch_shortcut(open);
    let settings = irori_ui::models(cx).settings.clone();
    let listen = move |cx: &mut App| {
        if cfg!(target_os = "linux")
            && irori_ui::models(cx)
                .settings
                .read(cx)
                .app
                .ui
                .quick_launch_shortcut
        {
            let b = irori_ui::backend(cx);
            b.clone()
                .spawn(async move { b.listen_for_clip_shortcut().await });
        }
    };
    listen(cx);
    cx.observe(&settings, move |_, cx| listen(cx)).detach();
    #[cfg(any(target_os = "macos", windows))]
    native::follow(settings, cx);
}

/// macOS and Windows: Irori takes Ctrl+Alt+K (⌃⌥K) itself, with the
/// in-game overlay's hot key registration, while the setting is on.
#[cfg(any(target_os = "macos", windows))]
mod native {
    use futures::StreamExt as _;
    use gpui_kit::{App, Entity, Global};
    use irori_bigpicture::overlay::{GlobalShortcut, QUICK_LAUNCH_LABEL, register_quick_launch};
    use irori_services::QuickLaunchShortcut;

    struct Held(Option<GlobalShortcut>);
    impl Global for Held {}

    pub(super) fn follow(settings: Entity<irori_ui::models::SettingsModel>, cx: &mut App) {
        let (tx, mut rx) = futures::channel::mpsc::unbounded::<()>();
        cx.spawn(async move |cx| {
            while rx.next().await.is_some() {
                cx.update(super::toggle);
            }
        })
        .detach();
        cx.set_global(Held(None));
        let sync = move |cx: &mut App| {
            let wanted = irori_ui::models(cx)
                .settings
                .read(cx)
                .app
                .ui
                .quick_launch_shortcut;
            let held = cx.global::<Held>().0.is_some();
            let state = match (wanted, held) {
                (true, false) => match register_quick_launch(tx.clone()) {
                    Ok(shortcut) => {
                        cx.global_mut::<Held>().0 = Some(shortcut);
                        QuickLaunchShortcut::Keys(Some(QUICK_LAUNCH_LABEL.into()))
                    }
                    Err(e) => {
                        tracing::warn!("the quick launcher's shortcut isn't available: {e:#}");
                        QuickLaunchShortcut::Unavailable
                    }
                },
                (false, true) => {
                    cx.global_mut::<Held>().0 = None;
                    QuickLaunchShortcut::Off
                }
                _ => return,
            };
            irori_ui::backend(cx).set_quick_launch_shortcut(state);
        };
        sync(cx);
        cx.observe(&settings, move |_, cx| sync(cx)).detach();
    }
}

/// Where the launcher goes on a screen with `screen` bounds: its size (90%
/// of a smaller screen), centred across, a fifth of the way down, where the
/// eye already is.
pub(crate) fn placement(screen: Bounds<Pixels>) -> Bounds<Pixels> {
    let width = px(WIDTH).min(screen.size.width * 0.9);
    let height = px(HEIGHT).min(screen.size.height * 0.9);
    let x = screen.origin.x + (screen.size.width - width) / 2.;
    let y = screen.origin.y + ((screen.size.height - height) / 5.).max(px(0.));
    Bounds::new(point(x, y), size(width, height))
}

/// The launcher's window, while it is open.
fn open_window(cx: &mut App) -> Option<AnyWindowHandle> {
    let handle = cx.try_global::<Launcher>()?.window?;
    // It may have closed itself (focus lost, Escape).
    handle.update(cx, |_, _, _| ()).ok().map(|_| handle)
}

/// Whether `window` is the launcher's.
pub(crate) fn is_launcher(window: gpui_kit::WindowId, cx: &App) -> bool {
    cx.try_global::<Launcher>()
        .and_then(|l| l.window)
        .is_some_and(|w| w.window_id() == window)
}

/// Open the launcher, or close it when it is open.
pub(crate) fn toggle(cx: &mut App) {
    match open_window(cx) {
        Some(handle) => close_window(handle, cx),
        None => open(cx),
    }
}

fn close_window(handle: AnyWindowHandle, cx: &mut App) {
    let _ = handle.update(cx, |_, window, _| window.remove_window());
}

/// The screen the launcher opens on: the main window's while it is in front,
/// else the primary display.
fn screen(cx: &mut App) -> Bounds<Pixels> {
    let main = cx
        .try_global::<Launcher>()
        .and_then(|l| (l.shell.window)())
        .and_then(|w| {
            w.update(cx, |_, window, cx| {
                window
                    .is_window_active()
                    .then(|| window.display(cx).map(|d| d.bounds()))
                    .flatten()
            })
            .ok()
            .flatten()
        });
    main.or_else(|| cx.primary_display().map(|d| d.bounds()))
        .unwrap_or(Bounds::new(
            point(px(0.), px(0.)),
            size(px(WIDTH), px(HEIGHT)),
        ))
}

fn options(kind: WindowKind, cx: &mut App) -> WindowOptions {
    WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(placement(screen(cx)))),
        titlebar: None,
        focus: true,
        show: true,
        kind,
        is_movable: false,
        is_resizable: false,
        is_minimizable: false,
        app_id: Some("irori".into()),
        window_background: WindowBackgroundAppearance::Opaque,
        window_decorations: Some(gpui_kit::WindowDecorations::Client),
        ..WindowOptions::default()
    }
}

/// The window kind that comes above everything with the keyboard here.
fn kind(cx: &App) -> WindowKind {
    #[cfg(target_os = "linux")]
    {
        if cx.compositor_name() == "Wayland" {
            use gpui_kit::layer_shell::{KeyboardInteractivity, Layer, LayerShellOptions};
            return WindowKind::LayerShell(LayerShellOptions {
                namespace: "irori-quick-launch".into(),
                layer: Layer::Overlay,
                keyboard_interactivity: KeyboardInteractivity::Exclusive,
                ..LayerShellOptions::default()
            });
        }
        // An X11 pop-up is override-redirect and never gets the keyboard.
        WindowKind::Normal
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = cx;
        WindowKind::PopUp
    }
}

fn open(cx: &mut App) {
    irori_ui::i18n::register(crate::strings::CATALOGS);
    let build = |window: &mut gpui_kit::Window, cx: &mut App| {
        let view = cx.new(|cx| PaletteView::new(Host::Window, window, cx));
        let focus = view.clone();
        window.defer(cx, move |window, cx| PaletteView::focus(&focus, window, cx));
        view
    };
    let kind = kind(cx);
    let layer = !matches!(kind, WindowKind::Normal | WindowKind::PopUp);
    let opened = match gpui_kit::open_window(options(kind, cx), cx, build) {
        Ok(opened) => Ok(opened),
        // A Wayland compositor without layer-shell (GNOME).
        Err(e) if layer => {
            tracing::info!("the quick launcher opens as a pop-up: {e:#}");
            gpui_kit::open_window(options(WindowKind::PopUp, cx), cx, build)
        }
        Err(e) => Err(e),
    };
    match opened {
        Ok((handle, _)) => {
            // Not `cx.activate`: on a Mac that would bring the main window
            // forward too; the panel takes the keyboard without it.
            // Windows keeps the foreground for the program in front unless
            // Irori asks for it.
            irori_bigpicture::overlay::take_foreground(handle, cx);
            if cx.has_global::<Launcher>() {
                cx.global_mut::<Launcher>().window = Some(handle);
            }
        }
        Err(e) => {
            tracing::error!("opening the quick launcher: {e:#}");
            show_in_main(PaletteTarget::Route(irori_ui::Route::Library), cx);
            notify_main(
                Notification::error(format!("{e:#}")).title(t("shell.quick_launch.failed")),
                cx,
            );
        }
    }
}

/// Close the launcher.
pub(crate) fn close(cx: &mut App) {
    if let Some(handle) = open_window(cx) {
        close_window(handle, cx);
    }
}

/// Run what was confirmed in the launcher, which closes first. Games start
/// without the main window; what needs Irori's window shows it.
pub(crate) fn run(target: PaletteTarget, cx: &mut App) {
    close(cx);
    let Some(shell) = cx.try_global::<Launcher>().map(|l| l.shell.clone()) else {
        return;
    };
    let b = irori_ui::backend(cx);
    match target {
        PaletteTarget::Play(id) => (shell.launch)(id, cx),
        PaletteTarget::CancelLaunch(id) => {
            b.cancel_launch(&id);
        }
        PaletteTarget::Favourite(id, on) => b.userdata.set_favourite(&id, on),
        PaletteTarget::Hide(id, on) => b.userdata.set_hidden(&id, on),
        PaletteTarget::PlayStatus(id, status) => irori_ui::play_status::set(&id, status, cx),
        PaletteTarget::SaveClip => b.save_clip(),
        PaletteTarget::Refresh => irori_ui::models::refresh_library(false, cx),
        // These tell how they went in a window: the main one when it is
        // open, without bringing it up.
        target @ (PaletteTarget::BackToGame(_) | PaletteTarget::Stop(_)) => {
            match (shell.window)() {
                Some(_) => run_in_main(target, cx),
                None => show_in_main(target, cx),
            }
        }
        target => show_in_main(target, cx),
    }
}

/// Show the main window and run `target` there.
pub(crate) fn show_in_main(target: PaletteTarget, cx: &mut App) {
    if let Some(shell) = cx.try_global::<Launcher>().map(|l| l.shell.clone()) {
        (shell.show)(cx);
    }
    run_in_main(target, cx);
}

/// Run `target` in the main window, once the update that opened it is over.
fn run_in_main(target: PaletteTarget, cx: &mut App) {
    let Some(handle) = cx.try_global::<Launcher>().and_then(|l| (l.shell.window)()) else {
        return;
    };
    let _ = handle.update(cx, |_, window, cx| {
        window.defer(cx, move |window, cx| run_target(target, window, cx))
    });
}

fn notify_main(note: Notification, cx: &mut App) {
    let Some(handle) = cx.try_global::<Launcher>().and_then(|l| (l.shell.window)()) else {
        return;
    };
    let _ = handle.update(cx, |_, window, cx| window.push_notification(note, cx));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_launcher_sits_high_in_the_middle_of_its_screen() {
        let screen = Bounds::new(point(px(1920.), px(0.)), size(px(2560.), px(1440.)));
        let at = placement(screen);
        assert_eq!(at.size, size(px(WIDTH), px(HEIGHT)));
        assert_eq!(at.origin.x, px(1920. + (2560. - WIDTH) / 2.));
        assert_eq!(at.origin.y, px((1440. - HEIGHT) / 5.));
        // A small screen: 90% of it, never off it.
        let small = Bounds::new(point(px(0.), px(0.)), size(px(800.), px(480.)));
        let at = placement(small);
        assert_eq!(at.size, size(px(720.), px(432.)));
        assert!(at.origin.y >= px(0.) && at.origin.y + at.size.height <= px(480.));
    }
}
