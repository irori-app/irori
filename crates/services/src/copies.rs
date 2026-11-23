//! Copies of one game owned on several stores, shown as one library entry.
//!
//! Irori matches copies by itself only where a mistake is unlikely: the same
//! title once folded ([`crate::search::fold`]), on different stores, with no
//! store holding two games of that title (two editions, a remake) and no
//! release years more than a year apart (DOOM and DOOM). Games the user
//! added, ROMs and DLC are never matched. The user combines other copies
//! and separates wrong ones (`GameOverrides::copy_of`), which wins over the
//! matching and travels with sync like the other overrides.
//!
//! Each entry shows its default copy: an installed one first, then the one
//! the user picked (`GameOverrides::default_copy`), then the one played last,
//! then the first store in Irori's order. Every copy keeps its own
//! [`GameId`], playtime and achievements; only the list shows them as one.
//!
//! Games Irori leaves apart that are linked to the same Steam app (another
//! edition, a remaster sold under the same app) are offered as *possible
//! copies* ([`possible`]); the user links them or keeps them apart, which
//! stands like a separation.

use std::collections::HashMap;
use std::sync::Arc;

use irori_core::{GameId, GameInfo, Runner};

use crate::userdata::UserData;

/// The library's games, sorted into entries.
#[derive(Debug, Default)]
pub struct Grouped {
    /// One game per entry: its default copy, or the game itself.
    pub entries: Vec<Arc<GameInfo>>,
    /// The copies an entry's default copy stands for.
    pub others: Vec<Arc<GameInfo>>,
    /// Each entry of several copies: its copies, the default one first.
    pub groups: Vec<Arc<[GameId]>>,
}

/// Whether Irori may match copies of games from `runner` by itself.
fn matched_automatically(runner: Runner) -> bool {
    !matches!(runner, Runner::Sideload | Runner::Emulation)
}

/// The year a release date starts with.
fn release_year(game: &GameInfo) -> Option<i32> {
    game.release_date.as_deref()?.get(..4)?.parse().ok()
}

/// Union-find over the games' positions.
struct Sets(Vec<usize>);

impl Sets {
    fn find(&mut self, mut i: usize) -> usize {
        while self.0[i] != i {
            self.0[i] = self.0[self.0[i]];
            i = self.0[i];
        }
        i
    }

    fn union(&mut self, a: usize, b: usize) {
        let (a, b) = (self.find(a), self.find(b));
        if a != b {
            self.0[b.max(a)] = a.min(b);
        }
    }
}

/// Sort `games` into entries. With `enabled` false every game is its own.
pub fn group(games: Vec<Arc<GameInfo>>, user: &UserData, enabled: bool) -> Grouped {
    if !enabled || games.len() < 2 {
        return Grouped {
            entries: games,
            ..Default::default()
        };
    }
    let keys: Vec<String> = games.iter().map(|g| g.id().key()).collect();
    let position: HashMap<&str, usize> = keys
        .iter()
        .enumerate()
        .map(|(i, k)| (k.as_str(), i))
        .collect();
    let copy_of = |i: usize| {
        user.overrides
            .get(&keys[i])
            .and_then(|o| o.copy_of.as_deref())
    };
    let mut sets = Sets((0..games.len()).collect());

    // Irori's own matches, by folded title.
    let mut by_title: HashMap<String, Vec<usize>> = HashMap::new();
    for (i, g) in games.iter().enumerate() {
        if !matched_automatically(g.runner()) || g.is_dlc || copy_of(i) == Some("") {
            continue;
        }
        let title = crate::search::fold(&g.title);
        if !title.is_empty() {
            by_title.entry(title).or_default().push(i);
        }
    }
    for same in by_title.values().filter(|s| s.len() > 1) {
        let mut runners: Vec<Runner> = same.iter().map(|&i| games[i].runner()).collect();
        runners.sort();
        let one_per_store = runners.windows(2).all(|w| w[0] != w[1]);
        let years: Vec<i32> = same
            .iter()
            .filter_map(|&i| release_year(&games[i]))
            .collect();
        let same_years = match (years.iter().min(), years.iter().max()) {
            (Some(lo), Some(hi)) => hi - lo <= 1,
            _ => true,
        };
        if one_per_store && same_years {
            for &i in &same[1..] {
                sets.union(same[0], i);
            }
        }
    }
    // The user's own, which win.
    for i in 0..games.len() {
        if let Some(target) = copy_of(i).filter(|t| !t.is_empty())
            && let Some(&j) = position.get(target)
        {
            sets.union(i, j);
        }
    }

    let mut members: HashMap<usize, Vec<usize>> = HashMap::new();
    for i in 0..games.len() {
        let root = sets.find(i);
        members.entry(root).or_default().push(i);
    }
    let last_played = |i: usize| {
        user.playtime
            .get(&keys[i])
            .and_then(|p| p.last_played.as_deref())
            .and_then(irori_core::util::parse_rfc3339)
            .unwrap_or(i64::MIN)
    };
    let chosen = |i: usize| user.overrides.get(&keys[i]).is_some_and(|o| o.default_copy);
    let store_order = |i: usize| {
        Runner::ALL
            .iter()
            .position(|r| *r == games[i].runner())
            .unwrap_or(usize::MAX)
    };

    let mut out = Grouped::default();
    let mut roots: Vec<usize> = members.keys().copied().collect();
    roots.sort_unstable();
    for root in roots {
        let mut copies = members.remove(&root).unwrap_or_default();
        if copies.len() == 1 {
            out.entries.push(games[copies[0]].clone());
            continue;
        }
        copies.sort_by_key(|&i| {
            (
                !games[i].is_installed,
                !chosen(i),
                std::cmp::Reverse(last_played(i)),
                store_order(i),
                i,
            )
        });
        out.groups.push(
            copies
                .iter()
                .map(|&i| games[i].id())
                .collect::<Vec<_>>()
                .into(),
        );
        out.entries.push(games[copies[0]].clone());
        out.others
            .extend(copies[1..].iter().map(|&i| games[i].clone()));
    }
    out
}

/// One game that might be another copy of an entry: the game, the Steam
/// app it is linked to, and whether the user keeps it apart.
pub struct Candidate {
    pub game: Arc<GameInfo>,
    pub steam_app: Option<u32>,
    pub kept_apart: bool,
}

/// The games among `candidates` that may be copies of the entry whose
/// copies are `group` and whose Steam apps are `apps`: linked to one of
/// those apps, not DLC, not in the entry already and not kept apart by the
/// user. Each other entry is offered once, by the first of its copies
/// (`entry_of` names a game's entry).
pub fn possible(
    group: &[GameId],
    apps: &[u32],
    candidates: impl IntoIterator<Item = Candidate>,
    entry_of: impl Fn(&GameId) -> GameId,
) -> Vec<Arc<GameInfo>> {
    let mut seen: Vec<GameId> = Vec::new();
    let mut out = Vec::new();
    for c in candidates {
        let id = c.game.id();
        if c.game.is_dlc
            || c.kept_apart
            || group.contains(&id)
            || !c.steam_app.is_some_and(|app| apps.contains(&app))
        {
            continue;
        }
        let entry = entry_of(&id);
        if seen.contains(&entry) || group.contains(&entry) {
            continue;
        }
        seen.push(entry);
        out.push(c.game);
    }
    out
}

#[cfg(test)]
mod tests {
    use irori_core::game::GameOverrides;

    use super::*;

    fn game(runner: Runner, app: &str, title: &str) -> Arc<GameInfo> {
        with(runner, app, title, |_| {})
    }

    fn with(
        runner: Runner,
        app: &str,
        title: &str,
        f: impl FnOnce(&mut GameInfo),
    ) -> Arc<GameInfo> {
        let mut g = GameInfo {
            runner: Some(runner),
            app_name: app.into(),
            title: title.into(),
            ..Default::default()
        };
        f(&mut g);
        Arc::new(g.finalize())
    }

    fn groups(g: &Grouped) -> Vec<Vec<String>> {
        let mut out: Vec<Vec<String>> = g
            .groups
            .iter()
            .map(|ids| ids.iter().map(GameId::key).collect())
            .collect();
        out.sort();
        out
    }

    #[test]
    fn copies_of_one_title_on_several_stores_are_one_entry() {
        let games = vec![
            game(Runner::Steam, "1145360", "Hades"),
            game(Runner::Legendary, "Min", "HADES"),
            game(Runner::Gog, "1", "Celeste"),
            game(Runner::Gog, "2", "Hades"),
        ];
        let grouped = group(games, &UserData::default(), true);
        assert_eq!(grouped.entries.len(), 2);
        assert_eq!(grouped.others.len(), 2);
        assert_eq!(
            groups(&grouped),
            [vec!["legendary:Min", "gog:2", "steam:1145360"]],
            "first store in Irori's order leads when none is installed"
        );
        // Turned off, every copy is its own entry.
        let games = grouped
            .entries
            .iter()
            .chain(&grouped.others)
            .cloned()
            .collect();
        assert_eq!(group(games, &UserData::default(), false).entries.len(), 4);
    }

    #[test]
    fn unsure_matches_stay_apart() {
        let games = vec![
            // Two of one title on one store: editions, or a remake.
            game(Runner::Gog, "1", "Doom"),
            game(Runner::Gog, "2", "Doom"),
            game(Runner::Steam, "3", "Doom"),
            // Years apart.
            with(Runner::Steam, "4", "Prey", |g| {
                g.release_date = Some("2006-07-11".into())
            }),
            with(Runner::Legendary, "5", "Prey", |g| {
                g.release_date = Some("2017-05-05".into())
            }),
            // Added by the user, or a ROM.
            game(Runner::Sideload, "6", "Celeste"),
            game(Runner::Gog, "7", "Celeste"),
            game(Runner::Emulation, "8", "Tetris"),
            game(Runner::Steam, "9", "Tetris"),
            // Close years still match.
            with(Runner::Steam, "10", "Inside", |g| {
                g.release_date = Some("2016-06-29".into())
            }),
            with(Runner::Gog, "11", "Inside", |g| {
                g.release_date = Some("2017-07-07".into())
            }),
        ];
        let grouped = group(games, &UserData::default(), true);
        assert_eq!(groups(&grouped), [vec!["gog:11", "steam:10"]]);
    }

    #[test]
    fn the_installed_then_the_chosen_copy_leads() {
        let games = || {
            vec![
                game(Runner::Gog, "1", "Hades"),
                with(Runner::Steam, "2", "Hades", |g| g.is_installed = true),
                game(Runner::Legendary, "3", "Hades"),
            ]
        };
        let mut user = UserData::default();
        user.overrides.insert(
            "legendary:3".into(),
            GameOverrides {
                default_copy: true,
                ..Default::default()
            },
        );
        let grouped = group(games(), &user, true);
        assert_eq!(grouped.entries[0].id().key(), "steam:2");
        assert_eq!(grouped.groups[0][1].key(), "legendary:3");
        // Without an installed copy the chosen one leads.
        let mut uninstalled = games();
        uninstalled[1] = game(Runner::Steam, "2", "Hades");
        assert_eq!(
            group(uninstalled, &user, true).entries[0].id().key(),
            "legendary:3"
        );
    }

    #[test]
    fn games_of_one_steam_app_are_possible_copies() {
        let deluxe = game(Runner::Gog, "2", "Hades: Deluxe Edition");
        let other = game(Runner::Legendary, "3", "Hades");
        let candidates = || {
            vec![
                Candidate {
                    game: deluxe.clone(),
                    steam_app: Some(1145360),
                    kept_apart: false,
                },
                Candidate {
                    game: other.clone(),
                    steam_app: Some(1145360),
                    kept_apart: false,
                },
                Candidate {
                    game: game(Runner::Gog, "4", "Celeste"),
                    steam_app: Some(504230),
                    kept_apart: false,
                },
                Candidate {
                    game: game(Runner::Itch, "5", "Hades fan game"),
                    steam_app: Some(1145360),
                    kept_apart: true,
                },
            ]
        };
        let group = [GameId::new(Runner::Steam, "1145360")];
        // gog:2 and legendary:3 are one entry: offered once.
        let entry = |id: &GameId| match id.app_name.as_ref() {
            "3" => GameId::new(Runner::Gog, "2"),
            _ => id.clone(),
        };
        let found = possible(&group, &[1145360], candidates(), entry);
        let keys: Vec<String> = found.iter().map(|g| g.id().key()).collect();
        assert_eq!(keys, ["gog:2"]);
        // Already in the entry, or without a Steam app: nothing.
        let group = [group[0].clone(), deluxe.id(), other.id()];
        assert!(possible(&group, &[1145360], candidates(), |id| id.clone()).is_empty());
        assert!(possible(&group, &[], candidates(), |id| id.clone()).is_empty());
    }

    #[test]
    fn the_users_combinations_and_separations_win() {
        let games = vec![
            game(Runner::Steam, "1", "The Witcher 3: Wild Hunt"),
            game(Runner::Gog, "2", "The Witcher 3: Wild Hunt - GOTY"),
            game(Runner::Legendary, "3", "Hades"),
            game(Runner::Gog, "4", "Hades"),
            game(Runner::Sideload, "5", "My Hades"),
        ];
        let mut user = UserData::default();
        let set = |user: &mut UserData, key: &str, to: &str| {
            user.overrides.insert(
                key.into(),
                GameOverrides {
                    copy_of: Some(to.into()),
                    ..Default::default()
                },
            );
        };
        set(&mut user, "gog:2", "steam:1");
        set(&mut user, "sideload:5", "gog:4");
        let grouped = group(games.clone(), &user, true);
        assert_eq!(
            groups(&grouped),
            [
                vec!["gog:2", "steam:1"],
                vec!["legendary:3", "gog:4", "sideload:5"],
            ]
        );
        // Separated, a copy stands alone and the rest stay together.
        set(&mut user, "legendary:3", "");
        let grouped = group(games, &user, true);
        assert_eq!(
            groups(&grouped),
            [vec!["gog:2", "steam:1"], vec!["gog:4", "sideload:5"]]
        );
    }
}
