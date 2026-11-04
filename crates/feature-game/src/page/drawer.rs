//! The page as a summary in the library grid's drawer: the hero art with the
//! title or logo, the primary action, the game's tools and rating, three
//! tiles (played, last played, sessions), a few screenshots and the overview,
//! then the way to the full page. Every piece is the page's own, so the
//! drawer and the page never disagree.

use std::sync::Arc;

use gpui_kit::component::ActiveTheme as _;
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::{StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnchoredPositionMode, AnyElement, Context, InteractiveElement as _, IntoElement, ObjectFit,
    ParentElement, SharedString, StatefulInteractiveElement as _, Styled, StyledImage as _, Window,
    anchored, deferred, div, linear_color_stop, linear_gradient, point, px, rems,
};
use irori_core::GameInfo;
use irori_core::util::unix_now;
use irori_ui::components::{ArtImage, art_image, art_img, art_loading, art_missing};
use irori_ui::i18n::{t, t_args};
use irori_ui::models;
use irori_ui::style::KeyboardFocusable as _;

use super::GamePage;
use crate::i18n::playtime_label;
use crate::load::{Load, format_day};
use crate::state::PrimaryAction;

/// Screenshots the drawer shows, two to a row; the last counts the rest.
const SHOTS: usize = 4;
/// Pixel width screenshots are decoded at in the drawer.
const SHOT_WIDTH: u32 = 480;
/// Pixel width the hero art is decoded at: drawers stay narrow.
const HERO_WIDTH: u32 = 960;
/// Height of the logo over the hero art, in rem.
const LOGO_REM: f32 = 4.;

impl GamePage {
    /// The whole drawer, scrolling as one column.
    pub(super) fn render_drawer(
        &mut self,
        game: &Arc<GameInfo>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let action = self.primary_action(cx).unwrap_or(PrimaryAction::Install);
        let hero = self.render_drawer_hero(game, cx);
        let meta = self.render_meta_line(game);
        let primary = self.render_primary(action, game, true, cx);
        let status = self.render_status_line(game, cx);
        let tools = self.render_tools(game, cx);
        let rating = self.render_rating(cx);
        let tiles = self.render_drawer_tiles(cx);
        let shots = self.render_drawer_shots(cx);
        let overview = self.render_drawer_overview(game, cx);
        let muted = cx.theme().muted_foreground;
        let viewer = self.render_viewer(window, cx).map(|viewer| {
            // The drawer is too narrow for screenshots: the viewer covers the
            // window instead.
            let size = window.viewport_size();
            deferred(
                anchored()
                    .position_mode(AnchoredPositionMode::Window)
                    .position(point(px(0.), px(0.)))
                    .child(div().relative().w(size.width).h(size.height).child(viewer)),
            )
            .with_priority(1)
        });
        div()
            .size_full()
            .relative()
            .child(
                v_flex()
                    .id("game-drawer-scroll")
                    .size_full()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .child(hero)
                    .child(
                        v_flex()
                            .w_full()
                            .px_5()
                            .pt_3()
                            .pb_6()
                            .gap_4()
                            .children(
                                meta.map(|m| {
                                    div().text_sm().text_color(muted).line_clamp(2).child(m)
                                }),
                            )
                            .child(primary)
                            .children(status)
                            .child(tools)
                            .child(rating)
                            .child(tiles)
                            .children(shots)
                            .children(overview)
                            .child(
                                h_flex()
                                    .pt_2()
                                    .justify_center()
                                    .child(super::hero::open_page_button(&self.id)),
                            ),
                    ),
            )
            .vertical_scrollbar(&self.scroll)
            .children(viewer)
            .into_any_element()
    }

    /// The hero art across the drawer, fading into the page, with the
    /// store, and the logo (dark themes) or the title over its foot.
    fn render_drawer_hero(&self, game: &GameInfo, cx: &mut Context<Self>) -> AnyElement {
        let image = match game.hero_art() {
            Some(url) => art_image(url, HERO_WIDTH, false, cx),
            None => ArtImage::Missing,
        };
        // Store logos are drawn for dark backgrounds, as on the page.
        let logo = game
            .art_logo
            .clone()
            .filter(|l| !l.is_empty() && cx.theme().mode.is_dark())
            .and_then(|url| art_img(&url, 640, true, cx));
        let theme = cx.theme();
        let bg = theme.background;
        let store = game
            .system
            .as_deref()
            .and_then(irori_engines::systems::get)
            .map_or(game.runner().store_name(), |s| s.name.as_str());
        let title: AnyElement = match logo {
            Some(logo) => div()
                .h(rems(LOGO_REM))
                .max_w_full()
                .child(
                    logo.image_cache(&self.image_cache)
                        .h(rems(LOGO_REM))
                        .max_w_full()
                        .object_fit(ObjectFit::Contain),
                )
                .into_any_element(),
            None => div()
                .text_2xl()
                .font_extrabold()
                .line_clamp(2)
                .child(game.title.clone())
                .into_any_element(),
        };
        div()
            .relative()
            .w_full()
            .flex_shrink_0()
            .aspect_ratio(16. / 9.)
            .overflow_hidden()
            .bg(theme.muted)
            .child(match image {
                ArtImage::Ready(img) => img
                    .image_cache(&self.image_cache)
                    .size_full()
                    .object_fit(ObjectFit::Cover)
                    .into_any_element(),
                ArtImage::Loading => art_loading(),
                ArtImage::Missing => art_missing(cx),
            })
            .child(
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full()
                    .bg(linear_gradient(
                        180.,
                        linear_color_stop(bg.opacity(0.), 0.3),
                        linear_color_stop(bg, 1.),
                    )),
            )
            .child(
                v_flex()
                    .absolute()
                    .bottom_0()
                    .left_0()
                    .right_0()
                    .px_5()
                    .pb_1()
                    .gap_1()
                    .child(super::caption(store, theme.muted_foreground))
                    .child(title),
            )
            .into_any_element()
    }

    /// Played, last played and sessions, as three tiles.
    fn render_drawer_tiles(&self, cx: &Context<Self>) -> AnyElement {
        let record = models(cx)
            .library
            .read(cx)
            .user
            .playtime
            .get(&self.id.key())
            .cloned();
        let dash = || SharedString::from("—");
        let minutes = record.as_ref().map_or(0, |r| r.total_minutes);
        let last = record.as_ref().and_then(|r| r.last_played.clone());
        let played = match (minutes, &last) {
            (0, None) => dash(),
            _ => playtime_label(minutes, last.is_some()),
        };
        let last = last.map_or_else(dash, |l| format_day(&l, unix_now() as i64).into());
        let sessions = match &self.timeline.data {
            Load::Ready(timeline) => SharedString::from(timeline.played.to_string()),
            _ => dash(),
        };
        let theme = cx.theme();
        let tile = |label: SharedString, value: SharedString| {
            v_flex()
                .flex_1()
                .min_w_0()
                .px_3()
                .py_2()
                .gap_0p5()
                .rounded(theme.radius_lg)
                .bg(theme.secondary)
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .truncate()
                        .child(label),
                )
                .child(div().text_sm().font_semibold().truncate().child(value))
        };
        h_flex()
            .w_full()
            .gap_2()
            .child(tile(t("game.info.playtime"), played))
            .child(tile(t("game.info.last_played"), last))
            .child(tile(t("game.panel.sessions"), sessions))
            .into_any_element()
    }

    /// Up to four screenshots, two to a row; the last says how many more
    /// there are. Each opens the viewer.
    fn render_drawer_shots(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let shots = self.screenshots();
        if shots.is_empty() {
            return None;
        }
        let shown = shots.len().min(SHOTS);
        let more = (shots.len() > SHOTS).then(|| shots.len() - SHOTS + 1);
        let theme = cx.theme();
        let (scrim, on_scrim, muted_bg) = (
            theme.background.opacity(0.72),
            theme.foreground,
            theme.muted,
        );
        let mut tiles: Vec<AnyElement> = shots
            .iter()
            .take(shown)
            .enumerate()
            .map(|(index, url)| {
                let content = match art_image(url, SHOT_WIDTH, false, cx) {
                    ArtImage::Ready(img) => img
                        .image_cache(&self.image_cache)
                        .size_full()
                        .object_fit(ObjectFit::Cover)
                        .into_any_element(),
                    ArtImage::Loading => art_loading(),
                    ArtImage::Missing => art_missing(cx),
                };
                let extra = more.filter(|_| index + 1 == shown);
                div()
                    .id(("drawer-screenshot", index))
                    .keyboard_focusable(cx)
                    .relative()
                    .flex_1()
                    .min_w_0()
                    .aspect_ratio(16. / 9.)
                    .rounded_md()
                    .overflow_hidden()
                    .bg(muted_bg)
                    .cursor_pointer()
                    .child(content)
                    .when_some(extra, |d, n| {
                        d.child(
                            div()
                                .absolute()
                                .top_0()
                                .left_0()
                                .size_full()
                                .flex()
                                .items_center()
                                .justify_center()
                                .bg(scrim)
                                .text_color(on_scrim)
                                .text_xl()
                                .font_bold()
                                .child(t_args(
                                    "game.screenshots.more",
                                    &[("count", &n.to_string())],
                                )),
                        )
                    })
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.open_screenshot(index, window, cx)
                    }))
                    .into_any_element()
            })
            .collect();
        // A lone last screenshot keeps half the row.
        if tiles.len() % 2 == 1 && tiles.len() > 1 {
            tiles.push(div().flex_1().into_any_element());
        }
        let mut rows = Vec::new();
        let mut tiles = tiles.into_iter();
        while let Some(first) = tiles.next() {
            rows.push(
                h_flex()
                    .w_full()
                    .gap_2()
                    .child(first)
                    .children(tiles.next()),
            );
        }
        Some(
            v_flex()
                .w_full()
                .gap_2()
                .child(super::section_title(
                    t("game.info.tab_screenshots"),
                    cx.theme().foreground,
                ))
                .children(rows)
                .into_any_element(),
        )
    }

    /// The store's description, cut to a few paragraphs; the full page has
    /// the rest.
    fn render_drawer_overview(&self, game: &GameInfo, cx: &Context<Self>) -> Option<AnyElement> {
        let text = match &self.extra {
            Load::Ready(e) => e
                .short_description
                .clone()
                .or_else(|| e.description.clone())
                .or_else(|| game.description.clone()),
            _ => game.description.clone(),
        }
        .filter(|d| !d.trim().is_empty())?;
        Some(
            v_flex()
                .w_full()
                .gap_2()
                .child(super::section_title(
                    t("game.panel.overview"),
                    cx.theme().foreground,
                ))
                .child(
                    div()
                        .w_full()
                        .max_h(rems(16.))
                        .overflow_hidden()
                        .text_sm()
                        .child(super::info::rich_text("drawer-description", &text)),
                )
                .into_any_element(),
        )
    }
}
