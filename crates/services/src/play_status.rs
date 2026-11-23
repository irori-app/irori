//! Where the user is with each game: the play status they set (plan to play,
//! playing, beaten…) and the date they set it, kept with the rest of the
//! user's library data, and every real change of it ([`StatusChange`]), so
//! the backlog knows each day a game was beaten even after its status moved
//! on. Nothing sets a status on its own; a game without one counts as played
//! or not played from its playtime, and only in views that group games by
//! status (filters, stats).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// A play status the user picked for a game.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PlayStatus {
    NotPlayed,
    PlanToPlay,
    Playing,
    OnHold,
    Beaten,
    Abandoned,
}

impl PlayStatus {
    /// Every status, in the order menus list them.
    pub const ALL: [PlayStatus; 6] = [
        PlayStatus::NotPlayed,
        PlayStatus::PlanToPlay,
        PlayStatus::Playing,
        PlayStatus::OnHold,
        PlayStatus::Beaten,
        PlayStatus::Abandoned,
    ];
}

/// A game's play status and when the user set it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlayStatusRecord {
    pub status: PlayStatus,
    /// RFC 3339 time the status was set.
    pub since: String,
}

/// A game's status as filters and stats group it: the user's own, else
/// [`EffectiveStatus::Played`] for a game with playtime and
/// [`EffectiveStatus::NotPlayed`] for one without.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum EffectiveStatus {
    Beaten,
    Playing,
    PlanToPlay,
    Played,
    OnHold,
    Abandoned,
    NotPlayed,
}

impl EffectiveStatus {
    /// Every group, in the order charts and legends show them: finished and
    /// active games first, untouched ones last.
    pub const ALL: [EffectiveStatus; 7] = [
        EffectiveStatus::Beaten,
        EffectiveStatus::Playing,
        EffectiveStatus::PlanToPlay,
        EffectiveStatus::Played,
        EffectiveStatus::OnHold,
        EffectiveStatus::Abandoned,
        EffectiveStatus::NotPlayed,
    ];
}

impl From<PlayStatus> for EffectiveStatus {
    fn from(status: PlayStatus) -> Self {
        match status {
            PlayStatus::NotPlayed => Self::NotPlayed,
            PlayStatus::PlanToPlay => Self::PlanToPlay,
            PlayStatus::Playing => Self::Playing,
            PlayStatus::OnHold => Self::OnHold,
            PlayStatus::Beaten => Self::Beaten,
            PlayStatus::Abandoned => Self::Abandoned,
        }
    }
}

/// Most changes kept per game; the oldest go first.
pub const MAX_STATUS_CHANGES: usize = 64;

/// One real change of a game's play status: to `status`, or cleared
/// (`None`), at `at`. Every change the user makes is kept, so the backlog
/// knows each day a game was beaten even after its status moved on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusChange {
    /// RFC 3339 time of the change.
    pub at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<PlayStatus>,
}

impl StatusChange {
    /// When it happened, in milliseconds; changes whose time can't be read
    /// sort first.
    fn millis(&self) -> i64 {
        chrono::DateTime::parse_from_rfc3339(&self.at).map_or(i64::MIN, |t| t.timestamp_millis())
    }

    /// The same change, also when written with another offset.
    fn same(&self, other: &StatusChange) -> bool {
        self.status == other.status
            && match (self.millis(), other.millis()) {
                (i64::MIN, _) | (_, i64::MIN) => self.at == other.at,
                (a, b) => a == b,
            }
    }
}

/// Every game's status changes, oldest first: `GameId::key()` → changes.
pub type StatusHistory = BTreeMap<String, Vec<StatusChange>>;

/// Add `change` to a game's changes unless it is there already; returns
/// whether it was added. The list stays oldest first and at most
/// [`MAX_STATUS_CHANGES`] long.
pub(crate) fn record(changes: &mut Vec<StatusChange>, change: StatusChange) -> bool {
    if changes.iter().any(|c| c.same(&change)) {
        return false;
    }
    changes.push(change);
    changes.sort_by_key(StatusChange::millis);
    let extra = changes.len().saturating_sub(MAX_STATUS_CHANGES);
    changes.drain(..extra);
    true
}

/// Add every change of `from` to `into` (sync, restoring a backup): the
/// histories are sets. Returns how many changes were new.
pub(crate) fn merge_history(into: &mut StatusHistory, from: &StatusHistory) -> usize {
    let mut added = 0;
    for (key, changes) in from {
        let mine = into.entry(key.clone()).or_default();
        for change in changes {
            added += usize::from(record(mine, change.clone()));
        }
        if mine.is_empty() {
            into.remove(key);
        }
    }
    added
}

/// The RFC 3339 times a game was set to `status`: every recorded change to
/// it, and the date of its current status when that is `status` and no
/// change records it (set before changes were kept). Oldest first.
pub fn times_set<'a>(
    changes: &'a [StatusChange],
    current: Option<&'a PlayStatusRecord>,
    status: PlayStatus,
) -> Vec<&'a str> {
    let mut out: Vec<&StatusChange> = changes
        .iter()
        .filter(|c| c.status == Some(status))
        .collect();
    let recorded = current
        .filter(|r| r.status == status)
        .map(|r| StatusChange {
            at: r.since.clone(),
            status: Some(status),
        });
    let mut times: Vec<(i64, &str)> = out.drain(..).map(|c| (c.millis(), c.at.as_str())).collect();
    if let (Some(record), Some(change)) = (current, recorded)
        && !changes.iter().any(|c| c.same(&change))
    {
        times.push((change.millis(), record.since.as_str()));
    }
    times.sort();
    times.into_iter().map(|(_, at)| at).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn change(at: &str, status: Option<PlayStatus>) -> StatusChange {
        StatusChange {
            at: at.into(),
            status,
        }
    }

    #[test]
    fn a_change_is_kept_once_in_time_order_and_capped() {
        let mut list = Vec::new();
        assert!(record(
            &mut list,
            change("2026-11-02T10:00:00Z", Some(PlayStatus::Beaten))
        ));
        assert!(record(
            &mut list,
            change("2026-11-01T10:00:00Z", Some(PlayStatus::Playing))
        ));
        // The same change written with another offset.
        assert!(!record(
            &mut list,
            change("2026-11-02T11:00:00+01:00", Some(PlayStatus::Beaten))
        ));
        assert_eq!(list[0].status, Some(PlayStatus::Playing));
        for i in 0..100 {
            record(
                &mut list,
                change(&format!("2027-01-01T00:{:02}:{:02}Z", i / 60, i % 60), None),
            );
        }
        assert_eq!(list.len(), MAX_STATUS_CHANGES);
        assert_eq!(list.last().unwrap().at, "2027-01-01T00:01:39Z");
    }

    #[test]
    fn histories_merge_as_sets() {
        let mut mine = StatusHistory::new();
        mine.insert(
            "gog:1".into(),
            vec![change("2026-11-01T10:00:00Z", Some(PlayStatus::Playing))],
        );
        let mut theirs = StatusHistory::new();
        theirs.insert(
            "gog:1".into(),
            vec![
                change("2026-11-01T10:00:00Z", Some(PlayStatus::Playing)),
                change("2026-11-05T10:00:00Z", Some(PlayStatus::Beaten)),
            ],
        );
        theirs.insert("gog:2".into(), vec![change("2026-11-03T10:00:00Z", None)]);
        assert_eq!(merge_history(&mut mine, &theirs), 2);
        assert_eq!(mine["gog:1"].len(), 2);
        assert_eq!(merge_history(&mut mine, &theirs), 0);
    }

    #[test]
    fn times_set_include_a_status_set_before_changes_were_kept() {
        let changes = vec![
            change("2025-03-01T10:00:00Z", Some(PlayStatus::Beaten)),
            change("2026-01-01T10:00:00Z", Some(PlayStatus::Playing)),
        ];
        let now = PlayStatusRecord {
            status: PlayStatus::Playing,
            since: "2026-01-01T10:00:00Z".into(),
        };
        assert_eq!(
            times_set(&changes, Some(&now), PlayStatus::Beaten),
            ["2025-03-01T10:00:00Z"]
        );
        // Beaten before changes were kept, and again since.
        let old = PlayStatusRecord {
            status: PlayStatus::Beaten,
            since: "2024-05-01T10:00:00Z".into(),
        };
        assert_eq!(
            times_set(&changes[..1], Some(&old), PlayStatus::Beaten),
            ["2024-05-01T10:00:00Z", "2025-03-01T10:00:00Z"]
        );
        assert_eq!(
            times_set(&[], Some(&now), PlayStatus::Playing),
            ["2026-01-01T10:00:00Z"]
        );
    }
}
