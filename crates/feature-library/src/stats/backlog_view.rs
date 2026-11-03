//! The backlog tab: games beaten in the year over the art of the latest
//! one, the yearly goal with its ring and pace, the year as a timeline
//! (beaten games above the axis, abandoned ones below) and the whole library
//! as one bar by play status.

use chrono::{Datelike as _, NaiveDate};
use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::progress::ProgressCircle;
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, Context, ElementId, InteractiveElement as _, IntoElement, ParentElement as _,
    SharedString, StatefulInteractiveElement as _, Styled as _, Window, div, px, relative, rems,
};
use irori_services::{EffectiveStatus, LibrarySnapshot, PlayStatus};
use irori_ui::components::{GameCover, SectionTitle};
use irori_ui::i18n::{t, t_args};
use irori_ui::style::{KeyboardFocusable as _, panel};
use irori_ui::{Route, backend, models, navigate, play_status};

use super::backlog::{Backlog, Mark, Verdict};
use super::hero::{big_number, dotted, eyebrow};
use super::{StatsView, text};

/// A goal suggested when the user sets one for the first time.
const DEFAULT_GOAL: u32 = 12;
/// Room on each side of the axis without covers, in rem: month labels
/// below, the "today" label above.
const TIMELINE_MIN_REM: f32 = 3.;
/// Width of a cover on the timeline, in rem.
const MARK_REM: f32 = 2.25;
/// Covers closer than this share of the year stack instead of overlapping.
const MARK_GAP: f32 = 0.035;
/// Rows of covers on each side of the axis.
const MARK_LANES: usize = 3;

impl StatsView {
    pub(super) fn render_backlog(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(b) = self.backlog.clone() else {
            return div().into_any_element();
        };
        let library = models(cx).library.read(cx).snapshot.clone();
        let latest = b
            .beaten
            .iter()
            .rev()
            .find_map(|m| library.get(&m.id).cloned());
        let headline = self.render_backlog_headline(&b, cx);
        let goal = self.render_goal(&b, cx);
        let hero = self.render_hero(latest.as_ref(), headline, Some(goal), window, cx);
        v_flex()
            .gap_6()
            .child(hero)
            .child(self.render_year(&b, &library, cx))
            .child(self.render_library_bar(&b, cx))
            .into_any_element()
    }

    fn render_backlog_headline(&self, b: &Backlog, cx: &Context<Self>) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let facts = vec![
            text::count(
                "library.stats.backlog.abandoned_one",
                "library.stats.backlog.abandoned_other",
                b.abandoned.len() as u64,
            ),
            text::count(
                "library.stats.backlog.in_library_one",
                "library.stats.backlog.in_library_other",
                b.library_total() as u64,
            ),
        ];
        v_flex()
            .gap_2()
            .child(eyebrow(
                t_args(
                    "library.stats.backlog.eyebrow",
                    &[("year", &b.year.to_string())],
                ),
                cx,
            ))
            .child(
                h_flex()
                    .gap_3()
                    .items_baseline()
                    .child(big_number(b.beaten.len().to_string().into()))
                    .child(div().text_3xl().font_bold().child(text::count_word(
                        "library.stats.backlog.beaten_word_one",
                        "library.stats.backlog.beaten_word_other",
                        b.beaten.len() as u64,
                    ))),
            )
            .child(
                h_flex()
                    .flex_wrap()
                    .gap_x_3()
                    .text_sm()
                    .children(dotted(facts, muted)),
            )
            .into_any_element()
    }

    /// The goal card: a ring of games beaten against the goal, − goal +,
    /// and how the pace looks.
    fn render_goal(&self, b: &Backlog, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let (muted, success) = (theme.muted_foreground, theme.success);
        let year = b.year;
        let set_goal = move |goal: Option<u32>, cx: &mut gpui_kit::App| {
            let b = backend(cx);
            irori_ui::state::run_infallible(
                cx,
                async move { b.userdata.set_backlog_goal(year, goal) },
            )
            .detach();
        };
        let card = h_flex()
            .flex_shrink_0()
            .gap_4()
            .items_center()
            .p_4()
            .rounded(theme.radius_lg)
            .bg(theme.background.opacity(0.78))
            .border_1()
            .border_color(theme.border);
        let Some(goal) = b.goal else {
            return card
                .child(
                    v_flex()
                        .gap_2()
                        .max_w(rems(16.))
                        .child(div().font_semibold().child(t_args(
                            "library.stats.goal.title",
                            &[("year", &year.to_string())],
                        )))
                        .child(
                            div()
                                .text_sm()
                                .text_color(muted)
                                .child(t("library.stats.goal.none")),
                        )
                        .child(
                            Button::new("stats-goal-set")
                                .small()
                                .outline()
                                .icon(Icon::new(IconName::Target))
                                .label(t("library.stats.goal.set"))
                                .on_click(move |_, _, cx| set_goal(Some(DEFAULT_GOAL), cx)),
                        ),
                )
                .into_any_element();
        };
        let beaten = b.beaten.len() as u32;
        let ring_color = if beaten >= goal {
            success
        } else {
            theme.chart_1
        };
        let ring = ProgressCircle::new("stats-goal-ring")
            .size(rems(6.5))
            .color(ring_color)
            .value(beaten as f32 / goal as f32 * 100.)
            .accessibility_label(t_args(
                "library.stats.goal.progress",
                &[("beaten", &beaten.to_string()), ("goal", &goal.to_string())],
            ))
            .child(
                v_flex()
                    .items_center()
                    .child(
                        h_flex()
                            .items_baseline()
                            .child(div().text_2xl().font_extrabold().child(beaten.to_string()))
                            .child(div().text_sm().text_color(muted).child(format!("/{goal}"))),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(muted)
                            .child(play_status::label(PlayStatus::Beaten)),
                    ),
            );
        let verdict = b.verdict.map(|v| {
            let (key, color) = match v {
                Verdict::Ahead => ("library.stats.goal.ahead", success),
                Verdict::OnPace => ("library.stats.goal.on_pace", success),
                Verdict::SlightlyBehind => ("library.stats.goal.slightly_behind", theme.warning),
                Verdict::Behind => ("library.stats.goal.behind", theme.danger),
                Verdict::Reached => ("library.stats.goal.reached", success),
                Verdict::Missed => ("library.stats.goal.missed", theme.danger),
            };
            div()
                .text_sm()
                .font_semibold()
                .text_color(color)
                .child(t(key))
        });
        let planned = b.count(EffectiveStatus::PlanToPlay);
        let mut lines: Vec<SharedString> = Vec::new();
        if let Some(p) = b.pace {
            lines.push(t_args(
                "library.stats.goal.expected",
                &[("count", &format!("{:.1}", p.expected))],
            ));
            lines.push(t_args(
                "library.stats.goal.projected",
                &[("count", &p.projected.to_string())],
            ));
            if let Some(days) = p.every_days {
                lines.push(t_args(
                    "library.stats.goal.every",
                    &[
                        ("days", &days.to_string()),
                        ("remaining", &p.remaining.to_string()),
                    ],
                ));
            }
        }
        if planned > 0 && b.verdict != Some(Verdict::Missed) {
            lines.push(text::count(
                "library.stats.goal.planned_one",
                "library.stats.goal.planned_other",
                planned as u64,
            ));
        }
        card.child(ring)
            .child(
                v_flex()
                    .gap_1()
                    .max_w(rems(17.))
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(div().font_semibold().child(t_args(
                                "library.stats.goal.title",
                                &[("year", &year.to_string())],
                            )))
                            .child(
                                Button::new("stats-goal-less")
                                    .ghost()
                                    .xsmall()
                                    .icon(Icon::new(IconName::Minus))
                                    .accessibility_label(t("library.stats.goal.less"))
                                    .tooltip(t("library.stats.goal.less"))
                                    .on_click(move |_, _, cx| set_goal(Some(goal - 1), cx)),
                            )
                            .child(div().font_semibold().child(goal.to_string()))
                            .child(
                                Button::new("stats-goal-more")
                                    .ghost()
                                    .xsmall()
                                    .icon(Icon::new(IconName::Plus))
                                    .accessibility_label(t("library.stats.goal.more"))
                                    .tooltip(t("library.stats.goal.more"))
                                    .on_click(move |_, _, cx| set_goal(Some(goal + 1), cx)),
                            ),
                    )
                    .children(verdict)
                    .children(
                        lines
                            .into_iter()
                            .map(|l| div().text_xs().text_color(muted).child(l)),
                    ),
            )
            .into_any_element()
    }

    /// Figures for the year and the timeline.
    fn render_year(
        &self,
        b: &Backlog,
        library: &LibrarySnapshot,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let figure = |value: SharedString, label: SharedString| {
            v_flex()
                .flex_1()
                .min_w(rems(10.))
                .gap_0p5()
                .pl_3()
                .border_l_1()
                .border_color(theme.border)
                .child(div().text_2xl().font_bold().child(value))
                .child(div().text_xs().text_color(muted).child(label))
        };
        let median = b
            .median_days_to_beat
            .map(|d| {
                text::count(
                    "library.stats.year.days_one",
                    "library.stats.year.days_other",
                    u64::from(d),
                )
            })
            .unwrap_or_else(|| "—".into());
        let legend = |status: PlayStatus| {
            h_flex()
                .gap_1p5()
                .items_center()
                .child(
                    div()
                        .size_2()
                        .rounded_full()
                        .bg(play_status::color(status, cx)),
                )
                .child(play_status::label(status))
        };
        panel(cx)
            .gap_4()
            .child(
                h_flex()
                    .flex_wrap()
                    .justify_between()
                    .gap_2()
                    .child(
                        v_flex()
                            .gap_0p5()
                            .child(SectionTitle::new(t("library.stats.year.title")))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(muted)
                                    .child(t("library.stats.year.description")),
                            ),
                    )
                    .child(
                        h_flex()
                            .gap_4()
                            .text_xs()
                            .text_color(muted)
                            .child(legend(PlayStatus::Beaten))
                            .child(legend(PlayStatus::Abandoned)),
                    ),
            )
            .child(
                h_flex()
                    .flex_wrap()
                    .gap_4()
                    .child(figure(
                        text::hours(b.beaten_minutes),
                        t("library.stats.year.beaten_time"),
                    ))
                    .child(figure(median, t("library.stats.year.median")))
                    .child(figure(
                        text::hours(b.abandoned_minutes),
                        t("library.stats.year.abandoned_time"),
                    )),
            )
            .child(timeline(b, library, cx))
            .into_any_element()
    }

    /// The library as one bar by status, with a legend that opens the
    /// library filtered to a status.
    fn render_library_bar(&self, b: &Backlog, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let (muted, track, hover, radius) = (
            theme.muted_foreground,
            theme.muted,
            theme.list_hover,
            theme.radius,
        );
        let total = b.library_total().max(1) as f32;
        let segments = b.library.iter().filter(|(_, n)| *n > 0).map(|(status, n)| {
            div()
                .h_full()
                .w(relative(*n as f32 / total))
                .bg(play_status::color(*status, cx))
        });
        let legend = b.library.iter().map(|(status, n)| {
            let status = *status;
            h_flex()
                .id(ElementId::Name(format!("stats-library-{status:?}").into()))
                .keyboard_focusable(cx)
                .gap_1p5()
                .items_center()
                .px_1p5()
                .py_0p5()
                .rounded(radius)
                .cursor_pointer()
                .hover(move |s| s.bg(hover))
                .on_click(move |_, _, cx| show_in_library(status, cx))
                .child(
                    div()
                        .size_2()
                        .rounded_full()
                        .bg(play_status::color(status, cx)),
                )
                .child(div().text_color(muted).child(play_status::label(status)))
                .child(div().font_semibold().child(n.to_string()))
        });
        panel(cx)
            .gap_3()
            .child(
                h_flex()
                    .justify_between()
                    .gap_2()
                    .child(SectionTitle::new(t("library.stats.library.title")))
                    .child(div().text_xs().text_color(muted).child(text::count(
                        "library.stats.library.games_one",
                        "library.stats.library.games_other",
                        b.library_total() as u64,
                    ))),
            )
            .child(
                h_flex()
                    .w_full()
                    .h(px(12.))
                    .gap(px(2.))
                    .rounded_full()
                    .overflow_hidden()
                    .bg(track)
                    .children(segments),
            )
            .child(
                h_flex()
                    .flex_wrap()
                    .gap_x_2()
                    .gap_y_1()
                    .text_xs()
                    .children(legend),
            )
            .into_any_element()
    }
}

/// Open the library showing only games of `status`.
fn show_in_library(status: EffectiveStatus, cx: &mut gpui_kit::App) {
    if let Some(filters) = crate::filters_panel::active_filters(cx) {
        filters.update(cx, |m, cx| {
            m.update(
                |f| {
                    f.clear_all();
                    f.statuses = [status].into();
                },
                cx,
            )
        });
    }
    navigate(Route::Library, cx);
}

/// Where a day falls in its year, from 0 to 1.
fn year_fraction(day: NaiveDate) -> f32 {
    let days = if NaiveDate::from_ymd_opt(day.year(), 2, 29).is_some() {
        366.
    } else {
        365.
    };
    (day.ordinal0() as f32 + 0.5) / days
}

/// A lane per mark, so that covers too close together stack.
fn lanes(marks: &[Mark]) -> Vec<usize> {
    let mut last: Vec<f32> = Vec::new();
    marks
        .iter()
        .map(|m| {
            let x = year_fraction(m.day);
            let lane = match last.iter().position(|l| x - l >= MARK_GAP) {
                Some(lane) => lane,
                None if last.len() < MARK_LANES => {
                    last.push(f32::MIN);
                    last.len() - 1
                }
                // Out of room: the lane free the longest.
                None => last
                    .iter()
                    .enumerate()
                    .min_by(|a, b| a.1.total_cmp(b.1))
                    .map_or(0, |(i, _)| i),
            };
            last[lane] = x;
            lane
        })
        .collect()
}

/// The year from January to December: month ticks on the axis, beaten
/// games above it, abandoned ones below, and today.
fn timeline(b: &Backlog, library: &LibrarySnapshot, cx: &mut Context<StatsView>) -> AnyElement {
    let theme = cx.theme();
    let (muted, axis, fg) = (theme.muted_foreground, theme.border, theme.foreground);
    let cover_h = MARK_REM * 1.5;
    let (beaten_lanes, abandoned_lanes) = (lanes(&b.beaten), lanes(&b.abandoned));
    // The axis sits as low as the stacked covers above it need.
    let side = |lanes: &[usize]| {
        lanes
            .iter()
            .max()
            .map_or(TIMELINE_MIN_REM, |top| {
                0.7 + (*top as f32 + 1.) * (cover_h + 0.3) + 0.6
            })
            .max(TIMELINE_MIN_REM)
    };
    let middle = side(&beaten_lanes);
    let height = middle + side(&abandoned_lanes) + 0.6;
    let today = chrono::Local::now().date_naive();
    let months = (1..=12).filter_map(|m| NaiveDate::from_ymd_opt(b.year, m, 1));
    let ticks = months.map(|first| {
        div()
            .absolute()
            .left(relative(year_fraction(first) - 0.5 / 365.))
            .top(rems(middle))
            .child(div().w(px(1.)).h(rems(0.4)).bg(axis))
            .child(
                div()
                    .text_xs()
                    .text_color(muted)
                    .ml(rems(0.25))
                    .child(text::month_short(first)),
            )
    });
    let mut marks: Vec<AnyElement> = Vec::new();
    for (list, list_lanes, above, color) in [
        (
            &b.beaten,
            &beaten_lanes,
            true,
            play_status::color(PlayStatus::Beaten, cx),
        ),
        (
            &b.abandoned,
            &abandoned_lanes,
            false,
            play_status::color(PlayStatus::Abandoned, cx),
        ),
    ] {
        for (mark, lane) in list.iter().zip(list_lanes.iter().copied()) {
            let Some(game) = library.get(&mark.id).cloned() else {
                continue;
            };
            let x = year_fraction(mark.day).clamp(0.015, 0.985);
            let offset = 0.7 + lane as f32 * (cover_h + 0.3);
            let id = mark.id.clone();
            let tip: SharedString =
                format!("{} · {}", game.title, text::short_date(mark.day)).into();
            marks.push(
                div()
                    .absolute()
                    .left(relative(x))
                    .top(rems(middle - 0.25))
                    .ml(rems(-0.25))
                    .size(rems(0.5))
                    .rounded_full()
                    .bg(color)
                    .into_any_element(),
            );
            marks.push(
                div()
                    .id(ElementId::Name(
                        format!("stats-mark:{}", mark.id.key()).into(),
                    ))
                    .keyboard_focusable(cx)
                    .absolute()
                    .left(relative(x))
                    .ml(rems(-MARK_REM / 2.))
                    .map(|d| {
                        if above {
                            d.top(rems(middle - offset - cover_h))
                        } else {
                            d.top(rems(middle + offset + 0.6))
                        }
                    })
                    .rounded(px(4.))
                    .border_1()
                    .border_color(color)
                    .cursor_pointer()
                    .when(!above, |d| d.opacity(0.75))
                    .tooltip(move |window, cx| {
                        gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(window, cx)
                    })
                    .on_click(move |_, _, cx| navigate(Route::Game(id.clone()), cx))
                    .child(GameCover::new(game).width(rems(MARK_REM)).radius(px(3.)))
                    .into_any_element(),
            );
        }
    }
    let today_line = (today.year() == b.year).then(|| {
        div()
            .absolute()
            .left(relative(year_fraction(today)))
            .top_0()
            .h_full()
            .child(div().w(px(1.)).h_full().bg(fg.opacity(0.35)))
            .child(
                div()
                    .absolute()
                    .top_0()
                    .left(rems(0.3))
                    .whitespace_nowrap()
                    .text_xs()
                    .text_color(muted)
                    .child(t("library.stats.year.today")),
            )
    });
    let empty = (b.beaten.is_empty() && b.abandoned.is_empty()).then(|| {
        div()
            .absolute()
            .top(rems(1.))
            .left_0()
            .right_0()
            .text_center()
            .text_sm()
            .text_color(muted)
            .child(t("library.stats.year.empty"))
    });
    div()
        .relative()
        .w_full()
        .h(rems(height))
        .child(
            div()
                .absolute()
                .left_0()
                .right_0()
                .top(rems(middle))
                .h(px(1.))
                .bg(axis),
        )
        .children(ticks)
        .children(today_line)
        .children(marks)
        .children(empty)
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use irori_core::{GameId, Runner};

    fn mark(day: &str) -> Mark {
        Mark {
            id: GameId::new(Runner::Gog, day),
            day: NaiveDate::parse_from_str(day, "%Y-%m-%d").unwrap(),
        }
    }

    #[test]
    fn close_covers_stack_and_far_ones_share_a_lane() {
        let marks = [
            mark("2026-03-01"),
            mark("2026-03-03"),
            mark("2026-03-05"),
            mark("2026-03-06"),
            mark("2026-06-01"),
        ];
        assert_eq!(lanes(&marks), [0, 1, 2, 0, 0]);
    }
}
