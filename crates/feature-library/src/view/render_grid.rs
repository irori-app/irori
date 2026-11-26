//! Grid, Focus, and top-row rendering: cover tiles, "Continue playing" cards,
//! and category chips.

use std::ops::Range;
use std::sync::Arc;

use gpui_kit::AnimationExt as _;
use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::menu::ContextMenuExt as _;
use gpui_kit::component::tag::Tag;
use gpui_kit::component::{
    ActiveTheme as _, Icon, Sizable as _, StyledExt as _, ThemeStyled as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, BoxShadow, ClickEvent, Context, ElementId, InteractiveElement as _, IntoElement,
    ObjectFit, ParentElement as _, Rems, SharedString, SpringAnimation,
    StatefulInteractiveElement as _, Styled as _, StyledImage as _, Window, div, linear_color_stop,
    linear_gradient, point, px, rems,
};
use irori_core::GameInfo;
use irori_core::StatusKind;
use irori_core::settings::{CoverCaption, CoverFit, CoverHover, CoverShape, LibraryLayout};
use irori_ui::components::{
    ArtImage, BadgeMark, GameCover, MAX_BADGES, SectionTitle, StoreBadge, StoreLogo, art_image,
    art_img, art_loading, art_missing, badge_row, physical_width,
};
use irori_ui::i18n::{t, t_args};
use irori_ui::motion::{HOVER_SPRING, gliding_progress};
use irori_ui::style::{KeyboardFocusable as _, art_tint};
use irori_ui::{Route, models, navigate};

use super::menus::game_menu;
use super::render_list::pick_mark;
use super::{
    CARD_REM, FAVOURITE_REM, Item, LibraryView, caption, item_rem, status_text, store_badges_shown,
};
use crate::commands::{self, GameCommand};
use crate::filter::GroupKind;
use crate::grid;
use crate::top::TopKind;

/// How far a hovered or focused cover rises, in pixels. Rows leave
/// `grid::ROW_INSET_REM` above them, so this plus the focus ring must fit.
const TILE_LIFT_PX: f32 = 3.0;
/// Height of the logo art on a "Continue playing" card, in rem.
const CARD_LOGO_REM: f32 = 3.25;
/// The categories shown on a cover: the first two, then how many more, so
/// the line stays on the cover however many there are.
const COVER_CATEGORIES: usize = 2;

/// How a tile draws its cover: its width, its shape, and with art drawn
/// whole, the picture's own size in rem inside its cell.
#[derive(Debug, Clone, Copy)]
struct TileLook {
    width: Rems,
    cover: CoverShape,
    whole: Option<(f32, f32)>,
}

impl LibraryView {
    pub(super) fn render_grid_row(
        &mut self,
        range: Range<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let metrics = self.frame.metrics;
        let end = range.end.min(self.frame.games.len());
        let games: Vec<_> = self.frame.games[range.start.min(end)..end].to_vec();
        // Focus keeps its column of boxes; the grid takes the chosen shape,
        // and only boxes can be drawn whole in their cells.
        let cover = match self.layout {
            LibraryLayout::Grid => self.cover_shape,
            _ => CoverShape::Portrait,
        };
        let shapes = (cover == CoverShape::Portrait)
            .then(|| self.whole_shapes(&games, metrics.tile_rem, cx))
            .flatten();
        h_flex()
            .items_start()
            .h(rems(metrics.row_rem))
            .pt(rems(grid::ROW_INSET_REM))
            .px(rems(grid::EDGE_REM))
            .gap(rems(metrics.gap_rem))
            .children(games.iter().enumerate().map(|(ix, g)| {
                let shape = shapes.as_ref().and_then(|s| s.get(ix).copied());
                let look = TileLook {
                    width: rems(metrics.tile_rem),
                    cover,
                    whole: shape,
                };
                self.render_tile(g, look, false, window, cx)
            }))
            .into_any_element()
    }

    /// With covers drawn whole, each one's size in rem inside its cell
    /// ([`grid::whole_sizes`]) for one row of `games`; `None` while they are
    /// cropped to 2:3.
    fn whole_shapes(
        &self,
        games: &[Arc<GameInfo>],
        tile_rem: f32,
        cx: &mut Context<Self>,
    ) -> Option<Vec<(f32, f32)>> {
        if self.ui_prefs.cover_fit != CoverFit::Whole {
            return None;
        }
        let aspects: Vec<Option<f32>> = games
            .iter()
            .map(|g| g.cover_art().and_then(|url| irori_ui::art::aspect(url, cx)))
            .collect();
        Some(grid::whole_sizes(&aspects, tile_rem))
    }

    /// A cover tile with live status, markers, and Play/Install plus "…" on
    /// hover or keyboard focus.
    fn render_tile(
        &mut self,
        game: &Arc<GameInfo>,
        look: TileLook,
        in_recent: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let TileLook {
            width,
            cover: cover_shape,
            whole: shape,
        } = look;
        let id = game.id();
        let key = id.key();
        let state = self.state_for(game, in_recent, cx);
        let status = models(cx).statuses.read(cx).get(&id).cloned();
        let (show_badges, cover_categories) = {
            let ui = &models(cx).settings.read(cx).app.ui;
            (
                store_badges_shown(ui, self.frame.one_store),
                ui.cover_categories,
            )
        };
        let badges_always = self.ui_prefs.store_badges_always;
        let (corners, caption_kind, hover, favourite_star) = (
            self.ui_prefs.cover_corners,
            self.ui_prefs.cover_caption,
            self.ui_prefs.cover_hover,
            self.ui_prefs.cover_favourite,
        );
        let lifts = hover == CoverHover::Lift;
        let focus_view = self.layout == LibraryLayout::Focus;
        // The user's categories, at the cover's foot while nothing else is.
        let categories = (cover_categories && !focus_view)
            .then(|| {
                let badges = models(cx).library.read(cx).categories_of(&id);
                badges
                    .into_iter()
                    .map(|(name, colour)| {
                        (name, irori_ui::categories::colour(colour.as_deref(), cx))
                    })
                    .collect::<Vec<_>>()
            })
            .filter(|c| !c.is_empty());
        let keyboard =
            self.focused.as_ref() == Some(&id) && self.grid_focus.contains_focused(window, cx);
        let active =
            keyboard || self.hovered.as_ref() == Some(&id) || self.menu_open.as_ref() == Some(&id);
        // The game the drawer or the Focus page shows keeps its ring.
        let selected =
            !in_recent && self.panel_kind().is_some() && self.focused.as_ref() == Some(&id);
        let queue_place = models(cx).downloads.read(cx).queue.queue_place(&id);
        let busy = status.as_ref().filter(|s| {
            s.kind.is_busy() && !matches!(s.kind, StatusKind::Playing | StatusKind::Queued)
        });
        let theme = cx.theme();
        let bg = theme.background;
        let scrim = bg.opacity(0.88);
        let muted = theme.muted_foreground;
        let radius = irori_ui::components::cover_radius(corners, cx);

        // An entry of several stores' copies says so.
        let copies = models(cx).library.read(cx).snapshot.copies_of(&id).len();
        let markers = h_flex()
            .absolute()
            .top_2()
            .right_2()
            .gap_1()
            .when(copies > 1, |el| {
                el.child(
                    h_flex()
                        .gap_0p5()
                        .px_1()
                        .py_0p5()
                        .rounded_full()
                        .bg(scrim)
                        .text_xs()
                        .child(Icon::new(IconName::Layers).xsmall())
                        .child(copies.to_string()),
                )
            })
            .when(
                state.has_update && state.installed && !state.sideload,
                |el| {
                    el.child(
                        div()
                            .p_1()
                            .rounded_full()
                            .bg(scrim)
                            .child(Icon::new(IconName::CircleArrowDown).xsmall()),
                    )
                },
            )
            .when(state.favourite && favourite_star, |el| {
                el.child(
                    div()
                        .p_1()
                        .rounded_full()
                        .bg(scrim)
                        .child(Icon::new(IconName::Star).xsmall()),
                )
            });

        let busy_more = (active && busy.is_some()).then(|| self.more_button(game, in_recent, cx));
        let status_overlay = status.as_ref().and_then(|s| match s.kind {
            StatusKind::Playing => Some(
                div()
                    .absolute()
                    .bottom_2()
                    .left_2()
                    .child(Tag::success().xsmall().child(t("library.status.playing")))
                    .into_any_element(),
            ),
            StatusKind::Queued => Some(
                div()
                    .absolute()
                    .bottom_2()
                    .left_2()
                    .child(
                        Tag::secondary()
                            .xsmall()
                            .child(crate::i18n::queued_label(queue_place)),
                    )
                    .into_any_element(),
            ),
            kind if kind.is_busy() => {
                let pct = s.progress.and_then(|p| p.percent);
                Some(
                    v_flex()
                        .absolute()
                        .bottom_0()
                        .left_0()
                        .right_0()
                        .p_2()
                        .gap_1()
                        .bg(scrim)
                        .child(
                            h_flex()
                                .gap_2()
                                .text_xs()
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .truncate()
                                        .child(crate::i18n::status_label(kind)),
                                )
                                .children(pct.map(|p| {
                                    t_args(
                                        "library.status.percent",
                                        &[("percent", &format!("{p:.0}"))],
                                    )
                                }))
                                .children(busy_more),
                        )
                        .child(gliding_progress(
                            ElementId::Name(format!("progress:{key}").into()),
                            pct,
                        ))
                        .into_any_element(),
                )
            }
            _ => None,
        });

        let selecting = self.selection.is_active();
        let picked = self.selection.contains(&id);
        let actions_scrim = linear_gradient(
            180.,
            linear_color_stop(bg.opacity(0.), 0.),
            linear_color_stop(bg.opacity(0.78), 1.),
        );
        // Focus covers are small, and the page beside them has the actions.
        let actions = (active && busy.is_none() && !selecting && !focus_view).then(|| {
            let primary = commands::primary(&state).map(|command| {
                let label = command.label(&state);
                let id = id.clone();
                let button = Button::new(ElementId::Name(format!("tile-primary:{key}").into()))
                    .w_full()
                    .rounded_full()
                    .px_4()
                    .shadow_md()
                    .icon(command.icon(&state))
                    .label(label)
                    .loading(state.launching)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        cx.stop_propagation();
                        this.run_command(&id, command.clone(), window, cx)
                    }));
                if matches!(state.status, Some(StatusKind::Playing)) {
                    button
                } else {
                    button.primary()
                }
            });
            div()
                .absolute()
                .bottom_0()
                .left_0()
                .right_0()
                .h(rems(4.75))
                .rounded_b(radius)
                .bg(actions_scrim)
                .flex()
                .items_end()
                .justify_center()
                .p_2p5()
                .child(
                    h_flex()
                        .w_full()
                        .max_w(rems(13.25))
                        .min_w_0()
                        .gap_2()
                        .items_center()
                        .justify_center()
                        .children(
                            primary.map(|b| div().flex_1().max_w(rems(10.5)).min_w_0().child(b)),
                        )
                        .child(self.cover_more_button(game, in_recent, cx)),
                )
        });

        // Uninstalled games are dimmed until pointed at, so installed ones stand out.
        let dim = !state.installed && !active && status.is_none();
        let primary_color = cx.theme().primary;
        let cover = div()
            .relative()
            .rounded(radius)
            .when(active && lifts, |el| el.shadow_lg())
            .when(keyboard, |el| el.focus_ring_style(window, cx))
            .child(div().when(dim, |el| el.opacity(0.45)).child({
                let cover = GameCover::new(game.clone())
                    .shape(cover_shape)
                    .radius(radius);
                match shape {
                    Some((w, h)) => cover.width(rems(w)).height(rems(h)),
                    None => cover.width(width),
                }
            }))
            .when(active || selected, |el| {
                el.child(
                    div()
                        .absolute()
                        .top_0()
                        .left_0()
                        .size_full()
                        .rounded(radius)
                        .border_2()
                        .border_color(primary_color.opacity(if selected { 1. } else { 0.85 })),
                )
            })
            .when(
                show_badges && (active || badges_always) && !selecting && !focus_view,
                |el| {
                    // An entry of several stores' copies names every store,
                    // the first few and a count of the rest.
                    let row = badge_row(irori_ui::copies::badge_marks(game, cx), MAX_BADGES);
                    let pill = |b: StoreBadge| div().rounded_md().bg(scrim).child(b);
                    el.child(
                        h_flex()
                            .absolute()
                            .top_2()
                            .left_2()
                            .right_2()
                            .gap_1p5()
                            .items_center()
                            .flex_wrap()
                            .children(
                                row.shown
                                    .into_iter()
                                    .map(|mark| cover_store_mark(mark, scrim)),
                            )
                            .when(row.more > 0, |el| {
                                el.child(pill(StoreBadge::more(row.more)))
                            }),
                    )
                },
            )
            .child(markers)
            .when(actions.is_none() && status_overlay.is_none(), |el| {
                el.children(categories.map(|names| {
                    // A cover Irori draws itself has the title at its foot.
                    let has_art = irori_ui::components::cover_art_for(game, cover_shape).is_some();
                    category_chips(names, has_art, scrim)
                }))
            })
            .when(actions.is_none(), |el| el.children(status_overlay))
            .children(actions)
            // Selection mode: a ring and a tick on picked covers, an empty
            // circle on the others.
            .when(picked, |el| {
                el.child(
                    div()
                        .absolute()
                        .top_0()
                        .left_0()
                        .size_full()
                        .rounded(radius)
                        .border_3()
                        .border_color(primary_color),
                )
            })
            .when(selecting, |el| el.child(pick_mark(picked, cx)));

        let view = cx.entity().downgrade();
        let menu_game = game.clone();
        let hover_id = id.clone();
        let click_id = id.clone();
        let caption_text = caption(game, &state, status.as_ref());
        // The caption names the value the library is sorted by, so the order
        // reads at a glance; sections by play status or playtime read as a
        // backlog, so there each cover says how long it has been played.
        let sorted_by = (caption_kind == CoverCaption::Full)
            .then(|| {
                let lib = models(cx).library.read(cx);
                let minutes = lib.playtime_minutes(&id);
                let added = game
                    .date_added
                    .clone()
                    .or_else(|| lib.user.first_seen.get(&key).cloned());
                let sort = self.filters.read(cx).filter().sort;
                let backlog = self.frame.groups.first().is_some_and(|g| {
                    matches!(g.kind, GroupKind::Status(_) | GroupKind::Playtime(_))
                });
                crate::format::sort_value(
                    sort,
                    game,
                    minutes,
                    lib.last_played(&id),
                    added.as_deref(),
                    crate::format::now(),
                )
                .or_else(|| {
                    backlog
                        .then(|| crate::format::playtime(minutes))
                        .flatten()
                        .map(|p| (IconName::Clock, p))
                })
            })
            .flatten();
        v_flex()
            .id(ElementId::Name(format!("tile:{key}").into()))
            .w(width)
            .gap_2()
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                this.set_hovered(&hover_id, *hovered, cx)
            }))
            .on_click(cx.listener(move |this, e: &ClickEvent, window, cx| {
                this.click_game(&click_id, e.modifiers(), e.click_count(), window, cx)
            }))
            // Pointed-at and focused covers rise a little off the grid,
            // unless the user keeps them in place.
            .child({
                let cover = cover.with_spring(
                    SharedString::from(format!("tile-lift:{key}")),
                    SpringAnimation::new(HOVER_SPRING).to(if active && lifts { 1.0 } else { 0.0 }),
                    |el, lift: f32| el.top(px(-TILE_LIFT_PX * lift)),
                );
                // A cover drawn whole stands on its cell's foot, so the
                // captions of a row stay on one line.
                match shape {
                    Some(_) => v_flex()
                        .w(width)
                        .h(rems(width.0 * grid::COVER_RATIO))
                        .justify_end()
                        .items_center()
                        .child(cover)
                        .into_any_element(),
                    None => cover.into_any_element(),
                }
            })
            .when(caption_kind != CoverCaption::None, |tile| {
                tile.child(
                    v_flex()
                        .min_w_0()
                        .child(
                            div()
                                .text_sm()
                                .font_semibold()
                                .truncate()
                                .when(dim, |el| el.text_color(muted))
                                .child(game.title.clone()),
                        )
                        .when(
                            caption_kind == CoverCaption::Full
                                && (!caption_text.is_empty() || sorted_by.is_some()),
                            |el| {
                                el.child(
                                    h_flex()
                                        .min_w_0()
                                        .gap_1()
                                        .text_xs()
                                        .text_color(muted)
                                        .when_some(sorted_by, |el, (icon, value)| {
                                            el.child(Icon::new(icon).xsmall().flex_none())
                                                .child(div().flex_none().child(value))
                                                .when(!caption_text.is_empty(), |el| {
                                                    el.child(div().flex_none().child("·"))
                                                })
                                        })
                                        .child(div().min_w_0().truncate().child(caption_text)),
                                )
                            },
                        ),
                )
            })
            .context_menu(move |menu, window, cx| {
                game_menu(menu, &view, &menu_game, in_recent, window, cx)
            })
            .into_any_element()
    }

    pub(super) fn render_top(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let Some((kind, games)) = self.frame.top.clone() else {
            return div().into_any_element();
        };
        let cards: Vec<AnyElement> = match kind {
            TopKind::ContinuePlaying => games
                .iter()
                .map(|g| self.render_card(g, window, cx))
                .collect(),
            TopKind::Favourites => games
                .iter()
                .map(|g| {
                    let shape = self
                        .whole_shapes(std::slice::from_ref(g), FAVOURITE_REM, cx)
                        .and_then(|s| s.first().copied());
                    let look = TileLook {
                        width: rems(FAVOURITE_REM),
                        cover: CoverShape::Portrait,
                        whole: shape,
                    };
                    self.render_tile(g, look, false, window, cx)
                })
                .collect(),
        };
        v_flex()
            .h(rems(item_rem(Item::Top, Some(kind), &self.frame.metrics)))
            .child(
                div()
                    .h(rems(grid::HEADING_REM))
                    .px(rems(grid::EDGE_REM))
                    .flex()
                    .items_end()
                    .pb_2()
                    .child(SectionTitle::new(kind.title())),
            )
            .child(
                h_flex()
                    .id("library-top-row")
                    .items_start()
                    .overflow_x_scroll()
                    .pt(rems(grid::ROW_INSET_REM))
                    .px(rems(grid::EDGE_REM))
                    .gap(rems(grid::GAP_REM))
                    .children(cards),
            )
            .into_any_element()
    }

    /// A large landscape "Continue playing" card with one-click Play.
    fn render_card(
        &mut self,
        game: &Arc<GameInfo>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = game.id();
        let key = id.key();
        let state = self.state_for(game, true, cx);
        let status = models(cx).statuses.read(cx).get(&id).cloned();
        let (minutes, last) = {
            let lib = models(cx).library.read(cx);
            (
                lib.playtime_minutes(&id),
                lib.last_played(&id).map(str::to_string),
            )
        };
        let width = rems(CARD_REM);
        let height = rems(CARD_REM * 9.0 / 16.0);
        let image = match game.hero_art() {
            Some(url) => art_image(url, physical_width(width, window), false, cx),
            None => ArtImage::Missing,
        };
        // Store logos are drawn for dark backgrounds (often white lettering);
        // the card's fade is the theme's background, so light themes show
        // the title as text, as the game page does.
        let logo = game
            .art_logo
            .clone()
            .filter(|l| !l.is_empty() && cx.theme().mode.is_dark())
            .and_then(|url| {
                let px = physical_width(rems(CARD_REM * 0.6), window);
                art_img(&url, px, true, cx)
            });
        // Hovered cards glow in their art's colour.
        let glow = game
            .hero_art()
            .map(str::to_owned)
            .and_then(|url| irori_ui::art::ambient(&url, cx))
            .map(|a| BoxShadow {
                color: art_tint(a.rgb, cx).opacity(0.5),
                offset: point(px(0.), px(8.)),
                blur_radius: px(32.),
                spread_radius: px(-4.),
                inset: false,
            });
        let theme = cx.theme();
        let radius = theme.radius_lg;
        let muted_bg = theme.muted;
        let muted = theme.muted_foreground;

        let now = crate::format::now();
        let meta: Vec<String> = [
            status
                .as_ref()
                .filter(|s| s.kind.is_busy())
                .map(status_text),
            crate::format::last_played(last.as_deref(), now)
                .map(|when| t_args("library.card.last_played", &[("when", &when)]).to_string()),
            crate::format::playtime(minutes),
        ]
        .into_iter()
        .flatten()
        .collect();

        let bg = theme.background;
        let primary_color = theme.primary;
        let fade = linear_gradient(
            180.,
            linear_color_stop(bg.opacity(0.), 0.35),
            linear_color_stop(bg.opacity(0.92), 1.),
        );

        let primary = commands::primary(&state).map(|command| {
            let id = id.clone();
            let playing = command == GameCommand::Stop;
            let button = Button::new(ElementId::Name(format!("card-primary:{key}").into()))
                .icon(command.icon(&state))
                .label(command.label(&state))
                .loading(state.launching)
                .on_click(cx.listener(move |this, _, window, cx| {
                    cx.stop_propagation();
                    this.run_command(&id, command.clone(), window, cx)
                }));
            if playing { button } else { button.primary() }
        });

        let group = SharedString::from(format!("card-group:{key}"));
        let art = div()
            .relative()
            .w(width)
            .h(height)
            .rounded(radius)
            .overflow_hidden()
            .bg(muted_bg)
            .child(match image {
                ArtImage::Ready(img) => img
                    .size_full()
                    .rounded(radius)
                    .object_fit(ObjectFit::Cover)
                    .into_any_element(),
                ArtImage::Loading => art_loading(),
                ArtImage::Missing => art_missing(cx),
            })
            .child(div().absolute().top_0().left_0().size_full().bg(fade))
            .child(
                h_flex()
                    .absolute()
                    .bottom_0()
                    .left_0()
                    .right_0()
                    .p_3()
                    .gap_3()
                    .items_end()
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .child(match logo {
                                // The game's logo says its name in its own type.
                                // Its own height and width: sized only by its
                                // box, a logo drew at its own size, over the
                                // line below it.
                                Some(logo) => div()
                                    .h(rems(CARD_LOGO_REM))
                                    .max_w_full()
                                    .mb_1()
                                    .overflow_hidden()
                                    .child(
                                        logo.h(rems(CARD_LOGO_REM))
                                            .max_w(rems(CARD_REM * 0.6))
                                            .object_fit(ObjectFit::Contain),
                                    )
                                    .into_any_element(),
                                None => div()
                                    .text_lg()
                                    .font_extrabold()
                                    .truncate()
                                    .child(game.title.clone())
                                    .into_any_element(),
                            })
                            .child(
                                div()
                                    .text_xs()
                                    .font_medium()
                                    .text_color(muted)
                                    .truncate()
                                    .child(meta.join(" · ")),
                            ),
                    )
                    .children(primary)
                    .child(self.more_button(game, true, cx)),
            )
            .child(
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full()
                    .rounded(radius)
                    .border_2()
                    .border_color(primary_color.opacity(0.))
                    .group_hover(group.clone(), |s| {
                        s.border_color(primary_color.opacity(0.85))
                    }),
            );

        let view = cx.entity().downgrade();
        let menu_game = game.clone();
        let click_id = id.clone();
        div()
            .id(ElementId::Name(format!("card:{key}").into()))
            .keyboard_focusable(cx)
            .group(group)
            .w(width)
            .flex_shrink_0()
            .rounded(radius)
            .hover(move |s| match glow {
                Some(glow) => s.shadow(vec![glow]),
                None => s.shadow_lg(),
            })
            .on_click(move |_, _, cx| navigate(Route::Game(click_id.clone()), cx))
            .child(art)
            .context_menu(move |menu, window, cx| {
                game_menu(menu, &view, &menu_game, true, window, cx)
            })
            .into_any_element()
    }
}

/// A game's categories as small chips along its cover's foot, or its top
/// when the cover is drawn with the title at its foot.
fn category_chips(
    names: Vec<(String, Option<gpui_kit::Hsla>)>,
    at_foot: bool,
    scrim: gpui_kit::Hsla,
) -> AnyElement {
    let more = names.len().saturating_sub(COVER_CATEGORIES);
    h_flex()
        .absolute()
        .map(|el| if at_foot { el.bottom_2() } else { el.top_2() })
        .left_2()
        .right_2()
        .gap_1()
        .min_w_0()
        .overflow_hidden()
        .children(
            names
                .into_iter()
                .take(COVER_CATEGORIES)
                .map(|(name, colour)| {
                    h_flex()
                        .min_w_0()
                        .gap_1()
                        .px_1p5()
                        .rounded_md()
                        .bg(scrim)
                        .text_xs()
                        .when_some(colour, |el, colour| {
                            el.child(div().flex_none().size_1p5().rounded_full().bg(colour))
                        })
                        .child(div().min_w_0().truncate().child(name))
                }),
        )
        .when(more > 0, |el| {
            el.child(
                div()
                    .flex_none()
                    .px_1p5()
                    .rounded_md()
                    .bg(scrim)
                    .text_xs()
                    .child(format!("+{more}")),
            )
        })
        .into_any_element()
}

/// Render a store or launcher mark over cover art: SVG marks draw directly at
/// 20 px (`1.25 rem`) in white on a transparent background with a subtle
/// silhouette drop shadow for legibility over light or dark art; text-only
/// marks (`Amazon`, `ZOOM`, emulators) keep a compact pill.
fn cover_store_mark(mark: BadgeMark, scrim: gpui_kit::Hsla) -> AnyElement {
    let logo = match &mark {
        BadgeMark::Store(runner) => StoreLogo::of_runner(*runner),
        BadgeMark::Launcher(id) => StoreLogo::of_launcher(id),
        BadgeMark::Emulator { .. } => None,
    };
    let Some(logo) = logo else {
        return div()
            .rounded_md()
            .bg(scrim)
            .child(StoreBadge::mark(mark))
            .into_any_element();
    };
    let size = rems(1.25);
    let halo = |dx: f32, dy: f32, alpha: f32| {
        div().absolute().top(px(dy)).left(px(dx)).child(
            logo.icon()
                .size(size)
                .text_color(gpui_kit::black().opacity(alpha)),
        )
    };
    div()
        .relative()
        .size(size)
        .flex_shrink_0()
        .child(halo(0., 1.25, 0.65))
        .child(halo(-0.75, 0., 0.35))
        .child(halo(0.75, 0., 0.35))
        .child(halo(0., -0.75, 0.35))
        .child(
            div()
                .absolute()
                .top(px(0.))
                .left(px(0.))
                .child(logo.icon().size(size).text_color(gpui_kit::white())),
        )
        .into_any_element()
}
