//! The overlay's own surface, above the game.
//!
//! On a Wayland desktop with the wlr layer-shell protocol (KDE Plasma,
//! sway, Hyprland, COSMIC, niri, labwc) it is a layer surface on the
//! overlay layer, centred on the output, that never takes the keyboard on
//! its own (`on-demand`: a click on it gives it the keyboard, the game
//! keeps it otherwise). Compositors without layer-shell (GNOME) get an
//! ordinary window, which the desktop may show behind a fullscreen game,
//! and X11 an override-redirect notification window that stays on top but
//! never gets the keyboard. Inside gamescope's own session Steam decides
//! what is shown and the overlay isn't. The surface exists only while the
//! overlay is open.

use anyhow::Result;
use gpui_kit::{
    AnyWindowHandle, App, Bounds, Entity, Window, WindowBackgroundAppearance, WindowBounds,
    WindowKind, WindowOptions, px, size,
};

/// The overlay's largest size, in logical pixels (the window exists before
/// any rem does); smaller screens get 90% of theirs.
const WIDTH: f32 = 880.;
const HEIGHT: f32 = 620.;

fn options(kind: WindowKind, focus: bool, cx: &App) -> WindowOptions {
    let screen = cx
        .primary_display()
        .map(|d| d.bounds().size)
        .unwrap_or(size(px(WIDTH), px(HEIGHT)));
    let width = px(WIDTH).min(screen.width * 0.9);
    let height = px(HEIGHT).min(screen.height * 0.9);
    WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
            None,
            size(width, height),
            cx,
        ))),
        titlebar: None,
        focus,
        show: true,
        kind,
        is_movable: false,
        is_resizable: false,
        is_minimizable: false,
        app_id: Some("irori".into()),
        window_background: WindowBackgroundAppearance::Transparent,
        window_decorations: Some(gpui_kit::WindowDecorations::Client),
        ..WindowOptions::default()
    }
}

/// Open the overlay's surface with the view `build` makes.
pub(super) fn open<V: gpui_kit::Render>(
    cx: &mut App,
    build: impl Fn(&mut Window, &mut App) -> Entity<V>,
) -> Result<(AnyWindowHandle, Entity<V>)> {
    #[cfg(target_os = "linux")]
    if cx.compositor_name() == "Wayland" {
        use gpui_kit::layer_shell::{Anchor, KeyboardInteractivity, Layer, LayerShellOptions};
        let layer = WindowKind::LayerShell(LayerShellOptions {
            namespace: "irori-overlay".into(),
            layer: Layer::Overlay,
            anchor: Anchor::empty(),
            keyboard_interactivity: KeyboardInteractivity::OnDemand,
            ..LayerShellOptions::default()
        });
        match gpui_kit::open_window(options(layer, false, cx), cx, &build) {
            Ok(opened) => return Ok(opened),
            Err(e) => {
                tracing::info!("the overlay opens as a window: {e:#}");
                // Without layer-shell only a window that takes the focus
                // can come above the game.
                return gpui_kit::open_window(options(WindowKind::PopUp, true, cx), cx, &build);
            }
        }
    }
    gpui_kit::open_window(options(WindowKind::PopUp, false, cx), cx, &build)
}

/// Take the overlay's surface away.
pub(super) fn close(handle: AnyWindowHandle, cx: &mut App) {
    let _ = handle.update(cx, |_, window, _| window.remove_window());
}
