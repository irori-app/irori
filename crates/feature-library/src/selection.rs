//! Games picked for bulk actions. Selection mode starts with the toolbar's
//! "Select", a Ctrl-click or a Shift-click and ends with "Done" or Escape;
//! while it lasts, clicking a game picks it instead of opening it. The
//! keyboard picks too: Shift with the arrows, Home, End and the page keys
//! picks the games from where the steps began to the focused one, and
//! Shift+Space (Ctrl+Space) picks or unpicks the focused game.

use std::collections::HashSet;
use std::sync::Arc;

use irori_core::{GameId, GameInfo};

#[derive(Debug, Default)]
pub struct Selection {
    active: bool,
    picked: HashSet<GameId>,
    /// Where the last plain pick happened; Shift-click extends from here.
    anchor: Option<GameId>,
    /// While Shift+arrows extend a range: the picks from before it, so a
    /// step back unpicks what the range no longer covers.
    range_base: Option<HashSet<GameId>>,
    /// Counts changes to the picks, so what is worked out from them is
    /// worked out again only after one.
    revision: u64,
}

impl Selection {
    /// Whether selection mode is on (it can be on with nothing picked).
    pub fn is_active(&self) -> bool {
        self.active
    }

    pub fn start(&mut self) {
        self.active = true;
    }

    /// Leave selection mode, forgetting the picks.
    pub fn end(&mut self) {
        let revision = self.revision + 1;
        *self = Self {
            revision,
            ..Self::default()
        };
    }

    /// Changes with every change to the picks.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn contains(&self, id: &GameId) -> bool {
        self.picked.contains(id)
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.picked.len()
    }

    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.picked.is_empty()
    }

    /// Pick or unpick one game (a click or Ctrl-click).
    pub fn toggle(&mut self, id: &GameId) {
        self.active = true;
        self.revision += 1;
        if !self.picked.remove(id) {
            self.picked.insert(id.clone());
        }
        self.anchor = Some(id.clone());
        self.range_base = None;
    }

    /// Pick every game from the anchor to `id` in display order
    /// (Shift-click). Without an anchor this is a plain toggle.
    pub fn extend_to(&mut self, id: &GameId, order: &[Arc<GameInfo>]) {
        let position = |target: &GameId| order.iter().position(|g| &g.id() == target);
        let (Some(from), Some(to)) = (self.anchor.as_ref().and_then(position), position(id)) else {
            self.toggle(id);
            return;
        };
        self.active = true;
        self.revision += 1;
        let (lo, hi) = (from.min(to), from.max(to));
        self.picked.extend(order[lo..=hi].iter().map(|g| g.id()));
        self.range_base = None;
    }

    /// Shift with a moving key: pick the games from where the range began to
    /// `to` in display order, replacing what the range picked before, so
    /// stepping back unpicks the games it leaves. A new range begins at
    /// `from`, the game focused before the first step, and keeps the picks
    /// made before it.
    pub fn extend_range(&mut self, from: &GameId, to: &GameId, order: &[Arc<GameInfo>]) {
        if self.range_base.is_none() {
            self.range_base = Some(self.picked.clone());
            self.anchor = Some(from.clone());
        }
        let position = |target: &GameId| order.iter().position(|g| &g.id() == target);
        let (Some(start), Some(end)) = (self.anchor.as_ref().and_then(position), position(to))
        else {
            return;
        };
        self.active = true;
        self.revision += 1;
        let (lo, hi) = (start.min(end), start.max(end));
        let mut picked = self.range_base.clone().unwrap_or_default();
        picked.extend(order[lo..=hi].iter().map(|g| g.id()));
        self.picked = picked;
    }

    /// A move without Shift: the next Shift step begins a new range.
    pub fn end_range(&mut self) {
        self.range_base = None;
    }

    /// Pick every shown game.
    pub fn select_all(&mut self, order: &[Arc<GameInfo>]) {
        self.active = true;
        self.revision += 1;
        self.picked.extend(order.iter().map(|g| g.id()));
        self.range_base = None;
    }

    /// Unpick everything but stay in selection mode.
    #[cfg(test)]
    pub fn clear(&mut self) {
        self.picked.clear();
        self.anchor = None;
        self.range_base = None;
        self.revision += 1;
    }

    /// Drop picks that are no longer shown (a filter or search changed), so
    /// bulk actions never touch games the user cannot see.
    pub fn keep_shown(&mut self, order: &[Arc<GameInfo>]) {
        if self.picked.is_empty() {
            return;
        }
        let shown: HashSet<GameId> = order.iter().map(|g| g.id()).collect();
        let before = self.picked.len();
        self.picked.retain(|id| shown.contains(id));
        if self.picked.len() != before {
            self.revision += 1;
            self.range_base = None;
        }
    }

    /// The picked games in display order.
    pub fn games(&self, order: &[Arc<GameInfo>]) -> Vec<Arc<GameInfo>> {
        order
            .iter()
            .filter(|g| self.picked.contains(&g.id()))
            .cloned()
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use irori_core::Runner;

    fn order(n: usize) -> Vec<Arc<GameInfo>> {
        (0..n)
            .map(|i| {
                Arc::new(GameInfo {
                    runner: Some(Runner::Gog),
                    app_name: i.to_string(),
                    title: format!("Game {i}"),
                    ..Default::default()
                })
            })
            .collect()
    }

    fn id(i: usize) -> GameId {
        GameId::new(Runner::Gog, i.to_string())
    }

    fn picked(s: &Selection, order: &[Arc<GameInfo>]) -> Vec<String> {
        s.games(order).iter().map(|g| g.app_name.clone()).collect()
    }

    #[test]
    fn toggles_and_extends_ranges_from_the_anchor() {
        let order = order(6);
        let mut s = Selection::default();
        assert!(!s.is_active());
        s.toggle(&id(1));
        assert!(s.is_active());
        s.extend_to(&id(4), &order);
        assert_eq!(picked(&s, &order), ["1", "2", "3", "4"]);
        // Backwards from the same anchor adds, never removes.
        s.extend_to(&id(0), &order);
        assert_eq!(picked(&s, &order), ["0", "1", "2", "3", "4"]);
        s.toggle(&id(2));
        assert_eq!(picked(&s, &order), ["0", "1", "3", "4"]);
    }

    #[test]
    fn shift_click_without_an_anchor_picks_one() {
        let order = order(3);
        let mut s = Selection::default();
        s.extend_to(&id(2), &order);
        assert_eq!(picked(&s, &order), ["2"]);
    }

    #[test]
    fn shift_steps_pick_a_range_that_follows_the_focus() {
        let order = order(8);
        let mut s = Selection::default();
        // A pick made before the range stays.
        s.toggle(&id(7));
        s.end_range();
        s.extend_range(&id(2), &id(3), &order);
        assert_eq!(picked(&s, &order), ["2", "3", "7"]);
        s.extend_range(&id(3), &id(5), &order);
        assert_eq!(picked(&s, &order), ["2", "3", "4", "5", "7"]);
        // Stepping back unpicks what the range leaves, past its start too.
        s.extend_range(&id(5), &id(1), &order);
        assert_eq!(picked(&s, &order), ["1", "2", "7"]);
        // A plain move ends the range: the next one begins where it is.
        s.end_range();
        s.extend_range(&id(5), &id(6), &order);
        assert_eq!(picked(&s, &order), ["1", "2", "5", "6", "7"]);
        // Ticking one game ends it too.
        s.toggle(&id(0));
        s.extend_range(&id(3), &id(4), &order);
        assert_eq!(picked(&s, &order), ["0", "1", "2", "3", "4", "5", "6", "7"]);
        assert!(s.is_active());
    }

    #[test]
    fn hidden_games_drop_out_and_end_resets() {
        let all = order(4);
        let mut s = Selection::default();
        s.select_all(&all);
        assert_eq!(s.len(), 4);
        s.keep_shown(&all[..2]);
        assert_eq!(picked(&s, &all), ["0", "1"]);
        s.clear();
        assert!(s.is_empty() && s.is_active());
        s.end();
        assert!(!s.is_active());
    }

    #[test]
    fn every_change_to_the_picks_moves_the_revision() {
        let all = order(4);
        let mut s = Selection::default();
        let mut last = s.revision();
        let mut changed = |s: &Selection| {
            let moved = s.revision() != last;
            last = s.revision();
            moved
        };
        s.toggle(&id(0));
        assert!(changed(&s));
        s.select_all(&all);
        assert!(changed(&s));
        s.keep_shown(&all);
        assert!(!changed(&s), "nothing dropped");
        s.keep_shown(&all[..1]);
        assert!(changed(&s));
        s.end();
        assert!(changed(&s));
    }
}
