//! A game's cover as Big Picture shows it on shelves and in the grid. The
//! focused cover grows, lifts and glows on a spring; uninstalled games are
//! dimmed; running operations show their progress across the bottom.
//!
//! With the mouse, a click opens a cover's page; the cover under the pointer
//! offers Play or Install across its bottom (the same command as Alt).

use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::tag::Tag;
use gpui_kit::component::{Icon, Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnimationExt as _, AnyElement, BoxShadow, ClickEvent, Context, Div, Hsla,
    InteractiveElement as _, IntoElement, ObjectFit, ParentElement as _, Role, SharedString,
    SpringAnimation, Stateful, StatefulInteractiveElement as _, Styled as _, StyledImage as _,
    Window, div, point, px, rems,
};
use irori_core::{GameId, GameInfo, StatusKind};
use irori_ui::components::{ArtImage, art_image, art_loading, missing_art_fill, physical_width};
use irori_ui::i18n::{t, t_args};
use irori_ui::models;
use irori_ui::motion::{HOVER_SPRING, fade_in};

use super::{BigPictureView, Colors, LibraryFocus};
use crate::model::{click_acts, cover_action};
use crate::text::status_with_percent;
use crate::theme::CoverShape;

/// How far the focused cover rises, in rem.
const LIFT: f32 = 0.5;

/// A cover to draw.
pub(super) struct Cover {
    pub game: Arc<GameInfo>,
    /// Unique among the covers on screen (shelf or grid, and the game).
    pub id: SharedString,
    /// Width at rest, in rem.
    pub width: f32,
    /// Scale when focused.
    pub scale: f32,
    /// The art's shape; the cover is `width × ratio` tall.
    pub shape: CoverShape,
    /// Space around the cover in its slot, in rem.
    pub gap: f32,
    pub focused: bool,
    /// Where it sits, so a click can focus it.
    pub slot: CoverSlot,
}

/// Where a cover sits: a shelf on Home, or the Library grid.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CoverSlot {
    Shelf { row: usize, col: usize },
    Grid(usize),
}

impl BigPictureView {
    /// Move the focus to the cover at `slot`, as the controller would.
    fn focus_cover(&mut self, slot: CoverSlot, cx: &mut Context<Self>) {
        match slot {
            CoverSlot::Shelf { row, col } => {
                let lens = self.lens();
                self.grid.set(row, col, &lens);
            }
            CoverSlot::Grid(ix) => {
                if ix >= self.library.games.len() {
                    return;
                }
                self.library.ix = ix;
                self.library.focus = LibraryFocus::Grid;
            }
        }
        self.after_focus_change(cx);
    }

    /// A click on a cover focuses it and opens its game page, as Enter does.
    pub(super) fn click_cover(
        &mut self,
        slot: CoverSlot,
        click_count: usize,
        cx: &mut Context<Self>,
    ) {
        if !click_acts(click_count) {
            return;
        }
        self.focus_cover(slot, cx);
        if let Some(id) = self.tab_game().map(|g| g.id()) {
            self.open_page(id, cx);
        }
    }

    /// Focus the cover at `slot` and return its game when it is still `id`,
    /// the game drawn there: a button acts on the game it was drawn on, never
    /// on one focused earlier or moved into its place since.
    pub(super) fn select_cover_game(
        &mut self,
        slot: CoverSlot,
        id: &GameId,
        cx: &mut Context<Self>,
    ) -> Option<Arc<GameInfo>> {
        self.focus_cover(slot, cx);
        self.tab_game().filter(|g| g.id() == *id).cloned()
    }

    fn set_hovered_cover(&mut self, id: SharedString, hovered: bool, cx: &mut Context<Self>) {
        if hovered {
            self.hovered_cover = Some(id);
        } else if self.hovered_cover.as_ref() == Some(&id) {
            self.hovered_cover = None;
        } else {
            return;
        }
        cx.notify();
    }
}

impl BigPictureView {
    /// The cover in a slot sized for its focused scale, so neighbours never
    /// move when it grows.
    pub(super) fn render_cover(
        &self,
        cover: Cover,
        c: Colors,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (width, scale, ratio, gap) = (cover.width, cover.scale, cover.shape.ratio(), cover.gap);
        let id = cover.id.clone();
        let focused = cover.focused;
        let tile = self.cover_tile(&cover, c, window, cx);
        let lifted = tile.with_spring(
            SharedString::from(format!("{id}-focus")),
            SpringAnimation::new(HOVER_SPRING).to(if focused { 1.0 } else { 0.0 }),
            move |el, f: f32| {
                let w = width * (1.0 + (scale - 1.0) * f);
                focus_frame(el.w(rems(w)).h(rems(w * ratio)).top(rems(-LIFT * f)), c, f)
            },
        );
        div()
            .w(rems(width * scale + gap))
            .h(rems(width * ratio * scale + gap))
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_center()
            .child(lifted)
            .into_any_element()
    }

    /// A cover without its size: art (or the title on a stand-in), badges,
    /// progress and the button under the pointer, and what a click does.
    pub(super) fn cover_tile(
        &self,
        cover: &Cover,
        c: Colors,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let Cover {
            game,
            id,
            width,
            scale,
            shape,
            focused,
            slot,
            ..
        } = cover;
        let (game, id, focused, slot) = (game.clone(), id.clone(), *focused, *slot);
        let state = Self::game_state(&game, cx);
        let status = models(cx).statuses.read(cx).get(&game.id()).cloned();
        let image = match shape.art(&game) {
            Some(url) => art_image(url, physical_width(rems(width * scale), window), false, cx),
            None => ArtImage::Missing,
        };

        let badge = if state.playing {
            Some(Tag::primary().small().child(t("bigpicture.badge.playing")))
        } else if state.has_update {
            Some(Tag::secondary().small().child(t("bigpicture.badge.update")))
        } else {
            None
        };
        // Uninstalled games are dimmed instead of badged, so installed ones
        // stand out; the focused one shows in full.
        let dim = !state.installed && !state.in_queue && !focused && state.busy.is_none();
        let progress = status
            .filter(|s| s.kind.is_busy() && s.kind != StatusKind::Playing)
            .map(|s| {
                let pct = s.progress.and_then(|p| p.percent);
                (status_with_percent(s.kind, pct), pct)
            });
        // The button under the pointer; a running operation's progress
        // takes its place.
        let action = cover_action(state)
            .filter(|_| progress.is_none() && self.hovered_cover.as_ref() == Some(&id));
        let label = if state.installed {
            SharedString::from(game.title.clone())
        } else {
            t_args(
                "bigpicture.tile.not_installed",
                &[("title", game.title.as_str())],
            )
        };

        div()
            .id(id.clone())
            .group(id.clone())
            .role(Role::Button)
            .aria_label(label)
            .aria_selected(focused)
            .when(focused, |d| d.aria_active_descendant())
            .relative()
            .flex_shrink_0()
            .overflow_hidden()
            .rounded(c.radius_lg)
            .bg(c.muted)
            .border_2()
            .cursor_pointer()
            .map(|d| match image {
                ArtImage::Ready(img) => d.child(
                    img.size_full()
                        .rounded(c.radius_lg)
                        .object_fit(ObjectFit::Cover)
                        .when(dim, |img| img.opacity(0.5)),
                ),
                ArtImage::Loading => d.child(art_loading()),
                ArtImage::Missing => d.child(
                    v_flex()
                        .size_full()
                        .justify_between()
                        .p_3()
                        .bg(missing_art_fill(cx))
                        .child(Icon::new(IconName::Gamepad2).text_color(c.muted_foreground))
                        .child(
                            div()
                                .text_lg()
                                .font_bold()
                                .line_clamp(4)
                                .text_color(c.foreground)
                                .child(game.title.clone()),
                        ),
                ),
            })
            .when_some(badge, |d, badge| {
                d.child(
                    h_flex()
                        .absolute()
                        .top_0()
                        .left_0()
                        .right_0()
                        .p_2()
                        .min_w_0()
                        .overflow_hidden()
                        .child(badge),
                )
            })
            .when_some(progress, |d, (label, pct)| {
                d.child(
                    v_flex()
                        .absolute()
                        .bottom_0()
                        .left_0()
                        .right_0()
                        .p_2()
                        .gap_1()
                        .bg(c.background.opacity(0.85))
                        .child(
                            div()
                                .min_w_0()
                                .truncate()
                                .text_xs()
                                .text_color(c.foreground)
                                .child(label),
                        )
                        .child(irori_ui::motion::gliding_progress(
                            SharedString::from(format!("{id}-progress")),
                            pct,
                        )),
                )
            })
            .when_some(action, |d, action| {
                let game_id = game.id();
                let button = Button::new(SharedString::from(format!("{id}-action")))
                    .primary()
                    .w_full()
                    .icon(action.icon())
                    .label(action.label())
                    .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                        // The cover under the button must not take this
                        // click too.
                        cx.stop_propagation();
                        if let Some(game) = this.select_cover_game(slot, &game_id, cx) {
                            this.cover_command(game, window, cx);
                        }
                        this.focus.focus(window, cx);
                    }));
                // Drawn (and clickable) only while the pointer is really over
                // the cover: the hover callback doesn't fire for a cover that
                // scrolled away or was hidden by the page while hovered.
                d.child(
                    div()
                        .absolute()
                        .bottom_0()
                        .left_0()
                        .right_0()
                        .p_2()
                        .invisible()
                        .group_hover(id.clone(), |s| s.visible())
                        .child(fade_in(
                            SharedString::from(format!("{id}-action-in")),
                            button,
                        )),
                )
            })
            .on_hover(cx.listener({
                let id = id.clone();
                move |this, hovered: &bool, _, cx| this.set_hovered_cover(id.clone(), *hovered, cx)
            }))
            .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                this.click_cover(slot, event.click_count(), cx);
                this.focus.focus(window, cx);
            }))
    }
}

/// The focused cover's edge and glow, `f` of the way in (0 at rest, so a
/// cover at rest draws no shadow).
pub(super) fn focus_frame(el: Stateful<Div>, c: Colors, f: f32) -> Stateful<Div> {
    let (ring, edge): (Hsla, Hsla) = (c.ring, c.foreground);
    el.border_color(edge.opacity(0.9 * f)).when(f > 0.01, |el| {
        el.shadow(vec![BoxShadow {
            color: ring.opacity(0.55 * f),
            offset: point(px(0.), px(8.)),
            blur_radius: px(36.),
            spread_radius: px(2.),
            inset: false,
        }])
    })
}
