//! The play history's Journal view: today, every day the game was played
//! and any day with a note, newest first, with the user's note for it: its title,
//! the start of its text and the screenshots it links. A day without a note
//! offers to write one. A row opens the note editor on click or Enter; the
//! rows are virtualized like the timeline's, at fixed heights.

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
use irori_ui::components::{art_img, physical_width};
use irori_ui::i18n::t;
use irori_ui::journal::shot_url;

use super::GamePage;
use super::timeline::{Timeline, TimelineRow, duration};
use crate::load::calendar_date;

/// Height of a day without a note, in rem.
const EMPTY_ROW_REM: f32 = 3.0;
/// Height of a day with a note, in rem.
const NOTE_ROW_REM: f32 = 6.25;
/// Extra height of a note that links screenshots, in rem.
const SHOTS_REM: f32 = 3.5;
/// Most linked screenshots a row shows.
const ROW_SHOTS: usize = 4;
/// Width of a linked screenshot in a row, in rem.
const SHOT_REM: f32 = 5.;

/// One day of the journal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct JournalRow {
    pub day: NaiveDate,
    /// Time played that day; zero for a note on a day the history lacks.
    pub seconds: u64,
    pub entry: Option<JournalEntry>,
}

impl JournalRow {
    /// Height of the row, in rem.
    pub fn rem(&self) -> f32 {
        match &self.entry {
            None => EMPTY_ROW_REM,
            Some(e) if e.screenshots.is_empty() => NOTE_ROW_REM,
            Some(_) => NOTE_ROW_REM + SHOTS_REM,
        }
    }
}

/// Today, the days played and the days with a note, newest first. Today is
/// always there, so a game not played yet (or not today) can get a note.
pub(crate) fn rows(
    timeline: &Timeline,
    notes: Option<&BTreeMap<NaiveDate, JournalEntry>>,
    today: NaiveDate,
) -> Vec<JournalRow> {
    let mut days: BTreeMap<NaiveDate, JournalRow> = BTreeMap::new();
    days.insert(
        today,
        JournalRow {
            day: today,
            seconds: 0,
            entry: None,
        },
    );
    for row in &timeline.rows {
        if let TimelineRow::Day(d) = row {
            days.insert(
                d.day,
                JournalRow {
                    day: d.day,
                    seconds: d.seconds,
                    entry: None,
                },
            );
        }
    }
    for (day, entry) in notes.into_iter().flatten() {
        days.entry(*day)
            .or_insert(JournalRow {
                day: *day,
                seconds: 0,
                entry: None,
            })
            .entry = Some(entry.clone());
    }
    days.into_values().rev().collect()
}

impl GamePage {
    /// Open the note of the journal row `ix`.
    pub(super) fn open_journal_row(
        &mut self,
        ix: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(row) = self.timeline.journal.get(ix) else {
            return;
        };
        let today = Local::now().date_naive();
        let date = calendar_date(row.day, row.day.year() != today.year());
        let title = self
            .title()
            .map(|t| t.to_string())
            .unwrap_or_else(|| self.id.app_name.to_string());
        irori_ui::journal::open_editor(self.id.clone(), row.day, &title, &date, window, cx);
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
        let (radius, fill) = (theme.radius, theme.muted);
        let active = self.timeline.active == Some(ix);
        let highlight = theme.muted.opacity(0.5);
        let played = (row.seconds > 0).then(|| duration(row.seconds));
        let body = match &row.entry {
            None => h_flex()
                .flex_1()
                .min_w_0()
                .pt(rems(0.5))
                .gap_2()
                .items_center()
                .text_sm()
                .text_color(ink.panel_muted)
                .child(Icon::new(IconName::NotebookPen).small())
                .child(div().min_w_0().truncate().child(t("game.journal.write")))
                .into_any_element(),
            Some(entry) => {
                let width = physical_width(rems(SHOT_REM), window);
                let shots: Vec<AnyElement> = entry
                    .screenshots
                    .iter()
                    .take(ROW_SHOTS)
                    .map(|path| {
                        let frame = div()
                            .w(rems(SHOT_REM))
                            .h(rems(SHOT_REM * 9. / 16.))
                            .flex_shrink_0()
                            .rounded(radius)
                            .overflow_hidden()
                            .bg(fill);
                        match art_img(&shot_url(path), width, false, cx) {
                            Some(img) => frame
                                .child(img.size_full().object_fit(ObjectFit::Cover))
                                .into_any_element(),
                            None => frame.into_any_element(),
                        }
                    })
                    .collect();
                let heading = if entry.title.is_empty() {
                    None
                } else {
                    Some(entry.title.clone())
                };
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .pt(rems(0.4))
                    .pr_2()
                    .gap_1()
                    .when_some(heading, |c, h| {
                        c.child(
                            div()
                                .w_full()
                                .truncate()
                                .text_sm()
                                .font_semibold()
                                .text_color(ink.text)
                                .child(h),
                        )
                    })
                    .child(
                        div()
                            .w_full()
                            .text_sm()
                            .text_color(ink.text)
                            .line_clamp(if entry.title.is_empty() { 3 } else { 2 })
                            .child(entry.text.clone()),
                    )
                    .when(!shots.is_empty(), |c| {
                        c.child(h_flex().pt_1().gap_2().children(shots))
                    })
                    .into_any_element()
            }
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
            .child(self.date_column(super::timeline::day_label(row.day, today), played))
            .child(self.rail(ix == 0, last, Some(super::timeline::Dot::Day), cx))
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

    #[test]
    fn days_played_and_days_with_notes_newest_first() {
        let span = |d: u32, seconds: u64| Span {
            start: day(d).and_hms_opt(20, 0, 0).unwrap(),
            seconds,
            crashed: false,
        };
        let timeline = build(&[span(3, 600), span(11, 1200), span(11, 60)], None);
        let note = JournalEntry {
            text: "Met the ferryman".into(),
            ..Default::default()
        };
        // A note restored for a day the history no longer has.
        let notes = BTreeMap::from([(day(3), note.clone()), (day(1), note.clone())]);
        let list = rows(&timeline, Some(&notes), day(11));
        let shape: Vec<(u32, u64, bool)> = list
            .iter()
            .map(|r| (r.day.day(), r.seconds, r.entry.is_some()))
            .collect();
        assert_eq!(shape, [(11, 1260, false), (3, 600, true), (1, 0, true)]);
        assert_eq!(list[0].rem(), EMPTY_ROW_REM);
        assert_eq!(list[1].rem(), NOTE_ROW_REM);
        let with_shots = JournalRow {
            entry: Some(JournalEntry {
                screenshots: vec!["/a.png".into()],
                ..note
            }),
            ..list[1].clone()
        };
        assert_eq!(with_shots.rem(), NOTE_ROW_REM + SHOTS_REM);
    }

    #[test]
    fn a_game_never_played_offers_todays_note() {
        let list = rows(&Timeline::default(), None, day(20));
        assert_eq!(
            list,
            [JournalRow {
                day: day(20),
                seconds: 0,
                entry: None,
            }]
        );
        let note = JournalEntry {
            text: "Bought it".into(),
            ..Default::default()
        };
        let notes = BTreeMap::from([(day(18), note)]);
        let days: Vec<u32> = rows(&Timeline::default(), Some(&notes), day(20))
            .iter()
            .map(|r| r.day.day())
            .collect();
        assert_eq!(days, [20, 18]);
    }
}
