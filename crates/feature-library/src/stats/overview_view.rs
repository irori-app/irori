//! The overview tab: headline over the art of the period's most played
//! game, insight cards, the chart stacked by game, and the most played games
//! as covers.

use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, Context, ElementId, Hsla, InteractiveElement as _, IntoElement,
    ParentElement as _, SharedString, StatefulInteractiveElement as _, Styled as _, Window, div,
    px, relative, rems,
};
use irori_core::{GameId, GameInfo};
use irori_services::LibrarySnapshot;
use irori_ui::components::{GameCover, SectionTitle};
use irori_ui::i18n::{t, t_args};
use irori_ui::style::{KeyboardFocusable as _, panel};
use irori_ui::{Route, models, navigate};

use super::backlog::Verdict;
use super::hero::chip;
use super::overview::{Chart, Overview};
use super::period::{Span, Unit};
use super::{StatsView, Tab, text};

/// Height of the chart's plot, in rem.
const CHART_REM: f32 = 12.;
/// Most played games shown as covers.
const TOP_GAMES: usize = 12;
/// Width of a top game's cover, in rem.
const TOP_COVER_REM: f32 = 8.5;

impl StatsView {
    pub(super) fn render_overview(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(o) = self.overview.clone() else {
            return div().into_any_element();
        };
        let library = models(cx).library.read(cx).snapshot.clone();
        let top = o.games.iter().find_map(|g| library.get(&g.id).cloned());
        let credit = top.as_ref().map(|game| {
            let why = t_args("library.stats.hero.art_credit", &[("title", &game.title)]);
            self.render_art_credit(game, why, cx)
        });
        let headline = self.render_overview_headline(cx);
        let art = top.as_ref().and_then(|g| g.hero_art()).map(str::to_owned);
        let hero = self.render_hero(art, headline, credit, window, cx);
        v_flex()
            .gap_6()
            .child(hero)
            .child(self.render_insights(&o, &library, cx))
            .child(self.render_chart(&o, &library, cx))
            .when(!o.games.is_empty(), |d| {
                d.child(self.render_top_games(&o, &library, cx))
            })
            .child(self.render_achievements_section(&library, window, cx))
            .into_any_element()
    }

    fn render_insights(
        &self,
        o: &Overview,
        library: &LibrarySnapshot,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let game = |id: &GameId| library.get(id).cloned();
        let title = |id: &GameId| -> SharedString {
            library
                .get(id)
                .map(|g| g.title.clone().into())
                .unwrap_or_else(|| id.app_name.to_string().into())
        };
        let mut cards: Vec<AnyElement> = Vec::new();

        if let Some(p) = &o.parallel {
            let first = p.games.first();
            cards.push(insight(
                "stats-insight-parallel",
                IconName::Layers,
                t("library.stats.insight.parallel.title"),
                text::count(
                    "library.stats.insight.parallel_one",
                    "library.stats.insight.parallel_other",
                    p.count as u64,
                ),
                first.and_then(game),
                first.cloned().map(Target::Game),
                cx,
            ));
        }

        if let Some(b) = &self.backlog {
            let latest = b.beaten.last().map(|m| m.id.clone());
            let body = match (b.goal, b.verdict) {
                (Some(goal), Some(Verdict::Reached)) => t_args(
                    "library.stats.insight.goal.reached",
                    &[("goal", &goal.to_string())],
                ),
                (Some(goal), _) => t_args(
                    "library.stats.insight.goal.progress",
                    &[
                        ("beaten", &b.beaten.len().to_string()),
                        ("goal", &goal.to_string()),
                    ],
                ),
                (None, _) => text::count(
                    "library.stats.insight.goal.beaten_one",
                    "library.stats.insight.goal.beaten_other",
                    b.beaten.len() as u64,
                ),
            };
            cards.push(insight(
                "stats-insight-goal",
                IconName::Flag,
                t_args(
                    "library.stats.insight.goal.title",
                    &[("year", &b.year.to_string())],
                ),
                body,
                latest.as_ref().and_then(game),
                Some(Target::Backlog),
                cx,
            ));
        }

        if let Some(c) = &o.comeback {
            cards.push(insight(
                "stats-insight-comeback",
                IconName::Undo2,
                t("library.stats.insight.comeback.title"),
                t_args(
                    "library.stats.insight.comeback",
                    &[
                        ("title", &title(&c.id)),
                        ("days", &c.after_days.to_string()),
                        ("date", &text::short_date(c.day)),
                    ],
                ),
                game(&c.id),
                Some(Target::Game(c.id.clone())),
                cx,
            ));
        }

        let new: Vec<&GameId> = o.new_games().map(|g| &g.id).collect();
        if let Some(first) = new.first() {
            let body = if new.len() == 1 {
                t_args("library.stats.insight.new.one", &[("title", &title(first))])
            } else {
                t_args(
                    "library.stats.insight.new.other",
                    &[("count", &new.len().to_string()), ("title", &title(first))],
                )
            };
            cards.push(insight(
                "stats-insight-new",
                IconName::Sparkles,
                t("library.stats.insight.new.title"),
                body,
                game(first),
                Some(Target::Game((*first).clone())),
                cx,
            ));
        }

        if let Some((body, latest)) = self.achievement_insight(o.period, library) {
            cards.push(insight(
                "stats-insight-achievements",
                IconName::Trophy,
                t("library.stats.insight.achievements.title"),
                body,
                latest.as_ref().and_then(game),
                latest.map(Target::Game),
                cx,
            ));
        }

        let album = self
            .album
            .as_ref()
            .filter(|a| a.period == o.period && !a.shots.is_empty());
        if let Some((album, (top, _))) = album.and_then(|a| Some((a, a.top()?))) {
            let count = album.shots.len().to_string();
            let body = match (album.shots.len(), album.games.len()) {
                (1, _) => t_args(
                    "library.stats.insight.screenshots.one",
                    &[("title", &title(top))],
                ),
                (_, 1) => t_args(
                    "library.stats.insight.screenshots.one_game",
                    &[("count", &count), ("title", &title(top))],
                ),
                _ => t_args(
                    "library.stats.insight.screenshots.other",
                    &[("count", &count), ("title", &title(top))],
                ),
            };
            cards.push(insight(
                "stats-insight-screenshots",
                IconName::Camera,
                t("library.stats.insight.screenshots.title"),
                body,
                game(top),
                Some(Target::Album),
                cx,
            ));
        }

        h_flex()
            .w_full()
            .flex_wrap()
            .items_stretch()
            .gap_4()
            .children(cards)
            .into_any_element()
    }

    fn render_chart(
        &self,
        o: &Overview,
        library: &LibrarySnapshot,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let colors = series_colors(&o.chart, cx);
        let name = |id: &Option<GameId>| -> SharedString {
            match id {
                Some(id) => library
                    .get(id)
                    .map(|g| g.title.clone().into())
                    .unwrap_or_else(|| id.app_name.to_string().into()),
                None => t("library.stats.chart.other_games"),
            }
        };
        let title = t(match o.chart.unit {
            Unit::Day => "library.stats.chart.days",
            Unit::Month => "library.stats.chart.months",
            Unit::Year => "library.stats.chart.years",
        });
        let busiest = o.busiest().map(|(start, seconds)| {
            t_args(
                match o.chart.unit {
                    Unit::Day => "library.stats.chart.busiest_day",
                    Unit::Month => "library.stats.chart.busiest_month",
                    Unit::Year => "library.stats.chart.busiest_year",
                },
                &[
                    ("when", &text::bar_name(o.chart.unit, start)),
                    ("duration", &text::duration(seconds)),
                ],
            )
        });
        let names: Vec<SharedString> = o.chart.series.iter().map(name).collect();
        let legend = h_flex()
            .flex_wrap()
            .gap_x_4()
            .gap_y_1()
            .text_xs()
            .text_color(muted)
            .children(names.iter().zip(&colors).map(|(n, c)| {
                h_flex()
                    .gap_1p5()
                    .items_center()
                    .min_w_0()
                    .child(div().size_2().flex_shrink_0().rounded_full().bg(*c))
                    .child(div().max_w(rems(14.)).truncate().child(n.clone()))
            }));
        let plot = if o.chart.bars.iter().any(|b| b.total() > 0) {
            plot(o, &colors, &names, cx)
        } else {
            div()
                .h(rems(CHART_REM))
                .w_full()
                .flex()
                .items_center()
                .justify_center()
                .text_sm()
                .text_color(muted)
                .child(t("library.stats.chart.empty"))
                .into_any_element()
        };
        panel(cx)
            .gap_4()
            .child(
                h_flex()
                    .flex_wrap()
                    .justify_between()
                    .gap_2()
                    .child(SectionTitle::new(title))
                    .children(busiest.map(|b| div().text_xs().text_color(muted).child(b))),
            )
            .child(plot)
            .when(!o.chart.series.is_empty(), |d| d.child(legend))
            .when_some(
                self.history.as_ref().and_then(|h| h.first_day()),
                |d, first| {
                    d.when(o.period.span == Span::All || first > o.period.start, |d| {
                        d.child(div().text_xs().text_color(muted).child(t_args(
                            "library.stats.recorded_since",
                            &[("date", &text::month_year(first))],
                        )))
                    })
                },
            )
            .into_any_element()
    }

    fn render_top_games(
        &self,
        o: &Overview,
        library: &LibrarySnapshot,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme();
        let (muted, radius, ring) = (theme.muted_foreground, theme.radius_lg, theme.ring);
        let new_count = o.new_games().count();
        let covers: Vec<AnyElement> = o
            .games
            .iter()
            .filter_map(|g| Some((g, library.get(&g.id).cloned()?)))
            .take(TOP_GAMES)
            .enumerate()
            .map(|(rank, (g, info))| {
                let id = g.id.clone();
                let new = g
                    .new
                    .then(|| chip(None, t("library.stats.top.new"), theme.info));
                v_flex()
                    .id(ElementId::Name(format!("stats-top:{}", g.id.key()).into()))
                    .keyboard_focusable(cx)
                    .w(rems(TOP_COVER_REM))
                    .flex_shrink_0()
                    .gap_1p5()
                    .rounded(radius)
                    .cursor_pointer()
                    .on_click(move |_, _, cx| navigate(Route::Game(id.clone()), cx))
                    .child(
                        div()
                            .relative()
                            .rounded(radius)
                            .when(rank == 0, |d| d.border_2().border_color(ring))
                            .child(GameCover::new(info.clone()).width(rems(TOP_COVER_REM)))
                            .child(
                                div()
                                    .absolute()
                                    .top_1p5()
                                    .left_1p5()
                                    .size_6()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded_full()
                                    .bg(theme.background.opacity(0.75))
                                    .text_xs()
                                    .font_bold()
                                    .child((rank + 1).to_string()),
                            )
                            .children(new.map(|n| div().absolute().top_1p5().right_1p5().child(n))),
                    )
                    .child(
                        div()
                            .text_sm()
                            .font_medium()
                            .truncate()
                            .child(info.title.clone()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(muted)
                            .child(text::duration(g.seconds)),
                    )
                    .into_any_element()
            })
            .collect();
        panel(cx)
            .gap_4()
            .child(
                h_flex()
                    .justify_between()
                    .gap_2()
                    .child(SectionTitle::new(t("library.stats.top.title")))
                    .when(new_count > 0, |d| {
                        d.child(div().text_xs().text_color(muted).child(text::count(
                            "library.stats.top.new_count_one",
                            "library.stats.top.new_count_other",
                            new_count as u64,
                        )))
                    }),
            )
            .child(
                div()
                    .id("stats-top-games")
                    .w_full()
                    .overflow_x_scrollbar()
                    .child(h_flex().gap_4().pb_2().items_start().children(covers)),
            )
            .into_any_element()
    }
}

/// Where an insight card leads.
#[derive(Clone)]
enum Target {
    Game(GameId),
    Backlog,
    Album,
}

/// A card with an icon, a caption, one line of text and the cover of the
/// game it is about.
fn insight(
    id: &'static str,
    icon: IconName,
    caption: SharedString,
    body: SharedString,
    game: Option<Arc<GameInfo>>,
    target: Option<Target>,
    cx: &mut Context<StatsView>,
) -> AnyElement {
    let theme = cx.theme();
    let (muted, hover) = (theme.muted_foreground, theme.list_hover);
    h_flex()
        .id(id)
        .keyboard_focusable(cx)
        .flex_1()
        .min_w(rems(16.))
        .gap_3()
        .items_center()
        .p_3()
        .rounded(theme.radius_lg)
        .bg(theme.secondary)
        .border_1()
        .border_color(theme.border)
        .when(target.is_some(), |d| {
            d.cursor_pointer().hover(move |s| s.bg(hover))
        })
        .child(
            div()
                .size_9()
                .flex_shrink_0()
                .flex()
                .items_center()
                .justify_center()
                .rounded_full()
                .bg(theme.muted)
                .child(Icon::new(icon).small().text_color(muted)),
        )
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .gap_0p5()
                .child(
                    div()
                        .text_xs()
                        .font_bold()
                        .text_color(muted)
                        .truncate()
                        .child(SharedString::from(caption.to_uppercase())),
                )
                .child(div().text_sm().line_clamp(3).child(body)),
        )
        .children(game.map(|g| GameCover::new(g).width(rems(2.25)).radius(px(4.))))
        .when(target.is_some(), |d| {
            d.child(Icon::new(IconName::ChevronRight).small().text_color(muted))
        })
        .when_some(target, |d, target| {
            d.on_click(cx.listener(move |this, _, _, cx| match &target {
                Target::Game(id) => navigate(Route::Game(id.clone()), cx),
                Target::Backlog => this.set_tab(Tab::Backlog, cx),
                Target::Album => this.set_tab(Tab::Album, cx),
            }))
        })
        .into_any_element()
}

/// One colour per chart series: the theme's chart colours, then a quiet one
/// for all other games.
fn series_colors(chart: &Chart, cx: &App) -> Vec<Hsla> {
    let theme = cx.theme();
    let palette = [
        theme.chart_1,
        theme.chart_2,
        theme.chart_3,
        theme.chart_4,
        theme.chart_5,
    ];
    chart
        .series
        .iter()
        .enumerate()
        .map(|(ix, id)| match id {
            Some(_) => palette[ix % palette.len()],
            None => theme.muted_foreground.opacity(0.35),
        })
        .collect()
}

/// The largest value of the hours axis: a round number at or above `hours`.
fn axis_top(hours: f64) -> f64 {
    let steps = [0.5, 1., 2., 3., 4., 6., 8., 10., 12., 16., 20., 24.];
    if let Some(step) = steps.iter().find(|s| **s >= hours) {
        return *step;
    }
    let magnitude = 10f64.powf(hours.log10().floor());
    (hours / magnitude).ceil() * magnitude
}

/// Bars stacked per game, with an hours axis and a label under each bar.
fn plot(
    o: &Overview,
    colors: &[Hsla],
    names: &[SharedString],
    cx: &mut Context<StatsView>,
) -> AnyElement {
    let theme = cx.theme();
    let (muted, grid, radius) = (theme.muted_foreground, theme.border.opacity(0.6), px(3.));
    let max = o.chart.bars.iter().map(|b| b.total()).max().unwrap_or(0);
    let top = axis_top(max as f64 / 3600.);
    let today = chrono::Local::now().date_naive();
    let unit = o.chart.unit;
    let ticks = [1.0f32, 0.5, 0.];
    let axis = div()
        .relative()
        .w(rems(2.75))
        .h(rems(CHART_REM))
        .flex_shrink_0()
        .children(ticks.iter().map(|f| {
            div()
                .absolute()
                .right_2()
                .top(relative(1. - f))
                .mt(rems(-0.45))
                .text_xs()
                .text_color(muted)
                .child(text::hours_tick(top * f64::from(*f)))
        }));
    let bars = o.chart.bars.iter().enumerate().map(|(ix, bar)| {
        let total = bar.total();
        let height = (total as f64 / 3600. / top).clamp(0., 1.) as f32;
        let mut tip = vec![format!(
            "{} · {}",
            text::bar_name(unit, bar.start),
            text::duration(total)
        )];
        for (part, name) in bar.parts.iter().zip(names) {
            if *part > 0 {
                tip.push(format!("{name}: {}", text::duration(*part)));
            }
        }
        let tip: SharedString = tip.join("\n").into();
        let stack = v_flex()
            .w_full()
            .max_w(rems(2.25))
            .h(relative(height))
            .when(total > 0, |d| d.min_h(px(2.)))
            .rounded_t(radius)
            .overflow_hidden()
            // The most played game sits at the bottom of each bar.
            .children(
                bar.parts
                    .iter()
                    .zip(colors)
                    .rev()
                    .filter(|(p, _)| **p > 0)
                    .map(|(part, color)| {
                        div()
                            .w_full()
                            .h(relative(*part as f32 / total.max(1) as f32))
                            .bg(*color)
                    }),
            );
        div()
            .id(ElementId::Name(format!("stats-bar-{ix}").into()))
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .flex_col()
            .justify_end()
            .items_center()
            .when(total > 0, |d| {
                d.tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
            })
            .child(stack)
    });
    let labels = o.chart.bars.iter().map(|bar| {
        let is_today = unit == Unit::Day && bar.start == today;
        div()
            .flex_1()
            .min_w_0()
            .text_center()
            .text_xs()
            .overflow_hidden()
            .text_color(if is_today { theme.foreground } else { muted })
            .when(is_today, |d| d.font_bold())
            .child(text::bar_label(unit, bar.start, o.period.span))
    });
    // Narrow pages clip labels rather than wrap them.
    let gridlines = ticks.iter().map(|f| {
        div()
            .absolute()
            .left_0()
            .right_0()
            .top(relative(1. - f))
            .h(px(1.))
            .bg(grid)
    });
    h_flex()
        .w_full()
        .items_start()
        .child(axis)
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .gap_1p5()
                .child(
                    div()
                        .relative()
                        .w_full()
                        .h(rems(CHART_REM))
                        .children(gridlines)
                        .child(h_flex().size_full().gap(px(3.)).children(bars)),
                )
                .child(h_flex().w_full().gap(px(3.)).children(labels)),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::axis_top;

    #[test]
    fn axis_tops_are_round() {
        assert_eq!(axis_top(0.2), 0.5);
        assert_eq!(axis_top(4.7), 6.);
        assert_eq!(axis_top(30.), 30.);
        assert_eq!(axis_top(31.), 40.);
        assert_eq!(axis_top(490.), 500.);
    }
}
