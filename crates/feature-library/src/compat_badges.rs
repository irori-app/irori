//! ProtonDB and Steam Deck compatibility badges for library covers and the
//! List view's "On Linux" column.
//!
//! Fetches run off the UI thread with at most [`MAX_CONCURRENT`] lookups at a
//! time, checking the on-disk cache (`Backend::cached_compat`) first before
//! falling back to `Backend::fetch_compat` when online. Turning the setting off
//! cancels all queued and in-flight work.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, StyledExt as _, h_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{AnyElement, App, Hsla, IntoElement as _, ParentElement as _, Styled as _, div};
use irori_core::{GameId, GameInfo, Runner};
use irori_extras::wiki::{ProtonDbTier, SteamCompat, SteamDeckCompat};

/// Maximum concurrent compatibility lookups in flight.
pub const MAX_CONCURRENT: usize = 2;

/// Maximum queued (not yet started) lookups kept in memory. When scrolling
/// rapidly through a large library, oldest off-screen requests are evicted so
/// currently visible covers stay at the front of the queue.
pub const MAX_PENDING: usize = 64;

/// Maximum cached compatibility results kept in memory.
const MAX_ENTRIES: usize = 2048;

/// Displayable compatibility summary for a single game.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompatBadge {
    pub tier: Option<ProtonDbTier>,
    pub deck_verified: bool,
}

impl CompatBadge {
    /// Extract displayable ProtonDB tier (ignoring `Pending`/`Unknown`) and
    /// Steam Deck Verified status from `compat`. Returns `None` when nothing
    /// meaningful can be shown.
    pub fn from_compat(compat: &SteamCompat) -> Option<Self> {
        let tier = compat
            .protondb
            .filter(|t| !matches!(t, ProtonDbTier::Pending | ProtonDbTier::Unknown));
        let deck_verified = compat.steam_deck == Some(SteamDeckCompat::Verified);
        (tier.is_some() || deck_verified).then_some(Self {
            tier,
            deck_verified,
        })
    }
}

/// Theme token colour for a ProtonDB tier's indicator dot.
pub fn tier_color(tier: ProtonDbTier, cx: &App) -> Hsla {
    let theme = cx.theme();
    match tier {
        ProtonDbTier::Native => theme.info,
        ProtonDbTier::Platinum => theme.success,
        ProtonDbTier::Gold => theme.warning,
        ProtonDbTier::Silver | ProtonDbTier::Bronze => theme.muted_foreground,
        ProtonDbTier::Borked => theme.danger,
        ProtonDbTier::Pending | ProtonDbTier::Unknown => theme.muted_foreground,
    }
}

/// Render the bottom-left compatibility pill(s) on a cover tile.
pub fn render_cover_pill(badge: CompatBadge, scrim: Hsla, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let fg = theme.foreground;
    let success = theme.success;
    h_flex()
        .gap_1()
        .items_center()
        .when_some(badge.tier, |el, tier| {
            el.child(
                h_flex()
                    .gap_1()
                    .px_1p5()
                    .py_0p5()
                    .rounded_full()
                    .bg(scrim)
                    .text_xs()
                    .font_semibold()
                    .text_color(fg)
                    .items_center()
                    .child(
                        div()
                            .flex_none()
                            .size_1p5()
                            .rounded_full()
                            .bg(tier_color(tier, cx)),
                    )
                    .child(tier.label()),
            )
        })
        .when(badge.deck_verified, |el| {
            el.child(
                h_flex()
                    .gap_0p5()
                    .px_1p5()
                    .py_0p5()
                    .rounded_full()
                    .bg(scrim)
                    .text_xs()
                    .font_semibold()
                    .text_color(success)
                    .items_center()
                    .child(Icon::new(IconName::Check).xsmall())
                    .child("Deck"),
            )
        })
        .into_any_element()
}

/// Render the "On Linux" cell in the List layout.
pub fn render_list_cell(badge: Option<CompatBadge>, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let Some(badge) = badge else {
        return div()
            .text_sm()
            .text_color(theme.muted_foreground)
            .child("—")
            .into_any_element();
    };
    let fg = theme.foreground;
    let chip_bg = theme.secondary;
    let success = theme.success;
    h_flex()
        .gap_1()
        .items_center()
        .min_w_0()
        .overflow_hidden()
        .when_some(badge.tier, |el, tier| {
            el.child(
                h_flex()
                    .gap_1()
                    .px_1p5()
                    .py_0p5()
                    .rounded_full()
                    .bg(chip_bg)
                    .text_xs()
                    .font_semibold()
                    .text_color(fg)
                    .items_center()
                    .child(
                        div()
                            .flex_none()
                            .size_1p5()
                            .rounded_full()
                            .bg(tier_color(tier, cx)),
                    )
                    .child(tier.label()),
            )
        })
        .when(badge.deck_verified && badge.tier.is_none(), |el| {
            el.child(
                h_flex()
                    .gap_0p5()
                    .px_1p5()
                    .py_0p5()
                    .rounded_full()
                    .bg(chip_bg)
                    .text_xs()
                    .font_semibold()
                    .text_color(success)
                    .items_center()
                    .child(Icon::new(IconName::Check).xsmall())
                    .child("Deck"),
            )
        })
        .when(badge.deck_verified && badge.tier.is_some(), |el| {
            el.child(Icon::new(IconName::Check).xsmall().text_color(success))
        })
        .into_any_element()
}

/// Bounded queue and in-memory cache of per-game compatibility badges.
#[derive(Debug, Default)]
pub struct CompatQueue {
    entries: HashMap<GameId, Option<CompatBadge>>,
    order: VecDeque<GameId>,
    pending: VecDeque<Arc<GameInfo>>,
    queued: HashSet<GameId>,
    in_flight: HashSet<GameId>,
    /// Games skipped due to offline mode (disk cache miss while offline);
    /// cleared when returning online so they are retried without poisoning
    /// `entries`.
    offline_skipped: HashSet<GameId>,
    generation: u64,
}

impl CompatQueue {
    /// Cached badge for `id`, if loaded and displayable.
    pub fn get(&self, id: &GameId) -> Option<CompatBadge> {
        self.entries.get(id).copied().flatten()
    }

    /// Enqueue `game` if it is eligible and not already cached, queued, in
    /// flight, or skipped while offline. Cover badges pass
    /// `allow_installed = false`; the List view's "On Linux" column passes
    /// `allow_installed = true`.
    pub fn request(&mut self, game: &Arc<GameInfo>, allow_installed: bool) -> bool {
        if (!allow_installed && game.is_installed) || game.runner() == Runner::Emulation {
            return false;
        }
        let id = game.id();
        if self.entries.contains_key(&id)
            || self.queued.contains(&id)
            || self.in_flight.contains(&id)
            || self.offline_skipped.contains(&id)
        {
            return false;
        }
        while self.pending.len() >= MAX_PENDING {
            if let Some(evicted) = self.pending.pop_front() {
                self.queued.remove(&evicted.id());
            } else {
                break;
            }
        }
        self.queued.insert(id);
        self.pending.push_back(game.clone());
        true
    }

    /// Pop up to `MAX_CONCURRENT - in_flight` most recently requested
    /// (currently visible) games to start fetching now.
    pub fn take_next(&mut self) -> (u64, Vec<Arc<GameInfo>>) {
        let mut batch = Vec::new();
        while self.in_flight.len() < MAX_CONCURRENT {
            let Some(game) = self.pending.pop_back() else {
                break;
            };
            let id = game.id();
            self.queued.remove(&id);
            self.in_flight.insert(id);
            batch.push(game);
        }
        (self.generation, batch)
    }

    /// Record the finished lookup for `id`. Stale completions from before a
    /// [`Self::cancel_all`] are ignored and return `false`.
    pub fn finish(&mut self, generation: u64, id: &GameId, compat: Option<SteamCompat>) -> bool {
        if generation != self.generation {
            return false;
        }
        self.in_flight.remove(id);
        let badge = compat.as_ref().and_then(CompatBadge::from_compat);
        self.insert_entry(id.clone(), badge);
        true
    }

    /// Record that `id` missed the disk cache while offline: avoids infinite
    /// re-queuing while offline without poisoning [`Self::entries`], so going
    /// back online can fetch it once [`Self::clear_offline_skips`] is called.
    pub fn finish_offline(&mut self, generation: u64, id: &GameId) -> bool {
        if generation != self.generation {
            return false;
        }
        self.in_flight.remove(id);
        self.offline_skipped.insert(id.clone());
        true
    }

    /// Allow games previously skipped while offline to be fetched again.
    pub fn clear_offline_skips(&mut self) {
        self.offline_skipped.clear();
    }

    /// Cancel all queued and in-flight requests (for example when the user
    /// turns compatibility badges off or switches to a layout that does not
    /// show them). Cached entries are kept so turning it back on is instant.
    pub fn cancel_all(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.pending.clear();
        self.queued.clear();
        self.in_flight.clear();
        self.offline_skipped.clear();
    }

    fn insert_entry(&mut self, id: GameId, badge: Option<CompatBadge>) {
        if let std::collections::hash_map::Entry::Occupied(mut e) = self.entries.entry(id.clone()) {
            e.insert(badge);
            return;
        }
        while self.entries.len() >= MAX_ENTRIES {
            if let Some(oldest) = self.order.pop_front() {
                self.entries.remove(&oldest);
            } else {
                break;
            }
        }
        self.order.push_back(id.clone());
        self.entries.insert(id, badge);
    }

    #[cfg(test)]
    pub fn in_flight_count(&self) -> usize {
        self.in_flight.len()
    }

    #[cfg(test)]
    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::filter::tests::game;

    #[test]
    fn badge_extracts_visible_tier_and_deck_verified() {
        assert_eq!(
            CompatBadge::from_compat(&SteamCompat {
                protondb: Some(ProtonDbTier::Pending),
                steam_deck: Some(SteamDeckCompat::Playable),
            }),
            None
        );
        assert_eq!(
            CompatBadge::from_compat(&SteamCompat {
                protondb: Some(ProtonDbTier::Unknown),
                steam_deck: Some(SteamDeckCompat::Verified),
            }),
            Some(CompatBadge {
                tier: None,
                deck_verified: true,
            })
        );
        assert_eq!(
            CompatBadge::from_compat(&SteamCompat {
                protondb: Some(ProtonDbTier::Platinum),
                steam_deck: Some(SteamDeckCompat::Verified),
            }),
            Some(CompatBadge {
                tier: Some(ProtonDbTier::Platinum),
                deck_verified: true,
            })
        );
    }

    #[test]
    fn queue_caps_concurrency_deduplicates_and_skips_installed() {
        let mut q = CompatQueue::default();
        let installed = game(Runner::Gog, "1", "Installed", true);
        let a = game(Runner::Gog, "2", "A", false);
        let b = game(Runner::Gog, "3", "B", false);
        let c = game(Runner::Gog, "4", "C", false);
        let rom = game(Runner::Emulation, "5", "ROM", false);

        assert!(!q.request(&installed, false));
        assert!(!q.request(&rom, true));
        assert!(q.request(&a, false));
        assert!(!q.request(&a, false), "already queued");
        assert!(q.request(&b, false));
        assert!(q.request(&c, false));
        assert_eq!(q.pending_count(), 3);

        let (gen1, batch) = q.take_next();
        assert_eq!(batch.len(), MAX_CONCURRENT);
        // LIFO prioritizes the most recently painted (currently visible) games.
        assert_eq!(batch[0].id(), c.id());
        assert_eq!(batch[1].id(), b.id());
        assert_eq!(q.in_flight_count(), MAX_CONCURRENT);
        assert_eq!(q.pending_count(), 1);
        assert!(!q.request(&c, false), "already in flight");

        // Taking again while full yields nothing.
        assert!(q.take_next().1.is_empty());

        // Finishing one frees a slot for `a` and caches `c`.
        assert!(q.finish(
            gen1,
            &c.id(),
            Some(SteamCompat {
                protondb: Some(ProtonDbTier::Gold),
                steam_deck: None,
            })
        ));
        assert_eq!(
            q.get(&c.id()),
            Some(CompatBadge {
                tier: Some(ProtonDbTier::Gold),
                deck_verified: false,
            })
        );
        assert!(!q.request(&c, false), "already cached");

        let (_, next) = q.take_next();
        assert_eq!(next.len(), 1);
        assert_eq!(next[0].id(), a.id());
    }

    #[test]
    fn offline_misses_do_not_poison_cache_once_back_online() {
        let mut q = CompatQueue::default();
        let a = game(Runner::Steam, "100", "A", false);
        assert!(q.request(&a, false));
        let (gen1, batch) = q.take_next();
        assert_eq!(batch.len(), 1);
        assert!(q.finish_offline(gen1, &a.id()));
        // While still offline, does not re-queue in a loop.
        assert!(!q.request(&a, false));
        assert_eq!(q.get(&a.id()), None);

        // Returning online clears offline skips so the game can be fetched.
        q.clear_offline_skips();
        assert!(q.request(&a, false));
    }

    #[test]
    fn pending_queue_caps_size_and_evicts_oldest_unseen() {
        let mut q = CompatQueue::default();
        for i in 0..(MAX_PENDING + 10) {
            let g = game(Runner::Steam, &i.to_string(), "G", false);
            assert!(q.request(&g, false));
        }
        assert_eq!(q.pending_count(), MAX_PENDING);
        // Oldest scrolled-past game ("0") was evicted and can be re-requested.
        let oldest = game(Runner::Steam, "0", "G", false);
        assert!(q.request(&oldest, false));
        // Most recently requested game comes out first.
        let (_, batch) = q.take_next();
        assert_eq!(batch[0].id(), oldest.id());
    }

    #[test]
    fn cancel_all_drops_pending_and_ignores_stale_results() {
        let mut q = CompatQueue::default();
        let a = game(Runner::Steam, "10", "A", false);
        let b = game(Runner::Steam, "20", "B", false);
        let c = game(Runner::Steam, "30", "C", false);
        q.request(&a, false);
        q.request(&b, false);
        q.request(&c, false);
        let (old_gen, batch) = q.take_next();
        assert_eq!(batch.len(), 2);

        q.cancel_all();
        assert_eq!(q.in_flight_count(), 0);
        assert_eq!(q.pending_count(), 0);

        // Late completion from before cancel is ignored.
        assert!(!q.finish(
            old_gen,
            &a.id(),
            Some(SteamCompat {
                protondb: Some(ProtonDbTier::Platinum),
                steam_deck: None,
            })
        ));
        assert_eq!(q.get(&a.id()), None);
    }

    #[test]
    fn cache_evicts_oldest_entries_at_cap() {
        let mut q = CompatQueue::default();
        for i in 0..(MAX_ENTRIES + 5) {
            let g = game(Runner::Steam, &i.to_string(), "G", false);
            q.request(&g, false);
            let (gen_id, _) = q.take_next();
            q.finish(gen_id, &g.id(), None);
        }
        assert_eq!(q.entries.len(), MAX_ENTRIES);
        assert!(!q.entries.contains_key(&GameId::new(Runner::Steam, "0")));
        let last = (MAX_ENTRIES + 4).to_string();
        assert!(
            q.entries
                .contains_key(&GameId::new(Runner::Steam, last.as_str()))
        );
    }
}
