//! The selected game beside the covers: the grid's drawer and the Focus
//! view's page. Both show the game feature's own page, reached through
//! [`irori_ui::hooks::game_panel`] so the library doesn't depend on it.
//!
//! Views are made when a game is selected and kept for the last few games,
//! so stepping back and forth is instant; keyboard steps wait for the keys to
//! rest before a new one is made. A new game crossfades in over the last.

use std::time::Duration;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{ActiveTheme as _, Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::{
    Animation, AnimationExt as _, AnyElement, AnyView, AppContext as _, Context, DragMoveEvent,
    Empty, InteractiveElement as _, IntoElement, KeyDownEvent, ParentElement as _, Pixels,
    SharedString, StatefulInteractiveElement as _, Styled as _, Task, Window, div, px, rems,
};
use irori_core::GameId;
use irori_core::settings::LibraryLayout;
use irori_ui::components::{EmptyState, GameCover};
use irori_ui::hooks::GamePanel;
use irori_ui::i18n::t;
use irori_ui::motion::QUICK;
use irori_ui::style::{KeyboardFocusable as _, reduce_motion};

use super::{LibraryView, persist_ui};
use crate::layouts;

/// Views kept per shape, newest last.
const KEPT: usize = 4;
/// How long the keyboard must rest on a game before its view is made.
const SETTLE: Duration = Duration::from_millis(140);
/// Width of the drawer's grab handle, in pixels.
const HANDLE_PX: f32 = 6.0;
/// How long the drawer waits after a resize before saving its width.
const SAVE_AFTER: Duration = Duration::from_millis(400);

/// The drag payload of the drawer's edge.
#[derive(Clone)]
pub(super) struct DrawerResize;

/// The views of selected games and the drawer's state.
pub(super) struct Panels {
    /// Views made recently, newest last.
    kept: Vec<(GameId, GamePanel, AnyView)>,
    /// The view shown.
    shown: Option<(GameId, GamePanel, AnyView)>,
    /// The view fading out under the shown one, with its fade's number.
    leaving: Option<(GameId, AnyView, usize)>,
    /// Counts views shown, naming each crossfade.
    shown_count: usize,
    /// A selection waiting for the keyboard to rest.
    settling: Option<Task<()>>,
    /// Whether the grid's drawer is open.
    pub drawer_open: bool,
    /// The drawer's requested width, in rem (kept within limits when drawn).
    pub drawer_rem: f32,
    saving: Option<Task<()>>,
}

impl Panels {
    pub fn new(drawer_rem: f32) -> Self {
        Self {
            kept: Vec::new(),
            shown: None,
            leaving: None,
            shown_count: 0,
            settling: None,
            drawer_open: false,
            drawer_rem,
            saving: None,
        }
    }

    /// The game whose view is shown, if any.
    pub fn shown_game(&self) -> Option<&GameId> {
        self.shown.as_ref().map(|(id, _, _)| id)
    }
}

/// Which shape the selected game takes in `layout`, if any.
fn panel_for(layout: LibraryLayout, drawer_open: bool) -> Option<GamePanel> {
    match layout {
        LibraryLayout::Focus => Some(GamePanel::Focus),
        LibraryLayout::Grid if drawer_open => Some(GamePanel::Drawer),
        _ => None,
    }
}

impl LibraryView {
    /// The shape the selected game takes now, if any.
    pub(super) fn panel_kind(&self) -> Option<GamePanel> {
        panel_for(self.layout, self.panels.drawer_open)
    }

    /// Show the focused game in the drawer or Focus page: at once after a
    /// click, after the keys rest when `settle` (arrow keys held down step
    /// through many games).
    pub(super) fn show_focused(
        &mut self,
        settle: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.panel_kind().is_none() {
            return;
        }
        if !settle {
            self.panels.settling = None;
            self.sync_panel(window, cx);
            return;
        }
        self.panels.settling = Some(cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(SETTLE).await;
            this.update_in(cx, |this, window, cx| {
                this.panels.settling = None;
                this.sync_panel(window, cx);
                cx.notify();
            })
            .ok();
        }));
    }

    /// Make the shown view match the focused game and the current view.
    /// Cheap when nothing changed; called every frame.
    pub(super) fn sync_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(kind) = self.panel_kind() else {
            self.panels.shown = None;
            self.panels.leaving = None;
            return;
        };
        // While the keys are moving, the last game stays on screen.
        if self.panels.settling.is_some() {
            return;
        }
        let Some(id) = self.focused.clone() else {
            self.panels.shown = None;
            return;
        };
        if self
            .panels
            .shown
            .as_ref()
            .is_some_and(|(shown, k, _)| *shown == id && *k == kind)
        {
            return;
        }
        let kept = self
            .panels
            .kept
            .iter()
            .position(|(g, k, _)| *g == id && *k == kind)
            .map(|ix| self.panels.kept.remove(ix));
        let entry = match kept {
            Some(entry) => entry,
            None => match irori_ui::hooks::game_panel(id.clone(), kind, window, cx) {
                Some(view) => (id.clone(), kind, view),
                None => {
                    // Without the game feature (tests) the cover stands in.
                    self.panels.shown = None;
                    return;
                }
            },
        };
        self.panels.kept.push(entry.clone());
        let same_kind = self
            .panels
            .kept
            .iter()
            .filter(|(_, k, _)| *k == kind)
            .count();
        if same_kind > KEPT
            && let Some(oldest) = self.panels.kept.iter().position(|(_, k, _)| *k == kind)
        {
            self.panels.kept.remove(oldest);
        }
        let previous = self.panels.shown.replace(entry);
        self.panels.shown_count += 1;
        self.panels.leaving = previous
            .filter(|(g, k, _)| *k == kind && *g != id && !reduce_motion(cx))
            .map(|(g, _, view)| (g, view, self.panels.shown_count));
        if self.panels.leaving.is_some() {
            let fade = self.panels.shown_count;
            cx.spawn_in(window, async move |this, cx| {
                cx.background_executor().timer(QUICK).await;
                this.update(cx, |this, cx| {
                    if this
                        .panels
                        .leaving
                        .as_ref()
                        .is_some_and(|(_, _, n)| *n == fade)
                    {
                        this.panels.leaving = None;
                        cx.notify();
                    }
                })
                .ok();
            })
            .detach();
        }
    }

    /// Open the grid's drawer on the focused game.
    pub(super) fn open_drawer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.panels.drawer_open {
            self.panels.drawer_open = true;
            cx.notify();
        }
        self.show_focused(false, window, cx);
    }

    /// Close the grid's drawer; `true` when it was open.
    pub(super) fn close_drawer(&mut self, cx: &mut Context<Self>) -> bool {
        if !self.panels.drawer_open {
            return false;
        }
        self.panels.drawer_open = false;
        self.panels.shown = None;
        self.panels.leaving = None;
        cx.notify();
        true
    }

    /// Set the drawer's width (kept within its limits when drawn) and save
    /// it once the resizing stops.
    fn resize_drawer(&mut self, rem: f32, window: &mut Window, cx: &mut Context<Self>) {
        let available = f32::from(self.viewport.width) / f32::from(window.rem_size());
        let width = layouts::drawer_width(rem, available);
        if (width - self.panels.drawer_rem).abs() < 0.05 {
            return;
        }
        self.panels.drawer_rem = width;
        cx.notify();
        self.panels.saving = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SAVE_AFTER).await;
            this.update(cx, |this, cx| {
                this.panels.saving = None;
                persist_ui(cx, move |ui| ui.library_drawer_width = width);
            })
            .ok();
        }));
    }

    /// The drawer's width now, in rem.
    pub(super) fn drawer_rem(&self, window: &Window) -> f32 {
        let available = f32::from(self.viewport.width) / f32::from(window.rem_size());
        layouts::drawer_width(self.panels.drawer_rem, available)
    }

    /// The shown view crossfading in over the one leaving; the selected
    /// game's cover and title without the game feature.
    fn render_shown(&self, kind: GamePanel, cx: &mut Context<Self>) -> AnyElement {
        let Some((id, _, view)) = self.panels.shown.clone() else {
            return self.render_stand_in(cx);
        };
        let layer = |view: AnyView| div().absolute().top_0().left_0().size_full().child(view);
        let fading = self.panels.leaving.is_some();
        let entering = layer(view);
        let entering = if fading {
            entering
                .with_animation(
                    SharedString::from(format!(
                        "panel-in:{}:{}",
                        self.panels.shown_count,
                        id.key()
                    )),
                    Animation::new(QUICK),
                    |el, t| el.opacity(t),
                )
                .into_any_element()
        } else {
            entering.into_any_element()
        };
        let leaving = self.panels.leaving.clone().map(|(_, view, n)| {
            layer(view).with_animation(
                SharedString::from(format!("panel-out:{n}")),
                Animation::new(QUICK),
                |el, t| el.opacity(1. - t),
            )
        });
        div()
            .id(match kind {
                GamePanel::Focus => "library-focus-page",
                GamePanel::Drawer => "library-drawer-page",
            })
            .relative()
            .size_full()
            .children(leaving)
            .child(entering)
            .into_any_element()
    }

    /// The selected game's cover and title, where the game page can't be
    /// shown (no game feature), or nothing selected yet.
    fn render_stand_in(&self, cx: &mut Context<Self>) -> AnyElement {
        let game = self.focused.as_ref().and_then(|id| self.game(id, cx));
        match game {
            Some(game) => v_flex()
                .size_full()
                .items_center()
                .justify_center()
                .gap_4()
                .p_6()
                .child(GameCover::new(game.clone()).width(rems(12.)))
                .child(
                    div()
                        .max_w_full()
                        .text_lg()
                        .font_semibold()
                        .truncate()
                        .child(game.title.clone()),
                )
                .into_any_element(),
            None => EmptyState::new(IconName::LayoutPanelLeft, t("library.focus.empty"))
                .into_any_element(),
        }
    }

    /// The Focus view's page beside the covers.
    pub(super) fn render_focus_page(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .flex_1()
            .min_w_0()
            .h_full()
            .child(self.render_shown(GamePanel::Focus, cx))
            .into_any_element()
    }

    /// The grid's drawer at its width, with its grab handle on the leading
    /// edge: drag it, or focus it and use the arrow keys.
    pub(super) fn render_drawer(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let width = self.drawer_rem(window);
        let theme = cx.theme();
        let (border, ring, scrim) = (theme.border, theme.ring, theme.background.opacity(0.88));
        let surface = irori_ui::glass::page(theme.background, cx);
        let handle = div()
            .id("library-drawer-handle")
            .keyboard_focusable(cx)
            .absolute()
            .top_0()
            .bottom_0()
            .left(px(-HANDLE_PX / 2.))
            .w(px(HANDLE_PX))
            .cursor_col_resize()
            .hover(move |s| s.bg(ring.opacity(0.5)))
            .focus(move |s| s.bg(ring))
            .on_key_down(cx.listener(move |this, e: &KeyDownEvent, window, cx| {
                let step = match e.keystroke.key.as_str() {
                    "left" => layouts::DRAWER_STEP_REM,
                    "right" => -layouts::DRAWER_STEP_REM,
                    _ => return,
                };
                cx.stop_propagation();
                this.resize_drawer(width + step, window, cx);
            }))
            .on_drag(DrawerResize, |_, _, _, cx| {
                cx.stop_propagation();
                cx.new(|_| Empty)
            })
            .tooltip(|window, cx| Tooltip::new(t("library.drawer.resize")).build(window, cx));
        let close = Button::new("library-drawer-close")
            .ghost()
            .small()
            .icon(IconName::X)
            .accessibility_label(t("library.drawer.close"))
            .tooltip(t("library.drawer.close"))
            .on_click(cx.listener(|this, _, window, cx| {
                this.close_drawer(cx);
                this.grid_focus.focus(window, cx);
            }));
        div()
            .id("library-drawer")
            .relative()
            .flex_shrink_0()
            .h_full()
            .w(rems(width))
            .bg(surface)
            .border_l_1()
            .border_color(border)
            .shadow_lg()
            .child(self.render_shown(GamePanel::Drawer, cx))
            // Over the game's art, like the markers on covers.
            .child(
                h_flex()
                    .absolute()
                    .top_2()
                    .right_2()
                    .rounded_full()
                    .bg(scrim)
                    .child(close),
            )
            .child(handle)
            .into_any_element()
    }

    /// Follow a drag of the drawer's edge anywhere over the library body,
    /// whose right edge the drawer keeps.
    pub(super) fn on_drawer_drag(
        &mut self,
        event: &DragMoveEvent<DrawerResize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let right: Pixels = event.bounds.right();
        let width = (right - event.event.position.x).max(px(0.)) / window.rem_size();
        self.resize_drawer(width, window, cx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_selected_game_takes_the_shape_of_the_view() {
        assert_eq!(
            panel_for(LibraryLayout::Focus, false),
            Some(GamePanel::Focus)
        );
        assert_eq!(
            panel_for(LibraryLayout::Focus, true),
            Some(GamePanel::Focus)
        );
        assert_eq!(
            panel_for(LibraryLayout::Grid, true),
            Some(GamePanel::Drawer)
        );
        assert_eq!(panel_for(LibraryLayout::Grid, false), None);
        // The list opens pages; it has no drawer.
        assert_eq!(panel_for(LibraryLayout::List, true), None);
    }
}
