//! Play history of the game: its sessions from `sessions.jsonl`, one row per
//! day played (newest first), the longest day marked, and a quiet line where
//! the game rested for a while. Rows are virtualized, so years of history
//! cost what fits in the panel.

use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;

use chrono::{Datelike as _, Local, NaiveDate};
use gpui_kit::component::tag::Tag;
use gpui_kit::component::{
    ActiveTheme as _, Sizable as _, StyledExt as _, VirtualListScrollHandle, h_flex, v_flex,
    v_virtual_list,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, Context, InteractiveElement as _, IntoElement, ParentElement, Pixels, SharedString,
    Size, Styled, Window, div, px, rems, size,
};
use irori_ui::i18n::{t, t_args};
use irori_ui::{backend, models};

use super::GamePage;
use crate::load::{Load, calendar_date};

/// Height of a day row, in rem.
const DAY_ROW_REM: f32 = 3.25;
/// Height of a "days without playing" row, in rem.
const GAP_ROW_REM: f32 = 2.0;
/// Tallest the list grows before it scrolls, in rem.
const MAX_LIST_REM: f32 = 24.0;
/// Days without play shown as their own line; shorter breaks pass unremarked.
const MIN_GAP_DAYS: i64 = 3;

/// One day the game was played.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PlayDay {
    pub day: NaiveDate,
    pub seconds: u64,
    /// Sessions that ran, not counting crashes at start.
    pub sessions: u32,
    pub crashes: u32,
    /// The day with the most play, when there are several days.
    pub longest: bool,
}

/// A line of the timeline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TimelineRow {
    Day(PlayDay),
    /// Whole days without play between two rows.
    Rest(i64),
}

/// The game's play history, ready to show.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Timeline {
    pub rows: Vec<TimelineRow>,
    pub sessions: u32,
    /// The first day recorded.
    pub since: Option<NaiveDate>,
}

/// One recorded session: its local start day, length and whether it crashed.
pub(crate) type DaySession = (NaiveDate, u64, bool);

/// Group `sessions` (in any order) into days, newest first.
pub(crate) fn build(sessions: &[DaySession]) -> Timeline {
    let mut days: Vec<PlayDay> = Vec::new();
    let mut sorted = sessions.to_vec();
    sorted.sort_by_key(|s| std::cmp::Reverse(s.0));
    for (day, seconds, crashed) in sorted {
        if days.last().is_none_or(|d| d.day != day) {
            days.push(PlayDay {
                day,
                seconds: 0,
                sessions: 0,
                crashes: 0,
                longest: false,
            });
        }
        let d = days.last_mut().expect("pushed above");
        d.seconds += seconds;
        if crashed {
            d.crashes += 1;
        } else {
            d.sessions += 1;
        }
    }
    if days.len() > 1
        && let Some(most) = days.iter().map(|d| d.seconds).max().filter(|m| *m > 0)
        && let Some(d) = days.iter_mut().find(|d| d.seconds == most)
    {
        d.longest = true;
    }
    let mut rows = Vec::with_capacity(days.len() * 2);
    for (ix, d) in days.iter().enumerate() {
        if let Some(older) = days.get(ix + 1) {
            rows.push(TimelineRow::Day(d.clone()));
            let rest = (d.day - older.day).num_days() - 1;
            if rest >= MIN_GAP_DAYS {
                rows.push(TimelineRow::Rest(rest));
            }
        } else {
            rows.push(TimelineRow::Day(d.clone()));
        }
    }
    Timeline {
        sessions: days.iter().map(|d| d.sessions).sum(),
        since: days.last().map(|d| d.day),
        rows,
    }
}

/// The timeline's rows and their heights, rebuilt when the history or the
/// rem size change.
pub(crate) struct TimelineView {
    pub data: Load<Arc<Timeline>>,
    key: Option<(usize, Pixels)>,
    sizes: Rc<Vec<Size<Pixels>>>,
    scroll: VirtualListScrollHandle,
}

impl Default for TimelineView {
    fn default() -> Self {
        Self {
            data: Load::Idle,
            key: None,
            sizes: Rc::default(),
            scroll: VirtualListScrollHandle::new(),
        }
    }
}

impl TimelineView {
    fn sync(&mut self, timeline: &Arc<Timeline>, rem: Pixels) {
        let key = (Arc::as_ptr(timeline) as usize, rem);
        if self.key == Some(key) {
            return;
        }
        self.sizes = Rc::new(
            timeline
                .rows
                .iter()
                .map(|r| size(px(0.), rem * row_rem(r)))
                .collect(),
        );
        self.key = Some(key);
    }
}

fn row_rem(row: &TimelineRow) -> f32 {
    match row {
        TimelineRow::Day(_) => DAY_ROW_REM,
        TimelineRow::Rest(_) => GAP_ROW_REM,
    }
}

/// "45 min", "2 h 5 min"; "< 1 min" for a session cut short.
fn duration(seconds: u64) -> SharedString {
    crate::i18n::playtime_label(seconds / 60, true)
}

/// `one` for a count of one, else `other`, with the count filled in.
fn plural(count: i64, one: &str, other: &str) -> SharedString {
    let key = if count == 1 { one } else { other };
    t_args(key, &[("count", &count.to_string())])
}

/// A day relative to `today`: "Today", "Yesterday", "Oct 11", "Oct 11, 2024".
fn day_label(day: NaiveDate, today: NaiveDate) -> String {
    match (today - day).num_days() {
        0 => t("game.date.today").to_string(),
        1 => t("game.date.yesterday").to_string(),
        _ => calendar_date(day, day.year() != today.year()),
    }
}

impl GamePage {
    /// Read the history off the UI thread and rebuild the timeline.
    pub(super) fn load_timeline(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let b = backend(cx);
        let key = self.id.key();
        if self.timeline.data.ready().is_none() {
            self.timeline.data = Load::Loading;
        }
        let reader = b.clone();
        let job = b.spawn_blocking(move || {
            let sessions: Vec<DaySession> = reader
                .sessions()
                .into_iter()
                .filter(|s| s.game == key)
                .filter_map(|s| {
                    let started = chrono::DateTime::parse_from_rfc3339(&s.started).ok()?;
                    let day = started.with_timezone(&Local).date_naive();
                    Some((day, s.seconds, s.crashed))
                })
                .collect();
            build(&sessions)
        });
        let task = irori_ui::run(cx, async move { Ok(job.await?) });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                this.timeline.data = Load::from_result(result.map(Arc::new));
                cx.notify();
            });
        })
        .detach();
    }

    /// The play history panel, or nothing when nothing was recorded.
    pub(super) fn render_timeline(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let timeline = self.timeline.data.ready()?.clone();
        let ink = self.ink;
        let today = Local::now().date_naive();
        let summary = timeline.since.map(|since| {
            let date = calendar_date(since, since.year() != today.year());
            let key = if timeline.sessions == 1 {
                "game.timeline.summary_one"
            } else {
                "game.timeline.summary_other"
            };
            t_args(
                key,
                &[("count", &timeline.sessions.to_string()), ("date", &date)],
            )
        });
        let theme = cx.theme();
        let header = h_flex()
            .w_full()
            .gap_3()
            .justify_between()
            .child(irori_ui::style::caption(t("game.timeline.title"), cx))
            .when_some(summary, |row, s| {
                row.child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_xs()
                        .text_color(ink.panel_muted)
                        .child(s),
                )
            });
        let body = if timeline.rows.is_empty() {
            div()
                .text_sm()
                .text_color(ink.panel_muted)
                .child(t("game.timeline.empty"))
                .into_any_element()
        } else {
            let rem = window.rem_size();
            self.timeline.sync(&timeline, rem);
            let height: f32 = timeline.rows.iter().map(row_rem).sum();
            let list = v_virtual_list(
                cx.entity(),
                "timeline-rows",
                self.timeline.sizes.clone(),
                |this, range, _, cx| this.render_timeline_rows(range, cx),
            )
            .track_scroll(&self.timeline.scroll)
            .size_full();
            div()
                .id("timeline-list")
                .w_full()
                .h(rems(height.min(MAX_LIST_REM)))
                .child(list)
                .into_any_element()
        };
        Some(
            v_flex()
                .w_full()
                .gap_3()
                .p_4()
                .rounded(theme.radius_lg)
                .bg(ink.panel)
                .border_1()
                .border_color(theme.border)
                .child(header)
                .child(body)
                .into_any_element(),
        )
    }

    fn render_timeline_rows(&self, range: Range<usize>, cx: &Context<Self>) -> Vec<AnyElement> {
        let Some(timeline) = self.timeline.data.ready().cloned() else {
            return Vec::new();
        };
        let today = Local::now().date_naive();
        let count = timeline.rows.len();
        range
            .filter_map(|ix| {
                let row = timeline.rows.get(ix)?;
                Some(self.render_timeline_row(row, ix == 0, ix + 1 == count, today, cx))
            })
            .collect()
    }

    fn render_timeline_row(
        &self,
        row: &TimelineRow,
        first: bool,
        last: bool,
        today: NaiveDate,
        cx: &Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme();
        let ink = self.ink;
        // The rail: a line through every row, a dot on each day played.
        let dot_rem = 0.75;
        let rail = div().relative().w(rems(1.)).h_full().flex_shrink_0().child(
            div()
                .absolute()
                .left(rems(0.5))
                .ml(px(-1.))
                .w(px(2.))
                .top(if first { rems(1.) } else { rems(0.) })
                .bottom(if last {
                    rems(DAY_ROW_REM - 1.)
                } else {
                    rems(0.)
                })
                .bg(theme.border),
        );
        match row {
            TimelineRow::Rest(days) => h_flex()
                .h(rems(GAP_ROW_REM))
                .w_full()
                .gap_3()
                .child(rail)
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_xs()
                        .italic()
                        .text_color(ink.panel_muted)
                        .child(plural(
                            *days,
                            "game.timeline.rest_one",
                            "game.timeline.rest_other",
                        )),
                )
                .into_any_element(),
            TimelineRow::Day(d) => {
                let rail = rail.child(
                    div()
                        .absolute()
                        .left(rems(0.5 - dot_rem / 2.))
                        .top(rems(1. - dot_rem / 2.))
                        .size(rems(dot_rem))
                        .rounded_full()
                        .bg(if first {
                            theme.primary
                        } else {
                            theme.muted_foreground
                        })
                        .border_2()
                        .border_color(ink.panel),
                );
                let mut detail: Vec<SharedString> = Vec::new();
                if d.sessions > 1 {
                    detail.push(plural(
                        d.sessions.into(),
                        "game.timeline.sessions_one",
                        "game.timeline.sessions_other",
                    ));
                }
                h_flex()
                    .h(rems(DAY_ROW_REM))
                    .w_full()
                    .items_start()
                    .gap_3()
                    .child(rail)
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .pt(rems(0.5))
                            .gap_0p5()
                            .child(
                                h_flex()
                                    .w_full()
                                    .gap_2()
                                    .text_sm()
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .truncate()
                                            .font_semibold()
                                            .child(day_label(d.day, today)),
                                    )
                                    .child(div().flex_shrink_0().text_color(ink.text).child(
                                        t_args(
                                            "game.timeline.played",
                                            &[("duration", &duration(d.seconds))],
                                        ),
                                    )),
                            )
                            .child(
                                h_flex()
                                    .w_full()
                                    .gap_2()
                                    .min_w_0()
                                    .text_xs()
                                    .text_color(ink.panel_muted)
                                    .when(!detail.is_empty(), |r| {
                                        r.child(div().truncate().child(detail.join(" · ")))
                                    })
                                    .when(d.longest, |r| {
                                        r.child(
                                            Tag::info()
                                                .xsmall()
                                                .child(t("game.timeline.longest_day")),
                                        )
                                    })
                                    .when(d.crashes > 0, |r| {
                                        r.child(Tag::danger().xsmall().child(plural(
                                            d.crashes.into(),
                                            "game.timeline.crashes_one",
                                            "game.timeline.crashes_other",
                                        )))
                                    }),
                            ),
                    )
                    .into_any_element()
            }
        }
    }

    /// Whether the session that ended last was this game's, so the history
    /// has a new entry.
    pub(super) fn played_last(&self, cx: &Context<Self>) -> bool {
        models(cx)
            .sessions
            .read(cx)
            .exited
            .back()
            .is_some_and(|e| e.id == self.id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn day(m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, m, d).unwrap()
    }

    #[test]
    fn groups_days_newest_first_with_rests_and_the_longest_day() {
        let t = build(&[
            (day(10, 3), 3600, false),
            (day(10, 11), 1200, false),
            (day(10, 3), 5400, false),
            (day(10, 7), 1440, false),
            (day(10, 11), 5400, false),
            (day(10, 11), 4, true),
            (day(10, 4), 840, false),
        ]);
        let shape: Vec<String> = t
            .rows
            .iter()
            .map(|r| match r {
                TimelineRow::Day(d) => format!(
                    "{} {}s {}x{}{}",
                    d.day.format("%m-%d"),
                    d.seconds,
                    d.sessions,
                    if d.crashes > 0 { " crash" } else { "" },
                    if d.longest { " longest" } else { "" }
                ),
                TimelineRow::Rest(n) => format!("rest {n}"),
            })
            .collect();
        assert_eq!(
            shape,
            [
                "10-11 6604s 2x crash",
                "rest 3",
                "10-07 1440s 1x",
                "10-04 840s 1x",
                "10-03 9000s 2x longest",
            ]
        );
        assert_eq!(t.sessions, 6);
        assert_eq!(t.since, Some(day(10, 3)));
    }

    #[test]
    fn a_single_day_is_not_the_longest_and_nothing_is_empty() {
        let t = build(&[(day(1, 2), 60, false)]);
        assert!(matches!(&t.rows[..], [TimelineRow::Day(d)] if !d.longest));
        assert_eq!(build(&[]), Timeline::default());
    }
}
