//! What the backlog tab shows for one year: the games beaten and abandoned
//! in it (on the day their status was set, by its history of changes, so a
//! game beaten in March still counts after it is played again), the yearly
//! goal and the pace towards it, and the whole library grouped by play
//! status. Beside them ([`extras`]): the queue of Plan to play games with
//! when each would be done at the user's pace, the shelf of games in play or
//! on hold without a session for 30 days, the pile (added, started, beaten
//! each month) and the year's beaten games against How Long To Beat.

use std::collections::BTreeMap;

use chrono::{Datelike as _, NaiveDate};
use irori_core::GameId;
use irori_services::{EffectiveStatus, PlayStatus, UserData};

use super::model::{History, game_id, rfc3339_day};

/// A game whose status was set on `day`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mark {
    pub id: GameId,
    pub day: NaiveDate,
}

/// How the year's beaten games compare with its goal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Ahead,
    OnPace,
    SlightlyBehind,
    Behind,
    /// The goal is met (any year).
    Reached,
    /// A past year that ended short of its goal.
    Missed,
}

/// Progress towards the goal of the current year.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pace {
    /// Games that would be beaten by today at an even pace.
    pub expected: f32,
    /// Games beaten by the year's end at the pace so far.
    pub projected: u32,
    /// Days per game left to make the goal, when it is not met yet.
    pub every_days: Option<u32>,
    pub remaining: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Backlog {
    pub year: i32,
    /// Oldest first.
    pub beaten: Vec<Mark>,
    pub abandoned: Vec<Mark>,
    pub goal: Option<u32>,
    pub verdict: Option<Verdict>,
    /// Only for the current year with a goal.
    pub pace: Option<Pace>,
    /// Library games per status, in [`EffectiveStatus::ALL`] order.
    pub library: Vec<(EffectiveStatus, usize)>,
    /// Play time of the games beaten this year, in minutes.
    pub beaten_minutes: u64,
    pub abandoned_minutes: u64,
    /// Median days from first play to beaten, for this year's games.
    pub median_days_to_beat: Option<u32>,
}

impl Backlog {
    pub fn library_total(&self) -> usize {
        self.library.iter().map(|(_, n)| n).sum()
    }

    pub fn count(&self, status: EffectiveStatus) -> usize {
        self.library
            .iter()
            .find(|(s, _)| *s == status)
            .map_or(0, |(_, n)| *n)
    }
}

/// Compute the backlog of `year`. `library` lists the keys of the games in
/// the library (statuses of removed games are not counted).
pub fn compute<'a>(
    history: &History,
    user: &UserData,
    library: impl IntoIterator<Item = &'a str>,
    year: i32,
    today: NaiveDate,
) -> Backlog {
    let mut by_status: BTreeMap<EffectiveStatus, usize> = BTreeMap::new();
    let mut in_library = std::collections::HashSet::new();
    for key in library {
        *by_status.entry(user.effective_status(key)).or_default() += 1;
        in_library.insert(key.to_string());
    }
    let keys: std::collections::BTreeSet<&String> = user
        .play_status
        .keys()
        .chain(user.status_history.keys())
        .filter(|k| in_library.contains(k.as_str()))
        .collect();
    // A game counts once a year, on the last day of the year it got the
    // status.
    let marks = |wanted: PlayStatus| -> Vec<Mark> {
        let mut list: Vec<Mark> = keys
            .iter()
            .filter_map(|key| {
                let changes = user.status_history.get(*key).map_or(&[][..], Vec::as_slice);
                let day = irori_services::play_status::times_set(
                    changes,
                    user.play_status.get(*key),
                    wanted,
                )
                .into_iter()
                .filter_map(rfc3339_day)
                .filter(|d| d.year() == year)
                .max()?;
                Some(Mark {
                    id: game_id(key)?,
                    day,
                })
            })
            .collect();
        list.sort_by(|a, b| a.day.cmp(&b.day).then_with(|| a.id.key().cmp(&b.id.key())));
        list
    };
    let beaten = marks(PlayStatus::Beaten);
    let abandoned = marks(PlayStatus::Abandoned);
    let minutes = |list: &[Mark]| -> u64 {
        list.iter()
            .filter_map(|m| user.playtime.get(&m.id.key()))
            .map(|r| r.total_minutes)
            .sum()
    };
    let mut to_beat: Vec<u32> = beaten
        .iter()
        .filter_map(|m| {
            let first = history.first_played(&m.id, &user.playtime)?;
            Some((m.day - first).num_days().max(0) as u32)
        })
        .collect();
    to_beat.sort_unstable();
    let median_days_to_beat = match to_beat.len() {
        0 => None,
        n if n % 2 == 1 => Some(to_beat[n / 2]),
        n => Some((to_beat[n / 2 - 1] + to_beat[n / 2]) / 2),
    };

    let goal = user.backlog_goals.get(&year).copied().filter(|g| *g > 0);
    let (verdict, pace) = match goal {
        Some(goal) => judge(beaten.len() as u32, goal, year, today),
        None => (None, None),
    };

    Backlog {
        year,
        beaten_minutes: minutes(&beaten),
        abandoned_minutes: minutes(&abandoned),
        beaten,
        abandoned,
        goal,
        verdict,
        pace,
        library: EffectiveStatus::ALL
            .into_iter()
            .map(|s| (s, by_status.get(&s).copied().unwrap_or(0)))
            .collect(),
        median_days_to_beat,
    }
}

/// Weeks of play the pace of the queue is measured over.
const PACE_WEEKS: u64 = 12;
/// A game in play or on hold without a session for this many days is on
/// the shelf.
pub const SHELF_DAYS: u32 = 30;

/// A game waiting in the queue (Plan to play), with how long it should take
/// and when it would be done at the user's pace, one after another.
#[derive(Debug, Clone, PartialEq)]
pub struct Queued {
    pub id: GameId,
    /// Hours to beat it: How Long To Beat's main story, else the middle of
    /// the user's own beaten games (`guessed`).
    pub hours: Option<f32>,
    pub guessed: bool,
    pub done: Option<NaiveDate>,
}

/// A game in play or on hold that hasn't been played for a while.
#[derive(Debug, Clone, PartialEq)]
pub struct Shelved {
    pub id: GameId,
    pub status: PlayStatus,
    pub last: Option<NaiveDate>,
    /// Days since it was last played; `None` when it never was.
    pub idle_days: Option<u32>,
}

/// The pile in one month: games added, first played and beaten.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PileMonth {
    pub month: NaiveDate,
    pub added: usize,
    pub started: usize,
    pub beaten: usize,
}

/// A game beaten in the year beside How Long To Beat's main story.
#[derive(Debug, Clone, PartialEq)]
pub struct AgainstHltb {
    pub id: GameId,
    pub hours: f32,
    pub hltb: u32,
}

/// The backlog's other cards for one year.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Extras {
    /// Hours a week over the last twelve weeks.
    pub weekly_hours: f32,
    /// Oldest wish first.
    pub queue: Vec<Queued>,
    /// Longest idle first.
    pub shelf: Vec<Shelved>,
    /// Each month of the year up to today.
    pub pile: Vec<PileMonth>,
    pub against_hltb: Vec<AgainstHltb>,
}

/// The queue, shelf, pile and hours against How Long To Beat of `year`.
/// `hltb` holds main-story hours by game key.
pub fn extras<'a>(
    history: &History,
    user: &UserData,
    library: impl IntoIterator<Item = &'a str>,
    hltb: &std::collections::HashMap<String, u32>,
    year: i32,
    today: NaiveDate,
) -> Extras {
    let keys: Vec<&str> = library.into_iter().collect();
    let since = today - chrono::Days::new(PACE_WEEKS * 7);
    let recent: u64 = history
        .between(
            since.succ_opt().unwrap_or(since),
            today.succ_opt().unwrap_or(today),
        )
        .iter()
        .map(|p| p.seconds)
        .sum();
    let weekly_hours = recent as f32 / 3600. / PACE_WEEKS as f32;
    let hours_of = |key: &str| {
        user.playtime.get(key).map_or(0., |r| {
            (r.total_minutes * 60 + r.extra_seconds) as f32 / 3600.
        })
    };
    let beaten_now = |key: &str| user.play_status(key) == Some(PlayStatus::Beaten);
    let mut own: Vec<f32> = keys
        .iter()
        .filter(|k| beaten_now(k))
        .map(|k| hours_of(k))
        .filter(|h| *h > 0.)
        .collect();
    own.sort_by(f32::total_cmp);
    let middle = own.get(own.len() / 2).copied();

    // The queue, oldest wish first, done one after another.
    let mut wishes: Vec<(&str, &str)> = keys
        .iter()
        .filter_map(|k| {
            let r = user.play_status.get(*k)?;
            (r.status == PlayStatus::PlanToPlay).then_some((*k, r.since.as_str()))
        })
        .collect();
    wishes.sort_by(|a, b| a.1.cmp(b.1).then(a.0.cmp(b.0)));
    let mut ahead = 0.;
    let queue = wishes
        .into_iter()
        .filter_map(|(key, _)| {
            let (hours, guessed) = match hltb.get(key) {
                Some(h) if *h > 0 => (Some(*h as f32), false),
                _ => (middle, true),
            };
            let done = hours.filter(|_| weekly_hours > 0.).map(|h| {
                ahead += h;
                today + chrono::Days::new((ahead / weekly_hours * 7.).ceil() as u64)
            });
            Some(Queued {
                id: game_id(key)?,
                hours,
                guessed: guessed && hours.is_some(),
                done,
            })
        })
        .collect();

    let mut shelf: Vec<Shelved> = keys
        .iter()
        .filter_map(|key| {
            let status = user.play_status(key)?;
            if !matches!(status, PlayStatus::Playing | PlayStatus::OnHold) {
                return None;
            }
            let id = game_id(key)?;
            let session = history
                .sessions
                .iter()
                .rev()
                .find(|p| p.game == id)
                .map(|p| p.day);
            let recorded = user
                .playtime
                .get(*key)
                .and_then(|r| r.last_played.as_deref())
                .and_then(rfc3339_day);
            let last = session.max(recorded);
            let idle_days = last.map(|d| (today - d).num_days().max(0) as u32);
            idle_days
                .is_none_or(|d| d >= SHELF_DAYS)
                .then_some(Shelved {
                    id,
                    status,
                    last,
                    idle_days,
                })
        })
        .collect();
    shelf.sort_by(|a, b| {
        a.last
            .cmp(&b.last)
            .then_with(|| a.id.key().cmp(&b.id.key()))
    });

    let months = if year < today.year() {
        12
    } else if year == today.year() {
        today.month()
    } else {
        0
    };
    let mut pile: Vec<PileMonth> = (1..=months)
        .filter_map(|m| {
            Some(PileMonth {
                month: NaiveDate::from_ymd_opt(year, m, 1)?,
                added: 0,
                started: 0,
                beaten: 0,
            })
        })
        .collect();
    let slot = |pile: &[PileMonth], day: NaiveDate| -> Option<usize> {
        (day.year() == year)
            .then(|| day.month() as usize - 1)
            .filter(|ix| *ix < pile.len())
    };
    for key in &keys {
        if let Some(day) = user.first_seen.get(*key).and_then(|t| rfc3339_day(t))
            && let Some(ix) = slot(&pile, day)
        {
            pile[ix].added += 1;
        }
        if let Some(id) = game_id(key)
            && let Some(day) = history.first_played(&id, &user.playtime)
            && let Some(ix) = slot(&pile, day)
        {
            pile[ix].started += 1;
        }
        let changes = user.status_history.get(*key).map_or(&[][..], Vec::as_slice);
        let months: std::collections::BTreeSet<usize> = irori_services::play_status::times_set(
            changes,
            user.play_status.get(*key),
            PlayStatus::Beaten,
        )
        .into_iter()
        .filter_map(rfc3339_day)
        .filter_map(|d| slot(&pile, d))
        .collect();
        for ix in months {
            pile[ix].beaten += 1;
        }
    }

    let beaten_keys: Vec<String> = compute(history, user, keys.iter().copied(), year, today)
        .beaten
        .iter()
        .map(|m| m.id.key())
        .collect();
    let against_hltb = beaten_keys
        .iter()
        .filter_map(|key| {
            let hltb = *hltb.get(key).filter(|h| **h > 0)?;
            let hours = hours_of(key);
            let id = game_id(key)?;
            (hours > 0.).then_some(AgainstHltb { id, hours, hltb })
        })
        .collect();

    Extras {
        weekly_hours,
        queue,
        shelf,
        pile,
        against_hltb,
    }
}

/// How `beaten` games compare with `goal` in `year`, seen on `today`.
fn judge(beaten: u32, goal: u32, year: i32, today: NaiveDate) -> (Option<Verdict>, Option<Pace>) {
    if beaten >= goal {
        return (Some(Verdict::Reached), None);
    }
    if year < today.year() {
        return (Some(Verdict::Missed), None);
    }
    if year > today.year() {
        return (None, None);
    }
    let days_in_year = if NaiveDate::from_ymd_opt(year, 2, 29).is_some() {
        366
    } else {
        365
    };
    let elapsed = today.ordinal() as f32;
    let expected = goal as f32 * elapsed / days_in_year as f32;
    let projected = (beaten as f32 * days_in_year as f32 / elapsed).floor() as u32;
    let remaining = goal - beaten;
    let days_left = days_in_year - today.ordinal() + 1;
    let every_days = Some((days_left / remaining).max(1));
    let behind = expected - beaten as f32;
    let verdict = if behind <= -0.5 {
        Verdict::Ahead
    } else if behind < 0.5 {
        Verdict::OnPace
    } else if behind < 2.0 {
        Verdict::SlightlyBehind
    } else {
        Verdict::Behind
    };
    (
        Some(verdict),
        Some(Pace {
            expected,
            projected,
            every_days,
            remaining,
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::super::model::tests::{day, history, session};
    use super::*;
    use irori_core::game::PlaytimeRecord;
    use irori_services::PlayStatusRecord;

    fn set(user: &mut UserData, key: &str, status: PlayStatus, since: &str) {
        user.play_status.insert(
            key.into(),
            PlayStatusRecord {
                status,
                // Noon UTC is the same local day nearly everywhere.
                since: format!("{since}T12:00:00Z"),
            },
        );
    }

    #[test]
    fn beaten_and_abandoned_games_of_the_year() {
        let mut user = UserData::default();
        set(&mut user, "gog:1", PlayStatus::Beaten, "2026-03-10");
        set(&mut user, "gog:2", PlayStatus::Beaten, "2026-01-20");
        set(&mut user, "gog:3", PlayStatus::Beaten, "2025-12-31");
        set(&mut user, "gog:4", PlayStatus::Abandoned, "2026-05-01");
        set(&mut user, "gog:5", PlayStatus::PlanToPlay, "2026-05-01");
        // No longer in the library.
        set(&mut user, "gog:gone", PlayStatus::Beaten, "2026-02-01");
        user.playtime.insert(
            "gog:1".into(),
            PlaytimeRecord {
                total_minutes: 600,
                ..Default::default()
            },
        );
        user.playtime.insert(
            "gog:6".into(),
            PlaytimeRecord {
                total_minutes: 5,
                ..Default::default()
            },
        );
        let h = history(vec![
            session("gog:1", "2026-02-28", 60),
            session("gog:2", "2026-01-10", 60),
        ]);
        let keys = [
            "gog:1", "gog:2", "gog:3", "gog:4", "gog:5", "gog:6", "gog:7",
        ];
        let b = compute(&h, &user, keys, 2026, day("2026-11-03"));
        let beaten: Vec<_> = b.beaten.iter().map(|m| m.id.key()).collect();
        assert_eq!(beaten, ["gog:2", "gog:1"]);
        assert_eq!(b.abandoned.len(), 1);
        assert_eq!(b.beaten_minutes, 600);
        // 10 and 10 days from first play.
        assert_eq!(b.median_days_to_beat, Some(10));
        assert_eq!(b.count(EffectiveStatus::Beaten), 3);
        assert_eq!(b.count(EffectiveStatus::Played), 1);
        assert_eq!(b.count(EffectiveStatus::NotPlayed), 1);
        assert_eq!(b.count(EffectiveStatus::PlanToPlay), 1);
        assert_eq!(b.library_total(), 7);
        assert_eq!(b.goal, None);
        assert_eq!(b.verdict, None);
    }

    #[test]
    fn pace_towards_the_goal() {
        // Day 307 of 365.
        let today = day("2026-11-03");
        let (verdict, pace) = judge(9, 12, 2026, today);
        let pace = pace.unwrap();
        assert!((pace.expected - 10.09).abs() < 0.01, "{}", pace.expected);
        assert_eq!(verdict, Some(Verdict::SlightlyBehind));
        assert_eq!(pace.projected, 10);
        assert_eq!(pace.remaining, 3);
        // 59 days left for 3 games.
        assert_eq!(pace.every_days, Some(19));

        assert_eq!(judge(11, 12, 2026, today).0, Some(Verdict::Ahead));
        assert_eq!(judge(10, 12, 2026, today).0, Some(Verdict::OnPace));
        assert_eq!(judge(4, 12, 2026, today).0, Some(Verdict::Behind));
        assert_eq!(judge(12, 12, 2026, today), (Some(Verdict::Reached), None));
        assert_eq!(judge(3, 12, 2025, today), (Some(Verdict::Missed), None));
        assert_eq!(judge(0, 12, 2027, today), (None, None));
    }

    #[test]
    fn the_goal_comes_from_user_data() {
        let mut user = UserData::default();
        user.backlog_goals.insert(2026, 2);
        set(&mut user, "gog:1", PlayStatus::Beaten, "2026-03-10");
        set(&mut user, "gog:2", PlayStatus::Beaten, "2026-04-10");
        let b = compute(
            &History::default(),
            &user,
            ["gog:1", "gog:2"],
            2026,
            day("2026-11-03"),
        );
        assert_eq!(b.goal, Some(2));
        assert_eq!(b.verdict, Some(Verdict::Reached));
        assert_eq!(b.median_days_to_beat, None);
    }

    #[test]
    fn a_game_beaten_then_played_again_still_counts_in_its_year() {
        let mut user = UserData::default();
        set(&mut user, "gog:1", PlayStatus::Playing, "2026-06-01");
        user.status_history.insert(
            "gog:1".into(),
            vec![
                irori_services::StatusChange {
                    at: "2026-03-10T12:00:00Z".into(),
                    status: Some(PlayStatus::Beaten),
                },
                irori_services::StatusChange {
                    at: "2026-06-01T12:00:00Z".into(),
                    status: Some(PlayStatus::Playing),
                },
            ],
        );
        let b = compute(&history(vec![]), &user, ["gog:1"], 2026, day("2026-10-01"));
        assert_eq!(b.beaten.len(), 1);
        assert_eq!(b.beaten[0].day, day("2026-03-10"));
        let before = compute(&history(vec![]), &user, ["gog:1"], 2025, day("2026-10-01"));
        assert!(before.beaten.is_empty());
    }

    #[test]
    fn the_queue_shelf_pile_and_how_long_to_beat() {
        let today = day("2026-10-25");
        let mut user = UserData::default();
        set(&mut user, "gog:q1", PlayStatus::PlanToPlay, "2026-01-01");
        set(&mut user, "gog:q2", PlayStatus::PlanToPlay, "2026-02-01");
        set(&mut user, "gog:idle", PlayStatus::OnHold, "2026-03-01");
        set(&mut user, "gog:busy", PlayStatus::Playing, "2026-03-01");
        set(&mut user, "gog:done", PlayStatus::Beaten, "2026-09-10");
        user.first_seen
            .insert("gog:q1".into(), "2026-01-05T12:00:00Z".into());
        user.playtime.insert(
            "gog:done".into(),
            PlaytimeRecord {
                total_minutes: 30 * 60,
                ..Default::default()
            },
        );
        // Twelve weeks of 2 h a week.
        let mut sessions: Vec<_> = (0..12)
            .map(|w| {
                let d = today - chrono::Days::new(7 * w + 1);
                session("gog:busy", &d.format("%Y-%m-%d").to_string(), 120)
            })
            .collect();
        sessions.push(session("gog:idle", "2026-08-01", 60));
        sessions.push(session("gog:done", "2026-07-20", 60));
        let h = history(sessions);
        let hltb = std::collections::HashMap::from([
            ("gog:q1".to_string(), 10u32),
            ("gog:done".to_string(), 25u32),
        ]);
        let keys = ["gog:q1", "gog:q2", "gog:idle", "gog:busy", "gog:done"];
        let e = extras(&h, &user, keys, &hltb, 2026, today);
        assert!((e.weekly_hours - 2.).abs() < 0.01);
        // 10 h at 2 h a week: 5 weeks; then the median of beaten games (30 h),
        // 20 weeks from today in all.
        assert_eq!(e.queue.len(), 2);
        assert_eq!(e.queue[0].hours, Some(10.));
        assert_eq!(e.queue[0].done, Some(day("2026-11-29")));
        assert!(e.queue[1].guessed);
        assert_eq!(e.queue[1].hours, Some(30.));
        assert_eq!(e.queue[1].done, Some(day("2027-03-14")));
        // The game on hold went quiet; the one played every week didn't.
        assert_eq!(e.shelf.len(), 1);
        assert_eq!(e.shelf[0].id.app_name.as_ref(), "idle");
        assert_eq!(e.shelf[0].idle_days, Some(85));
        assert_eq!(e.pile.len(), 10);
        assert_eq!(e.pile[0].added, 1);
        assert_eq!(e.pile[8].beaten, 1);
        assert!(e.pile.iter().map(|m| m.started).sum::<usize>() >= 3);
        assert_eq!(e.against_hltb.len(), 1);
        assert_eq!((e.against_hltb[0].hours, e.against_hltb[0].hltb), (30., 25));
    }
}
