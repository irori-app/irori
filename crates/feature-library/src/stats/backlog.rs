//! What the backlog tab shows for one year: the games beaten and abandoned
//! in it (on the day their status was set), the yearly goal and the pace
//! towards it, and the whole library grouped by play status.

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
    let marks = |wanted: PlayStatus| -> Vec<Mark> {
        let mut list: Vec<Mark> = user
            .play_status
            .iter()
            .filter(|(key, r)| r.status == wanted && in_library.contains(key.as_str()))
            .filter_map(|(key, r)| {
                let day = rfc3339_day(&r.since)?;
                (day.year() == year).then_some(Mark {
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
}
