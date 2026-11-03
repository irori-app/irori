//! Play history of the game, from `sessions.jsonl`. The timeline shows one
//! row per day played (newest first) on a rail: the date with the session
//! count or start time under it, the time played, the longest day and failed
//! starts marked, a quiet line where the game rested for a while, and where it
//! all began at the bottom. Hovering or focusing a day lists its sessions'
//! times. The sessions view lists every session on its own.
//!
//! Rows are virtualized, so years of history cost what fits in the panel.
//! The list is one Tab stop; the arrow keys, Home and End move between days,
//! which is also how a controller reaches it.

use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;

use chrono::{Datelike as _, Local, NaiveDate, NaiveDateTime, NaiveTime, Timelike as _};
use gpui_kit::component::tab::{Tab, TabBar};
use gpui_kit::component::tag::Tag;
use gpui_kit::component::{
    ActiveTheme as _, Sizable as _, StyledExt as _, VirtualListScrollHandle, h_flex, v_flex,
    v_virtual_list,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, Context, FocusHandle, InteractiveElement as _, IntoElement, KeyDownEvent,
    ParentElement, Pixels, ScrollStrategy, SharedString, Size, StatefulInteractiveElement as _,
    Styled, Window, div, px, rems, size,
};
use irori_ui::i18n::{t, t_args};
use irori_ui::style::KeyboardFocusable as _;
use irori_ui::{backend, models};

use super::GamePage;
use crate::load::{Load, calendar_date};

/// Height of a day row, in rem.
const DAY_ROW_REM: f32 = 3.5;
/// Height of a "days without playing" row, in rem.
const REST_ROW_REM: f32 = 2.0;
/// Height of the "first played" row closing the timeline, in rem.
const START_ROW_REM: f32 = 2.5;
/// Height of a row of the sessions view, in rem.
const SESSION_ROW_REM: f32 = 2.5;
/// Width of the date column, in rem. Its lines are short ("Nov 1",
/// "2 sessions", "11:51 PM") and are cut short rather than move the rail.
const DATE_COLUMN_REM: f32 = 5.5;
/// Width of the rail, in rem.
const RAIL_REM: f32 = 1.25;
/// Size of a day's dot on the rail, in rem.
const DOT_REM: f32 = 0.75;
/// Distance from a row's top to the centre of its dot, in rem.
const DOT_CENTRE_REM: f32 = 0.95;
/// Tallest the list grows before it scrolls, in rem.
const MAX_LIST_REM: f32 = 30.0;
/// Days without play shown as their own line; shorter breaks pass unremarked.
const MIN_REST_DAYS: i64 = 3;
/// How far a row rises as it appears, in pixels.
const RISE_PX: f32 = 6.0;

/// One session as the timeline needs it: local start, length, failed start.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Span {
    pub start: NaiveDateTime,
    pub seconds: u64,
    pub crashed: bool,
}

/// One day the game was played.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PlayDay {
    pub day: NaiveDate,
    pub seconds: u64,
    /// Sessions that ran, not counting failed starts.
    pub sessions: u32,
    pub crashes: u32,
    /// The day with the most play, when there are several days.
    pub longest: bool,
    /// The day's sessions, earliest first.
    pub spans: Vec<Span>,
}

/// A line of the timeline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TimelineRow {
    Day(PlayDay),
    /// Whole days without play between two rows.
    Rest(i64),
    /// The first day the game was played, closing the timeline.
    Start(NaiveDate),
}

impl TimelineRow {
    /// Height of the row, in rem.
    fn rem(&self) -> f32 {
        match self {
            TimelineRow::Day(_) => DAY_ROW_REM,
            TimelineRow::Rest(_) => REST_ROW_REM,
            TimelineRow::Start(_) => START_ROW_REM,
        }
    }
}

/// The game's play history, ready to show.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Timeline {
    pub rows: Vec<TimelineRow>,
    /// Every session, newest first, for the sessions view.
    pub sessions: Vec<Span>,
    /// Sessions that ran, not counting failed starts.
    pub played: u32,
    /// The first day in the history.
    pub since: Option<NaiveDate>,
}

/// Group `spans` (in any order) into days, newest first. `first_played` is
/// the first play the playtime record knows of, which may predate the
/// history; the timeline closes on the earlier of the two.
pub(crate) fn build(spans: &[Span], first_played: Option<NaiveDate>) -> Timeline {
    let mut sessions = spans.to_vec();
    sessions.sort_by_key(|s| std::cmp::Reverse(s.start));
    let mut days: Vec<PlayDay> = Vec::new();
    for s in &sessions {
        let day = s.start.date();
        if days.last().is_none_or(|d| d.day != day) {
            days.push(PlayDay {
                day,
                seconds: 0,
                sessions: 0,
                crashes: 0,
                longest: false,
                spans: Vec::new(),
            });
        }
        let d = days.last_mut().expect("pushed above");
        d.seconds += s.seconds;
        if s.crashed {
            d.crashes += 1;
        } else {
            d.sessions += 1;
        }
        d.spans.insert(0, *s);
    }
    if days.len() > 1
        && let Some(most) = days.iter().map(|d| d.seconds).max().filter(|m| *m > 0)
        && let Some(d) = days.iter_mut().find(|d| d.seconds == most)
    {
        d.longest = true;
    }
    let since = days.last().map(|d| d.day);
    let mut rows = Vec::with_capacity(days.len() * 2 + 1);
    for (ix, d) in days.iter().enumerate() {
        rows.push(TimelineRow::Day(d.clone()));
        if let Some(older) = days.get(ix + 1) {
            let rest = (d.day - older.day).num_days() - 1;
            if rest >= MIN_REST_DAYS {
                rows.push(TimelineRow::Rest(rest));
            }
        }
    }
    let start = match (since, first_played) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    };
    if let Some(start) = start.filter(|_| !rows.is_empty()) {
        rows.push(TimelineRow::Start(start));
    }
    Timeline {
        played: days.iter().map(|d| d.sessions).sum(),
        since,
        sessions,
        rows,
    }
}

/// Which list the panel shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum TimelineMode {
    /// One row per day.
    #[default]
    Days,
    /// One row per session.
    Sessions,
}

/// The play history panel's state: the data, the measured rows, the row the
/// keyboard or pointer is on, and the list's focus and scroll.
pub(crate) struct TimelineView {
    pub data: Load<Arc<Timeline>>,
    mode: TimelineMode,
    key: Option<(usize, TimelineMode, Pixels)>,
    sizes: Rc<Vec<Size<Pixels>>>,
    /// The row shown highlighted, with its session times for a day.
    active: Option<usize>,
    focus: Option<FocusHandle>,
    scroll: VirtualListScrollHandle,
}

impl Default for TimelineView {
    fn default() -> Self {
        Self {
            data: Load::Idle,
            mode: TimelineMode::Days,
            key: None,
            sizes: Rc::default(),
            active: None,
            focus: None,
            scroll: VirtualListScrollHandle::new(),
        }
    }
}

impl TimelineView {
    fn sync(&mut self, timeline: &Arc<Timeline>, rem: Pixels) {
        let key = (Arc::as_ptr(timeline) as usize, self.mode, rem);
        if self.key == Some(key) {
            return;
        }
        let heights: Vec<f32> = match self.mode {
            TimelineMode::Days => timeline.rows.iter().map(TimelineRow::rem).collect(),
            TimelineMode::Sessions => vec![SESSION_ROW_REM; timeline.sessions.len()],
        };
        self.sizes = Rc::new(heights.iter().map(|h| size(px(0.), rem * *h)).collect());
        self.key = Some(key);
        self.active = None;
    }

    fn height_rem(&self, timeline: &Timeline) -> f32 {
        match self.mode {
            TimelineMode::Days => timeline.rows.iter().map(TimelineRow::rem).sum(),
            TimelineMode::Sessions => SESSION_ROW_REM * timeline.sessions.len() as f32,
        }
    }
}

/// The next day row from `from` in `step`'s direction (staying put at the
/// end), or the first or last day row without a current one.
fn next_day(rows: &[TimelineRow], from: Option<usize>, step: isize) -> Option<usize> {
    let is_day = |ix: &usize| matches!(rows.get(*ix), Some(TimelineRow::Day(_)));
    match from {
        None if step > 0 => (0..rows.len()).find(is_day),
        None => (0..rows.len()).rev().find(is_day),
        Some(ix) if step > 0 => (ix + 1..rows.len()).find(is_day).or(Some(ix)),
        Some(ix) => (0..ix).rev().find(is_day).or(Some(ix)),
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

/// A time of day as the interface language writes it ("11:51 PM", "23:51").
fn clock(time: NaiveTime) -> String {
    let (hour, minute) = (time.hour(), time.minute());
    if t("game.time.clock").as_ref() == "12" {
        let h12 = match hour % 12 {
            0 => 12,
            h => h,
        };
        let text = format!("{h12}:{minute:02}");
        let key = if hour < 12 {
            "game.time.am"
        } else {
            "game.time.pm"
        };
        t_args(key, &[("time", &text)]).to_string()
    } else {
        format!("{hour:02}:{minute:02}")
    }
}

/// A day relative to `today`: "Today", "Yesterday", "Oct 11", "Oct 11, 2024".
fn day_label(day: NaiveDate, today: NaiveDate) -> String {
    match (today - day).num_days() {
        0 => t("game.date.today").to_string(),
        1 => t("game.date.yesterday").to_string(),
        _ => calendar_date(day, day.year() != today.year()),
    }
}

/// "Played 1 h 49 min" with the duration in bold.
fn played_line(seconds: u64) -> AnyElement {
    let template = t("game.timeline.played");
    let value = duration(seconds);
    let (before, after) = template
        .split_once("{duration}")
        .unwrap_or((template.as_ref(), ""));
    h_flex()
        .flex_shrink_0()
        .whitespace_nowrap()
        .when(!before.is_empty(), |r| {
            r.child(SharedString::from(before.to_string()))
        })
        .child(div().font_semibold().child(value))
        .when(!after.is_empty(), |r| {
            r.child(SharedString::from(after.to_string()))
        })
        .into_any_element()
}

/// What marks a row on the rail.
#[derive(Debug, Clone, Copy)]
enum Dot {
    /// The most recent day.
    Latest,
    Day,
    /// The first day played: a ring.
    Start,
}

impl GamePage {
    /// Read the history off the UI thread and rebuild the timeline.
    pub(super) fn load_timeline(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let b = backend(cx);
        let key = self.id.key();
        let first_played = models(cx)
            .library
            .read(cx)
            .user
            .playtime
            .get(&key)
            .and_then(|r| r.first_played.as_deref())
            .and_then(|f| chrono::DateTime::parse_from_rfc3339(f).ok())
            .map(|f| f.with_timezone(&Local).date_naive());
        if self.timeline.data.ready().is_none() {
            self.timeline.data = Load::Loading;
        }
        let reader = b.clone();
        let job = b.spawn_blocking(move || {
            let spans: Vec<Span> = reader
                .sessions()
                .into_iter()
                .filter(|s| s.game == key)
                .filter_map(|s| {
                    let started = chrono::DateTime::parse_from_rfc3339(&s.started).ok()?;
                    Some(Span {
                        start: started.with_timezone(&Local).naive_local(),
                        seconds: s.seconds,
                        crashed: s.crashed,
                    })
                })
                .collect();
            build(&spans, first_played)
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

    /// The play history panel: a header with the count and the view toggle,
    /// then the list.
    pub(super) fn render_timeline(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let ink = self.ink;
        let focus = self
            .timeline
            .focus
            .get_or_insert_with(|| cx.focus_handle())
            .clone();
        let timeline = self.timeline.data.ready().cloned().unwrap_or_default();
        let today = Local::now().date_naive();
        let summary = timeline.since.map(|since| {
            let date = calendar_date(since, since.year() != today.year());
            let key = if timeline.played == 1 {
                "game.timeline.summary_one"
            } else {
                "game.timeline.summary_other"
            };
            t_args(
                key,
                &[("count", &timeline.played.to_string()), ("date", &date)],
            )
        });
        let modes = [TimelineMode::Days, TimelineMode::Sessions];
        let selected = modes
            .iter()
            .position(|m| *m == self.timeline.mode)
            .unwrap_or(0);
        let has_rows = !timeline.rows.is_empty();
        let header = h_flex()
            .w_full()
            .gap_3()
            .flex_wrap()
            .child(
                super::section_title(t("game.timeline.title"), ink.text)
                    .flex_1()
                    .min_w_0()
                    .truncate(),
            )
            .when_some(summary, |row, s| {
                row.child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_xs()
                        .text_color(ink.muted)
                        .child(s),
                )
            })
            .when(has_rows, |row| {
                row.child(
                    TabBar::new("timeline-mode")
                        .segmented()
                        .xsmall()
                        .selected_index(selected)
                        .child(Tab::new().label(t("game.timeline.view_days")))
                        .child(Tab::new().label(t("game.timeline.view_sessions")))
                        .on_click(cx.listener(move |this, ix: &usize, _, cx| {
                            if let Some(mode) = modes.get(*ix) {
                                this.timeline.mode = *mode;
                                cx.notify();
                            }
                        })),
                )
            });
        let body =
            if !has_rows {
                div()
                    .text_sm()
                    .text_color(ink.panel_muted)
                    .when(!self.timeline.data.is_loading(), |d| {
                        d.child(t("game.timeline.empty"))
                    })
                    .into_any_element()
            } else {
                self.timeline.sync(&timeline, window.rem_size());
                let height = self.timeline.height_rem(&timeline).min(MAX_LIST_REM);
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
                    .track_focus(&focus)
                    .keyboard_focusable(cx)
                    .rounded_md()
                    .w_full()
                    .h(rems(height))
                    .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                        this.on_timeline_key(event, cx)
                    }))
                    .child(list)
                    .into_any_element()
            };
        let theme = cx.theme();
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
            .into_any_element()
    }

    /// Up, Down, Home and End move between rows (days, in the timeline); the
    /// list keeps the row in view.
    fn on_timeline_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        let Some(timeline) = self.timeline.data.ready().cloned() else {
            return;
        };
        let count = match self.timeline.mode {
            TimelineMode::Days => timeline.rows.len(),
            TimelineMode::Sessions => timeline.sessions.len(),
        };
        let current = self.timeline.active;
        let last = count.saturating_sub(1);
        let next = match (self.timeline.mode, event.keystroke.key.as_str()) {
            (TimelineMode::Days, "down") => next_day(&timeline.rows, current, 1),
            (TimelineMode::Days, "up") => next_day(&timeline.rows, current, -1),
            (TimelineMode::Days, "home") => next_day(&timeline.rows, None, 1),
            (TimelineMode::Days, "end") => next_day(&timeline.rows, None, -1),
            (TimelineMode::Sessions, "down") => Some(current.map_or(0, |ix| (ix + 1).min(last))),
            (TimelineMode::Sessions, "up") => Some(current.map_or(0, |ix| ix.saturating_sub(1))),
            (TimelineMode::Sessions, "home") => Some(0),
            (TimelineMode::Sessions, "end") => Some(last),
            _ => return,
        };
        cx.stop_propagation();
        if let Some(ix) = next.filter(|ix| *ix < count) {
            self.timeline.active = Some(ix);
            self.timeline
                .scroll
                .scroll_to_item(ix, ScrollStrategy::Center);
            cx.notify();
        }
    }

    fn render_timeline_rows(
        &mut self,
        range: Range<usize>,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let Some(timeline) = self.timeline.data.ready().cloned() else {
            return Vec::new();
        };
        let today = Local::now().date_naive();
        let motion = !irori_ui::style::reduce_motion(cx);
        let mode = self.timeline.mode;
        range
            .filter_map(|ix| {
                let row = match mode {
                    TimelineMode::Days => {
                        let row = timeline.rows.get(ix)?;
                        let last = ix + 1 == timeline.rows.len();
                        self.render_timeline_row(ix, row, last, today, cx)
                    }
                    TimelineMode::Sessions => {
                        let span = timeline.sessions.get(ix)?;
                        self.render_session_row(ix, span, today, cx)
                    }
                };
                Some(if motion {
                    let name = match mode {
                        TimelineMode::Days => "timeline-day-in",
                        TimelineMode::Sessions => "timeline-session-in",
                    };
                    irori_ui::motion::rise_in((name, ix), div().w_full().child(row), RISE_PX)
                } else {
                    row
                })
            })
            .collect()
    }

    /// The rail through a row: a line from top to bottom (from the dot down
    /// on the first row, down to it on the last) and the row's dot, if any.
    fn rail(&self, first: bool, last: bool, dot: Option<Dot>, cx: &Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let centre = RAIL_REM / 2.;
        div()
            .relative()
            .w(rems(RAIL_REM))
            .h_full()
            .flex_shrink_0()
            .child(
                div()
                    .absolute()
                    .left(rems(centre))
                    .ml(px(-1.))
                    .w(px(2.))
                    .top(if first {
                        rems(DOT_CENTRE_REM)
                    } else {
                        rems(0.)
                    })
                    .when_else(last, |d| d.h(rems(DOT_CENTRE_REM)), |d| d.bottom_0())
                    .bg(theme.border),
            )
            .when_some(dot, |rail, dot| {
                let base = div()
                    .absolute()
                    .left(rems(centre - DOT_REM / 2.))
                    .top(rems(DOT_CENTRE_REM - DOT_REM / 2.))
                    .size(rems(DOT_REM))
                    .rounded_full();
                rail.child(match dot {
                    Dot::Latest => base.bg(theme.primary),
                    Dot::Day => base.bg(theme.muted_foreground),
                    Dot::Start => base
                        .border_2()
                        .border_color(theme.muted_foreground)
                        .bg(self.ink.panel),
                })
            })
            .into_any_element()
    }

    /// The date column of a timeline row: the day, and under it `under`.
    fn date_column(&self, day: String, under: Option<SharedString>) -> AnyElement {
        v_flex()
            .w(rems(DATE_COLUMN_REM))
            .flex_shrink_0()
            .items_end()
            .pt(rems(0.5))
            .child(
                div()
                    .max_w_full()
                    .truncate()
                    .text_sm()
                    .font_semibold()
                    .text_color(self.ink.text)
                    .child(day),
            )
            .when_some(under, |c, u| {
                c.child(
                    div()
                        .max_w_full()
                        .truncate()
                        .text_xs()
                        .text_color(self.ink.panel_muted)
                        .child(u),
                )
            })
            .into_any_element()
    }

    fn render_timeline_row(
        &self,
        ix: usize,
        row: &TimelineRow,
        last: bool,
        today: NaiveDate,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let ink = self.ink;
        let first = ix == 0;
        match row {
            TimelineRow::Rest(days) => h_flex()
                .h(rems(REST_ROW_REM))
                .w_full()
                .gap_2()
                .child(div().w(rems(DATE_COLUMN_REM)).flex_shrink_0())
                .child(self.rail(false, last, None, cx))
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_xs()
                        .text_color(ink.panel_muted)
                        .child(plural(
                            *days,
                            "game.timeline.rest_one",
                            "game.timeline.rest_other",
                        )),
                )
                .into_any_element(),
            TimelineRow::Start(day) => h_flex()
                .h(rems(START_ROW_REM))
                .w_full()
                .items_start()
                .gap_2()
                .child(self.date_column(day_label(*day, today), None))
                .child(self.rail(first, true, Some(Dot::Start), cx))
                .child(
                    div()
                        .pt(rems(0.5))
                        .min_w_0()
                        .truncate()
                        .text_sm()
                        .text_color(ink.panel_muted)
                        .child(t("game.timeline.first_played")),
                )
                .into_any_element(),
            TimelineRow::Day(d) => {
                let active = self.timeline.active == Some(ix);
                // One session: when it started; several: how many.
                let under = if d.sessions <= 1 {
                    d.spans
                        .first()
                        .map(|s| SharedString::from(clock(s.start.time())))
                } else {
                    Some(plural(
                        d.sessions.into(),
                        "game.timeline.sessions_one",
                        "game.timeline.sessions_other",
                    ))
                };
                // The session times of the day under the pointer or the keyboard.
                let times = active.then(|| {
                    d.spans
                        .iter()
                        .map(|s| {
                            let end = s.start + chrono::Duration::seconds(s.seconds as i64);
                            format!("{}–{}", clock(s.start.time()), clock(end.time()))
                        })
                        .collect::<Vec<_>>()
                        .join(" · ")
                });
                let highlight = cx.theme().muted.opacity(0.5);
                h_flex()
                    .id(("timeline-day", ix))
                    .h(rems(DAY_ROW_REM))
                    .w_full()
                    .items_start()
                    .gap_2()
                    .rounded_md()
                    .when(active, |r| r.bg(highlight))
                    .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                        this.hover_timeline_row(ix, *hovered, cx)
                    }))
                    .child(self.date_column(day_label(d.day, today), under))
                    .child(self.rail(
                        first,
                        last,
                        Some(if first { Dot::Latest } else { Dot::Day }),
                        cx,
                    ))
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .pt(rems(0.5))
                            .pr_2()
                            .gap_1()
                            .child(
                                h_flex()
                                    .w_full()
                                    .min_w_0()
                                    .gap_2()
                                    .text_sm()
                                    .text_color(ink.text)
                                    .child(played_line(d.seconds))
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
                            )
                            .when_some(times, |c, times| {
                                c.child(
                                    div()
                                        .w_full()
                                        .truncate()
                                        .text_xs()
                                        .text_color(ink.panel_muted)
                                        .child(times),
                                )
                            }),
                    )
                    .into_any_element()
            }
        }
    }

    fn render_session_row(
        &self,
        ix: usize,
        span: &Span,
        today: NaiveDate,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let ink = self.ink;
        let active = self.timeline.active == Some(ix);
        let highlight = cx.theme().muted.opacity(0.5);
        h_flex()
            .id(("timeline-session", ix))
            .h(rems(SESSION_ROW_REM))
            .w_full()
            .px_2()
            .gap_3()
            .rounded_md()
            .text_sm()
            .when(active, |r| r.bg(highlight))
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                this.hover_timeline_row(ix, *hovered, cx)
            }))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_color(ink.text)
                    .child(format!(
                        "{} · {}",
                        day_label(span.start.date(), today),
                        clock(span.start.time())
                    )),
            )
            .when(span.crashed, |r| {
                r.child(Tag::danger().xsmall().child(plural(
                    1,
                    "game.timeline.crashes_one",
                    "game.timeline.crashes_other",
                )))
            })
            .child(
                div()
                    .flex_shrink_0()
                    .font_semibold()
                    .text_color(if span.crashed {
                        ink.panel_muted
                    } else {
                        ink.text
                    })
                    .child(duration(span.seconds)),
            )
            .into_any_element()
    }

    fn hover_timeline_row(&mut self, ix: usize, hovered: bool, cx: &mut Context<Self>) {
        if hovered {
            self.timeline.active = Some(ix);
        } else if self.timeline.active == Some(ix) {
            self.timeline.active = None;
        }
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(m: u32, d: u32, h: u32) -> NaiveDateTime {
        NaiveDate::from_ymd_opt(2026, m, d)
            .unwrap()
            .and_hms_opt(h, 0, 0)
            .unwrap()
    }

    fn span(m: u32, d: u32, h: u32, seconds: u64, crashed: bool) -> Span {
        Span {
            start: at(m, d, h),
            seconds,
            crashed,
        }
    }

    fn shape(t: &Timeline) -> Vec<String> {
        t.rows
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
                TimelineRow::Start(d) => format!("start {}", d.format("%m-%d")),
            })
            .collect()
    }

    #[test]
    fn groups_days_newest_first_with_rests_the_longest_day_and_the_start() {
        let t = build(
            &[
                span(10, 3, 18, 3600, false),
                span(10, 11, 20, 1200, false),
                span(10, 3, 21, 5400, false),
                span(10, 7, 23, 1440, false),
                span(10, 11, 22, 5400, false),
                span(10, 11, 19, 4, true),
                span(10, 4, 0, 840, false),
            ],
            None,
        );
        assert_eq!(
            shape(&t),
            [
                "10-11 6604s 2x crash",
                "rest 3",
                "10-07 1440s 1x",
                "10-04 840s 1x",
                "10-03 9000s 2x longest",
                "start 10-03",
            ]
        );
        assert_eq!(t.played, 6);
        assert_eq!(t.since, NaiveDate::from_ymd_opt(2026, 10, 3));
        // Sessions newest first; a day's spans earliest first.
        assert_eq!(t.sessions.first().map(|s| s.start), Some(at(10, 11, 22)));
        let TimelineRow::Day(d) = &t.rows[0] else {
            panic!("a day comes first")
        };
        let hours: Vec<u32> = d.spans.iter().map(|s| s.start.hour()).collect();
        assert_eq!(hours, [19, 20, 22]);
    }

    #[test]
    fn the_start_is_the_earlier_of_the_history_and_the_record() {
        let spans = [span(10, 3, 18, 60, false)];
        let earlier = NaiveDate::from_ymd_opt(2025, 1, 2);
        assert_eq!(
            build(&spans, earlier).rows.last(),
            earlier.map(TimelineRow::Start).as_ref()
        );
        let later = NaiveDate::from_ymd_opt(2026, 12, 1);
        assert_eq!(shape(&build(&spans, later)).last().unwrap(), "start 10-03");
        assert_eq!(build(&[], earlier), Timeline::default());
    }

    #[test]
    fn a_single_day_is_not_the_longest() {
        let t = build(&[span(1, 2, 10, 60, false)], None);
        assert!(matches!(&t.rows[0], TimelineRow::Day(d) if !d.longest));
    }

    #[test]
    fn arrows_step_over_rests_and_the_start() {
        let t = build(
            &[span(10, 11, 20, 60, false), span(10, 1, 20, 60, false)],
            None,
        );
        // Day, rest, day, start.
        assert_eq!(t.rows.len(), 4);
        assert_eq!(next_day(&t.rows, None, 1), Some(0));
        assert_eq!(next_day(&t.rows, Some(0), 1), Some(2));
        assert_eq!(next_day(&t.rows, Some(2), 1), Some(2));
        assert_eq!(next_day(&t.rows, Some(2), -1), Some(0));
        assert_eq!(next_day(&t.rows, None, -1), Some(2));
    }

    #[test]
    fn clock_follows_the_language() {
        crate::i18n::register();
        assert_eq!(
            clock(NaiveTime::from_hms_opt(23, 51, 0).unwrap()),
            "11:51 PM"
        );
        assert_eq!(clock(NaiveTime::from_hms_opt(0, 3, 0).unwrap()), "12:03 AM");
    }
}
