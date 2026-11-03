//! The album tab: the period's screenshots over the latest one, the games
//! they show as chips that narrow the grid, and the grid itself. The page is
//! one list that draws only the rows on screen. A screenshot opens in the system's image viewer;
//! its menu also shows it in its folder or opens its game.

use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::component::menu::{ContextMenuExt as _, PopupMenuItem};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::{ActiveTheme as _, Icon, StyledExt as _, h_flex, v_flex, v_virtual_list};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, Context, ElementId, InteractiveElement as _, IntoElement, ObjectFit,
    ParentElement as _, Pixels, SharedString, Size, StatefulInteractiveElement as _, Styled as _,
    StyledImage as _, Window, canvas, div, px, rems, size,
};
use irori_core::GameId;
use irori_services::LibrarySnapshot;
use irori_ui::components::{EmptyState, GameCover, art_img, art_loading};
use irori_ui::i18n::{t, t_args};
use irori_ui::style::KeyboardFocusable as _;
use irori_ui::{Route, models, navigate};

use super::album::{Album, Shot};
use super::hero::{HERO_REM, big_number, dotted, eyebrow};
use super::period::Span;
use super::{StatsView, text};

/// Narrowest a screenshot is drawn, in rem.
const TILE_MIN_REM: f32 = 14.;
/// Space between screenshots, in rem.
const GAP_REM: f32 = 1.;
/// Height of the title and date under a screenshot, in rem.
const CAPTION_REM: f32 = 2.75;
/// The page's padding, in rem.
const PAD_REM: f32 = 1.5;
/// Height of the game chips with the space below them, in rem.
const GAMES_REM: f32 = 3.5;

/// One entry of the album's list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum AlbumItem {
    Hero,
    Games,
    /// A row of screenshots.
    Row(usize),
    /// Room below the last row.
    End,
}

impl AlbumItem {
    fn height_rem(&self, layout: &GridLayout) -> f32 {
        match self {
            AlbumItem::Hero => PAD_REM + HERO_REM + PAD_REM,
            AlbumItem::Games => GAMES_REM,
            AlbumItem::Row(_) => layout.row_rem(),
            AlbumItem::End => PAD_REM,
        }
    }
}

/// The grid's columns and the size of one screenshot, from the width it has.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct GridLayout {
    pub columns: usize,
    /// Width of a screenshot, in rem.
    pub tile_rem: f32,
}

impl GridLayout {
    pub fn for_width(width_rem: f32) -> Self {
        let columns = ((width_rem + GAP_REM) / (TILE_MIN_REM + GAP_REM))
            .floor()
            .max(1.) as usize;
        let tile_rem = ((width_rem - GAP_REM * (columns - 1) as f32) / columns as f32).max(1.);
        Self { columns, tile_rem }
    }

    /// Height of one row with its caption and the gap below it, in rem.
    pub fn row_rem(&self) -> f32 {
        self.tile_rem * 9. / 16. + CAPTION_REM + GAP_REM
    }

    pub fn rows(&self, shots: usize) -> usize {
        shots.div_ceil(self.columns)
    }
}

impl StatsView {
    /// The album tab when the period has no screenshots: the headline and
    /// where Irori looks, inside the page's usual scrolling body.
    pub(super) fn render_album(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(album) = self.album.clone() else {
            return div().into_any_element();
        };
        v_flex()
            .gap_6()
            .child(self.render_album_hero(&album, window, cx))
            .child(self.render_album_empty(&album, cx))
            .into_any_element()
    }

    /// The album tab with screenshots: the whole page is one list (the
    /// headline, the game chips, then rows of screenshots) so it scrolls as
    /// one and draws only the rows on screen.
    pub(super) fn render_album_page(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(album) = self.album.clone() else {
            return div().into_any_element();
        };
        let rem = window.rem_size();
        let width = if self.album_width > px(0.) {
            self.album_width
        } else {
            window.viewport_size().width
        };
        // The page keeps its padding on both sides of the grid.
        let layout = GridLayout::for_width(f32::from(width) / f32::from(rem) - 2. * PAD_REM);
        let rows = layout.rows(self.album_shown.len());
        let mut items = vec![AlbumItem::Hero];
        if album.games.len() > 1 {
            items.push(AlbumItem::Games);
        }
        items.extend((0..rows).map(AlbumItem::Row));
        items.push(AlbumItem::End);
        let sizes: Vec<Size<Pixels>> = items
            .iter()
            .map(|item| size(px(0.), rem * item.height_rem(&layout)))
            .collect();
        if *self.album_sizes != sizes {
            self.album_sizes = Rc::new(sizes);
        }
        self.album_items = Rc::new(items);
        self.album_layout = layout;

        let view = cx.entity().downgrade();
        let measure = canvas(
            move |bounds, _, cx| {
                let width = bounds.size.width;
                view.update(cx, |this, cx| {
                    if (this.album_width - width).abs() > px(0.5) {
                        this.album_width = width;
                        cx.notify();
                    }
                })
                .ok();
            },
            |_, _, _, _| {},
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full();
        let list = v_virtual_list(
            cx.entity(),
            "stats-album",
            self.album_sizes.clone(),
            |this, range, window, cx| this.render_album_items(range, window, cx),
        )
        .track_scroll(&self.album_scroll)
        .size_full();
        div()
            .relative()
            .size_full()
            .child(measure)
            .child(list)
            .into_any_element()
    }

    fn render_album_items(
        &mut self,
        range: Range<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let Some(album) = self.album.clone() else {
            return Vec::new();
        };
        let items = self.album_items.clone();
        let layout = self.album_layout;
        let library = models(cx).library.read(cx).snapshot.clone();
        range
            .filter_map(|ix| {
                let item = items.get(ix)?;
                let content = match item {
                    AlbumItem::Hero => self.render_album_hero(&album, window, cx),
                    AlbumItem::Games => self.render_album_games(&album, &library, cx),
                    AlbumItem::Row(row) => self.render_album_row(*row, &library, window, cx),
                    AlbumItem::End => div().into_any_element(),
                };
                Some(
                    div()
                        .w_full()
                        .h(rems(item.height_rem(&layout)))
                        .px(rems(PAD_REM))
                        .when(*item == AlbumItem::Hero, |d| d.pt(rems(PAD_REM)))
                        .child(content)
                        .into_any_element(),
                )
            })
            .collect()
    }

    /// The art frame over the latest screenshot, with the period's count.
    fn render_album_hero(
        &self,
        album: &Album,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let library = models(cx).library.read(cx).snapshot.clone();
        let latest = album.shots.first();
        let credit = latest.and_then(|shot| {
            let game = library.get(&shot.id)?;
            let why = t_args(
                "library.stats.album.latest",
                &[
                    ("title", &game.title),
                    ("date", &text::short_date(shot.day)),
                ],
            );
            Some(self.render_art_credit(game, why, cx))
        });
        let headline = self.render_album_headline(album, &library, cx);
        let art = latest.map(|s| s.url.to_string());
        self.render_hero(art, headline, credit, window, cx)
    }

    fn render_album_headline(
        &self,
        album: &Album,
        library: &LibrarySnapshot,
        cx: &Context<Self>,
    ) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let mut facts = vec![text::count(
            "library.stats.album.games_one",
            "library.stats.album.games_other",
            album.games.len() as u64,
        )];
        if let Some(game) = album
            .top()
            .filter(|_| album.games.len() > 1)
            .and_then(|(id, _)| library.get(id))
        {
            facts.push(t_args(
                "library.stats.album.most_in",
                &[("title", &game.title)],
            ));
        }
        v_flex()
            .gap_2()
            .min_w_0()
            .child(eyebrow(
                t_args(
                    "library.stats.album.eyebrow",
                    &[("period", &text::period(&album.period))],
                ),
                cx,
            ))
            .child(big_number(album.shots.len().to_string().into()))
            .when(!album.shots.is_empty(), |d| {
                d.child(
                    h_flex()
                        .flex_wrap()
                        .gap_x_3()
                        .gap_y_1()
                        .items_center()
                        .text_sm()
                        .children(dotted(facts, muted)),
                )
            })
            .into_any_element()
    }

    /// No screenshots in the period: say where Irori looks.
    fn render_album_empty(&self, album: &Album, cx: &Context<Self>) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let folders: Vec<SharedString> = self
            .screenshots
            .as_ref()
            .map(|index| {
                index
                    .folders
                    .iter()
                    .map(|f| f.display().to_string().into())
                    .collect()
            })
            .unwrap_or_default();
        v_flex()
            .gap_3()
            .items_center()
            .child(
                EmptyState::new(
                    IconName::Camera,
                    if album.period.span == Span::All {
                        t("library.stats.album.empty.title_all")
                    } else {
                        t_args(
                            "library.stats.album.empty.title",
                            &[("period", &text::period(&album.period))],
                        )
                    },
                )
                .description(t("library.stats.album.empty.description")),
            )
            .when(!folders.is_empty(), |d| {
                d.child(
                    v_flex()
                        .max_w_full()
                        .gap_1()
                        .items_center()
                        .text_xs()
                        .text_color(muted)
                        .children(
                            folders
                                .into_iter()
                                .map(|f| div().max_w_full().truncate().child(f)),
                        ),
                )
            })
            .into_any_element()
    }

    /// Chips for every game in the album, narrowing the grid to one.
    fn render_album_games(
        &self,
        album: &Album,
        library: &LibrarySnapshot,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme();
        let (track, active, active_fg, hover, muted) = (
            theme.secondary,
            theme.foreground,
            theme.background,
            theme.list_hover,
            theme.muted_foreground,
        );
        let chip = |id: ElementId,
                    selected: bool,
                    cover: Option<AnyElement>,
                    label: SharedString,
                    count: usize,
                    pick: Option<GameId>,
                    cx: &mut Context<Self>| {
            h_flex()
                .id(id)
                .keyboard_focusable(cx)
                .flex_shrink_0()
                .max_w(rems(18.))
                .gap_2()
                .items_center()
                .pl(if cover.is_some() {
                    rems(0.25)
                } else {
                    rems(0.75)
                })
                .pr_3()
                .py_1()
                .rounded_full()
                .text_sm()
                .cursor_pointer()
                .map(|d| {
                    if selected {
                        d.bg(active).text_color(active_fg)
                    } else {
                        d.bg(track).hover(move |s| s.bg(hover))
                    }
                })
                .children(cover)
                .child(div().min_w_0().truncate().font_medium().child(label))
                .child(
                    div()
                        .text_xs()
                        .when(!selected, |d| d.text_color(muted))
                        .child(count.to_string()),
                )
                .on_click(cx.listener(move |this, _, _, cx| this.set_album_game(pick.clone(), cx)))
                .into_any_element()
        };
        let mut chips = vec![chip(
            "stats-album-all".into(),
            self.album_game.is_none(),
            None,
            t("library.stats.album.all_games"),
            album.shots.len(),
            None,
            cx,
        )];
        for (id, count) in &album.games {
            let Some(game) = library.get(id) else {
                continue;
            };
            let cover = GameCover::new(game.clone())
                .width(rems(1.25))
                .radius(px(3.))
                .into_any_element();
            chips.push(chip(
                ElementId::Name(format!("stats-album-game:{}", id.key()).into()),
                self.album_game.as_ref() == Some(id),
                Some(cover),
                game.title.clone().into(),
                *count,
                Some(id.clone()),
                cx,
            ));
        }
        div()
            .id("stats-album-games")
            .w_full()
            .overflow_x_scrollbar()
            .child(h_flex().gap_2().pb_2().children(chips))
            .into_any_element()
    }

    /// One row of the grid.
    fn render_album_row(
        &self,
        row: usize,
        library: &LibrarySnapshot,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let layout = self.album_layout;
        let shots = self.album_shown.clone();
        let physical = {
            let logical: f32 = (window.rem_size() * layout.tile_rem).into();
            (logical * window.scale_factor()).ceil() as u32
        };
        let start = row * layout.columns;
        let end = (start + layout.columns).min(shots.len());
        h_flex()
            .w_full()
            .items_start()
            .gap(rems(GAP_REM))
            .children(
                (start..end)
                    .map(|ix| self.render_shot(ix, &shots[ix], library, layout, physical, cx)),
            )
            .into_any_element()
    }

    fn render_shot(
        &self,
        ix: usize,
        shot: &Shot,
        library: &LibrarySnapshot,
        layout: GridLayout,
        physical: u32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme();
        let (radius, muted, bg) = (theme.radius_lg, theme.muted_foreground, theme.muted);
        let title: SharedString = library
            .get(&shot.id)
            .map(|g| g.title.clone().into())
            .unwrap_or_else(|| shot.id.app_name.to_string().into());
        let picture = match art_img(&shot.url, physical, false, cx) {
            Some(img) => img
                .size_full()
                .object_fit(ObjectFit::Cover)
                .into_any_element(),
            None => art_loading(),
        };
        let path = shot.path.clone();
        let (open, reveal, id) = (path.clone(), path, shot.id.clone());
        v_flex()
            .id(ElementId::Name(format!("stats-shot-{ix}").into()))
            .keyboard_focusable(cx)
            .w(rems(layout.tile_rem))
            .flex_shrink_0()
            .gap_1p5()
            .rounded(radius)
            .cursor_pointer()
            .on_click(move |_, _, cx| cx.open_with_system(&open))
            .context_menu(move |menu, _, _| {
                let (open, reveal, id) = (reveal.clone(), reveal.clone(), id.clone());
                menu.item(
                    PopupMenuItem::new(t("library.stats.album.open"))
                        .icon(Icon::new(IconName::ExternalLink))
                        .on_click(move |_, _, cx| cx.open_with_system(&open)),
                )
                .item(
                    PopupMenuItem::new(t("library.stats.album.show_in_folder"))
                        .icon(Icon::new(IconName::FolderOpen))
                        .on_click(move |_, _, cx| cx.reveal_path(&reveal)),
                )
                .item(
                    PopupMenuItem::new(t("library.stats.album.go_to_game"))
                        .icon(Icon::new(IconName::Gamepad2))
                        .on_click(move |_, _, cx| navigate(Route::Game(id.clone()), cx)),
                )
            })
            .child(
                div()
                    .w_full()
                    .h(rems(layout.tile_rem * 9. / 16.))
                    .rounded(radius)
                    .overflow_hidden()
                    .bg(bg)
                    .child(picture),
            )
            .child(
                v_flex()
                    .h(rems(CAPTION_REM - 0.375))
                    .min_w_0()
                    .child(div().text_sm().font_medium().truncate().child(title))
                    .child(
                        div()
                            .text_xs()
                            .text_color(muted)
                            .child(text::short_date(shot.day)),
                    ),
            )
            .into_any_element()
    }

    /// Show only `game`'s screenshots, or all of them.
    pub(super) fn set_album_game(&mut self, game: Option<GameId>, cx: &mut Context<Self>) {
        self.album_game = game;
        self.refresh_album_shown();
        self.album_scroll
            .scroll_to_item(0, gpui_kit::ScrollStrategy::Top);
        cx.notify();
    }

    /// The screenshots the grid shows: the chosen game's while it still has
    /// any in the period, else all.
    pub(super) fn refresh_album_shown(&mut self) {
        let Some(album) = &self.album else {
            self.album_shown = Arc::default();
            return;
        };
        if self
            .album_game
            .as_ref()
            .is_some_and(|g| !album.games.iter().any(|(id, _)| id == g))
        {
            self.album_game = None;
        }
        self.album_shown = Arc::new(album.shown(self.album_game.as_ref()));
    }
}

#[cfg(test)]
mod tests {
    use super::GridLayout;

    #[test]
    fn the_grid_fills_its_width() {
        let narrow = GridLayout::for_width(10.);
        assert_eq!(narrow.columns, 1);
        assert_eq!(narrow.tile_rem, 10.);
        let wide = GridLayout::for_width(60.);
        assert_eq!(wide.columns, 4);
        assert_eq!(wide.tile_rem, (60. - 3.) / 4.);
        assert_eq!(wide.rows(9), 3);
        assert_eq!(wide.rows(0), 0);
    }
}
