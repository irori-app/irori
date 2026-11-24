//! Unlock notices over the running game, with `iroriAchievementNotices` and
//! `iroriAchievementToast.overGame` on: a card in the top right corner, one
//! unlock after another, with the lines the user chose. It shares its queue
//! with the overlay's panel ([`super::toasts`]): while the panel is open it
//! shows them in its own corner, and each unlock shows once.
//!
//! The card's surface never takes the keyboard or the focus from the game
//! and exists only while a card shows:
//!
//! - Wayland with wlr layer-shell: a layer surface on the overlay layer,
//!   anchored to the top right, without keyboard interactivity. Without
//!   layer-shell (GNOME) only a window that takes the focus could come above
//!   the game, so nothing opens and the unlock becomes a notice.
//! - X11, macOS and Windows: GPUI's pop-up window (an override-redirect
//!   notification window, a non-activating panel, a topmost tool window),
//!   shown without activating. Windows can't cover a game in exclusive
//!   fullscreen; the unlock becomes a notice then.
//!
//! While Irori's own window is active the unlock is a notice in it instead.

use std::time::Duration;

use gpui_kit::assets::IconName;
use gpui_kit::component::{ActiveTheme as _, Icon, StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, AnyWindowHandle, App, AppContext as _, Bounds, Context, Entity,
    InteractiveElement as _, IntoElement, ObjectFit, ParentElement as _, Render, Role,
    SharedString, StatefulInteractiveElement as _, Styled as _, StyledImage as _, Window,
    WindowBackgroundAppearance, WindowBounds, WindowKind, WindowOptions, div, point, px, rems,
    size,
};
use irori_core::{Achievement, GameId};
use irori_ui::art::LruImageCache;
use irori_ui::components::art_img;
use irori_ui::i18n::{t, t_args};
use irori_ui::models;

use super::model::{Toast, ToastLine};

/// How long one notice shows.
pub(super) const TOAST_TIME: Duration = Duration::from_secs(5);
/// The card's width and the room kept from the screen's edges, in logical
/// pixels (the window exists before any rem does).
const WIDTH: f32 = 380.;
const MARGIN: f32 = 24.;
/// Achievement picture size, in rem.
const ICON_REM: f32 = 3.;

/// The card's window while it shows.
pub(super) struct Shown {
    pub window: AnyWindowHandle,
    pub view: Entity<NoticeView>,
}

/// Whether the settings ask for unlocks over the game and this system can
/// show them there.
fn allowed(cx: &App) -> bool {
    let Some(models) = cx.try_global::<irori_ui::Models>() else {
        return false;
    };
    let app = &models.settings.read(cx).app;
    app.achievement_notices
        && app.achievement_toast.over_game
        && super::support::Support::current().available
        && !super::window::covered_by_exclusive_fullscreen()
}

/// Whether unlocks show over the running game now: they may, and Irori's
/// own window isn't the one in front (they are notices in it then).
fn wanted(cx: &App) -> bool {
    allowed(cx) && cx.active_window().is_none()
}

/// Go on showing the notices left over the game, after the panel closed.
pub(super) fn resume(cx: &mut App) {
    // Not after the game: they would show over the desktop.
    if super::toasts(cx).current().is_none()
        || super::state(cx).notice.is_some()
        || super::running_game(cx).is_none()
        || !allowed(cx)
    {
        return;
    }
    let lines = lines_shown(&models(cx).settings.read(cx).app.achievement_toast);
    match open(lines, cx) {
        Ok(shown) => super::state(cx).notice = Some(shown),
        Err(e) => tracing::info!("unlocks can't show over the game: {e:#}"),
    }
    super::start_toasts(cx);
}

/// Show `unlocks` of the running game `game` over it. Returns false when
/// they can't show there (see the module docs), so they become a notice.
pub fn show_unlocks(game: GameId, unlocks: Vec<Achievement>, cx: &mut App) -> bool {
    let panel = super::is_open(cx);
    if !panel && !wanted(cx) {
        return false;
    }
    let backend = irori_ui::backend(cx);
    let title = backend
        .game(&game)
        .map(|g| g.title.clone())
        .unwrap_or_default();
    let progress = backend.achievement_progress(&game);
    let prefs = models(cx).settings.read(cx).app.achievement_toast.clone();
    if !panel && super::state(cx).notice.is_none() {
        match open(lines_shown(&prefs), cx) {
            Ok(shown) => super::state(cx).notice = Some(shown),
            Err(e) => {
                tracing::info!("unlocks can't show over the game: {e:#}");
                return false;
            }
        }
    }
    super::toasts(cx).offer_unlocks(&game.key(), &unlocks, |a| {
        Toast::new(a, &title, progress, &prefs)
    });
    super::start_toasts(cx);
    true
}

/// How many lines under the name a card may have with `prefs`.
fn lines_shown(prefs: &irori_core::AchievementToastPrefs) -> usize {
    [
        prefs.show_game,
        prefs.show_description,
        prefs.show_rarity,
        prefs.show_progress,
        prefs.show_points,
    ]
    .into_iter()
    .filter(|on| *on)
    .count()
}

/// The card's height for `lines` lines under the name, in logical pixels:
/// the "Unlocked" label, the name and the lines, or the picture if taller.
fn height(lines: usize) -> f32 {
    let text = 18. + 22. + 18. * lines as f32;
    text.max(48.) + 2. * 12. + 2.
}

fn options(kind: WindowKind, bounds: Bounds<gpui_kit::Pixels>) -> WindowOptions {
    WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
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

/// Open the card's surface.
fn open(lines: usize, cx: &mut App) -> anyhow::Result<Shown> {
    let card = size(px(WIDTH), px(height(lines)));
    let build = |_: &mut Window, cx: &mut App| cx.new(NoticeView::new);
    #[cfg(target_os = "linux")]
    if cx.compositor_name() == "Wayland" {
        use gpui_kit::layer_shell::{Anchor, KeyboardInteractivity, Layer, LayerShellOptions};
        let kind = WindowKind::LayerShell(LayerShellOptions {
            namespace: "irori-notice".into(),
            layer: Layer::Overlay,
            anchor: Anchor::TOP | Anchor::RIGHT,
            margin: Some((px(MARGIN), px(MARGIN), px(0.), px(0.))),
            keyboard_interactivity: KeyboardInteractivity::None,
            ..LayerShellOptions::default()
        });
        let bounds = Bounds::new(point(px(0.), px(0.)), card);
        let (window, view) = gpui_kit::open_window(options(kind, bounds), cx, build)?;
        return Ok(Shown { window, view });
    }
    let screen = cx
        .primary_display()
        .map(|d| d.visible_bounds())
        .unwrap_or_else(|| Bounds::new(point(px(0.), px(0.)), size(px(1280.), px(720.))));
    let origin = point(
        screen.origin.x + screen.size.width - card.width - px(MARGIN),
        screen.origin.y + px(MARGIN),
    );
    let bounds = Bounds::new(origin, card);
    let (window, view) = gpui_kit::open_window(options(WindowKind::PopUp, bounds), cx, build)?;
    Ok(Shown { window, view })
}

/// Take the card's surface away.
pub(super) fn close(shown: Shown, cx: &mut App) {
    let _ = shown
        .window
        .update(cx, |_, window, _| window.remove_window());
}

/// The card's window: the notice shown now, from the shared queue.
pub(super) struct NoticeView {
    image_cache: Entity<LruImageCache>,
}

impl NoticeView {
    fn new(cx: &mut Context<Self>) -> Self {
        crate::register_catalogs();
        Self {
            image_cache: LruImageCache::new(4, cx),
        }
    }
}

impl Render for NoticeView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let toasts = super::toasts(cx);
        let (toast, waiting) = (toasts.current().cloned(), toasts.waiting());
        div()
            .image_cache(self.image_cache.clone())
            .size_full()
            .children(toast.map(|toast| {
                card("irori-unlock-notice", &toast, waiting, window, cx)
                    .size_full()
                    .cursor_pointer()
                    // A click puts it away early.
                    .on_click(|_, _, cx| super::next_toast(cx))
            }))
    }
}

/// One unlock notice: the picture, "Achievement unlocked", the name and the
/// lines chosen, and how many wait behind it.
pub(super) fn card<V: 'static>(
    id: &'static str,
    toast: &Toast,
    waiting: usize,
    window: &mut Window,
    cx: &mut Context<V>,
) -> gpui_kit::Stateful<gpui_kit::Div> {
    let px_wide = irori_ui::components::physical_width(rems(ICON_REM), window);
    let image = (!toast.image.is_empty())
        .then(|| art_img(&toast.image, px_wide, true, cx))
        .flatten();
    let theme = cx.theme();
    let muted = theme.muted_foreground;
    let lines: Vec<AnyElement> = toast
        .lines
        .iter()
        .map(|line| {
            div()
                .text_xs()
                .text_color(muted)
                .truncate()
                .child(line_text(line))
                .into_any_element()
        })
        .collect();
    h_flex()
        .id(id)
        .role(Role::Status)
        .aria_label(SharedString::from(format!(
            "{}: {}",
            t("bigpicture.overlay.unlocked"),
            toast.name
        )))
        .p_3()
        .gap_3()
        .items_center()
        .rounded(theme.radius_lg)
        .border_1()
        .border_color(theme.border)
        .bg(theme.popover)
        .text_color(theme.popover_foreground)
        .shadow_lg()
        .child(
            div()
                .size(rems(ICON_REM))
                .flex_shrink_0()
                .rounded(theme.radius)
                .overflow_hidden()
                .bg(theme.muted)
                .map(|d| match image {
                    Some(img) => d.child(img.size_full().object_fit(ObjectFit::Cover)),
                    None => d
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(Icon::new(IconName::Trophy)),
                }),
        )
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .child(
                    div()
                        .text_xs()
                        .text_color(muted)
                        .truncate()
                        .child(t("bigpicture.overlay.unlocked")),
                )
                .child(div().font_semibold().truncate().child(toast.name.clone()))
                .children(lines)
                .when(waiting > 0, |d| {
                    d.child(
                        div()
                            .text_xs()
                            .text_color(muted)
                            .truncate()
                            .child(if waiting == 1 {
                                t("bigpicture.overlay.more_unlocks_one")
                            } else {
                                t_args(
                                    "bigpicture.overlay.more_unlocks_other",
                                    &[("count", waiting.to_string().as_str())],
                                )
                            }),
                    )
                }),
        )
}

/// A line of a notice in words.
fn line_text(line: &ToastLine) -> SharedString {
    match line {
        ToastLine::Game(title) => title.clone().into(),
        ToastLine::Description(text) => text.clone().into(),
        ToastLine::Rarity { percent, label } => {
            let share = t_args(
                "bigpicture.overlay.notice.rarity",
                &[("percent", &format!("{percent:.1}"))],
            );
            match (label.is_empty(), *percent > 0.) {
                (false, true) => format!("{label} · {share}").into(),
                (false, false) => label.clone().into(),
                _ => share,
            }
        }
        ToastLine::Progress { unlocked, total } => t_args(
            "bigpicture.overlay.achievements_progress",
            &[
                ("unlocked", unlocked.to_string().as_str()),
                ("total", total.to_string().as_str()),
            ],
        ),
        ToastLine::Points(points) => t_args(
            if *points == 1 {
                "bigpicture.overlay.notice.points_one"
            } else {
                "bigpicture.overlay.notice.points_other"
            },
            &[("points", points.to_string().as_str())],
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_card_grows_with_its_lines() {
        let none = irori_core::AchievementToastPrefs {
            show_description: false,
            show_rarity: false,
            show_progress: false,
            ..Default::default()
        };
        assert_eq!(lines_shown(&none), 0);
        assert_eq!(lines_shown(&Default::default()), 3);
        assert!(height(0) >= 48. + 24.);
        assert!(height(5) > height(3));
    }
}
