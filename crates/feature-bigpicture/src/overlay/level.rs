//! The volume level shown over the game for a moment after Guide + Up/Down
//! (`iroriOverlay.levelNotice`), so the player sees how loud it now is
//! without opening the overlay.
//!
//! It is a small surface of its own at the top centre of the screen, made
//! like the overlay's (`window.rs`): a layer surface on the overlay layer
//! that never takes the keyboard on Wayland desktops with layer-shell, else
//! a pop-up window shown without activating. It exists only while it shows:
//! each change shows the new level and starts its time again, and it goes
//! [`SHOWN_FOR`] after the last one. Nothing runs while it is gone.

use std::time::Duration;

use anyhow::Result;
use gpui_kit::assets::IconName;
use gpui_kit::component::{ActiveTheme as _, Icon, h_flex};
use gpui_kit::{
    AnyWindowHandle, App, AppContext as _, Bounds, Context, Entity, Global,
    InteractiveElement as _, IntoElement, ParentElement as _, Pixels, Render, Role,
    StatefulInteractiveElement as _, Styled as _, Task, Window, WindowBackgroundAppearance,
    WindowBounds, WindowKind, WindowOptions, div, point, px, rems, size,
};
use irori_services::Volume;
use irori_ui::i18n::t;

/// How long the level stays after the last change.
pub(crate) const SHOWN_FOR: Duration = Duration::from_millis(1500);
/// The surface's size and its distance from the top of the screen, in
/// logical pixels.
const WIDTH: f32 = 320.;
const HEIGHT: f32 = 64.;
const TOP: f32 = 32.;

/// Where the level goes on a screen with `screen` bounds: centred at the
/// top, [`TOP`] down.
pub(crate) fn placement(screen: Bounds<Pixels>) -> Bounds<Pixels> {
    let width = px(WIDTH).min(screen.size.width);
    let x = screen.origin.x + (screen.size.width - width) / 2.;
    Bounds::new(point(x, screen.origin.y + px(TOP)), size(width, px(HEIGHT)))
}

/// The level as a filled share of the bar, 0 to 1 (muted shows empty).
pub(crate) fn fill(volume: Volume) -> f32 {
    if volume.muted {
        0.
    } else {
        (volume.percent.min(100) as f32) / 100.
    }
}

struct LevelView {
    volume: Volume,
}

impl Render for LevelView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let text = if self.volume.muted {
            t("bigpicture.overlay.muted")
        } else {
            format!("{}%", self.volume.percent).into()
        };
        let icon = if self.volume.muted {
            IconName::VolumeX
        } else {
            IconName::Volume2
        };
        h_flex()
            .id("irori-volume-level")
            .role(Role::Status)
            .aria_label(t("bigpicture.overlay.system_volume"))
            .size_full()
            .px_4()
            .gap_3()
            .items_center()
            .rounded(theme.radius_lg)
            .border_1()
            .border_color(theme.border)
            .bg(theme.popover)
            .text_color(theme.popover_foreground)
            .child(Icon::new(icon))
            .child(
                div()
                    .flex_1()
                    .h(rems(0.5))
                    .rounded_full()
                    .bg(theme.muted)
                    .overflow_hidden()
                    .child(
                        div()
                            .h_full()
                            .w(gpui_kit::relative(fill(self.volume)))
                            .bg(theme.primary),
                    ),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .min_w(rems(3.5))
                    .text_right()
                    .child(text),
            )
    }
}

struct Shown {
    window: AnyWindowHandle,
    view: Entity<LevelView>,
    _hide: Task<()>,
}

#[derive(Default)]
struct LevelState {
    shown: Option<Shown>,
}

impl Global for LevelState {}

fn options(kind: WindowKind, cx: &App) -> WindowOptions {
    let screen = cx
        .primary_display()
        .map(|d| d.bounds())
        .unwrap_or(Bounds::new(
            point(px(0.), px(0.)),
            size(px(WIDTH), px(HEIGHT)),
        ));
    WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(placement(screen))),
        titlebar: None,
        focus: false,
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

fn open(volume: Volume, cx: &mut App) -> Result<(AnyWindowHandle, Entity<LevelView>)> {
    let build = move |_: &mut Window, cx: &mut App| cx.new(|_| LevelView { volume });
    #[cfg(target_os = "linux")]
    if cx.compositor_name() == "Wayland" {
        use gpui_kit::layer_shell::{Anchor, KeyboardInteractivity, Layer, LayerShellOptions};
        let layer = WindowKind::LayerShell(LayerShellOptions {
            namespace: "irori-volume".into(),
            layer: Layer::Overlay,
            anchor: Anchor::TOP,
            margin: Some((px(TOP), px(0.), px(0.), px(0.))),
            keyboard_interactivity: KeyboardInteractivity::None,
            ..LayerShellOptions::default()
        });
        if let Ok(opened) = gpui_kit::open_window(options(layer, cx), cx, &build) {
            return Ok(opened);
        }
        // Without layer-shell a window would take the focus from the game
        // to come above it; the level isn't worth that.
        anyhow::bail!("the volume level needs layer-shell on Wayland");
    }
    gpui_kit::open_window(options(WindowKind::PopUp, cx), cx, build)
}

/// Show `volume` over the game (again), for [`SHOWN_FOR`].
pub(crate) fn show(volume: Volume, cx: &mut App) {
    let hide = cx.spawn(async move |cx| {
        cx.background_executor().timer(SHOWN_FOR).await;
        cx.update(|cx| {
            if let Some(shown) = cx.default_global::<LevelState>().shown.take() {
                let _ = shown
                    .window
                    .update(cx, |_, window, _| window.remove_window());
            }
        });
    });
    if let Some(shown) = cx.default_global::<LevelState>().shown.take() {
        // The compositor may have taken the surface away meanwhile.
        if shown.window.update(cx, |_, _, _| ()).is_ok() {
            shown.view.update(cx, |view, cx| {
                view.volume = volume;
                cx.notify();
            });
            cx.default_global::<LevelState>().shown = Some(Shown {
                _hide: hide,
                ..shown
            });
            return;
        }
    }
    match open(volume, cx) {
        Ok((window, view)) => {
            cx.default_global::<LevelState>().shown = Some(Shown {
                window,
                view,
                _hide: hide,
            });
        }
        Err(e) => tracing::info!("showing the volume level: {e:#}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_level_sits_at_the_top_centre_and_fills_by_volume() {
        let screen = Bounds::new(point(px(1920.), px(0.)), size(px(2560.), px(1440.)));
        let at = placement(screen);
        assert_eq!(at.origin, point(px(1920. + (2560. - WIDTH) / 2.), px(TOP)));
        assert_eq!(at.size, size(px(WIDTH), px(HEIGHT)));
        let v = |percent, muted| Volume { percent, muted };
        assert_eq!(fill(v(40, false)), 0.4);
        assert_eq!(fill(v(40, true)), 0., "muted shows empty");
        assert_eq!(fill(v(150, false)), 1.);
    }
}
