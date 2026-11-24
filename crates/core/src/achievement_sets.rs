//! A game's achievements by set: stores that group achievements (Steam's
//! Achievement Sets: the base game, each DLC) are shown a heading and a
//! progress per set by the game page and the in-game overlay alike. Pure
//! logic, shared so both count the same way.

use std::cmp::Ordering;

use crate::{Achievement, AchievementGroup};

/// One set of a game's achievements and how much of it is unlocked.
#[derive(Debug, Clone, PartialEq)]
pub struct SetProgress {
    /// `None` for the achievements no set holds.
    pub group: Option<AchievementGroup>,
    pub unlocked: usize,
    pub total: usize,
}

impl SetProgress {
    /// Whether every achievement is unlocked, or the store says the set
    /// was completed (a set may need fewer than all of them).
    pub fn complete(&self) -> bool {
        (self.total > 0 && self.unlocked == self.total) || self.completed_at().is_some()
    }

    /// Whether the store says the player can't earn this set.
    pub fn unearnable(&self) -> bool {
        self.group
            .as_ref()
            .is_some_and(AchievementGroup::unearnable)
    }

    /// When the store says the player completed the set.
    pub fn completed_at(&self) -> Option<&str> {
        self.group
            .as_ref()
            .and_then(|g| g.completed_at.as_deref())
            .filter(|d| !d.is_empty())
    }
}

/// The sets `list` falls into, in the store's order with the achievements
/// of no set last. Empty when there is at most one set, so a game without
/// sets (or with only its base game's) keeps one plain list.
pub fn sets(list: &[Achievement]) -> Vec<SetProgress> {
    let mut out: Vec<SetProgress> = Vec::new();
    for a in list {
        let ix = match out.iter().position(|s| s.group == a.group) {
            Some(ix) => ix,
            None => {
                out.push(SetProgress {
                    group: a.group.clone(),
                    unlocked: 0,
                    total: 0,
                });
                out.len() - 1
            }
        };
        out[ix].total += 1;
        out[ix].unlocked += usize::from(a.is_unlocked());
    }
    if out.len() < 2 {
        return Vec::new();
    }
    out.sort_by(|a, b| match (&a.group, &b.group) {
        (Some(a), Some(b)) => a
            .order
            .cmp(&b.order)
            .then_with(|| a.dlc_app_id.is_some().cmp(&b.dlc_app_id.is_some()))
            .then_with(|| a.id.cmp(&b.id)),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    });
    out
}
