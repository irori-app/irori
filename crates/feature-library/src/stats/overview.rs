//! What the overview tab shows for one period: the headline total against
//! the period before, days played, streak and sessions; play time per day
//! (or month, or year) stacked by game; the most played games; and a few
//! insights (games played side by side, a game picked up again after a long
//! break, games played for the first time).
//!
//! Times come from the session history, except all time's total and top
//! games, which come from the playtime records: they reach back before the
//! history began (imports from Heroic, time played in a store's launcher).

use std::collections::{BTreeMap, BTreeSet, HashMap};

use chrono::{Days, NaiveDate};
use irori_core::GameId;
use irori_core::game::PlaytimeRecord;

use super::model::{History, current_streak, game_id, longest_streak};
use super::period::{Period, Span, Unit};

/// Games with a colour of their own in the chart; the rest share one.
pub const CHART_GAMES: usize = 5;
/// A game counts as picked up again after this many days without it.
const COMEBACK_DAYS: i64 = 30;
/// Games count as played side by side within this many days.
const PARALLEL_DAYS: usize = 7;

/// A game's play time in the period.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeriodGame {
    pub id: GameId,
    pub seconds: u64,
    pub sessions: usize,
    /// First played in this period.
    pub new: bool,
}

/// One bar of the chart.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bar {
    pub start: NaiveDate,
    /// Seconds per [`Chart::series`] entry.
    pub parts: Vec<u64>,
}

impl Bar {
    pub fn total(&self) -> u64 {
        self.parts.iter().sum()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chart {
    pub unit: Unit,
    /// Games with their own colour, most played first, then `None` for all
    /// other games when there are any.
    pub series: Vec<Option<GameId>>,
    pub bars: Vec<Bar>,
}

/// The streak shown for a period.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Streak {
    /// The run going on now (current periods).
    Current(u32),
    /// The longest run inside a past period.
    Longest(u32),
}

/// The most games played within [`PARALLEL_DAYS`] days of each other.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Parallel {
    pub count: usize,
    /// Those games, most played in the period first.
    pub games: Vec<GameId>,
}

/// A game played again after a long break.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Comeback {
    pub id: GameId,
    pub after_days: u32,
    pub day: NaiveDate,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Overview {
    pub period: Period,
    pub seconds: u64,
    /// The period before's total, when the history reaches into it: up to
    /// the same day of it while this period is still going on.
    pub previous_seconds: Option<u64>,
    /// `previous_seconds` covers only the first days of the period before.
    pub previous_so_far: bool,
    pub days_played: u32,
    pub days: u32,
    pub streak: Streak,
    pub sessions: usize,
    /// Every game played in the period, most played first.
    pub games: Vec<PeriodGame>,
    pub chart: Chart,
    pub parallel: Option<Parallel>,
    pub comeback: Option<Comeback>,
}

impl Overview {
    /// Change against the period before, in percent, when it had any play.
    pub fn change_percent(&self) -> Option<i64> {
        let previous = self.previous_seconds.filter(|s| *s > 0)?;
        let change = (self.seconds as f64 - previous as f64) / previous as f64 * 100.0;
        Some(change.round() as i64)
    }

    /// The bar with the most play, and its total.
    pub fn busiest(&self) -> Option<(NaiveDate, u64)> {
        self.chart
            .bars
            .iter()
            .map(|b| (b.start, b.total()))
            .filter(|(_, s)| *s > 0)
            .max_by_key(|(start, s)| (*s, std::cmp::Reverse(*start)))
    }

    /// Games first played in this period, most played first.
    pub fn new_games(&self) -> impl Iterator<Item = &PeriodGame> {
        self.games.iter().filter(|g| g.new)
    }
}

/// Compute the overview of `period`. `playtime` holds the all-time totals by
/// game key; `is_game` says which games are still in the library (records
/// can name DLC or removed games, see the stats page's notes).
pub fn compute(
    history: &History,
    period: Period,
    playtime: &BTreeMap<String, PlaytimeRecord>,
    is_game: impl Fn(&GameId) -> bool,
    today: NaiveDate,
) -> Overview {
    let end = period.end(today);
    let sessions = history.between(period.start, end);

    let mut per_game: HashMap<&GameId, (u64, usize)> = HashMap::new();
    for p in sessions {
        let entry = per_game.entry(&p.game).or_default();
        entry.0 += p.seconds;
        entry.1 += 1;
    }
    let is_new = |id: &GameId| {
        history
            .first_played(id, playtime)
            .is_some_and(|first| first >= period.start)
            && history.first_session.get(id).is_some_and(|d| *d < end)
    };
    let mut games: Vec<PeriodGame> = if period.span == Span::All {
        // The records reach further back than the history.
        playtime
            .iter()
            .filter(|(_, r)| r.total_minutes > 0)
            .filter_map(|(key, r)| {
                let id = game_id(key).filter(|id| is_game(id))?;
                let recorded = per_game.get(&id).copied().unwrap_or_default();
                Some(PeriodGame {
                    seconds: (r.total_minutes * 60).max(recorded.0),
                    sessions: recorded.1,
                    new: false,
                    id,
                })
            })
            .collect()
    } else {
        per_game
            .iter()
            .map(|(id, (seconds, count))| PeriodGame {
                id: (*id).clone(),
                seconds: *seconds,
                sessions: *count,
                new: is_new(id),
            })
            .collect()
    };
    games.sort_by(|a, b| {
        b.seconds
            .cmp(&a.seconds)
            .then_with(|| a.id.key().cmp(&b.id.key()))
    });

    let seconds = if period.span == Span::All {
        let recorded: u64 = sessions.iter().map(|p| p.seconds).sum();
        games.iter().map(|g| g.seconds).sum::<u64>().max(recorded)
    } else {
        sessions.iter().map(|p| p.seconds).sum()
    };
    // A period still going on is compared with as many days of the one
    // before, not with all of it.
    let so_far = period.is_current(today);
    let previous_seconds = period.previous().and_then(|before| {
        let first = history.first_day()?;
        let before_end = if so_far {
            (before.start + Days::new(u64::from(period.days_elapsed(today)))).min(before.end(today))
        } else {
            before.end(today)
        };
        (first < before_end).then(|| {
            history
                .between(before.start, before_end)
                .iter()
                .map(|p| p.seconds)
                .sum()
        })
    });

    let days_played = history.played_days.range(period.start..end).count() as u32;
    let streak = if period.is_current(today) {
        Streak::Current(current_streak(&history.played_days, today))
    } else {
        Streak::Longest(longest_streak(&history.played_days, period.start, end))
    };

    Overview {
        period,
        seconds,
        previous_seconds,
        previous_so_far: so_far,
        days_played,
        days: period.days_elapsed(today),
        streak,
        sessions: sessions.len(),
        chart: chart(history, period, &games, today),
        parallel: parallel(history, period, &games, today),
        comeback: comeback(history, period, today),
        games,
    }
}

fn chart(history: &History, period: Period, games: &[PeriodGame], today: NaiveDate) -> Chart {
    let unit = period.unit(today);
    // Rank by time in the history, so all time's colours follow its bars.
    let mut ranked: Vec<(&GameId, u64)> = Vec::new();
    {
        let mut seconds: HashMap<&GameId, u64> = HashMap::new();
        for p in history.between(period.start, period.end(today)) {
            *seconds.entry(&p.game).or_default() += p.seconds;
        }
        ranked.extend(seconds);
        let order: HashMap<&GameId, usize> =
            games.iter().enumerate().map(|(i, g)| (&g.id, i)).collect();
        ranked.sort_by_key(|(id, s)| (std::cmp::Reverse(*s), order.get(id).copied()));
    }
    let mut series: Vec<Option<GameId>> = ranked
        .iter()
        .take(CHART_GAMES)
        .map(|(id, _)| Some((*id).clone()))
        .collect();
    if ranked.len() > CHART_GAMES {
        series.push(None);
    }
    let slot: HashMap<&GameId, usize> = ranked
        .iter()
        .enumerate()
        .map(|(i, (id, _))| (*id, i.min(CHART_GAMES)))
        .collect();
    let starts = period.bars(today);
    let mut bars: Vec<Bar> = starts
        .iter()
        .map(|start| Bar {
            start: *start,
            parts: vec![0; series.len()],
        })
        .collect();
    for p in history.between(period.start, period.end(today)) {
        let bar = starts.partition_point(|s| *s <= p.day).saturating_sub(1);
        if let (Some(bar), Some(slot)) = (bars.get_mut(bar), slot.get(&p.game)) {
            bar.parts[*slot] += p.seconds;
        }
    }
    Chart { unit, series, bars }
}

fn parallel(
    history: &History,
    period: Period,
    games: &[PeriodGame],
    today: NaiveDate,
) -> Option<Parallel> {
    let end = period.end(today);
    let mut by_day: BTreeMap<NaiveDate, BTreeSet<&GameId>> = BTreeMap::new();
    for p in history.between(period.start, end) {
        by_day.entry(p.day).or_default().insert(&p.game);
    }
    let mut best: Option<BTreeSet<&GameId>> = None;
    for day in by_day.keys() {
        let window_end = *day + Days::new(PARALLEL_DAYS as u64);
        let in_window: BTreeSet<&GameId> = by_day
            .range(*day..window_end.min(end))
            .flat_map(|(_, g)| g.iter().copied())
            .collect();
        if best.as_ref().is_none_or(|b| in_window.len() > b.len()) {
            best = Some(in_window);
        }
    }
    let best = best.filter(|b| b.len() >= 2)?;
    let order: HashMap<&GameId, usize> =
        games.iter().enumerate().map(|(i, g)| (&g.id, i)).collect();
    let mut list: Vec<GameId> = best.into_iter().cloned().collect();
    list.sort_by_key(|id| order.get(id).copied().unwrap_or(usize::MAX));
    Some(Parallel {
        count: list.len(),
        games: list,
    })
}

fn comeback(history: &History, period: Period, today: NaiveDate) -> Option<Comeback> {
    let end = period.end(today);
    let mut last: HashMap<&GameId, NaiveDate> = HashMap::new();
    let mut best: Option<Comeback> = None;
    for p in &history.sessions {
        if p.day >= end {
            break;
        }
        if let Some(before) = last.insert(&p.game, p.day)
            && p.day >= period.start
        {
            let gap = (p.day - before).num_days();
            if gap >= COMEBACK_DAYS && best.as_ref().is_none_or(|b| gap > i64::from(b.after_days)) {
                best = Some(Comeback {
                    id: p.game.clone(),
                    after_days: gap as u32,
                    day: p.day,
                });
            }
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::super::model::tests::{day, history, session};
    use super::*;
    use irori_core::Runner;

    fn id(app: &str) -> GameId {
        GameId::new(Runner::Gog, app)
    }

    fn month(d: &str) -> Period {
        Period::containing(Span::Month, day(d), None)
    }

    #[test]
    fn totals_change_days_and_streak_of_a_month() {
        let h = history(vec![
            session("gog:1", "2026-09-10", 60),
            session("gog:1", "2026-10-01", 30),
            session("gog:2", "2026-10-01", 30),
            session("gog:1", "2026-10-02", 60),
            session("gog:1", "2026-10-03", 30),
            session("gog:1", "2026-10-20", 30),
        ]);
        let today = day("2026-10-21");
        let o = compute(&h, month("2026-10-05"), &BTreeMap::new(), |_| true, today);
        assert_eq!(o.seconds, 180 * 60);
        // October is still going on: compared with September 1–21.
        assert_eq!(o.previous_seconds, Some(3600));
        assert!(o.previous_so_far);
        assert_eq!(o.change_percent(), Some(200));
        assert_eq!((o.days_played, o.days), (4, 21));
        // Yesterday was played, today not yet.
        assert_eq!(o.streak, Streak::Current(1));
        assert_eq!(o.sessions, 5);
        assert_eq!(o.games[0].id, id("1"));
        assert_eq!(o.games[0].sessions, 4);
        // Game 2 is new this month; game 1 was played in September.
        assert!(o.games[1].new && !o.games[0].new);
        assert_eq!(o.busiest(), Some((day("2026-10-01"), 3600)));

        let september = compute(&h, month("2026-09-01"), &BTreeMap::new(), |_| true, today);
        assert_eq!(september.streak, Streak::Longest(1));
        // The history starts in September: August has nothing to compare.
        assert_eq!(september.previous_seconds, None);
        assert!(!september.previous_so_far);
        assert_eq!(september.change_percent(), None);
    }

    #[test]
    fn chart_stacks_the_top_games_and_groups_the_rest() {
        let mut sessions = Vec::new();
        for (ix, app) in ["a", "b", "c", "d", "e", "f", "g"].iter().enumerate() {
            sessions.push(session(&format!("gog:{app}"), "2026-10-02", 70 - ix as u64));
        }
        let h = history(sessions);
        let today = day("2026-10-31");
        let o = compute(&h, month("2026-10-01"), &BTreeMap::new(), |_| true, today);
        assert_eq!(o.chart.unit, Unit::Day);
        assert_eq!(o.chart.bars.len(), 31);
        assert_eq!(o.chart.series.len(), CHART_GAMES + 1);
        assert_eq!(o.chart.series[0], Some(id("a")));
        assert_eq!(o.chart.series[CHART_GAMES], None);
        let second = &o.chart.bars[1];
        assert_eq!(second.parts[0], 70 * 60);
        // "f" and "g" share the last series.
        assert_eq!(second.parts[CHART_GAMES], (65 + 64) * 60);
        assert_eq!(o.chart.bars[0].total(), 0);
    }

    #[test]
    fn year_and_all_time_chart_by_month_and_all_time_counts_records() {
        let h = history(vec![
            session("gog:1", "2026-02-10", 60),
            session("gog:1", "2026-10-10", 60),
        ]);
        let today = day("2026-11-03");
        let year = Period::containing(Span::Year, today, None);
        let o = compute(&h, year, &BTreeMap::new(), |_| true, today);
        assert_eq!(o.chart.bars.len(), 12);
        assert_eq!(o.chart.bars[1].total(), 3600);
        assert_eq!(o.chart.bars[9].total(), 3600);

        let mut playtime = BTreeMap::new();
        playtime.insert(
            "gog:1".to_string(),
            PlaytimeRecord {
                total_minutes: 600,
                ..Default::default()
            },
        );
        playtime.insert(
            "gog:dlc".to_string(),
            PlaytimeRecord {
                total_minutes: 600,
                ..Default::default()
            },
        );
        let all = Period::containing(Span::All, today, h.first_day());
        let o = compute(&h, all, &playtime, |g| g.app_name.as_ref() == "1", today);
        assert_eq!(o.seconds, 600 * 60);
        assert_eq!(o.games.len(), 1);
        assert_eq!(o.games[0].sessions, 2);
        assert_eq!(o.chart.unit, Unit::Month);
        assert_eq!(o.previous_seconds, None);
    }

    #[test]
    fn finds_games_played_side_by_side_and_comebacks() {
        let h = history(vec![
            session("gog:old", "2026-06-01", 60),
            session("gog:a", "2026-10-01", 60),
            session("gog:b", "2026-10-03", 30),
            session("gog:c", "2026-10-07", 10),
            // A week after the first three: never with "a" in one window.
            session("gog:d", "2026-10-20", 10),
            session("gog:old", "2026-10-21", 90),
        ]);
        let today = day("2026-10-31");
        let o = compute(&h, month("2026-10-01"), &BTreeMap::new(), |_| true, today);
        let parallel = o.parallel.clone().unwrap();
        assert_eq!(parallel.count, 3);
        assert_eq!(parallel.games, [id("a"), id("b"), id("c")]);
        assert_eq!(
            o.comeback,
            Some(Comeback {
                id: id("old"),
                after_days: 142,
                day: day("2026-10-21"),
            })
        );
        let new: Vec<_> = o.new_games().map(|g| g.id.clone()).collect();
        assert_eq!(new, [id("a"), id("b"), id("c"), id("d")]);
    }

    #[test]
    fn a_week_without_play_is_empty() {
        let h = history(vec![session("gog:1", "2026-10-01", 60)]);
        let today = day("2026-11-03");
        let week = Period::containing(Span::Week, today, None);
        let o = compute(&h, week, &BTreeMap::new(), |_| true, today);
        assert_eq!((o.seconds, o.sessions), (0, 0));
        assert_eq!(o.chart.bars.len(), 7);
        assert!(o.parallel.is_none() && o.comeback.is_none() && o.busiest().is_none());
        assert_eq!(o.previous_seconds, Some(0));
        assert_eq!(o.change_percent(), None);
    }
}
