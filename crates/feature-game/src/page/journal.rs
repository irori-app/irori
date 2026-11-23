//! The play history's Journal view: today, every day the game was played
//! and any day with notes, newest first, each day's notes in the day's
//! order (the whole-day note, then by time): its time, title, the start of
//! its text and the pictures it shows. A day without notes offers to write
//! one and a day with notes to add another. Moments marked in game show
//! until they get words. A search finds notes by their words (accents and
//! case ignored), and "Only days with notes" leaves the empty days out. A
//! row opens the note editor on click or Enter; the rows are virtualized
//! like the timeline's, at fixed heights.

use std::collections::BTreeMap;

use chrono::{Datelike as _, Local, NaiveDate};
use gpui_kit::assets::IconName;
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, Context, InteractiveElement as _, IntoElement, ObjectFit, ParentElement as _,
    StatefulInteractiveElement as _, Styled as _, StyledImage as _, Window, div, rems,
};
use irori_services::JournalEntry;
use irori_ui::components::{art_img, clip_mark, is_clip, physical_width};
use irori_ui::i18n::t;
use irori_ui::journal::{shot_url, time_label};

use super::GamePage;
use super::timeline::{Timeline, TimelineRow, duration};
use crate::load::calendar_date;

/// Height of a day without notes, in rem.
const EMPTY_ROW_REM: f32 = 3.0;
/// Height of the row after a day's notes that adds another, in rem.
const ADD_ROW_REM: f32 = 2.25;
/// Height of a note, in rem.
const NOTE_ROW_REM: f32 = 6.25;
/// Extra height of a note that shows pictures, in rem.
const SHOTS_REM: f32 = 3.5;
/// Most pictures a row shows.
const ROW_SHOTS: usize = 4;
/// Width of a picture in a row, in rem.
const SHOT_REM: f32 = 5.;

/// Which notes the journal shows.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct JournalFilter {
    /// Words every note shown holds (accents and case ignored).
    pub query: String,
    /// Leave out days without notes.
    pub notes_only: bool,
}

/// One row of the journal: a note, or the row that writes one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct JournalRow {
    pub day: NaiveDate,
    /// Time played that day, on the day's first row; zero on the others
    /// and for a day the history lacks.
    pub seconds: u64,
    /// The day's first row, which shows its date.
    pub first: bool,
    /// `None` for the row that writes a new note of the day.
    pub entry: Option<JournalEntry>,
}

impl JournalRow {
    /// Height of the row, in rem.
    pub fn rem(&self) -> f32 {
        match &self.entry {
            None if self.first => EMPTY_ROW_REM,
            None => ADD_ROW_REM,
            Some(e) if e.pictures().next().is_none() => NOTE_ROW_REM,
            Some(_) => NOTE_ROW_REM + SHOTS_REM,
        }
    }
}

/// Today, the days played and the days with notes, newest first, as
/// `filter` asks. Today is always there without a search, so a game not
/// played yet (or not today) can get a note.
pub(crate) fn rows(
    timeline: &Timeline,
    notes: Option<&BTreeMap<NaiveDate, Vec<JournalEntry>>>,
    today: NaiveDate,
    filter: &JournalFilter,
) -> Vec<JournalRow> {
    let mut played: BTreeMap<NaiveDate, u64> = BTreeMap::new();
    played.insert(today, 0);
    for row in &timeline.rows {
        if let TimelineRow::Day(d) = row {
            played.insert(d.day, d.seconds);
        }
    }
    let empty = BTreeMap::new();
    let notes = notes.unwrap_or(&empty);
    let folded = irori_services::search::fold(&filter.query);
    let searching = !folded.is_empty();
    let days: std::collections::BTreeSet<NaiveDate> =
        played.keys().chain(notes.keys()).copied().collect();
    let mut out = Vec::new();
    for day in days.into_iter().rev() {
        let seconds = played.get(&day).copied().unwrap_or(0);
        let shown: Vec<&JournalEntry> = notes
            .get(&day)
            .into_iter()
            .flatten()
            .filter(|n| n.matches(&folded))
            .collect();
        if shown.is_empty() {
            if !searching && !filter.notes_only {
                out.push(JournalRow {
                    day,
                    seconds,
                    first: true,
                    entry: None,
                });
            }
            continue;
        }
        for (ix, note) in shown.into_iter().enumerate() {
            out.push(JournalRow {
                day,
                seconds: if ix == 0 { seconds } else { 0 },
                first: ix == 0,
                entry: Some(note.clone()),
            });
        }
        if !searching {
            out.push(JournalRow {
                day,
                seconds: 0,
                first: false,
                entry: None,
            });
        }
    }
    out
}

impl GamePage {
    /// Open the note of the journal row `ix`, or a new note of its day.
    pub(super) fn open_journal_row(
        &mut self,
        ix: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(row) = self.timeline.journal.get(ix) else {
            return;
        };
        let note = row.entry.as_ref().map(|e| e.id.clone());
        self.open_note(row.day, note, window, cx);
    }

    /// Open one note of `day`, or a new one with `note` `None`.
    pub(super) fn open_note(
        &mut self,
        day: NaiveDate,
        note: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let today = Local::now().date_naive();
        let date = calendar_date(day, day.year() != today.year());
        let title = self
            .title()
            .map(|t| t.to_string())
            .unwrap_or_else(|| self.id.app_name.to_string());
        irori_ui::journal::open_editor(self.id.clone(), day, note, &title, &date, window, cx);
    }

    pub(super) fn render_journal_row(
        &self,
        ix: usize,
        row: &JournalRow,
        last: bool,
        today: NaiveDate,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let ink = self.ink;
        let theme = cx.theme();
        let (radius, fill, warning) = (theme.radius, theme.muted, theme.warning);
        let active = self.timeline.active == Some(ix);
        let highlight = theme.muted.opacity(0.5);
        let played = (row.first && row.seconds > 0).then(|| duration(row.seconds));
        let body = match &row.entry {
            None => h_flex()
                .flex_1()
                .min_w_0()
                .pt(rems(if row.first { 0.5 } else { 0.25 }))
                .gap_2()
                .items_center()
                .text_sm()
                .text_color(ink.panel_muted)
                .child(
                    Icon::new(if row.first {
                        IconName::NotebookPen
                    } else {
                        IconName::Plus
                    })
                    .small(),
                )
                .child(div().min_w_0().truncate().child(if row.first {
                    t("game.journal.write")
                } else {
                    t("game.journal.add")
                }))
                .into_any_element(),
            Some(entry) => {
                let width = physical_width(rems(SHOT_REM), window);
                let shots: Vec<AnyElement> = entry
                    .pictures()
                    .take(ROW_SHOTS)
                    .map(|path| {
                        let image = art_img(&shot_url(path), width, false, cx)
                            .map(|img| img.size_full().object_fit(ObjectFit::Cover));
                        let clip = std::path::Path::new(path.as_str());
                        div()
                            .relative()
                            .w(rems(SHOT_REM))
                            .h(rems(SHOT_REM * 9. / 16.))
                            .flex_shrink_0()
                            .rounded(radius)
                            .overflow_hidden()
                            .bg(fill)
                            .children(image)
                            .when(is_clip(clip), |d| d.child(clip_mark(clip, cx)))
                            .into_any_element()
                    })
                    .collect();
                let heading = (!entry.title.is_empty()).then(|| entry.title.clone());
                let time = time_label(entry.time);
                let waiting = entry.is_waiting();
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .pt(rems(0.4))
                    .pr_2()
                    .gap_1()
                    .child(
                        h_flex()
                            .w_full()
                            .gap_2()
                            .items_center()
                            .when_some(time, |r, time| {
                                r.child(
                                    div()
                                        .flex_shrink_0()
                                        .text_xs()
                                        .font_semibold()
                                        .text_color(ink.panel_muted)
                                        .child(time),
                                )
                            })
                            .when_some(heading, |r, h| {
                                r.child(
                                    div()
                                        .min_w_0()
                                        .truncate()
                                        .text_sm()
                                        .font_semibold()
                                        .text_color(ink.text)
                                        .child(h),
                                )
                            })
                            .when(waiting, |r| {
                                r.child(
                                    h_flex()
                                        .min_w_0()
                                        .gap_1()
                                        .items_center()
                                        .text_xs()
                                        .text_color(warning)
                                        .child(Icon::new(IconName::Bookmark).xsmall())
                                        .child(
                                            div()
                                                .min_w_0()
                                                .truncate()
                                                .child(t("game.journal.marked")),
                                        ),
                                )
                            }),
                    )
                    .when(!entry.text.is_empty(), |c| {
                        c.child(
                            div()
                                .w_full()
                                .text_sm()
                                .text_color(ink.text)
                                .line_clamp(if entry.title.is_empty() { 3 } else { 2 })
                                .child(entry.text.clone()),
                        )
                    })
                    .when(!shots.is_empty(), |c| {
                        c.child(h_flex().pt_1().gap_2().children(shots))
                    })
                    .into_any_element()
            }
        };
        let label = if row.first {
            super::timeline::day_label(row.day, today)
        } else {
            String::new()
        };
        h_flex()
            .id(("journal-day", ix))
            .h(rems(row.rem()))
            .w_full()
            .items_start()
            .gap_2()
            .rounded_md()
            .cursor_pointer()
            .when(active, |r| r.bg(highlight))
            .hover(move |s| s.bg(highlight))
            .on_click(cx.listener(move |this, _, window, cx| this.open_journal_row(ix, window, cx)))
            .child(self.date_column(label, played))
            .child(self.rail(
                ix == 0,
                last,
                row.first.then_some(super::timeline::Dot::Day),
                cx,
            ))
            .child(body)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::page::timeline::{Span, build};

    fn day(d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 10, d).unwrap()
    }

    fn note(id: &str, text: &str) -> JournalEntry {
        JournalEntry {
            id: id.into(),
            text: text.into(),
            ..Default::default()
        }
    }

    /// Each row as (day, seconds, first, note text or "+").
    fn shape(list: &[JournalRow]) -> Vec<(u32, u64, bool, String)> {
        list.iter()
            .map(|r| {
                let text = r.entry.as_ref().map_or("+".into(), |e| e.text.clone());
                (r.day.day(), r.seconds, r.first, text)
            })
            .collect()
    }

    #[test]
    fn days_played_and_days_with_notes_newest_first() {
        let span = |d: u32, seconds: u64| Span {
            start: day(d).and_hms_opt(20, 0, 0).unwrap(),
            seconds,
            crashed: false,
        };
        let timeline = build(&[span(3, 600), span(11, 1200), span(11, 60)], None);
        // Two notes on the 3rd; one restored for a day the history no
        // longer has.
        let notes = BTreeMap::from([
            (
                day(3),
                vec![note("a", "Met the ferryman"), note("b", "Lost")],
            ),
            (day(1), vec![note("c", "Bought it")]),
        ]);
        let list = rows(&timeline, Some(&notes), day(11), &JournalFilter::default());
        assert_eq!(
            shape(&list),
            [
                (11, 1260, true, "+".to_string()),
                (3, 600, true, "Met the ferryman".to_string()),
                (3, 0, false, "Lost".to_string()),
                (3, 0, false, "+".to_string()),
                (1, 0, true, "Bought it".to_string()),
                (1, 0, false, "+".to_string()),
            ]
        );
        assert_eq!(list[0].rem(), EMPTY_ROW_REM);
        assert_eq!(list[1].rem(), NOTE_ROW_REM);
        assert_eq!(list[3].rem(), ADD_ROW_REM);
        let with_capture = JournalRow {
            entry: Some(JournalEntry {
                capture: Some("/a.png".into()),
                ..note("d", "")
            }),
            ..list[1].clone()
        };
        assert_eq!(with_capture.rem(), NOTE_ROW_REM + SHOTS_REM);

        // Only days with notes.
        let only = JournalFilter {
            notes_only: true,
            ..Default::default()
        };
        let days: Vec<u32> = rows(&timeline, Some(&notes), day(11), &only)
            .iter()
            .map(|r| r.day.day())
            .collect();
        assert_eq!(days, [3, 3, 3, 1, 1]);

        // A search shows only the notes it finds, without rows to add.
        let search = JournalFilter {
            query: "FERRY".into(),
            ..Default::default()
        };
        assert_eq!(
            shape(&rows(&timeline, Some(&notes), day(11), &search)),
            [(3, 600, true, "Met the ferryman".to_string())]
        );
    }

    #[test]
    fn a_game_never_played_offers_todays_note() {
        let filter = JournalFilter::default();
        let list = rows(&Timeline::default(), None, day(20), &filter);
        assert_eq!(
            list,
            [JournalRow {
                day: day(20),
                seconds: 0,
                first: true,
                entry: None,
            }]
        );
        let notes = BTreeMap::from([(day(18), vec![note("a", "Bought it")])]);
        let days: Vec<u32> = rows(&Timeline::default(), Some(&notes), day(20), &filter)
            .iter()
            .map(|r| r.day.day())
            .collect();
        assert_eq!(days, [20, 18, 18]);
    }
}
