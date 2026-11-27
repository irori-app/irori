//! "Pick a random game", for when the backlog is longer than the evening:
//! from the games shown, the installed ones, those never played, the
//! favourites, the whole library or one category. The last few picks are
//! remembered for the run, so "Pick another" doesn't offer the same game
//! twice in a row while there are others.

use std::collections::{BTreeSet, VecDeque};
use std::hash::{BuildHasher as _, Hasher as _};
use std::sync::Arc;

use gpui_kit::component::WindowExt as _;
use gpui_kit::component::notification::Notification;
use gpui_kit::{App, Global, SharedString, Window};
use irori_core::{GameId, GameInfo};
use irori_services::{CategoryRef, EffectiveStatus};
use irori_ui::i18n::{t, t_args};
use irori_ui::{Route, models, navigate};

/// How many recent picks are left out while others remain.
const REMEMBERED: usize = 5;

/// The games a random pick is made from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pool {
    /// The games the library shows now, with its filters.
    Shown,
    Installed,
    NeverPlayed,
    Favourites,
    /// Every game in the library.
    All,
    /// The games of one category of either kind.
    Category(CategoryRef),
}

impl Pool {
    /// The row that offers picking from this pool; `name` names a category.
    pub fn label(&self, name: &str) -> SharedString {
        crate::i18n::register();
        match self {
            Self::Shown => t("library.random.pool_shown"),
            Self::Installed => t("library.random.pool_installed"),
            Self::NeverPlayed => t("library.random.pool_never_played"),
            Self::Favourites => t("library.random.pool_favourites"),
            Self::All => t("library.random.pool_all"),
            Self::Category(_) => t_args("library.random.pool_category", &[("name", name)]),
        }
    }
}

/// The recent picks of this run, newest last.
#[derive(Default)]
struct RecentPicks(VecDeque<String>);
impl Global for RecentPicks {}

fn remember(id: &GameId, cx: &mut App) {
    let recent = &mut cx.default_global::<RecentPicks>().0;
    let key = id.key();
    recent.retain(|k| *k != key);
    recent.push_back(key);
    while recent.len() > REMEMBERED {
        recent.pop_front();
    }
}

/// Choose one of `pool` by `roll`. Hidden games are never picked; recent
/// picks (`recent`, newest last) only when nothing else is left, and then
/// never the very last one while another exists.
pub fn choose(
    pool: &[Arc<GameInfo>],
    hidden: &BTreeSet<String>,
    recent: &[String],
    roll: u64,
) -> Option<GameId> {
    let candidates: Vec<GameId> = pool
        .iter()
        .map(|g| g.id())
        .filter(|id| !hidden.contains(&id.key()))
        .collect();
    let fresh: Vec<&GameId> = candidates
        .iter()
        .filter(|id| !recent.contains(&id.key()))
        .collect();
    let from = if fresh.is_empty() {
        let last = recent.last();
        let others: Vec<&GameId> = candidates
            .iter()
            .filter(|id| Some(&id.key()) != last)
            .collect();
        if others.is_empty() {
            candidates.iter().collect()
        } else {
            others
        }
    } else {
        fresh
    };
    if from.is_empty() {
        return None;
    }
    Some(from[(roll % from.len() as u64) as usize].clone())
}

/// A game to suggest: an installed one when any is installed, else any game
/// in the library. Hidden games are never picked; see [`choose`].
pub fn pick(
    games: &[Arc<GameInfo>],
    hidden: &BTreeSet<String>,
    recent: &[String],
    roll: u64,
) -> Option<GameId> {
    let installed: Vec<Arc<GameInfo>> = games
        .iter()
        .filter(|g| g.is_installed && !hidden.contains(&g.id().key()))
        .cloned()
        .collect();
    if installed.is_empty() {
        choose(games, hidden, recent, roll)
    } else {
        choose(&installed, hidden, recent, roll)
    }
}

/// The games of `pool` now.
fn pool_games(pool: &Pool, cx: &App) -> Vec<Arc<GameInfo>> {
    if *pool == Pool::Shown {
        return crate::view::remove::shown_games(cx)
            .map(|games| games.to_vec())
            .unwrap_or_default();
    }
    let library = models(cx).library.read(cx);
    let user = &library.user;
    library
        .snapshot
        .games
        .iter()
        .filter(|g| {
            let id = g.id();
            match pool {
                Pool::Shown | Pool::All => true,
                Pool::Installed => g.is_installed,
                Pool::NeverPlayed => user.effective_status(&id.key()) == EffectiveStatus::NotPlayed,
                Pool::Favourites => user.favourites.contains(&id.key()),
                Pool::Category(CategoryRef::Manual(name)) => user
                    .categories
                    .get(name)
                    .is_some_and(|games| games.contains(&id.key())),
                Pool::Category(CategoryRef::Smart(smart)) => {
                    library.categories.contains(smart, &id)
                }
            }
        })
        .cloned()
        .collect()
}

/// The pools there is something to pick from, with their labels: the games
/// shown while a library view exists, the usual sets, then every category.
pub fn pools(cx: &App) -> Vec<(Pool, SharedString)> {
    let mut pools = vec![];
    if crate::view::remove::shown_games(cx).is_some() {
        pools.push(Pool::Shown);
    }
    pools.extend([
        Pool::Installed,
        Pool::NeverPlayed,
        Pool::Favourites,
        Pool::All,
    ]);
    let categories = crate::category_list(cx);
    let hidden = &models(cx).library.read(cx).user.hidden;
    pools
        .into_iter()
        .map(|p| (p, String::new()))
        .chain(
            categories
                .into_iter()
                .filter(|c| c.count > 0)
                .map(|c| (Pool::Category(c.category), c.name)),
        )
        .filter(|(pool, _)| {
            pool_games(pool, cx)
                .iter()
                .any(|g| !hidden.contains(&g.id().key()))
        })
        .map(|(pool, name)| {
            let label = pool.label(&name);
            (pool, label)
        })
        .collect()
}

/// Pick a game from `pool`, avoiding the recent picks, and remember it.
pub fn pick_from(pool: &Pool, cx: &mut App) -> Option<GameId> {
    let games = pool_games(pool, cx);
    let recent: Vec<String> = cx
        .try_global::<RecentPicks>()
        .map(|r| r.0.iter().cloned().collect())
        .unwrap_or_default();
    let hidden = models(cx).library.read(cx).user.hidden.clone();
    let picked = choose(&games, &hidden, &recent, roll())?;
    remember(&picked, cx);
    Some(picked)
}

/// A random number, different on every call.
fn roll() -> u64 {
    let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
    hasher.write_u128(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos()),
    );
    hasher.finish()
}

/// Filter hidden games out of `pool` and draw one winner across the entire
/// remaining selection (installed and uninstalled alike), avoiding recent picks.
fn select_shuffle_winner(
    pool: Vec<Arc<GameInfo>>,
    hidden: &BTreeSet<String>,
    recent: &[String],
    roll: u64,
) -> Option<(Vec<Arc<GameInfo>>, Arc<GameInfo>)> {
    let visible_pool: Vec<Arc<GameInfo>> = pool
        .into_iter()
        .filter(|g| !hidden.contains(&g.id().key()))
        .collect();
    let winner_id = choose(&visible_pool, hidden, recent, roll)?;
    let winner = visible_pool.iter().find(|g| g.id() == winner_id)?.clone();
    Some((visible_pool, winner))
}

/// Pick a game for the "Surprise me" shuffle modal from the library's current
/// selection (falling back to the full library if nothing is shown), avoiding
/// hidden games and recent picks. Returns `(eligible_pool, winner)`.
pub(crate) fn pick_for_shuffle(cx: &mut App) -> Option<(Vec<Arc<GameInfo>>, Arc<GameInfo>)> {
    crate::i18n::register();
    let recent: Vec<String> = cx
        .try_global::<RecentPicks>()
        .map(|r| r.0.iter().cloned().collect())
        .unwrap_or_default();
    let (pool, hidden) = {
        let library = models(cx).library.read(cx);
        let shown = crate::view::remove::shown_games(cx)
            .filter(|g| !g.is_empty())
            .map(|g| g.to_vec())
            .unwrap_or_else(|| library.snapshot.games.clone());
        (shown, library.user.hidden.clone())
    };
    let (visible_pool, winner) = select_shuffle_winner(pool, &hidden, &recent, roll())?;
    remember(&winner.id(), cx);
    Some((visible_pool, winner))
}

/// Open the page of a random game (installed if any is), or say there is
/// nothing to pick from.
pub fn open_random_game(window: &mut Window, cx: &mut App) {
    crate::i18n::register();
    let recent: Vec<String> = cx
        .try_global::<RecentPicks>()
        .map(|r| r.0.iter().cloned().collect())
        .unwrap_or_default();
    let library = models(cx).library.read(cx);
    let picked = pick(
        &library.snapshot.games,
        &library.user.hidden,
        &recent,
        roll(),
    );
    match picked {
        Some(id) => {
            remember(&id, cx);
            navigate(Route::Game(id), cx);
        }
        None => window.push_notification(
            Notification::info(t("library.random.empty")).title(t("library.random.title")),
            cx,
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use irori_core::Runner;

    fn game(app: &str, installed: bool) -> Arc<GameInfo> {
        Arc::new(GameInfo {
            runner: Some(Runner::Gog),
            app_name: app.into(),
            is_installed: installed,
            ..Default::default()
        })
    }

    #[test]
    fn prefers_installed_games_and_skips_hidden_ones() {
        let games = [game("1", false), game("2", true), game("3", true)];
        let hidden: BTreeSet<String> = ["gog:3".to_string()].into();
        for roll in 0..10 {
            assert_eq!(
                pick(&games, &hidden, &[], roll),
                Some(GameId::new(Runner::Gog, "2"))
            );
        }
        let none_installed = [game("1", false), game("4", false)];
        let picks: BTreeSet<_> = (0..4)
            .filter_map(|r| pick(&none_installed, &BTreeSet::new(), &[], r))
            .collect();
        assert_eq!(picks.len(), 2);
        assert_eq!(pick(&[], &hidden, &[], 3), None);
        let all_hidden: BTreeSet<String> = ["gog:2".to_string(), "gog:3".to_string()].into();
        assert_eq!(
            pick(&games[1..], &all_hidden, &[], 1),
            None,
            "hidden games are never picked"
        );
    }

    #[test]
    fn recent_picks_wait_while_others_remain() {
        let games = [game("1", true), game("2", true), game("3", true)];
        let none = BTreeSet::new();
        let recent = ["gog:1".to_string(), "gog:2".to_string()];
        for roll in 0..10 {
            assert_eq!(
                choose(&games, &none, &recent, roll),
                Some(GameId::new(Runner::Gog, "3")),
                "the one game not picked lately"
            );
        }
        // Every game was picked lately: anything but the last pick.
        let all = [
            "gog:3".to_string(),
            "gog:1".to_string(),
            "gog:2".to_string(),
        ];
        for roll in 0..10 {
            assert_ne!(
                choose(&games, &none, &all, roll),
                Some(GameId::new(Runner::Gog, "2"))
            );
        }
        // One game only: it is picked again rather than nothing.
        assert_eq!(
            choose(&games[..1], &none, &["gog:1".to_string()], 7),
            Some(GameId::new(Runner::Gog, "1"))
        );
    }

    #[test]
    fn shuffle_draws_from_all_shown_games_not_only_installed() {
        let pool = vec![
            game("1", true),
            game("2", false),
            game("3", false),
            game("4", false),
        ];
        let hidden: BTreeSet<String> = ["gog:4".to_string()].into();
        let picks: BTreeSet<GameId> = (0..12)
            .filter_map(|roll| {
                let (eligible, winner) = select_shuffle_winner(pool.clone(), &hidden, &[], roll)?;
                assert_eq!(eligible.len(), 3, "hidden game excluded from reel pool");
                Some(winner.id())
            })
            .collect();
        assert_eq!(
            picks,
            [
                GameId::new(Runner::Gog, "1"),
                GameId::new(Runner::Gog, "2"),
                GameId::new(Runner::Gog, "3"),
            ]
            .into_iter()
            .collect(),
            "shuffle in All must draw across both installed and uninstalled games"
        );
        // Recent winner is skipped on the next roll while other games remain.
        let recent = ["gog:1".to_string()];
        for roll in 0..10 {
            let (_, winner) =
                select_shuffle_winner(pool.clone(), &hidden, &recent, roll).expect("winner");
            assert_ne!(winner.id(), GameId::new(Runner::Gog, "1"));
        }
    }
}
