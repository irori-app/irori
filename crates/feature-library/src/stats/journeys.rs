//! What the Journeys tab shows for one period: a lane per game played in
//! it, with the days it was played and how long, where it started (in the
//! period, or from before) and the days it was beaten or abandoned; how
//! many games were played on each day; and per game its time, sessions,
//! days to beat and pace. A week is too short for journeys, so a week's tab
//! shows its month. Pure functions over the history and user data.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use chrono::{Datelike as _, NaiveDate};
use irori_core::GameId;
use irori_services::{PlayStatus, UserData};

use super::model::{History, rfc3339_day};
use super::period::{Period, Span};

/// One game's journey through the period.
#[derive(Debug, Clone, PartialEq)]
pub struct Lane {
    pub id: GameId,
    /// Each day it was played in the period, with the seconds played.
    pub days: Vec<(NaiveDate, u64)>,
    pub seconds: u64,
    pub sessions: usize,
    /// Its first play ever falls in the period; otherwise it came from
    /// before.
    pub started_here: bool,
    /// The last day in the period it was beaten or abandoned.
    pub beaten: Option<NaiveDate>,
    pub abandoned: Option<NaiveDate>,
    /// Days from its first play ever to the day it was beaten.
    pub days_to_beat: Option<u32>,
    /// Hours a week while it was played: its time over the weeks between
    /// its first and last day in the period (one week at least).
    pub hours_per_week: f32,
    /// It is neither beaten nor abandoned now.
    pub ongoing: bool,
}

impl Lane {
    pub fn first(&self) -> Option<NaiveDate> {
        self.days.first().map(|(d, _)| *d)
    }

    pub fn last(&self) -> Option<NaiveDate> {
        self.days.last().map(|(d, _)| *d)
    }
}

/// Which lanes show.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LaneFilter {
    #[default]
    All,
    Beaten,
    Ongoing,
    Abandoned,
    StartedHere,
}

impl LaneFilter {
    pub const ALL: [LaneFilter; 5] = [
        LaneFilter::All,
        LaneFilter::Beaten,
        LaneFilter::Ongoing,
        LaneFilter::Abandoned,
        LaneFilter::StartedHere,
    ];

    fn keeps(self, lane: &Lane) -> bool {
        match self {
            LaneFilter::All => true,
            LaneFilter::Beaten => lane.beaten.is_some(),
            LaneFilter::Ongoing => lane.ongoing,
            LaneFilter::Abandoned => lane.abandoned.is_some(),
            LaneFilter::StartedHere => lane.started_here,
        }
    }
}

/// The order of the lanes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LaneSort {
    /// By the first day played in the period.
    #[default]
    Started,
    /// Most played first.
    Time,
    /// Played most recently first.
    Recent,
}

impl LaneSort {
    pub const ALL: [LaneSort; 3] = [LaneSort::Started, LaneSort::Time, LaneSort::Recent];
}

#[derive(Debug, Clone, PartialEq)]
pub struct Journeys {
    /// The period shown (a week's month).
    pub period: Period,
    pub lanes: Vec<Lane>,
    /// Games played on each day of the period that had play.
    pub at_once: Vec<(NaiveDate, usize)>,
    /// The most games played on one day.
    pub most_at_once: usize,
}

impl Journeys {
    /// The lanes `filter` keeps, in `sort` order, as indices into `lanes`.
    pub fn shown(&self, filter: LaneFilter, sort: LaneSort) -> Vec<usize> {
        let mut list: Vec<usize> = (0..self.lanes.len())
            .filter(|ix| filter.keeps(&self.lanes[*ix]))
            .collect();
        let key = |ix: &usize| self.lanes[*ix].id.key();
        match sort {
            LaneSort::Started => list.sort_by_key(|ix| (self.lanes[*ix].first(), key(ix))),
            LaneSort::Time => {
                list.sort_by_key(|ix| (std::cmp::Reverse(self.lanes[*ix].seconds), key(ix)))
            }
            LaneSort::Recent => {
                list.sort_by_key(|ix| (std::cmp::Reverse(self.lanes[*ix].last()), key(ix)))
            }
        }
        list
    }
}

/// The period Journeys can show for `period`: a week becomes its month.
pub fn shown_period(period: Period) -> Period {
    match period.span {
        Span::Week => Period::containing(Span::Month, period.start, None),
        _ => period,
    }
}

/// The journeys of `period` for games still in the library.
pub fn compute(
    history: &History,
    period: Period,
    user: &UserData,
    is_game: impl Fn(&GameId) -> bool,
    today: NaiveDate,
) -> Journeys {
    let period = shown_period(period);
    let end = period.end(today);
    let mut by_game: HashMap<GameId, (BTreeMap<NaiveDate, u64>, usize)> = HashMap::new();
    let mut games_by_day: BTreeMap<NaiveDate, BTreeSet<&GameId>> = BTreeMap::new();
    let sessions = history.between(period.start, end);
    for p in sessions {
        if !is_game(&p.game) {
            continue;
        }
        let (days, count) = by_game.entry(p.game.clone()).or_default();
        *days.entry(p.day).or_default() += p.seconds;
        *count += 1;
        games_by_day.entry(p.day).or_default().insert(&p.game);
    }
    let in_period = |time: &str| rfc3339_day(time).filter(|d| period.contains(*d, today));
    let mut lanes: Vec<Lane> = by_game
        .into_iter()
        .map(|(id, (days, sessions))| {
            let key = id.key();
            let changes = user.status_history.get(&key).map_or(&[][..], Vec::as_slice);
            let current = user.play_status.get(&key);
            let last_set = |status| {
                irori_services::play_status::times_set(changes, current, status)
                    .into_iter()
                    .filter_map(in_period)
                    .max()
            };
            let beaten = last_set(PlayStatus::Beaten);
            let abandoned = last_set(PlayStatus::Abandoned);
            let first_ever = history.first_played(&id, &user.playtime);
            let first = days.keys().next().copied().unwrap_or(period.start);
            let last = days.keys().next_back().copied().unwrap_or(first);
            let seconds: u64 = days.values().sum();
            let weeks = (((last - first).num_days() + 1) as f32 / 7.).max(1.);
            Lane {
                started_here: first_ever.is_none_or(|d| d >= period.start),
                days_to_beat: beaten
                    .zip(first_ever)
                    .map(|(b, f)| (b - f).num_days().max(0) as u32),
                hours_per_week: seconds as f32 / 3600. / weeks,
                ongoing: !matches!(
                    current.map(|r| r.status),
                    Some(PlayStatus::Beaten | PlayStatus::Abandoned)
                ),
                beaten,
                abandoned,
                seconds,
                sessions,
                days: days.into_iter().collect(),
                id,
            }
        })
        .collect();
    lanes.sort_by_key(|l| (l.first(), l.id.key()));
    let at_once: Vec<(NaiveDate, usize)> = games_by_day
        .into_iter()
        .map(|(d, games)| (d, games.len()))
        .collect();
    Journeys {
        period,
        most_at_once: at_once.iter().map(|(_, n)| *n).max().unwrap_or(0),
        at_once,
        lanes,
    }
}

/// Where `day` falls between the period's start (0) and end (1).
pub fn position(period: &Period, day: NaiveDate, today: NaiveDate) -> f32 {
    let end = period.end(today);
    let length = (end - period.start).num_days().max(1) as f32;
    ((day - period.start).num_days() as f32 / length).clamp(0., 1.)
}

/// The first day of each month (or year, for long spans) the axis marks.
pub fn ticks(period: &Period, today: NaiveDate) -> Vec<NaiveDate> {
    let end = period.end(today);
    let years = end.year() - period.start.year();
    let mut out = Vec::new();
    let mut at = period.start.with_day(1).unwrap_or(period.start);
    while at < end {
        if at >= period.start && (years <= 2 || at.month() == 1) {
            out.push(at);
        }
        at = at + chrono::Months::new(1);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stats::model::tests::{day, history, session};
    use irori_core::Runner;
    use irori_services::{PlayStatusRecord, StatusChange};

    fn id(app: &str) -> GameId {
        GameId::new(Runner::Gog, app)
    }

    #[test]
    fn lanes_mark_starts_beats_and_games_at_once() {
        let h = history(vec![
            session("gog:old", "2026-09-20", 60),
            session("gog:old", "2026-10-02", 60),
            session("gog:old", "2026-10-03", 30),
            session("gog:new", "2026-10-03", 120),
            session("gog:new", "2026-10-03", 60),
            session("gog:new", "2026-10-20", 60),
            session("gog:gone", "2026-10-03", 60),
        ]);
        let mut user = UserData::default();
        user.status_history.insert(
            "gog:new".into(),
            vec![StatusChange {
                at: "2026-10-21T12:00:00Z".into(),
                status: Some(PlayStatus::Beaten),
            }],
        );
        user.play_status.insert(
            "gog:new".into(),
            PlayStatusRecord {
                status: PlayStatus::Beaten,
                since: "2026-10-21T12:00:00Z".into(),
            },
        );
        // A week shows its month.
        let week = Period::containing(Span::Week, day("2026-10-21"), None);
        let j = compute(
            &h,
            week,
            &user,
            |g| g.app_name.as_ref() != "gone",
            day("2026-10-25"),
        );
        assert_eq!(j.period.span, Span::Month);
        assert_eq!(j.lanes.len(), 2);
        let old = &j.lanes[0];
        assert_eq!(old.id, id("old"));
        assert!(!old.started_here && old.ongoing);
        assert_eq!(
            old.days,
            [(day("2026-10-02"), 3600), (day("2026-10-03"), 1800)]
        );
        let new = &j.lanes[1];
        assert!(new.started_here && !new.ongoing);
        assert_eq!(new.sessions, 3);
        assert_eq!(new.beaten, Some(day("2026-10-21")));
        assert_eq!(new.days_to_beat, Some(18));
        // 4 hours over 18 days: 2.57 weeks.
        assert!((new.hours_per_week - 4. / (18. / 7.)).abs() < 0.01);
        assert_eq!(j.most_at_once, 2);
        assert_eq!(j.at_once[1], (day("2026-10-03"), 2));

        assert_eq!(j.shown(LaneFilter::Beaten, LaneSort::Started), [1]);
        assert_eq!(j.shown(LaneFilter::StartedHere, LaneSort::Started), [1]);
        assert_eq!(j.shown(LaneFilter::Ongoing, LaneSort::Started), [0]);
        assert_eq!(j.shown(LaneFilter::All, LaneSort::Time), [1, 0]);
        assert_eq!(j.shown(LaneFilter::All, LaneSort::Recent), [1, 0]);
        assert!(j.shown(LaneFilter::Abandoned, LaneSort::Time).is_empty());
    }

    #[test]
    fn positions_and_ticks_fit_the_period() {
        let today = day("2026-10-25");
        let year = Period::containing(Span::Year, today, None);
        assert_eq!(position(&year, day("2026-01-01"), today), 0.);
        assert!((position(&year, day("2026-07-02"), today) - 0.5).abs() < 0.01);
        assert_eq!(ticks(&year, today).len(), 12);
        let all = Period::containing(Span::All, today, Some(day("2020-03-04")));
        let marks = ticks(&all, today);
        assert!(marks.iter().all(|d| d.month() == 1));
        assert_eq!(marks.first(), Some(&day("2021-01-01")));
    }
}
