//! One game's numbers for the stats page's game sheet: its time in the
//! period and in all, a bar per day (week and month), week (year) or month
//! (all time), its sessions with their average and longest, the hour its
//! play usually starts, how much of it falls on weekends, its first and last
//! play and the days its status changed. Pure functions over the history.

use std::collections::BTreeMap;

use chrono::{Datelike as _, Days, Months, NaiveDate, Weekday};
use irori_core::GameId;
use irori_core::game::PlaytimeRecord;
use irori_services::{PlayStatus, StatusChange};

use super::model::{History, rfc3339_day};
use super::period::{Period, Span};

/// What one bar of a game's sheet covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SheetUnit {
    Day,
    Week,
    Month,
}

impl SheetUnit {
    /// The bars a period's sheet is drawn in.
    pub fn of(span: Span) -> Self {
        match span {
            Span::Week | Span::Month => SheetUnit::Day,
            Span::Year => SheetUnit::Week,
            Span::All => SheetUnit::Month,
        }
    }

    /// Start of the bar holding `day`.
    pub fn start_of(self, day: NaiveDate) -> NaiveDate {
        match self {
            SheetUnit::Day => day,
            SheetUnit::Week => day - Days::new(u64::from(day.weekday().num_days_from_monday())),
            SheetUnit::Month => day.with_day(1).unwrap_or(day),
        }
    }

    fn next(self, start: NaiveDate) -> NaiveDate {
        match self {
            SheetUnit::Day => start + Days::new(1),
            SheetUnit::Week => start + Days::new(7),
            SheetUnit::Month => start + Months::new(1),
        }
    }
}

/// A game's numbers over one period.
#[derive(Debug, Clone, PartialEq)]
pub struct GameSheet {
    pub id: GameId,
    pub period: Period,
    pub unit: SheetUnit,
    /// Seconds played in the period.
    pub seconds: u64,
    /// Seconds played in all: the larger of its playtime record and its
    /// sessions.
    pub total_seconds: u64,
    /// Each bar's start and seconds, in order.
    pub bars: Vec<(NaiveDate, u64)>,
    /// Sessions in the period.
    pub sessions: usize,
    pub average_seconds: u64,
    pub longest_seconds: u64,
    /// Days of the period it was played on.
    pub days: usize,
    /// The local hour most of its play in the period started in.
    pub usual_hour: Option<u8>,
    /// Share of its play in the period on Saturdays and Sundays.
    pub weekend_share: Option<f32>,
    pub first_played: Option<NaiveDate>,
    pub last_played: Option<NaiveDate>,
    /// Days its status changed, oldest first (`None` is cleared).
    pub changes: Vec<(NaiveDate, Option<PlayStatus>)>,
}

/// The sheet of `id` over `period`.
pub fn compute(
    history: &History,
    id: &GameId,
    period: Period,
    playtime: &BTreeMap<String, PlaytimeRecord>,
    changes: &[StatusChange],
    today: NaiveDate,
) -> GameSheet {
    let end = period.end(today);
    let unit = SheetUnit::of(period.span);
    let mut bars = Vec::new();
    let mut at = unit.start_of(period.start);
    while at < end {
        bars.push((at, 0u64));
        at = unit.next(at);
    }
    let mut seconds = 0;
    let mut sessions = 0;
    let mut longest = 0;
    let mut by_hour = [0u64; 24];
    let mut weekend = 0;
    let mut days = std::collections::BTreeSet::new();
    for p in history.between(period.start, end) {
        if p.game != *id {
            continue;
        }
        seconds += p.seconds;
        sessions += 1;
        longest = longest.max(p.seconds);
        by_hour[usize::from(p.hour.min(23))] += p.seconds;
        if matches!(p.day.weekday(), Weekday::Sat | Weekday::Sun) {
            weekend += p.seconds;
        }
        days.insert(p.day);
        let start = unit.start_of(p.day);
        if let Some(bar) = bars.iter_mut().find(|(s, _)| *s == start) {
            bar.1 += p.seconds;
        }
    }
    let all: Vec<_> = history.sessions.iter().filter(|p| p.game == *id).collect();
    let recorded = playtime.get(&id.key());
    let session_total: u64 = all.iter().map(|p| p.seconds).sum();
    let record_total = recorded.map_or(0, |r| r.total_minutes * 60 + r.extra_seconds);
    let last_recorded = recorded
        .and_then(|r| r.last_played.as_deref())
        .and_then(rfc3339_day);
    let usual_hour = (seconds > 0).then(|| {
        by_hour
            .iter()
            .enumerate()
            .max_by_key(|(h, s)| (**s, std::cmp::Reverse(*h)))
            .map_or(0, |(h, _)| h as u8)
    });
    GameSheet {
        id: id.clone(),
        period,
        unit,
        seconds,
        total_seconds: session_total.max(record_total),
        bars,
        sessions,
        average_seconds: if sessions > 0 {
            seconds / sessions as u64
        } else {
            0
        },
        longest_seconds: longest,
        days: days.len(),
        usual_hour,
        weekend_share: (seconds > 0).then(|| weekend as f32 / seconds as f32),
        first_played: history.first_played(id, playtime),
        last_played: all.last().map(|p| p.day).max(last_recorded),
        changes: changes
            .iter()
            .filter_map(|c| Some((rfc3339_day(&c.at)?, c.status)))
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stats::model::tests::{day, history, session, session_at};
    use irori_core::Runner;

    #[test]
    fn a_games_month_in_bars_with_its_habits() {
        let h = history(vec![
            // Saturday and Sunday evenings, a Wednesday lunch.
            session_at("gog:1", "2026-10-03", 21, 120),
            session_at("gog:1", "2026-10-04", 21, 60),
            session_at("gog:1", "2026-10-07", 12, 60),
            session_at("gog:1", "2026-09-30", 21, 600),
            session("gog:2", "2026-10-07", 30),
        ]);
        let id = GameId::new(Runner::Gog, "1");
        let period = Period::containing(Span::Month, day("2026-10-15"), None);
        let changes = vec![StatusChange {
            at: "2026-10-07T13:00:00Z".into(),
            status: Some(PlayStatus::Beaten),
        }];
        let s = compute(
            &h,
            &id,
            period,
            &BTreeMap::new(),
            &changes,
            day("2026-10-15"),
        );
        assert_eq!(s.unit, SheetUnit::Day);
        assert_eq!(s.bars.len(), 31);
        assert_eq!(s.bars[2], (day("2026-10-03"), 120 * 60));
        assert_eq!(s.seconds, 240 * 60);
        assert_eq!(s.total_seconds, 840 * 60);
        assert_eq!((s.sessions, s.days), (3, 3));
        assert_eq!(s.average_seconds, 80 * 60);
        assert_eq!(s.longest_seconds, 120 * 60);
        assert_eq!(s.usual_hour, Some(21));
        assert_eq!(s.weekend_share, Some(0.75));
        assert_eq!(s.first_played, Some(day("2026-09-30")));
        assert_eq!(s.last_played, Some(day("2026-10-07")));
        assert_eq!(s.changes.len(), 1);
        assert_eq!(s.changes[0].1, Some(PlayStatus::Beaten));
    }

    #[test]
    fn a_year_is_drawn_by_week_and_an_unplayed_period_has_no_habits() {
        let h = history(vec![session("gog:1", "2026-01-07", 60)]);
        let id = GameId::new(Runner::Gog, "1");
        let year = Period::containing(Span::Year, day("2026-06-01"), None);
        let s = compute(&h, &id, year, &BTreeMap::new(), &[], day("2026-06-01"));
        assert_eq!(s.unit, SheetUnit::Week);
        // The first week starts on the Monday before 1 January.
        assert_eq!(s.bars[0].0, day("2025-12-29"));
        assert_eq!(s.bars[1], (day("2026-01-05"), 3600));
        let week = Period::containing(Span::Week, day("2026-06-01"), None);
        let none = compute(&h, &id, week, &BTreeMap::new(), &[], day("2026-06-01"));
        assert_eq!(
            (none.seconds, none.usual_hour, none.weekend_share),
            (0, None, None)
        );
        assert_eq!(none.bars.len(), 7);
        assert_eq!(none.total_seconds, 3600);
    }
}
