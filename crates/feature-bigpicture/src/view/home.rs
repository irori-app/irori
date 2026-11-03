//! Home: the focused game's title, facts and description, then shelves of
//! covers. The focused shelf rises to just under the title and the focused
//! cover leads its shelf; both glide there on springs.

use gpui_kit::component::tag::Tag;
use gpui_kit::component::{StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnimationExt as _, AnyElement, App, Context, InteractiveElement as _, IntoElement, ObjectFit,
    ParentElement as _, Role, SharedString, SpringAnimation, StatefulInteractiveElement as _,
    Styled as _, StyledImage as _, Window, div, rems,
};
use irori_core::{GameInfo, StatusKind};
use irori_ui::components::{art_img, physical_width};
use irori_ui::i18n::{t, t_args};
use irori_ui::models;
use irori_ui::motion::rise_in;

use super::{BigPictureView, Colors};
use crate::layout::{FOCUS_SCALE, Metrics, SHELF_HEADING, strip_range, strip_start};
use crate::model::GameState;
use crate::text::{self, status_label, status_with_percent, store_name};
use crate::view::tile::{Cover, CoverSlot};

/// Spring that moves shelves and the grid: critically damped, a little
/// slower than focus feedback so the motion reads as travel.
pub(super) const SCROLL_SPRING: gpui_kit::SpringConfig =
    gpui_kit::SpringConfig::new(400.0, 40.0, 1.0);
/// Height of the focused game's logo, and its widest extent, in rem.
const LOGO_H: f32 = 5.0;
const LOGO_MAX_W: f32 = 30.0;

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

    fn render_home_hero(
        &self,
        c: Colors,
        m: Metrics,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let game = self.tab_game().cloned();
        let key = game.as_ref().map(|g| g.id().key()).unwrap_or_default();
        // The logo follows the backdrop, which waits for focus to rest, so
        // scrolling past covers does not fetch a logo for each.
        let logo = game
            .as_ref()
            .filter(|g| !m.compact && self.hero.as_ref().is_some_and(|h| h.id() == g.id()))
            .and_then(|g| g.art_logo.clone())
            .filter(|url| !url.is_empty())
            .and_then(|url| art_img(&url, physical_width(rems(LOGO_MAX_W), window), true, cx));
        let block = v_flex()
            .w_full()
            .h(rems(m.hero))
            .flex_shrink_0()
            .px(rems(m.gutter))
            .justify_end()
            .pb_4()
            .gap_2()
            .when_some(game, |d, game| {
                let state = Self::game_state(&game, cx);
                let description = (!m.compact).then(|| text::description(&game)).flatten();
                d.child(
                    div()
                        .id("bp-title")
                        .role(Role::Heading)
                        .aria_label(SharedString::from(game.title.clone()))
                        .max_w(rems(m.width * 0.7))
                        .map(|d| match logo {
                            Some(logo) => d.h(rems(LOGO_H)).max_w(rems(LOGO_MAX_W)).child(
                                logo.image_cache(&self.image_cache)
                                    .h_full()
                                    .object_fit(ObjectFit::Contain),
                            ),
                            None => d
                                .text_size(rems(if m.compact { 2.25 } else { 3.5 }))
                                .line_height(rems(if m.compact { 2.9 } else { 4.4 }))
                                .font_extrabold()
                                .line_clamp(1)
                                .child(game.title.clone()),
                        }),
                )
                .child(
                    h_flex()
                        .w_full()
                        .min_w_0()
                        .gap_3()
                        .items_center()
                        .text_lg()
                        .child(
                            div()
                                .min_w_0()
                                .truncate()
                                .text_color(c.foreground.opacity(0.8))
                                .child(Self::meta_line(&game, state, true, cx)),
                        )
                        .when_some(Self::status_pill(&game, state, cx), |d, pill| {
                            d.child(div().flex_shrink_0().child(pill))
                        }),
                )
                .when_some(description, |d, text| {
                    d.child(
                        div()
                            .max_w(rems(48.))
                            .text_base()
                            .line_clamp(2)
                            .text_color(c.foreground.opacity(0.7))
                            .child(text),
                    )
                })
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
        for row_ix in range {
            shelves.push(self.render_shelf(row_ix, focused, c, m, window, cx));
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

    fn render_shelf(
        &self,
        row_ix: usize,
        focused: Option<(usize, usize)>,
        c: Colors,
        m: Metrics,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let row = &self.rows[row_ix];
        let kind = row.kind;
        let is_row = focused.is_some_and(|(r, _)| r == row_ix);
        let col = self
            .grid
            .column(row_ix)
            .min(row.games.len().saturating_sub(1));
        let shown = m.shelf_tiles();
        let start = strip_start(col, row.games.len(), shown);
        let range = strip_range(start, row.games.len(), shown, 2);
        let first = range.start;
        let slot = m.slot();
        let mut tiles = Vec::with_capacity(range.len());
        for ix in range {
            let game = row.games[ix].clone();
            let key = game.id().key();
            let cover = Cover {
                game,
                id: SharedString::from(format!("bp-tile-{}-{key}", kind.id())),
                width: m.tile,
                scale: FOCUS_SCALE,
                focused: is_row && focused == Some((row_ix, ix)) && self.overlay.is_none(),
                slot: CoverSlot::Shelf {
                    row: row_ix,
                    col: ix,
                },
            };
            tiles.push(self.render_cover(cover, c, window, cx));
        }
        let count = row.games.len();
        v_flex()
            .id(SharedString::from(format!("bp-shelf-{}", kind.id())))
            .role(Role::Group)
            .aria_label(kind.title())
            .w_full()
            .h(rems(m.shelf()))
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
            .child(
                div().flex_1().min_h_0().w_full().relative().child(
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
                                let gutter = m.gutter - (slot - m.tile) / 2.0;
                                el.left(rems(gutter + (first as f32 - v) * slot))
                            },
                        ),
                ),
            )
            .into_any_element()
    }
}
