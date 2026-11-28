//! Pinned live preview of the library at the top of Settings → Appearance.
//!
//! Draws from the user's real covers, theme, accent, font, density, cover
//! shape, cover corners, overlays, view tabs strip, and library backdrop so
//! every Appearance adjustment is visible immediately without scrolling back
//! to the top.

use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, Context, IntoElement, ParentElement as _, Render, SharedString, Styled as _,
    Subscription, Window, div, px, rems,
};
use irori_core::GameInfo;
use irori_core::settings::{CoverCorners, CoverShape, LibraryBackground};
use irori_ui::components::{BackdropInk, GameCover, library_backdrop, library_backdrop_url};
use irori_ui::i18n::{t, t_args};
use irori_ui::style::{TEXT_CONTRAST, legible};
use irori_ui::{backend, models};

use super::ui_prefs;

/// Covers the preview shows.
const PREVIEW_COVERS: usize = 5;
/// Width of a portrait preview cover at comfortable density, in rem.
const PREVIEW_COVER_REM: f32 = 3.5;
/// Space between preview covers at comfortable density, in rem.
const PREVIEW_GAP_REM: f32 = 0.65;

/// A compact live library preview pinned at the top of Settings → Appearance.
pub(crate) struct AppearancePreview {
    _subscriptions: Vec<Subscription>,
}

impl AppearancePreview {
    pub(crate) fn new(cx: &mut Context<Self>) -> Self {
        let m = models(cx).clone();
        let art = irori_ui::art::art_store(cx);
        Self {
            _subscriptions: vec![
                cx.observe(&m.settings, |_, _, cx| cx.notify()),
                cx.observe(&m.library, |_, _, cx| cx.notify()),
                cx.observe(&art, |_, _, cx| cx.notify()),
            ],
        }
    }

    /// Covers to show: recently played games first, then any with art.
    fn games(cx: &App) -> Vec<Arc<GameInfo>> {
        let b = backend(cx);
        let mut games = b.recent_games(false);
        let library = models(cx).library.read(cx).snapshot.clone();
        for game in library.games.iter() {
            if games.len() >= PREVIEW_COVERS {
                break;
            }
            if game.cover_art().is_some() && !games.iter().any(|g| g.id() == game.id()) {
                games.push(game.clone());
            }
        }
        games.truncate(PREVIEW_COVERS);
        games
    }
}

impl Render for AppearancePreview {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let prefs = ui_prefs(cx);
        let games = Self::games(cx);
        let installed_count = models(cx)
            .library
            .read(cx)
            .snapshot
            .games
            .iter()
            .filter(|g| g.is_installed)
            .count();
        let url = library_backdrop_url(&prefs, games.first().map(|g| g.as_ref()), cx);
        let ink = BackdropInk::of(url.as_deref(), cx);
        let theme = cx.theme();
        let (border, radius_lg, ring, primary, secondary, muted_fg) = (
            theme.border,
            theme.radius_lg,
            theme.ring,
            theme.primary,
            theme.secondary,
            theme.muted_foreground,
        );
        let active_chip_fg = legible(primary, &[theme.background, secondary], TEXT_CONTRAST, cx);

        let cover_radius = match prefs.cover_corners {
            CoverCorners::Square => px(2.),
            CoverCorners::Rounded => radius_lg,
            CoverCorners::Round => radius_lg * 1.75,
        };
        let shape = prefs.cover_shape;
        let cover_w = rems(PREVIEW_COVER_REM * prefs.density.cover_scale() * shape.width_scale());
        let cover_h = rems(cover_w.0 * shape.ratio());
        let gap = rems(PREVIEW_GAP_REM * prefs.density.spacing_scale());
        let visible_count = if shape == CoverShape::Banner {
            3
        } else {
            PREVIEW_COVERS
        };

        let note = match prefs.library_background {
            LibraryBackground::None => None,
            _ if irori_ui::theme::reduce_transparency(cx) => {
                Some(t("settings.preview.background_off_transparency"))
            }
            LibraryBackground::Picture if prefs.library_background_picture.trim().is_empty() => {
                Some(t("settings.preview.background_no_picture"))
            }
            _ => None,
        };

        let subtitle: SharedString = if installed_count > 0 {
            let count_str = installed_count.to_string();
            let key = if installed_count == 1 {
                "settings.preview.installed_one"
            } else {
                "settings.preview.installed_other"
            };
            t_args(key, &[("count", &count_str)])
        } else {
            t("settings.preview.subtitle_empty")
        };

        let covers: Vec<AnyElement> = if games.is_empty() {
            (0..visible_count)
                .map(|ix| {
                    div()
                        .w(cover_w)
                        .h(cover_h)
                        .flex_shrink_0()
                        .rounded(cover_radius)
                        .bg(ink.panel)
                        .border_1()
                        .border_color(border)
                        .when(ix == 0, |d| d.border_2().border_color(ring))
                        .into_any_element()
                })
                .collect()
        } else {
            games
                .iter()
                .take(visible_count)
                .enumerate()
                .map(|(ix, game)| {
                    let show_star = prefs.cover_favourite && ix == 0;
                    v_flex()
                        .w(cover_w)
                        .flex_shrink_0()
                        .gap_1()
                        .child(
                            div()
                                .relative()
                                .rounded(cover_radius)
                                .when(ix == 0, |d| d.border_2().border_color(ring))
                                .child(
                                    GameCover::new(game.clone())
                                        .width(cover_w)
                                        .shape(shape)
                                        .radius(cover_radius),
                                )
                                .when(show_star, |d| {
                                    d.child(
                                        div()
                                            .absolute()
                                            .top_1()
                                            .right_1()
                                            .p_0p5()
                                            .rounded_full()
                                            .bg(theme.background.opacity(0.75))
                                            .child(
                                                Icon::new(IconName::Star)
                                                    .xsmall()
                                                    .text_color(primary),
                                            ),
                                    )
                                }),
                        )
                        .child(
                            div()
                                .text_xs()
                                .min_w_0()
                                .truncate()
                                .text_color(ink.text)
                                .child(game.title.clone()),
                        )
                        .into_any_element()
                })
                .collect()
        };

        v_flex()
            .gap_1p5()
            .child(
                div()
                    .relative()
                    .w_full()
                    .overflow_hidden()
                    .rounded(radius_lg)
                    .border_1()
                    .border_color(border)
                    .bg(theme.background)
                    .child(library_backdrop(&prefs, url))
                    .child(
                        v_flex()
                            .relative()
                            .p_3()
                            .gap(gap)
                            .child(
                                h_flex()
                                    .gap_3()
                                    .items_center()
                                    .justify_between()
                                    .child(
                                        v_flex()
                                            .flex_1()
                                            .min_w_0()
                                            .gap_1()
                                            .child(
                                                h_flex()
                                                    .items_baseline()
                                                    .gap_2()
                                                    .min_w_0()
                                                    .child(
                                                        div()
                                                            .text_base()
                                                            .font_semibold()
                                                            .text_color(ink.text)
                                                            .truncate()
                                                            .child(t("settings.preview.library")),
                                                    )
                                                    .child(
                                                        div()
                                                            .text_xs()
                                                            .text_color(ink.muted)
                                                            .truncate()
                                                            .child(subtitle),
                                                    ),
                                            )
                                            .when(prefs.library_quick_controls, |col| {
                                                col.child(
                                                    h_flex()
                                                        .gap_1p5()
                                                        .child(
                                                            div()
                                                                .px_2()
                                                                .py_0p5()
                                                                .rounded_md()
                                                                .bg(primary.opacity(0.18))
                                                                .text_xs()
                                                                .font_semibold()
                                                                .text_color(active_chip_fg)
                                                                .child(t("settings.preview.tab_all")),
                                                        )
                                                        .child(
                                                            div()
                                                                .px_2()
                                                                .py_0p5()
                                                                .rounded_md()
                                                                .bg(secondary.opacity(0.85))
                                                                .text_xs()
                                                                .text_color(muted_fg)
                                                                .child(t(
                                                                    "settings.preview.tab_installed",
                                                                )),
                                                        )
                                                        .child(
                                                            div()
                                                                .px_2()
                                                                .py_0p5()
                                                                .rounded_md()
                                                                .bg(secondary.opacity(0.85))
                                                                .text_xs()
                                                                .text_color(muted_fg)
                                                                .child(t(
                                                                    "settings.preview.tab_favourites",
                                                                )),
                                                        ),
                                                )
                                            }),
                                    )
                                    .child(
                                        Button::new("preview-play")
                                            .primary()
                                            .small()
                                            .icon(IconName::Play)
                                            .label(t("settings.preview.play")),
                                    ),
                            )
                            .child(
                                h_flex()
                                    .gap(gap)
                                    .items_start()
                                    .overflow_hidden()
                                    .children(covers),
                            ),
                    ),
            )
            .when_some(note, |v, note| {
                v.child(div().text_xs().text_color(muted_fg).child(note))
            })
    }
}
