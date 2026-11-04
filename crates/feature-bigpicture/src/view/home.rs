//! Home's Shelves layout: the focused game's title, facts and description,
//! then shelves of covers. The focused shelf rises to just under the title
//! and the focused cover leads its shelf; both glide there on springs.
//!
//! The pieces other layouts share live here too: a game's title block, a
//! shelf's strip of covers and the heading that switches shelves.

use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::component::tag::Tag;
use gpui_kit::component::{Icon, StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnimationExt as _, AnyElement, App, Context, Div, InteractiveElement as _, IntoElement,
    ObjectFit, ParentElement as _, Role, SharedString, SpringAnimation,
    StatefulInteractiveElement as _, Styled as _, StyledImage as _, Window, div, rems,
};
use irori_core::{GameInfo, StatusKind};
use irori_ui::components::{art_img, physical_width};
use irori_ui::i18n::{t, t_args};
use irori_ui::models;
use irori_ui::motion::rise_in;

use super::{BigPictureView, Colors};
use crate::grid::GridMove;
use crate::layout::{
    FOCUS_SCALE, Metrics, SHELF_HEADING, TILE_GAP, strip_range, strip_start, strip_tiles,
};
use crate::model::GameState;
use crate::text::{self, status_label, status_with_percent, store_name};
use crate::theme::CoverShape;
use crate::view::tile::{Cover, CoverSlot};

/// Spring that moves shelves and the grid: critically damped, a little
/// slower than focus feedback so the motion reads as travel.
pub(super) const SCROLL_SPRING: gpui_kit::SpringConfig =
    gpui_kit::SpringConfig::new(400.0, 40.0, 1.0);
/// Height of the focused game's logo on the Shelves layout, in rem.
const LOGO_H: f32 = 5.0;
/// A logo's widest extent next to its height.
const LOGO_ASPECT: f32 = 6.0;

/// How [`BigPictureView::game_info`] draws a game's title block.
#[derive(Debug, Clone, Copy)]
pub(super) struct InfoStyle {
    /// Size of the title's text, in rem.
    pub title: f32,
    /// Height of the logo in its place, in rem; 0 for none.
    pub logo: f32,
    /// Lines of description; 0 for none.
    pub lines: usize,
    pub centered: bool,
    /// Widest the title may be, in rem.
    pub max_w: f32,
}

impl BigPictureView {
    /// "Steam · Installed · 3 h 12 min played", the facts under a title;
    /// without the store where a caption above already names it.
    pub(super) fn meta_line(
        game: &GameInfo,
        state: GameState,
        with_store: bool,
        cx: &App,
    ) -> SharedString {
        let library = models(cx).library.read(cx);
        let minutes = library.playtime_minutes(&game.id());
        let played = minutes > 0 || library.last_played(&game.id()).is_some();
        let playtime = if played {
            t_args(
                "bigpicture.info.played",
                &[("playtime", text::playtime(minutes).as_str())],
            )
        } else {
            t("bigpicture.info.never_played")
        };
        let installed = if state.installed {
            t("bigpicture.state.installed")
        } else {
            t("bigpicture.state.not_installed")
        };
        let mut parts: Vec<SharedString> = Vec::with_capacity(4);
        if with_store {
            parts.push(store_name(game.runner()));
        }
        if let Some(system) = game.system.as_deref().and_then(irori_engines::systems::get) {
            parts.push(system.name.clone().into());
        }
        parts.extend([installed, playtime]);
        parts.join(" · ").into()
    }

    /// A pill for what is happening to the game now (playing, installing,
    /// an update waiting), if anything.
    pub(super) fn status_pill(game: &GameInfo, state: GameState, cx: &App) -> Option<AnyElement> {
        if state.playing {
            return Some(
                Tag::primary()
                    .child(status_label(StatusKind::Playing))
                    .into_any_element(),
            );
        }
        if let Some(kind) = state.busy {
            let pct = models(cx)
                .statuses
                .read(cx)
                .get(&game.id())
                .and_then(|s| s.progress)
                .and_then(|p| p.percent);
            return Some(
                Tag::info()
                    .child(status_with_percent(kind, pct))
                    .into_any_element(),
            );
        }
        state.has_update.then(|| {
            Tag::warning()
                .child(t("bigpicture.state.update_available"))
                .into_any_element()
        })
    }

    /// A game's title (its logo once the backdrop has caught up with the
    /// focus, so scrolling past covers fetches no logo for each), facts,
    /// status and description, drawn as `style` says.
    pub(super) fn game_info(
        &self,
        game: &Arc<GameInfo>,
        style: InfoStyle,
        c: Colors,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let logo = Some(game)
            .filter(|g| style.logo > 0.0 && self.hero.as_ref().is_some_and(|h| h.id() == g.id()))
            .and_then(|g| g.art_logo.clone())
            .filter(|url| !url.is_empty())
            .and_then(|url| {
                art_img(
                    &url,
                    physical_width(rems(style.logo * LOGO_ASPECT), window),
                    true,
                    cx,
                )
            });
        let state = Self::game_state(game, cx);
        let description = (style.lines > 0).then(|| text::description(game)).flatten();
        v_flex()
            .w_full()
            .min_w_0()
            .gap_2()
            .when(style.centered, |d| d.items_center())
            .child(
                div()
                    .id("bp-title")
                    .role(Role::Heading)
                    .aria_label(SharedString::from(game.title.clone()))
                    .max_w(rems(style.max_w))
                    .map(|d| match logo {
                        // Sized only by its box, a logo drew at its own
                        // size, over the facts and shelf below it.
                        Some(logo) => d
                            .h(rems(style.logo))
                            .max_w(rems(style.logo * LOGO_ASPECT))
                            .overflow_hidden()
                            .child(
                                logo.image_cache(&self.image_cache)
                                    .h(rems(style.logo))
                                    .max_w(rems(style.logo * LOGO_ASPECT))
                                    .object_fit(ObjectFit::Contain),
                            ),
                        None => d
                            .text_size(rems(style.title))
                            .line_height(rems(style.title * 1.25))
                            .font_extrabold()
                            .line_clamp(1)
                            .when(style.centered, |d| d.text_center())
                            .child(game.title.clone()),
                    }),
            )
            .child(
                h_flex()
                    .max_w_full()
                    .min_w_0()
                    .gap_3()
                    .items_center()
                    .text_lg()
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_color(c.foreground.opacity(0.8))
                            .child(Self::meta_line(game, state, true, cx)),
                    )
                    .when_some(Self::status_pill(game, state, cx), |d, pill| {
                        d.child(div().flex_shrink_0().child(pill))
                    }),
            )
            .when_some(description, |d, text| {
                d.child(
                    div()
                        .max_w(rems(48.))
                        .text_base()
                        .line_clamp(style.lines)
                        .when(style.centered, |d| d.text_center())
                        .text_color(c.foreground.opacity(0.7))
                        .child(text),
                )
            })
    }

    fn render_home_hero(
        &self,
        c: Colors,
        m: Metrics,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let game = self.tab_game().cloned();
        let key = game.as_ref().map(|g| g.id().key()).unwrap_or_default();
        let style = InfoStyle {
            title: if m.compact { 2.25 } else { 3.5 },
            logo: if m.compact { 0.0 } else { LOGO_H },
            lines: if m.compact { 0 } else { 2 },
            centered: false,
            max_w: m.width * 0.7,
        };
        let block = v_flex()
            .w_full()
            .h(rems(m.hero))
            .flex_shrink_0()
            .px(rems(m.gutter))
            .justify_end()
            .pb_4()
            .when_some(game, |d, game| {
                d.child(self.game_info(&game, style, c, window, cx))
            });
        rise_in(
            SharedString::from(format!("bp-hero-text-{key}")),
            block,
            12.,
        )
    }

    pub(super) fn render_home(
        &mut self,
        c: Colors,
        m: Metrics,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let lens = self.lens();
        let focused = self.grid.focused(&lens);
        let (focus_row, _) = focused.unwrap_or((0, 0));
        let shelf_h = m.shelf();
        let shown = m.shelves_shown();
        let start = strip_start(focus_row, self.rows.len(), shown);
        let range = strip_range(start, self.rows.len(), shown + 1, 1);
        let first = range.start;
        let mut shelves = Vec::with_capacity(range.len());
        let shape = self.look.theme().covers.shape;
        for row_ix in range {
            shelves.push(self.render_strip(row_ix, focused, m.tile, shape, c, m, window, cx));
        }
        v_flex()
            .size_full()
            .child(self.render_home_hero(c, m, window, cx))
            .child(
                div()
                    .id("bp-shelves")
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .relative()
                    .overflow_hidden()
                    .child(
                        v_flex()
                            .absolute()
                            .left_0()
                            .right_0()
                            .children(shelves)
                            .with_spring(
                                "bp-shelves-scroll",
                                SpringAnimation::new(SCROLL_SPRING).to(start as f32),
                                move |el, v: f32| el.top(rems((first as f32 - v) * shelf_h)),
                            ),
                    ),
            )
            .into_any_element()
    }

    /// A shelf: its heading over a strip of covers `tile` wide that glides
    /// to keep the focused cover leading.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn render_strip(
        &self,
        row_ix: usize,
        focused: Option<(usize, usize)>,
        tile: f32,
        shape: CoverShape,
        c: Colors,
        m: Metrics,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let row = &self.rows[row_ix];
        let kind = row.kind;
        let is_row = focused.is_some_and(|(r, _)| r == row_ix);
        let height = SHELF_HEADING + tile * shape.ratio() * FOCUS_SCALE + TILE_GAP;
        let count = row.games.len();
        v_flex()
            .id(SharedString::from(format!("bp-shelf-{}", kind.id())))
            .role(Role::Group)
            .aria_label(kind.title())
            .w_full()
            .h(rems(height))
            .child(
                h_flex()
                    .h(rems(SHELF_HEADING))
                    .px(rems(m.gutter))
                    .gap_2()
                    .items_center()
                    .text_xl()
                    .min_w_0()
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .font_bold()
                            .text_color(if is_row {
                                c.foreground
                            } else {
                                c.foreground.opacity(0.6)
                            })
                            .child(kind.title()),
                    )
                    .child(
                        div()
                            .flex_shrink_0()
                            .text_color(c.foreground.opacity(if is_row { 0.7 } else { 0.45 }))
                            .child(SharedString::from(format!("· {count}"))),
                    ),
            )
            .child(self.render_strip_covers(row_ix, focused, tile, shape, c, m, window, cx))
            .into_any_element()
    }

    /// A shelf's covers in a strip that glides to keep the focused one
    /// leading; only the covers in view and a few beside them are built.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn render_strip_covers(
        &self,
        row_ix: usize,
        focused: Option<(usize, usize)>,
        tile: f32,
        shape: CoverShape,
        c: Colors,
        m: Metrics,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(row) = self.rows.get(row_ix) else {
            return div().into_any_element();
        };
        let kind = row.kind;
        let col = self
            .grid
            .column(row_ix)
            .min(row.games.len().saturating_sub(1));
        let slot = tile * FOCUS_SCALE + TILE_GAP;
        let shown = strip_tiles(m.width, m.gutter, tile);
        let start = strip_start(col, row.games.len(), shown);
        let range = strip_range(start, row.games.len(), shown, 2);
        let first = range.start;
        let mut tiles = Vec::with_capacity(range.len());
        for ix in range {
            let game = row.games[ix].clone();
            let key = game.id().key();
            let cover = Cover {
                game,
                id: SharedString::from(format!("bp-tile-{}-{key}", kind.id())),
                width: tile,
                scale: FOCUS_SCALE,
                shape,
                gap: TILE_GAP,
                focused: focused == Some((row_ix, ix)) && self.overlay.is_none(),
                slot: CoverSlot::Shelf {
                    row: row_ix,
                    col: ix,
                },
            };
            tiles.push(self.render_cover(cover, c, window, cx));
        }
        div()
            .flex_1()
            .min_h_0()
            .w_full()
            .h_full()
            .relative()
            .child(
                h_flex()
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .items_center()
                    .children(tiles)
                    .with_spring(
                        SharedString::from(format!("bp-shelf-scroll-{}", kind.id())),
                        SpringAnimation::new(SCROLL_SPRING).to(start as f32),
                        move |el, v: f32| {
                            let gutter = m.gutter - (slot - tile) / 2.0;
                            el.left(rems(gutter + (first as f32 - v) * slot))
                        },
                    ),
            )
            .into_any_element()
    }
}

impl BigPictureView {
    /// The focused shelf's name and size, with buttons to the shelves
    /// before and after it (Up and Down, or `prev`/`next` where the layout
    /// turns them) and a dot per shelf.
    pub(super) fn shelf_switcher(
        &self,
        row_ix: usize,
        (prev, next): (IconName, IconName),
        centered: bool,
        c: Colors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(row) = self.rows.get(row_ix) else {
            return div().into_any_element();
        };
        let switch = |id: &'static str, icon: IconName, mv: GridMove, enabled: bool| {
            div()
                .id(id)
                .flex_shrink_0()
                .flex()
                .items_center()
                .justify_center()
                .size(rems(1.75))
                .rounded_full()
                .when(enabled, |d| {
                    d.cursor_pointer()
                        .bg(c.foreground.opacity(0.12))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            let lens = this.lens();
                            if this.grid.apply(mv, &lens) {
                                this.after_focus_change(cx);
                            }
                            this.focus.focus(window, cx);
                        }))
                })
                .text_color(c.foreground.opacity(if enabled { 0.9 } else { 0.25 }))
                .child(Icon::new(icon))
        };
        let dots = self.rows.len().min(12);
        h_flex()
            .id("bp-shelf-switcher")
            .role(Role::Group)
            .aria_label(row.kind.title())
            .h(rems(SHELF_HEADING))
            .min_w_0()
            .gap_3()
            .items_center()
            .when(centered, |d| d.justify_center())
            .child(switch("bp-shelf-prev", prev, GridMove::Up, row_ix > 0))
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .text_xl()
                    .font_bold()
                    .child(row.kind.title()),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .text_xl()
                    .text_color(c.foreground.opacity(0.6))
                    .child(SharedString::from(format!("· {}", row.games.len()))),
            )
            .child(switch(
                "bp-shelf-next",
                next,
                GridMove::Down,
                row_ix + 1 < self.rows.len(),
            ))
            .when(dots > 1, |d| {
                d.child(
                    h_flex()
                        .flex_shrink_0()
                        .gap_1()
                        .children((0..dots).map(|i| {
                            div()
                                .size(rems(0.4))
                                .rounded_full()
                                .bg(c.foreground.opacity(if i == row_ix { 0.9 } else { 0.25 }))
                        })),
                )
            })
            .into_any_element()
    }
}
