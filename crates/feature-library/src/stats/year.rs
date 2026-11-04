//! The numbers behind the Year in review: one calendar year of the session
//! history in local time — totals, the most played games and their share,
//! month by month, the longest session, streak and day, games picked up
//! again, games started and beaten, when in the day and week the user
//! plays, and on which stores and systems. Pure functions over plain data,
//! computed off the UI thread and unit-tested here.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use chrono::{Datelike as _, Duration, NaiveDate, NaiveDateTime, Timelike as _};
use irori_core::{GameId, Runner};
use irori_services::{PlayStatus, Session, UserData};

use super::model::{PLAYED_DAY_SECONDS, game_id, rfc3339_day};

/// A game counts as picked up again after this many days without it.
pub const COMEBACK_DAYS: i64 = 60;
/// Most comebacks the review tells about.
const MAX_COMEBACKS: usize = 3;
/// A session's time is spread over the hours it ran up to this long; a
/// longer one (a clock jump, a game left running for days) counts its
/// remainder in its last hour.
const MAX_SPREAD_SECONDS: u64 = 3 * 24 * 3600;

/// One session that ran, with its local start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Timed {
    pub game: GameId,
    pub start: NaiveDateTime,
    pub seconds: u64,
}

/// The sessions that ran (crashes left out), oldest first. `local` turns an
/// RFC 3339 start into local time; sessions it cannot read are left out.
pub fn timed(sessions: Vec<Session>, local: impl Fn(&str) -> Option<NaiveDateTime>) -> Vec<Timed> {
    let mut list: Vec<Timed> = sessions
        .into_iter()
        .filter(|s| !s.crashed)
        .filter_map(|s| {
            Some(Timed {
                game: game_id(&s.game)?,
                start: local(&s.started)?,
                seconds: s.seconds,
            })
        })
        .collect();
    list.sort_by_key(|t| t.start);
    list
}

/// The years with any play, newest first.
pub fn years(sessions: &[Timed]) -> Vec<i32> {
    let years: BTreeSet<i32> = sessions
        .iter()
        .filter(|s| s.seconds > 0)
        .map(|s| s.start.year())
        .collect();
    years.into_iter().rev().collect()
}

/// The year a review opens on: the current one in December, else the last
/// one that is over, else the current one so far.
pub fn default_year(today: NaiveDate, years: &[i32]) -> Option<i32> {
    let now = today.year();
    if today.month() == 12 && years.contains(&now) {
        return Some(now);
    }
    years
        .iter()
        .copied()
        .find(|y| *y < now)
        .or_else(|| years.first().copied())
}

/// Whether the stats page offers `year`'s review on `today`: through
/// December and January, for a year with play.
pub fn offered_year(today: NaiveDate, years: &[i32]) -> Option<i32> {
    let year = match today.month() {
        12 => today.year(),
        1 => today.year() - 1,
        _ => return None,
    };
    years.contains(&year).then_some(year)
}

/// A game's year.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GameYear {
    pub id: GameId,
    pub seconds: u64,
    pub sessions: u32,
    /// Days it was played on.
    pub days: u32,
}

/// A run of consecutive days played.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Run {
    pub start: NaiveDate,
    pub days: u32,
}

/// A game played again after a long break.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Comeback {
    pub id: GameId,
    /// Days without it before it came back.
    pub rested: i64,
    pub back: NaiveDate,
}

/// The part of the day most play falls in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Daypart {
    /// 05:00–11:59.
    Morning,
    /// 12:00–16:59.
    Afternoon,
    /// 17:00–21:59.
    Evening,
    /// 22:00–04:59.
    Night,
}

impl Daypart {
    fn of_hour(hour: usize) -> Self {
        match hour {
            5..=11 => Daypart::Morning,
            12..=16 => Daypart::Afternoon,
            17..=21 => Daypart::Evening,
            _ => Daypart::Night,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct YearReview {
    pub year: i32,
    pub seconds: u64,
    pub sessions: u32,
    /// Days with at least [`PLAYED_DAY_SECONDS`] of play.
    pub days_played: u32,
    /// The year before's total, when it had any play.
    pub previous_seconds: Option<u64>,
    /// Every game played, most played first.
    pub games: Vec<GameYear>,
    /// Seconds per month, January first.
    pub months: [u64; 12],
    /// Seconds per hour of the day, spread over the hours sessions ran.
    pub hours: [u64; 24],
    /// Seconds per weekday, Monday first, spread like `hours`.
    pub weekdays: [u64; 7],
    pub longest_session: Option<Timed>,
    pub longest_streak: Option<Run>,
    /// The day with the most play, and its total.
    pub busiest_day: Option<(NaiveDate, u64)>,
    /// The longest breaks that ended this year, longest first.
    pub comebacks: Vec<Comeback>,
    /// Games first played this year, most played first.
    pub new_games: Vec<GameId>,
    /// Games marked beaten this year, in the order they were.
    pub beaten: Vec<(GameId, NaiveDate)>,
    /// Play time by store, most first.
    pub stores: Vec<(Runner, u64)>,
    /// Play time of emulated games by system id, most first.
    pub systems: Vec<(String, u64)>,
}

impl YearReview {
    /// Whether there is anything to tell.
    pub fn is_empty(&self) -> bool {
        self.seconds == 0
    }

    /// Change against the year before, in percent.
    pub fn change_percent(&self) -> Option<i64> {
        let previous = self.previous_seconds.filter(|s| *s > 0)?;
        let change = (self.seconds as f64 - previous as f64) / previous as f64 * 100.0;
        Some(change.round() as i64)
    }

    /// A game's share of the year's play, from 0 to 1.
    pub fn share(&self, game: &GameYear) -> f32 {
        if self.seconds == 0 {
            0.
        } else {
            game.seconds as f32 / self.seconds as f32
        }
    }

    /// The month with the most play (0 is January).
    pub fn busiest_month(&self) -> Option<usize> {
        busiest(&self.months)
    }

    /// The weekday with the most play (0 is Monday).
    pub fn busiest_weekday(&self) -> Option<usize> {
        busiest(&self.weekdays)
    }

    /// The part of the day with the most play.
    pub fn daypart(&self) -> Option<Daypart> {
        let mut parts: Vec<(Daypart, u64)> = Vec::new();
        for (hour, seconds) in self.hours.iter().enumerate() {
            let part = Daypart::of_hour(hour);
            match parts.iter_mut().find(|(p, _)| *p == part) {
                Some((_, s)) => *s += seconds,
                None => parts.push((part, *seconds)),
            }
        }
        parts
            .into_iter()
            .filter(|(_, s)| *s > 0)
            .max_by_key(|(_, s)| *s)
            .map(|(p, _)| p)
    }

    /// The game most played in `month` (0 is January).
    pub fn top_of_month<'a>(&self, sessions: &'a [Timed], month: usize) -> Option<&'a GameId> {
        let mut by_game: HashMap<&GameId, u64> = HashMap::new();
        for s in sessions
            .iter()
            .filter(|s| s.start.year() == self.year && s.start.month0() as usize == month)
        {
            *by_game.entry(&s.game).or_default() += s.seconds;
        }
        by_game
            .into_iter()
            .max_by(|a, b| a.1.cmp(&b.1).then_with(|| b.0.key().cmp(&a.0.key())))
            .map(|(id, _)| id)
    }
}

/// The index of the largest non-zero value; the first of equal ones.
fn busiest(values: &[u64]) -> Option<usize> {
    let max = values.iter().copied().max().filter(|m| *m > 0)?;
    values.iter().position(|v| *v == max)
}

/// Add `seconds` from `start` to the hour and weekday each part of it ran in.
fn spread(start: NaiveDateTime, seconds: u64, hours: &mut [u64; 24], weekdays: &mut [u64; 7]) {
    let mut at = start;
    let mut left = seconds.min(MAX_SPREAD_SECONDS);
    while left > 0 {
        let into_hour = u64::from(at.minute() * 60 + at.second());
        let chunk = left.min(3600 - into_hour);
        hours[at.hour() as usize] += chunk;
        weekdays[at.weekday().num_days_from_monday() as usize] += chunk;
        left -= chunk;
        at += Duration::seconds(chunk as i64);
    }
    // What a session ran beyond the spread counts where it stopped spreading.
    let rest = seconds.saturating_sub(MAX_SPREAD_SECONDS);
    if rest > 0 {
        hours[at.hour() as usize] += rest;
        weekdays[at.weekday().num_days_from_monday() as usize] += rest;
    }
}

/// The longest run of consecutive days in `days`; the earliest of equal ones.
fn longest_run(days: &BTreeSet<NaiveDate>) -> Option<Run> {
    let mut best: Option<Run> = None;
    let mut current: Option<Run> = None;
    let mut previous: Option<NaiveDate> = None;
    for day in days {
        current = match (current, previous) {
            (Some(run), Some(p)) if p.succ_opt() == Some(*day) => Some(Run {
                days: run.days + 1,
                ..run
            }),
            _ => Some(Run {
                start: *day,
                days: 1,
            }),
        };
        if current.is_some_and(|c| best.is_none_or(|b| c.days > b.days)) {
            best = current;
        }
        previous = Some(*day);
    }
    best
}

/// The review of `year`. `sessions` is the whole history (from [`timed`]),
/// so breaks and first plays can reach back before the year; `system_of`
/// names an emulated game's system.
pub fn compute(
    sessions: &[Timed],
    year: i32,
    user: &UserData,
    system_of: impl Fn(&GameId) -> Option<String>,
) -> YearReview {
    let in_year: Vec<&Timed> = sessions.iter().filter(|s| s.start.year() == year).collect();
    let previous_seconds = Some(
        sessions
            .iter()
            .filter(|s| s.start.year() == year - 1)
            .map(|s| s.seconds)
            .sum::<u64>(),
    )
    .filter(|s| *s > 0);

    let mut months = [0u64; 12];
    let mut hours = [0u64; 24];
    let mut weekdays = [0u64; 7];
    let mut by_day: BTreeMap<NaiveDate, u64> = BTreeMap::new();
    let mut games: HashMap<GameId, (u64, u32, BTreeSet<NaiveDate>)> = HashMap::new();
    let mut stores: HashMap<Runner, u64> = HashMap::new();
    let mut systems: HashMap<String, u64> = HashMap::new();
    let mut longest_session: Option<&Timed> = None;
    for s in &in_year {
        let day = s.start.date();
        months[s.start.month0() as usize] += s.seconds;
        spread(s.start, s.seconds, &mut hours, &mut weekdays);
        *by_day.entry(day).or_default() += s.seconds;
        let g = games.entry(s.game.clone()).or_default();
        g.0 += s.seconds;
        g.1 += 1;
        g.2.insert(day);
        *stores.entry(s.game.runner).or_default() += s.seconds;
        if longest_session.is_none_or(|l| s.seconds > l.seconds) {
            longest_session = Some(s);
        }
    }
    for (id, (seconds, _, _)) in &games {
        if let Some(system) = system_of(id) {
            *systems.entry(system).or_default() += seconds;
        }
    }

    let mut games: Vec<GameYear> = games
        .into_iter()
        .map(|(id, (seconds, sessions, days))| GameYear {
            id,
            seconds,
            sessions,
            days: days.len() as u32,
        })
        .collect();
    games.sort_by(|a, b| {
        b.seconds
            .cmp(&a.seconds)
            .then_with(|| a.id.key().cmp(&b.id.key()))
    });

    let played_days: BTreeSet<NaiveDate> = by_day
        .iter()
        .filter(|(_, s)| **s >= PLAYED_DAY_SECONDS)
        .map(|(d, _)| *d)
        .collect();
    let busiest_day = by_day
        .iter()
        .filter(|(_, s)| **s > 0)
        .max_by(|a, b| a.1.cmp(b.1).then_with(|| b.0.cmp(a.0)))
        .map(|(d, s)| (*d, *s));

    // Breaks: from each game's previous day played, anywhere in the history.
    let mut last_day: HashMap<&GameId, NaiveDate> = HashMap::new();
    let mut comebacks: Vec<Comeback> = Vec::new();
    for s in sessions {
        let day = s.start.date();
        if let Some(previous) = last_day.insert(&s.game, day) {
            let rested = (day - previous).num_days() - 1;
            if day.year() == year && rested >= COMEBACK_DAYS {
                match comebacks.iter_mut().find(|c| c.id == s.game) {
                    Some(c) if c.rested >= rested => {}
                    Some(c) => {
                        c.rested = rested;
                        c.back = day;
                    }
                    None => comebacks.push(Comeback {
                        id: s.game.clone(),
                        rested,
                        back: day,
                    }),
                }
            }
        }
    }
    comebacks.sort_by(|a, b| b.rested.cmp(&a.rested).then(a.back.cmp(&b.back)));
    comebacks.truncate(MAX_COMEBACKS);

    // New: the first session ever is this year, and the playtime record
    // (imports, time before the history) knows of nothing earlier.
    let mut first_session: HashMap<&GameId, NaiveDate> = HashMap::new();
    for s in sessions {
        first_session.entry(&s.game).or_insert(s.start.date());
    }
    let new_games: Vec<GameId> = games
        .iter()
        .filter(|g| first_session.get(&g.id).is_some_and(|d| d.year() == year))
        .filter(|g| {
            user.playtime
                .get(&g.id.key())
                .and_then(|r| r.first_played.as_deref())
                .and_then(rfc3339_day)
                .is_none_or(|d| d.year() >= year)
        })
        .map(|g| g.id.clone())
        .collect();

    let mut beaten: Vec<(GameId, NaiveDate)> = user
        .play_status
        .iter()
        .filter(|(_, r)| r.status == PlayStatus::Beaten)
        .filter_map(|(key, r)| Some((game_id(key)?, rfc3339_day(&r.since)?)))
        .filter(|(_, day)| day.year() == year)
        .collect();
    beaten.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.key().cmp(&b.0.key())));

    let mut stores: Vec<(Runner, u64)> = stores.into_iter().collect();
    stores.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.id().cmp(b.0.id())));
    let mut systems: Vec<(String, u64)> = systems.into_iter().collect();
    systems.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));

    YearReview {
        year,
        seconds: in_year.iter().map(|s| s.seconds).sum(),
        sessions: in_year.len() as u32,
        days_played: played_days.len() as u32,
        previous_seconds,
        games,
        months,
        hours,
        weekdays,
        longest_session: longest_session.cloned(),
        longest_streak: longest_run(&played_days),
        busiest_day,
        comebacks,
        new_games,
        beaten,
        stores,
        systems,
    }
}

/// One full-window story of the review, in the order they are told.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Story {
    Intro,
    Numbers,
    TopGame,
    TopGames,
    Months,
    Records,
    Rhythm,
    Comebacks,
    NewAndBeaten,
    Stores,
    Summary,
}

/// The stories `review` has something to tell in; none for an empty year.
pub fn stories(review: &YearReview) -> Vec<Story> {
    if review.is_empty() {
        return Vec::new();
    }
    let distinct_months = review.months.iter().filter(|m| **m > 0).count();
    [
        (Story::Intro, true),
        (Story::Numbers, true),
        (Story::TopGame, !review.games.is_empty()),
        (Story::TopGames, review.games.len() > 1),
        (Story::Months, distinct_months > 1),
        (Story::Records, review.longest_session.is_some()),
        (Story::Rhythm, review.daypart().is_some()),
        (Story::Comebacks, !review.comebacks.is_empty()),
        (
            Story::NewAndBeaten,
            !review.new_games.is_empty() || !review.beaten.is_empty(),
        ),
        (
            Story::Stores,
            review.stores.len() > 1 || !review.systems.is_empty(),
        ),
        (Story::Summary, true),
    ]
    .into_iter()
    .filter(|(_, shown)| *shown)
    .map(|(story, _)| story)
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use irori_core::game::PlaytimeRecord;
    use irori_services::PlayStatusRecord;

    fn at(s: &str) -> NaiveDateTime {
        NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M").unwrap()
    }

    fn day(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    fn play(game: &str, start: &str, minutes: u64) -> Timed {
        Timed {
            game: game_id(game).unwrap(),
            start: at(start),
            seconds: minutes * 60,
        }
    }

    fn gog(app: &str) -> GameId {
        GameId::new(Runner::Gog, app)
    }

    fn history() -> Vec<Timed> {
        let mut list = vec![
            // 2025: one long evening of Hollow Hill.
            play("gog:hill", "2025-03-01 20:00", 120),
            // 2026.
            play("gog:hill", "2026-01-10 21:30", 90),
            play("gog:hill", "2026-01-11 21:00", 60),
            play("gog:hill", "2026-01-12 22:00", 60),
            play("legendary:Fowl", "2026-02-07 09:00", 30),
            play("legendary:Fowl", "2026-06-20 23:30", 240),
            play("emulation:abc", "2026-06-21 14:00", 45),
            play("gog:hill", "2026-11-02 20:00", 30),
        ];
        list.sort_by_key(|t| t.start);
        list
    }

    fn system(id: &GameId) -> Option<String> {
        (id.runner == Runner::Emulation).then(|| "snes".to_string())
    }

    #[test]
    fn reads_sessions_in_local_time_without_crashes() {
        let sessions = vec![
            Session {
                game: "gog:1".into(),
                started: "2026-01-02T10:00:00Z".into(),
                seconds: 60,
                crashed: false,
                exit_code: None,
            },
            Session {
                game: "gog:2".into(),
                started: "2025-12-31T23:00:00Z".into(),
                seconds: 5,
                crashed: true,
                exit_code: Some(1),
            },
            Session {
                game: "nonsense".into(),
                started: "2026-01-01T10:00:00Z".into(),
                seconds: 60,
                crashed: false,
                exit_code: None,
            },
        ];
        // Times read as if the user lived at UTC+2.
        let list = timed(sessions, |s| {
            chrono::DateTime::parse_from_rfc3339(s)
                .ok()
                .map(|t| t.naive_utc() + Duration::hours(2))
        });
        assert_eq!(list, [play("gog:1", "2026-01-02 12:00", 1)]);
    }

    #[test]
    fn totals_games_and_shares() {
        let r = compute(&history(), 2026, &UserData::default(), system);
        assert_eq!(r.seconds, (90 + 60 + 60 + 30 + 240 + 45 + 30) * 60);
        assert_eq!(r.sessions, 7);
        assert_eq!(r.days_played, 7);
        assert_eq!(r.previous_seconds, Some(120 * 60));
        assert_eq!(r.change_percent(), Some(363));
        let order: Vec<String> = r.games.iter().map(|g| g.id.key()).collect();
        assert_eq!(order, ["legendary:Fowl", "gog:hill", "emulation:abc"]);
        assert_eq!(r.games[1].sessions, 4);
        assert_eq!(r.games[1].days, 4);
        assert!((r.share(&r.games[1]) - 240. / 555.).abs() < 1e-6);
        assert_eq!(r.months[0], 210 * 60);
        assert_eq!(r.busiest_month(), Some(5));
        assert_eq!(
            r.top_of_month(&history(), 5),
            Some(&game_id("legendary:Fowl").unwrap())
        );
    }

    #[test]
    fn longest_session_streak_and_day() {
        let r = compute(&history(), 2026, &UserData::default(), system);
        assert_eq!(
            r.longest_session,
            Some(play("legendary:Fowl", "2026-06-20 23:30", 240))
        );
        assert_eq!(
            r.longest_streak,
            Some(Run {
                start: day("2026-01-10"),
                days: 3
            })
        );
        // The session crossing midnight counts on the day it started.
        assert_eq!(r.busiest_day, Some((day("2026-06-20"), 240 * 60)));
    }

    #[test]
    fn hours_and_weekdays_follow_the_time_sessions_ran() {
        let r = compute(&history(), 2026, &UserData::default(), system);
        // 23:30 for four hours: half an hour at 23, then 0, 1, 2 and half of 3.
        assert_eq!(r.hours[23], 30 * 60);
        assert_eq!(r.hours[0], 3600);
        assert_eq!(r.hours[3], 30 * 60);
        assert_eq!(r.hours.iter().sum::<u64>(), r.seconds);
        assert_eq!(r.weekdays.iter().sum::<u64>(), r.seconds);
        // Saturday 20 June 23:30 runs into Sunday.
        assert_eq!(r.weekdays[5], (90 + 30 + 30) * 60);
        assert_eq!(r.busiest_weekday(), Some(6));
        assert_eq!(r.daypart(), Some(Daypart::Night));
    }

    #[test]
    fn runaway_sessions_do_not_spread_forever() {
        let mut hours = [0; 24];
        let mut weekdays = [0; 7];
        spread(
            at("2026-01-01 00:00"),
            u64::MAX / 2,
            &mut hours,
            &mut weekdays,
        );
        assert_eq!(hours.iter().sum::<u64>(), u64::MAX / 2);
    }

    #[test]
    fn comebacks_new_games_and_beaten() {
        let mut user = UserData::default();
        user.play_status.insert(
            "legendary:Fowl".into(),
            PlayStatusRecord {
                status: PlayStatus::Beaten,
                since: "2026-06-21T12:00:00Z".into(),
            },
        );
        user.play_status.insert(
            "gog:old".into(),
            PlayStatusRecord {
                status: PlayStatus::Beaten,
                since: "2025-06-21T12:00:00Z".into(),
            },
        );
        // Played before the history began, per its imported record.
        user.playtime.insert(
            "emulation:abc".into(),
            PlaytimeRecord {
                first_played: Some("2024-05-01T12:00:00Z".into()),
                ..Default::default()
            },
        );
        let r = compute(&history(), 2026, &user, system);
        assert_eq!(
            r.comebacks,
            [
                Comeback {
                    id: gog("hill"),
                    rested: (day("2026-01-10") - day("2025-03-01")).num_days() - 1,
                    back: day("2026-01-10"),
                },
                Comeback {
                    id: game_id("legendary:Fowl").unwrap(),
                    rested: (day("2026-06-20") - day("2026-02-07")).num_days() - 1,
                    back: day("2026-06-20"),
                },
            ]
        );
        assert_eq!(r.new_games, [game_id("legendary:Fowl").unwrap()]);
        assert_eq!(
            r.beaten,
            [(game_id("legendary:Fowl").unwrap(), day("2026-06-21"))]
        );
    }

    #[test]
    fn stores_and_systems() {
        let r = compute(&history(), 2026, &UserData::default(), system);
        assert_eq!(
            r.stores,
            [
                (Runner::Legendary, 270 * 60),
                (Runner::Gog, 240 * 60),
                (Runner::Emulation, 45 * 60),
            ]
        );
        assert_eq!(r.systems, [("snes".to_string(), 45 * 60)]);
    }

    #[test]
    fn an_empty_year_has_nothing_to_tell() {
        let r = compute(&history(), 2024, &UserData::default(), system);
        assert!(r.is_empty());
        assert!(r.games.is_empty() && r.longest_session.is_none());
        assert_eq!(r.daypart(), None);
        assert_eq!(r.busiest_month(), None);
    }

    #[test]
    fn stories_skip_what_the_year_has_nothing_for() {
        let r = compute(&history(), 2026, &UserData::default(), system);
        assert_eq!(
            stories(&r),
            [
                Story::Intro,
                Story::Numbers,
                Story::TopGame,
                Story::TopGames,
                Story::Months,
                Story::Records,
                Story::Rhythm,
                Story::Comebacks,
                Story::NewAndBeaten,
                Story::Stores,
                Story::Summary,
            ]
        );
        // One evening of a new game: no ranking, months, comebacks or stores.
        let r = compute(&history(), 2025, &UserData::default(), system);
        assert_eq!(
            stories(&r),
            [
                Story::Intro,
                Story::Numbers,
                Story::TopGame,
                Story::Records,
                Story::Rhythm,
                Story::NewAndBeaten,
                Story::Summary,
            ]
        );
        assert!(stories(&compute(&history(), 2024, &UserData::default(), system)).is_empty());
    }

    #[test]
    fn which_year_opens_and_when_it_is_offered() {
        let years = years(&history());
        assert_eq!(years, [2026, 2025]);
        assert_eq!(default_year(day("2026-12-03"), &years), Some(2026));
        assert_eq!(default_year(day("2026-11-03"), &years), Some(2025));
        assert_eq!(default_year(day("2027-01-03"), &years), Some(2026));
        assert_eq!(default_year(day("2026-05-01"), &[2026]), Some(2026));
        assert_eq!(default_year(day("2026-05-01"), &[]), None);
        assert_eq!(offered_year(day("2026-12-01"), &years), Some(2026));
        assert_eq!(offered_year(day("2027-01-31"), &years), Some(2026));
        assert_eq!(offered_year(day("2026-11-30"), &years), None);
        assert_eq!(offered_year(day("2025-12-24"), &[2026]), None);
    }
}
