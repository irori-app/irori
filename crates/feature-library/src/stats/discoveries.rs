//! What the Discoveries tab says about one period: findings, each scored by
//! how unusual it is (the highest leads); the milestones reached, with their
//! days, and the next ones with when they would be reached at the pace of
//! the last 90 days; and what was played (and written) a year ago. Pure
//! functions over the history, the overview and the user data, so they can
//! be unit-tested.

use std::collections::{BTreeMap, BTreeSet};

use chrono::{Datelike as _, Days, Months, NaiveDate, Weekday};
use irori_core::GameId;
use irori_services::{PlayStatus, UserData};

use super::model::{History, longest_streak, rfc3339_day};
use super::overview::Overview;

/// Days counted for the pace towards the next milestones.
const PACE_DAYS: u64 = 90;
/// A game counts as a comeback after this many days without it.
const COMEBACK_DAYS: u32 = 60;
/// Days around today's date a year ago the memories cover, on each side.
const MEMORY_DAYS: u64 = 3;

/// One thing worth saying about the period.
#[derive(Debug, Clone, PartialEq)]
pub enum Finding {
    /// A game played again after a long break.
    Comeback {
        game: GameId,
        days_away: u32,
        day: NaiveDate,
    },
    /// The longest session.
    Marathon {
        game: GameId,
        seconds: u64,
        day: NaiveDate,
    },
    /// The longest run of days played.
    Streak { days: u32 },
    /// The most played game first played in the period.
    NewFavourite { game: GameId, seconds: u64 },
    /// The day with the most play.
    BusiestDay { day: NaiveDate, seconds: u64 },
    /// Share of play started late at night (22:00–04:59).
    NightOwl { share: f32 },
    /// Share of play started early (05:00–08:59).
    EarlyBird { share: f32 },
    /// Share of play on weekends.
    Weekends { share: f32 },
    /// One game took most of the time.
    Devotion { game: GameId, share: f32 },
    /// Many different games.
    Variety { games: usize },
    /// Games beaten in the period.
    Beaten { count: usize },
}

impl Finding {
    /// The id its kind's pin or hide is kept under.
    pub fn kind(&self) -> &'static str {
        match self {
            Finding::Comeback { .. } => "comeback",
            Finding::Marathon { .. } => "marathon",
            Finding::Streak { .. } => "streak",
            Finding::NewFavourite { .. } => "new_favourite",
            Finding::BusiestDay { .. } => "busiest_day",
            Finding::NightOwl { .. } => "night_owl",
            Finding::EarlyBird { .. } => "early_bird",
            Finding::Weekends { .. } => "weekends",
            Finding::Devotion { .. } => "devotion",
            Finding::Variety { .. } => "variety",
            Finding::Beaten { .. } => "beaten",
        }
    }

    /// The game it is about, if one.
    pub fn game(&self) -> Option<&GameId> {
        match self {
            Finding::Comeback { game, .. }
            | Finding::Marathon { game, .. }
            | Finding::NewFavourite { game, .. }
            | Finding::Devotion { game, .. } => Some(game),
            _ => None,
        }
    }
}

/// A finding with how unusual it is: 1 is worth saying, more is rarer.
#[derive(Debug, Clone, PartialEq)]
pub struct Scored {
    pub finding: Finding,
    pub score: f32,
}

/// What can be measured towards a milestone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Metric {
    Hours,
    Games,
    Beaten,
    Days,
}

impl Metric {
    pub const ALL: [Metric; 4] = [Metric::Hours, Metric::Games, Metric::Beaten, Metric::Days];

    fn steps(self) -> &'static [u64] {
        match self {
            Metric::Hours => &[10, 25, 50, 100, 250, 500, 1000, 2500, 5000, 10000],
            Metric::Games => &[5, 10, 25, 50, 100, 250, 500, 1000],
            Metric::Beaten => &[1, 5, 10, 25, 50, 100, 250],
            Metric::Days => &[7, 30, 100, 365, 500, 1000, 2000],
        }
    }
}

/// A milestone: `value` of `metric`, reached on `day` (`None` when it was
/// reached before the history begins).
#[derive(Debug, Clone, PartialEq)]
pub struct Milestone {
    pub metric: Metric,
    pub value: u64,
    pub day: Option<NaiveDate>,
}

/// The next milestone of a metric: how far there is to go and when it
/// would be reached at the pace of the last 90 days.
#[derive(Debug, Clone, PartialEq)]
pub struct NextMilestone {
    pub metric: Metric,
    pub value: u64,
    pub now: u64,
    pub estimate: Option<NaiveDate>,
}

/// What was played and written around today's date a year ago.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Memories {
    pub from: NaiveDate,
    pub to: NaiveDate,
    /// Most played first.
    pub games: Vec<(GameId, u64)>,
    /// Notes written then: game, day and the note's first words.
    pub notes: Vec<(GameId, NaiveDate, String)>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Discoveries {
    /// Highest score first.
    pub findings: Vec<Scored>,
    /// Newest first.
    pub reached: Vec<Milestone>,
    pub next: Vec<NextMilestone>,
    pub memories: Option<Memories>,
}

/// The discoveries of the overview's period.
pub fn compute(
    history: &History,
    overview: &Overview,
    user: &UserData,
    is_game: impl Fn(&GameId) -> bool,
    today: NaiveDate,
) -> Discoveries {
    let mut findings = findings(history, overview, user, today);
    findings.sort_by(|a, b| b.score.total_cmp(&a.score));
    let (reached, next) = milestones(history, user, &is_game, today);
    Discoveries {
        findings,
        reached,
        next,
        memories: memories(history, user, &is_game, today),
    }
}

fn findings(history: &History, o: &Overview, user: &UserData, today: NaiveDate) -> Vec<Scored> {
    let period = o.period;
    let end = period.end(today);
    let sessions = history.between(period.start, end);
    let mut out = Vec::new();
    let mut push = |finding, score: f32| {
        if score >= 1. {
            out.push(Scored { finding, score });
        }
    };
    if let Some(c) = &o.comeback
        && c.after_days >= COMEBACK_DAYS
    {
        push(
            Finding::Comeback {
                game: c.id.clone(),
                days_away: c.after_days,
                day: c.day,
            },
            1. + c.after_days as f32 / 120.,
        );
    }
    // The longest session, against the usual one of the whole history.
    if let Some(longest) = sessions.iter().max_by_key(|p| p.seconds) {
        let mut all: Vec<u64> = history.sessions.iter().map(|p| p.seconds).collect();
        all.sort_unstable();
        let median = all.get(all.len() / 2).copied().unwrap_or(0).max(600);
        let ratio = longest.seconds as f32 / median as f32;
        if longest.seconds >= 2 * 3600 {
            push(
                Finding::Marathon {
                    game: longest.game.clone(),
                    seconds: longest.seconds,
                    day: longest.day,
                },
                ratio / 2.,
            );
        }
    }
    let streak = longest_streak(&history.played_days, period.start, end);
    if streak >= 3 {
        push(Finding::Streak { days: streak }, streak as f32 / 4.);
    }
    if let Some(g) = o.games.iter().find(|g| g.new) {
        push(
            Finding::NewFavourite {
                game: g.id.clone(),
                seconds: g.seconds,
            },
            1. + g.seconds as f32 / o.seconds.max(1) as f32,
        );
    }
    let mut by_day: BTreeMap<NaiveDate, u64> = BTreeMap::new();
    for p in sessions {
        *by_day.entry(p.day).or_default() += p.seconds;
    }
    if by_day.len() >= 3
        && let Some((day, seconds)) = by_day
            .iter()
            .max_by_key(|(d, s)| (**s, std::cmp::Reverse(**d)))
    {
        let average = o.seconds as f32 / by_day.len() as f32;
        push(
            Finding::BusiestDay {
                day: *day,
                seconds: *seconds,
            },
            *seconds as f32 / average.max(1.) / 1.5,
        );
    }
    if o.seconds >= 3 * 3600 {
        let share = |keep: &dyn Fn(&super::model::Played) -> bool| {
            sessions
                .iter()
                .filter(|p| keep(p))
                .map(|p| p.seconds)
                .sum::<u64>() as f32
                / o.seconds as f32
        };
        let night = share(&|p| p.hour >= 22 || p.hour < 5);
        push(Finding::NightOwl { share: night }, night * 3.);
        let early = share(&|p| (5..9).contains(&p.hour));
        push(Finding::EarlyBird { share: early }, early * 4.);
        let weekend = share(&|p| matches!(p.day.weekday(), Weekday::Sat | Weekday::Sun));
        // Weekends hold 2/7 of the days; far from that is worth a word.
        push(
            Finding::Weekends { share: weekend },
            (weekend - 2. / 7.).abs() * 3.5,
        );
        if let Some(top) = o.games.first() {
            let share = top.seconds as f32 / o.seconds as f32;
            if o.games.len() >= 2 {
                push(
                    Finding::Devotion {
                        game: top.id.clone(),
                        share,
                    },
                    share * 1.6,
                );
            }
        }
    }
    push(
        Finding::Variety {
            games: o.games.len(),
        },
        o.games.len() as f32 / 6.,
    );
    let beaten = user
        .play_status
        .keys()
        .chain(user.status_history.keys())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .filter(|key| {
            let changes = user.status_history.get(*key).map_or(&[][..], Vec::as_slice);
            irori_services::play_status::times_set(
                changes,
                user.play_status.get(*key),
                PlayStatus::Beaten,
            )
            .into_iter()
            .filter_map(rfc3339_day)
            .any(|d| period.contains(d, today))
        })
        .count();
    if beaten > 0 {
        push(Finding::Beaten { count: beaten }, 1. + beaten as f32 / 2.);
    }
    out
}

/// The days each metric grew, one entry per unit, oldest first; and the
/// count reached before the history began.
fn growth(
    history: &History,
    user: &UserData,
    is_game: &impl Fn(&GameId) -> bool,
    metric: Metric,
) -> (u64, Vec<(NaiveDate, u64)>) {
    match metric {
        Metric::Hours => {
            // Time beyond the sessions (imports, store totals) counts as
            // played before the history.
            let mut by_game: BTreeMap<String, u64> = BTreeMap::new();
            let mut by_day: BTreeMap<NaiveDate, u64> = BTreeMap::new();
            for p in &history.sessions {
                *by_game.entry(p.game.key()).or_default() += p.seconds;
                *by_day.entry(p.day).or_default() += p.seconds;
            }
            let before: u64 = user
                .playtime
                .iter()
                .filter(|(key, _)| super::model::game_id(key).is_some_and(|id| is_game(&id)))
                .map(|(key, r)| {
                    (r.total_minutes * 60 + r.extra_seconds)
                        .saturating_sub(by_game.get(key).copied().unwrap_or(0))
                })
                .sum();
            (before, by_day.into_iter().collect())
        }
        Metric::Games => {
            let mut keys: BTreeSet<String> = user.playtime.keys().cloned().collect();
            keys.extend(history.first_session.keys().map(GameId::key));
            let mut before = 0;
            let mut by_day: BTreeMap<NaiveDate, u64> = BTreeMap::new();
            for id in keys.iter().filter_map(|k| super::model::game_id(k)) {
                if !is_game(&id) {
                    continue;
                }
                let played = user
                    .playtime
                    .get(&id.key())
                    .is_some_and(|r| r.total_minutes > 0 || r.last_played.is_some())
                    || history.first_session.contains_key(&id);
                if !played {
                    continue;
                }
                match history.first_played(&id, &user.playtime) {
                    Some(day) => *by_day.entry(day).or_default() += 1,
                    None => before += 1,
                }
            }
            (before, by_day.into_iter().collect())
        }
        Metric::Beaten => {
            let mut by_day: BTreeMap<NaiveDate, u64> = BTreeMap::new();
            let keys: BTreeSet<&String> = user
                .play_status
                .keys()
                .chain(user.status_history.keys())
                .collect();
            for key in keys {
                let changes = user.status_history.get(key).map_or(&[][..], Vec::as_slice);
                // A game counts once, on the first day it was beaten.
                let first = irori_services::play_status::times_set(
                    changes,
                    user.play_status.get(key),
                    PlayStatus::Beaten,
                )
                .into_iter()
                .filter_map(rfc3339_day)
                .min();
                if let Some(day) = first {
                    *by_day.entry(day).or_default() += 1;
                }
            }
            (0, by_day.into_iter().collect())
        }
        Metric::Days => (0, history.played_days.iter().map(|d| (*d, 1)).collect()),
    }
}

/// Milestones reached (newest first) and the next one of each metric.
pub fn milestones(
    history: &History,
    user: &UserData,
    is_game: &impl Fn(&GameId) -> bool,
    today: NaiveDate,
) -> (Vec<Milestone>, Vec<NextMilestone>) {
    let mut reached = Vec::new();
    let mut next = Vec::new();
    let since = today - Days::new(PACE_DAYS);
    for metric in Metric::ALL {
        let (before, days) = growth(history, user, is_game, metric);
        let scale = if metric == Metric::Hours { 3600 } else { 1 };
        let mut total = before;
        let mut recent = 0;
        let mut steps = metric.steps().iter().peekable();
        // Steps already passed before the history.
        while let Some(step) = steps.next_if(|s| total >= **s * scale) {
            reached.push(Milestone {
                metric,
                value: *step,
                day: None,
            });
        }
        for (day, amount) in &days {
            total += amount;
            if *day > since && *day <= today {
                recent += amount;
            }
            while let Some(step) = steps.next_if(|s| total >= **s * scale) {
                reached.push(Milestone {
                    metric,
                    value: *step,
                    day: Some(*day),
                });
            }
        }
        if let Some(step) = steps.next() {
            let left = (step * scale).saturating_sub(total);
            let estimate = (recent > 0).then(|| {
                let days = (left as f64 * PACE_DAYS as f64 / recent as f64).ceil() as u64;
                today + Days::new(days.max(1))
            });
            next.push(NextMilestone {
                metric,
                value: *step,
                now: total / scale,
                estimate,
            });
        }
    }
    reached.sort_by_key(|m| std::cmp::Reverse(m.day));
    (reached, next)
}

/// What was played and written in the week around today's date a year ago;
/// `None` when nothing was.
pub fn memories(
    history: &History,
    user: &UserData,
    is_game: &impl Fn(&GameId) -> bool,
    today: NaiveDate,
) -> Option<Memories> {
    let then = today.checked_sub_months(Months::new(12))?;
    let from = then - Days::new(MEMORY_DAYS);
    let to = then + Days::new(MEMORY_DAYS);
    let mut games: BTreeMap<GameId, u64> = BTreeMap::new();
    for p in history.between(from, to + Days::new(1)) {
        if is_game(&p.game) {
            *games.entry(p.game.clone()).or_default() += p.seconds;
        }
    }
    let mut games: Vec<(GameId, u64)> = games.into_iter().collect();
    games.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.key().cmp(&b.0.key())));
    let notes: Vec<(GameId, NaiveDate, String)> = irori_services::journal::all(&user.journal)
        .into_iter()
        .filter(|(_, day, _)| (from..=to).contains(day))
        .filter_map(|(key, day, note)| {
            let id = super::model::game_id(key).filter(|id| is_game(id))?;
            let words = if note.title.is_empty() {
                note.text.lines().next().unwrap_or_default()
            } else {
                note.title.as_str()
            };
            (!words.trim().is_empty()).then(|| (id, day, words.trim().to_string()))
        })
        .collect();
    (!games.is_empty() || !notes.is_empty()).then_some(Memories {
        from,
        to,
        games,
        notes,
    })
}

/// The findings a user keeps: pinned kinds first (in `pinned` order), the
/// others by score, hidden kinds left out.
pub fn arranged<'a>(
    findings: &'a [Scored],
    pinned: &[String],
    hidden: &[String],
) -> Vec<&'a Scored> {
    let rank = |f: &Scored| pinned.iter().position(|p| p == f.finding.kind());
    let mut list: Vec<&Scored> = findings
        .iter()
        .filter(|f| !hidden.iter().any(|h| h == f.finding.kind()))
        .collect();
    list.sort_by(|a, b| match (rank(a), rank(b)) {
        (Some(x), Some(y)) => x.cmp(&y),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => b.score.total_cmp(&a.score),
    });
    list
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stats::model::tests::{day, history, session, session_at};
    use crate::stats::overview;
    use crate::stats::period::{Period, Span};
    use irori_core::Runner;
    use irori_core::game::PlaytimeRecord;
    use irori_services::{PlayStatusRecord, StatusChange};

    fn id(app: &str) -> GameId {
        GameId::new(Runner::Gog, app)
    }

    fn kinds(d: &Discoveries) -> Vec<&'static str> {
        d.findings.iter().map(|f| f.finding.kind()).collect()
    }

    #[test]
    fn a_long_break_ended_leads_the_findings() {
        let today = day("2026-10-25");
        let h = history(vec![
            session_at("gog:1", "2025-06-01", 20, 60),
            session_at("gog:1", "2026-10-10", 23, 300),
            session_at("gog:2", "2026-10-11", 23, 60),
            session_at("gog:2", "2026-10-12", 23, 60),
            session_at("gog:2", "2026-10-13", 23, 60),
        ]);
        let mut user = UserData::default();
        user.status_history.insert(
            "gog:2".into(),
            vec![StatusChange {
                at: "2026-10-13T20:00:00Z".into(),
                status: Some(PlayStatus::Beaten),
            }],
        );
        let period = Period::containing(Span::Month, today, None);
        let o = overview::compute(&h, period, &user.playtime, |_| true, today);
        let d = compute(&h, &o, &user, |_| true, today);
        assert_eq!(kinds(&d)[0], "comeback");
        assert!(matches!(
            d.findings[0].finding,
            Finding::Comeback { days_away, .. } if days_away > 400
        ));
        assert!(kinds(&d).contains(&"streak"));
        assert!(kinds(&d).contains(&"marathon"));
        assert!(kinds(&d).contains(&"night_owl"));
        assert!(kinds(&d).contains(&"beaten"));
        // Pinned kinds lead, hidden ones go.
        let arranged = arranged(&d.findings, &["streak".into()], &["comeback".into()]);
        assert_eq!(arranged[0].finding.kind(), "streak");
        assert!(arranged.iter().all(|f| f.finding.kind() != "comeback"));
    }

    #[test]
    fn milestones_have_days_and_the_next_ones_an_estimate() {
        let today = day("2026-10-25");
        let mut sessions = Vec::new();
        for d in 1..=8 {
            sessions.push(session(
                &format!("gog:{d}"),
                &format!("2026-10-{d:02}"),
                120,
            ));
        }
        let h = history(sessions);
        let mut user = UserData::default();
        // Five hours from before the history.
        user.playtime.insert(
            "gog:1".into(),
            PlaytimeRecord {
                total_minutes: 120 + 300,
                ..Default::default()
            },
        );
        user.play_status.insert(
            "gog:1".into(),
            PlayStatusRecord {
                status: PlayStatus::Beaten,
                since: "2026-10-02T12:00:00Z".into(),
            },
        );
        let (reached, next) = milestones(&h, &user, &|_| true, today);
        let find = |m: Metric, v: u64| reached.iter().find(|r| r.metric == m && r.value == v);
        // 5 h before, then 2 h a day: 10 h on the 3rd.
        assert_eq!(
            find(Metric::Hours, 10).unwrap().day,
            Some(day("2026-10-03"))
        );
        assert_eq!(find(Metric::Games, 5).unwrap().day, Some(day("2026-10-05")));
        assert_eq!(
            find(Metric::Beaten, 1).unwrap().day,
            Some(day("2026-10-02"))
        );
        assert_eq!(find(Metric::Days, 7).unwrap().day, Some(day("2026-10-07")));
        // Newest first.
        assert_eq!(reached[0].day, Some(day("2026-10-07")));
        let hours = next.iter().find(|n| n.metric == Metric::Hours).unwrap();
        assert_eq!((hours.value, hours.now), (25, 21));
        // 16 h in 90 days: 4 h more take 23 days.
        assert_eq!(hours.estimate, Some(day("2026-11-17")));
    }

    #[test]
    fn memories_are_the_week_around_a_year_ago() {
        let today = day("2026-10-25");
        let h = history(vec![
            session("gog:1", "2025-10-23", 60),
            session("gog:2", "2025-10-27", 90),
            session("gog:3", "2025-10-30", 60),
        ]);
        let mut user = UserData::default();
        user.journal.entry("gog:1".into()).or_default().insert(
            day("2025-10-24"),
            vec![irori_services::JournalEntry {
                text: "First boss\nand more".into(),
                ..Default::default()
            }],
        );
        let m = memories(&h, &user, &|_| true, today).unwrap();
        assert_eq!(m.games, [(id("2"), 5400), (id("1"), 3600)]);
        assert_eq!(
            m.notes,
            [(id("1"), day("2025-10-24"), "First boss".to_string())]
        );
        assert!(memories(&h, &user, &|_| true, day("2026-06-01")).is_none());
    }
}
