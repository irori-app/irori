//! The session history as the stats page reads it: each session reduced to
//! its local calendar day, game and length, with the facts every period
//! needs (first session per game, days played). Built once per change of
//! the history; the overview and backlog are derived from it per period.
//! Pure functions over plain data so they can be unit-tested.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use chrono::{DateTime, Local, NaiveDate};
use irori_core::game::PlaytimeRecord;
use irori_core::{GameId, Runner};
use irori_services::Session;

/// A day counts as played (and towards a streak) once games ran this long
/// on it.
pub const PLAYED_DAY_SECONDS: u64 = 60;

/// One finished session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Played {
    /// Local calendar day it started on.
    pub day: NaiveDate,
    pub game: GameId,
    pub seconds: u64,
}

/// The recorded sessions, oldest first, without crashes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct History {
    pub sessions: Vec<Played>,
    /// Day of each game's first recorded session.
    pub first_session: HashMap<GameId, NaiveDate>,
    /// Days with at least [`PLAYED_DAY_SECONDS`] of play.
    pub played_days: BTreeSet<NaiveDate>,
}

impl History {
    /// Build from raw sessions; `day_of` turns a start time into a local
    /// day (sessions whose time does not parse are left out).
    pub fn new(sessions: Vec<Session>, day_of: impl Fn(&Session) -> Option<NaiveDate>) -> Self {
        let mut list: Vec<Played> = sessions
            .into_iter()
            .filter(|s| !s.crashed)
            .filter_map(|s| {
                Some(Played {
                    day: day_of(&s)?,
                    game: game_id(&s.game)?,
                    seconds: s.seconds,
                })
            })
            .collect();
        list.sort_by_key(|p| p.day);
        let mut first_session = HashMap::new();
        let mut day_seconds: BTreeMap<NaiveDate, u64> = BTreeMap::new();
        for p in &list {
            first_session.entry(p.game.clone()).or_insert(p.day);
            *day_seconds.entry(p.day).or_default() += p.seconds;
        }
        let played_days = day_seconds
            .into_iter()
            .filter(|(_, s)| *s >= PLAYED_DAY_SECONDS)
            .map(|(d, _)| d)
            .collect();
        Self {
            sessions: list,
            first_session,
            played_days,
        }
    }

    /// Day of the first recorded session.
    pub fn first_day(&self) -> Option<NaiveDate> {
        self.sessions.first().map(|p| p.day)
    }

    /// Sessions from `start` up to (not including) `end`.
    pub fn between(&self, start: NaiveDate, end: NaiveDate) -> &[Played] {
        let from = self.sessions.partition_point(|p| p.day < start);
        let to = self.sessions.partition_point(|p| p.day < end);
        &self.sessions[from..to.max(from)]
    }

    /// The day a game was first played: its first recorded session, or the
    /// earlier first play its playtime record knows (imports from Heroic and
    /// time before the history began).
    pub fn first_played(
        &self,
        id: &GameId,
        playtime: &BTreeMap<String, PlaytimeRecord>,
    ) -> Option<NaiveDate> {
        let recorded = playtime
            .get(&id.key())
            .and_then(|r| r.first_played.as_deref())
            .and_then(rfc3339_day);
        match (self.first_session.get(id).copied(), recorded) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }
}

/// A session's start as a local calendar day.
pub fn local_day(session: &Session) -> Option<NaiveDate> {
    rfc3339_day(&session.started)
}

/// An RFC 3339 time as a local calendar day.
pub fn rfc3339_day(time: &str) -> Option<NaiveDate> {
    DateTime::parse_from_rfc3339(time)
        .ok()
        .map(|t| t.with_timezone(&Local).date_naive())
}

/// The game a `GameId::key()` names.
pub fn game_id(key: &str) -> Option<GameId> {
    let (runner, app) = key.split_once(':')?;
    Some(GameId::new(Runner::from_id(runner)?, app))
}

/// The run of consecutive played days ending today, or yesterday while
/// today has no play yet (the day is not over).
pub fn current_streak(played: &BTreeSet<NaiveDate>, today: NaiveDate) -> u32 {
    let mut day = if played.contains(&today) {
        today
    } else {
        today.pred_opt().unwrap_or(today)
    };
    let mut run = 0;
    while played.contains(&day) {
        run += 1;
        match day.pred_opt() {
            Some(d) => day = d,
            None => break,
        }
    }
    run
}

/// The longest run of consecutive played days from `start` up to (not
/// including) `end`.
pub fn longest_streak(played: &BTreeSet<NaiveDate>, start: NaiveDate, end: NaiveDate) -> u32 {
    let mut longest = 0;
    let mut run = 0;
    let mut previous: Option<NaiveDate> = None;
    for day in played.range(start..end) {
        run = match previous {
            Some(p) if p.succ_opt() == Some(*day) => run + 1,
            _ => 1,
        };
        longest = longest.max(run);
        previous = Some(*day);
    }
    longest
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn day(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    pub(crate) fn session(game: &str, d: &str, minutes: u64) -> Session {
        Session {
            game: game.into(),
            started: format!("{d}T12:00:00Z"),
            seconds: minutes * 60,
            crashed: false,
            exit_code: None,
        }
    }

    /// A history whose days are the dates written in the sessions.
    pub(crate) fn history(sessions: Vec<Session>) -> History {
        History::new(sessions, |s| s.started.get(..10).map(day))
    }

    #[test]
    fn leaves_out_crashes_and_unknown_games_and_sorts_by_day() {
        let mut crash = session("gog:2", "2026-10-02", 1);
        crash.crashed = true;
        let h = history(vec![
            session("gog:1", "2026-10-05", 30),
            session("nonsense", "2026-10-01", 30),
            crash,
            session("legendary:Fowl", "2026-10-03", 10),
            // Too short to count as a played day.
            Session {
                seconds: 20,
                ..session("gog:1", "2026-10-04", 0)
            },
        ]);
        let days: Vec<_> = h.sessions.iter().map(|p| p.day).collect();
        assert_eq!(
            days,
            [day("2026-10-03"), day("2026-10-04"), day("2026-10-05")]
        );
        assert_eq!(h.first_day(), Some(day("2026-10-03")));
        assert_eq!(
            h.first_session[&GameId::new(Runner::Gog, "1")],
            day("2026-10-04")
        );
        assert!(!h.played_days.contains(&day("2026-10-04")));
        assert_eq!(h.between(day("2026-10-04"), day("2026-10-05")).len(), 1);
        assert!(h.between(day("2026-11-01"), day("2026-12-01")).is_empty());
    }

    #[test]
    fn first_play_takes_the_earlier_of_history_and_record() {
        let h = history(vec![session("gog:1", "2026-10-05", 30)]);
        let id = GameId::new(Runner::Gog, "1");
        let mut playtime = BTreeMap::new();
        assert_eq!(h.first_played(&id, &playtime), Some(day("2026-10-05")));
        playtime.insert(
            "gog:1".to_string(),
            PlaytimeRecord {
                first_played: Some("2024-01-02T12:00:00Z".into()),
                ..Default::default()
            },
        );
        // The record's time is read as a local day, like session starts.
        assert_eq!(
            h.first_played(&id, &playtime),
            rfc3339_day("2024-01-02T12:00:00Z")
        );
    }

    #[test]
    fn streaks_count_consecutive_days() {
        let played: BTreeSet<NaiveDate> = [
            "2026-10-01",
            "2026-10-02",
            "2026-10-03",
            "2026-10-04",
            "2026-10-21",
            "2026-10-22",
        ]
        .into_iter()
        .map(day)
        .collect();
        // Today has no play yet, so yesterday's run is still current.
        assert_eq!(current_streak(&played, day("2026-10-23")), 2);
        assert_eq!(current_streak(&played, day("2026-10-25")), 0);
        assert_eq!(
            longest_streak(&played, day("2026-10-01"), day("2026-11-01")),
            4
        );
        assert_eq!(
            longest_streak(&played, day("2026-10-03"), day("2026-10-22")),
            2
        );
    }
}
