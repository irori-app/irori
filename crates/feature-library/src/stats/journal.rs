//! What the journal tab shows for one period: the notes the user wrote on
//! days in it, newest first, each with the time the game was played that
//! day.

use chrono::{Days, NaiveDate};
use irori_core::GameId;
use irori_services::{Journal, JournalEntry};

use super::model::{History, game_id};
use super::period::Period;

/// One note in the journal tab.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Note {
    pub id: GameId,
    pub day: NaiveDate,
    pub entry: JournalEntry,
    /// Time the game was played that day.
    pub seconds: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JournalPage {
    pub period: Period,
    /// Newest day first.
    pub notes: Vec<Note>,
    /// Games with a note in the period.
    pub games: usize,
}

/// The notes of `period` for games still in the library.
pub fn compute(
    journal: &Journal,
    history: &History,
    period: Period,
    is_game: impl Fn(&GameId) -> bool,
    today: NaiveDate,
) -> JournalPage {
    let end = period.end(today);
    let mut notes: Vec<Note> = irori_services::journal::all(journal)
        .into_iter()
        .filter(|(_, day, _)| *day >= period.start && *day < end)
        .filter_map(|(key, day, entry)| {
            let id = game_id(key).filter(&is_game)?;
            let next = day.checked_add_days(Days::new(1)).unwrap_or(day);
            let seconds = history
                .between(day, next)
                .iter()
                .filter(|p| p.game == id)
                .map(|p| p.seconds)
                .sum();
            Some(Note {
                id,
                day,
                entry: entry.clone(),
                seconds,
            })
        })
        .collect();
    notes.sort_by(|a, b| b.day.cmp(&a.day).then_with(|| a.id.key().cmp(&b.id.key())));
    let mut games: Vec<&GameId> = notes.iter().map(|n| &n.id).collect();
    games.sort_by_key(|id| id.key());
    games.dedup();
    JournalPage {
        period,
        games: games.len(),
        notes,
    }
}

/// The day of the oldest note, so the period stepper can reach it.
pub fn first_day(journal: &Journal) -> Option<NaiveDate> {
    journal
        .values()
        .filter_map(|days| days.keys().next())
        .min()
        .copied()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stats::model::tests::{day, history, session};
    use crate::stats::period::Span;

    fn note(text: &str) -> JournalEntry {
        JournalEntry {
            text: text.into(),
            ..Default::default()
        }
    }

    #[test]
    fn notes_of_the_period_newest_first_with_that_days_play() {
        let mut journal = Journal::new();
        journal.entry("gog:1".into()).or_default().extend([
            (day("2026-10-05"), note("a")),
            (day("2026-09-30"), note("too early")),
        ]);
        journal
            .entry("gog:2".into())
            .or_default()
            .insert(day("2026-10-20"), note("b"));
        journal
            .entry("gog:gone".into())
            .or_default()
            .insert(day("2026-10-21"), note("removed game"));
        let h = history(vec![
            session("gog:1", "2026-10-05", 30),
            session("gog:1", "2026-10-05", 15),
            session("gog:2", "2026-10-05", 60),
        ]);
        let period = Period::containing(Span::Month, day("2026-10-25"), None);
        let page = compute(
            &journal,
            &h,
            period,
            |id| id.app_name.as_ref() != "gone",
            day("2026-10-25"),
        );
        let texts: Vec<&str> = page.notes.iter().map(|n| n.entry.text.as_str()).collect();
        assert_eq!(texts, ["b", "a"]);
        assert_eq!(page.notes[1].seconds, 45 * 60);
        assert_eq!(page.notes[0].seconds, 0);
        assert_eq!(page.games, 2);
        assert_eq!(first_day(&journal), Some(day("2026-09-30")));
        assert_eq!(first_day(&Journal::new()), None);
    }
}
