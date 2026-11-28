//! On-demand game news and patch notes for linked games and the unified library
//! feed. Never polls at startup or in the background; reads local cache first
//! and fetches fresh items only when opened or refreshed.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::Result;
use futures::stream::{self, StreamExt as _};
use irori_core::{Event, GameId, GameInfo, Paths, Runner, json_store};
use irori_steam_api::{NewsKind, SteamError, SteamNewsItem};
use once_cell::sync::Lazy;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use crate::smart_categories::{CategoryIndex, CategoryRef};
use crate::{Backend, LibrarySnapshot, SteamLink, UserData};

/// Cached Steam announcements stay fresh for 30 minutes.
pub const FRESH_FOR: Duration = Duration::from_secs(30 * 60);
/// Default announcements fetched per app on the unified feed.
const FEED_ITEMS_PER_APP: u32 = 8;
/// Default announcements fetched per app on the Game Page News tab.
const GAME_ITEMS_PER_APP: u32 = 20;
/// Maximum stale apps refreshed in one pass.
const MAX_REFRESH_APPS: usize = 30;
/// Maximum GamesDB store-ID lookups in one feed refresh.
const MAX_GAMESDB_LOOKUPS: usize = 12;
/// Maximum concurrent Steam news requests.
const REFRESH_CONCURRENCY: usize = 4;
/// Minimum spacing between starting requests in a batch.
const REQUEST_SPACING: Duration = Duration::from_millis(150);
/// Fallback cap when no games are installed or followed yet.
const RECENT_FALLBACK_CAP: usize = 15;
/// Remember GamesDB misses in memory for 7 days so refreshes stay fast.
const GAMESDB_MISS_TTL_SECS: u64 = 7 * 24 * 60 * 60;

/// Library slice shown in `Stores -> News -> My games`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum NewsScope {
    /// Installed games plus explicitly followed backlog games (minus muted).
    #[default]
    Installed,
    /// Favourited games (minus muted).
    Favorites,
    /// Any manual category or dynamic collection from [`CategoryIndex`].
    Collection(CategoryRef),
}

/// Result of loading news for one game's `News` tab.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GameNewsResult {
    pub app_id: u32,
    pub via_steam: bool,
    pub items: Vec<SteamNewsItem>,
    pub offline: bool,
    pub rate_limited: bool,
}

/// One news card in the unified `My games` feed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GameNewsEntry {
    pub game_id: GameId,
    pub title: String,
    pub store: Runner,
    pub via_steam: bool,
    pub app_id: u32,
    pub is_installed: bool,
    pub item: SteamNewsItem,
}

/// Snapshot returned to `Stores -> News -> My games`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NewsFeedSnapshot {
    pub entries: Vec<GameNewsEntry>,
    pub last_visited_at: u64,
    pub offline: bool,
    pub rate_limited: bool,
    pub total_games: usize,
    pub resolved_apps: usize,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
struct CachedAppNews {
    fetched_at: u64,
    items: Vec<SteamNewsItem>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
struct NewsVisitState {
    last_visited_at: u64,
}

/// In-memory GamesDB miss cache keyed by `GameId::key()`.
static GAMESDB_MISSES: Lazy<Mutex<HashMap<String, u64>>> = Lazy::new(|| Mutex::new(HashMap::new()));

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn cache_file(app_id: u32) -> PathBuf {
    Paths::get()
        .cache
        .join("news")
        .join(format!("steam-{app_id}.json"))
}

fn state_file() -> PathBuf {
    Paths::get().data.join("news-state.json")
}

fn read_cached_app(app_id: u32) -> Option<CachedAppNews> {
    let path = cache_file(app_id);
    match json_store::read(&path) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!("reading Steam news cache for app {app_id}: {e:#}");
            None
        }
    }
}

fn write_cached_app(app_id: u32, items: &[SteamNewsItem], fetched_at: u64) {
    let payload = CachedAppNews {
        fetched_at,
        items: items.to_vec(),
    };
    if let Err(e) = json_store::write(&cache_file(app_id), &payload) {
        tracing::warn!("saving Steam news cache for app {app_id}: {e:#}");
    }
}

fn is_fresh(cached: &CachedAppNews, now: u64) -> bool {
    now.saturating_sub(cached.fetched_at) < FRESH_FOR.as_secs()
}

fn gamesdb_supported(runner: Runner) -> bool {
    matches!(
        runner,
        Runner::Legendary | Runner::Gog | Runner::Nile | Runner::Zoom | Runner::Itch
    )
}

fn is_gamesdb_miss_fresh(key: &str, now: u64) -> bool {
    GAMESDB_MISSES
        .lock()
        .get(key)
        .is_some_and(|&at| now.saturating_sub(at) < GAMESDB_MISS_TTL_SECS)
}

fn note_gamesdb_miss(key: String, now: u64) {
    let mut guard = GAMESDB_MISSES.lock();
    if guard.len() > 2048 {
        guard.retain(|_, &mut at| now.saturating_sub(at) < GAMESDB_MISS_TTL_SECS);
    }
    guard.insert(key, now);
}

fn entry_in_set(
    library: &LibrarySnapshot,
    set: &std::collections::BTreeSet<String>,
    game: &GameInfo,
) -> bool {
    let id = game.id();
    let copies = library.copies_of(&id);
    if copies.is_empty() {
        set.contains(&id.key())
    } else {
        copies.iter().any(|c| set.contains(&c.key()))
    }
}

fn entry_in_collection(
    library: &LibrarySnapshot,
    user: &UserData,
    categories: &CategoryIndex,
    cat: &CategoryRef,
    game: &GameInfo,
) -> bool {
    match cat {
        CategoryRef::Smart(smart_id) => categories.contains(smart_id, &game.id()),
        CategoryRef::Manual(name) => user
            .categories
            .get(name)
            .is_some_and(|members| entry_in_set(library, members, game)),
    }
}

fn scope_games(
    library: &LibrarySnapshot,
    user: &UserData,
    categories: &CategoryIndex,
    scope: &NewsScope,
    hidden_locked: bool,
) -> Vec<Arc<GameInfo>> {
    let visible = |g: &&Arc<GameInfo>| {
        g.runner() != Runner::Emulation
            && !(hidden_locked && entry_in_set(library, &user.hidden, g))
            && !entry_in_set(library, &user.news_muted, g)
    };
    let mut picked: Vec<Arc<GameInfo>> = match scope {
        NewsScope::Installed => library
            .games
            .iter()
            .filter(visible)
            .filter(|g| {
                let any_installed = g.is_installed
                    || library
                        .copies_of(&g.id())
                        .iter()
                        .filter_map(|c| library.get(c))
                        .any(|c| c.is_installed);
                any_installed || entry_in_set(library, &user.news_followed, g)
            })
            .cloned()
            .collect(),
        NewsScope::Favorites => library
            .games
            .iter()
            .filter(visible)
            .filter(|g| entry_in_set(library, &user.favourites, g))
            .cloned()
            .collect(),
        NewsScope::Collection(cat) => library
            .games
            .iter()
            .filter(visible)
            .filter(|g| entry_in_collection(library, user, categories, cat, g))
            .cloned()
            .collect(),
    };

    // Click-and-Play fallback: if Installed has no games yet, fall back to recent plays.
    if matches!(scope, NewsScope::Installed) && picked.is_empty() {
        let mut seen = HashSet::new();
        for key in user.recent_keys() {
            if picked.len() >= RECENT_FALLBACK_CAP {
                break;
            }
            let Some((r, app)) = key.split_once(':') else {
                continue;
            };
            let Some(runner) = Runner::from_id(r) else {
                continue;
            };
            let id = GameId::new(runner, app);
            let Some(entry) = library.entry_of(&id) else {
                continue;
            };
            if visible(&entry) && seen.insert(entry.id()) {
                picked.push(entry.clone());
            }
        }
    }

    // Prioritize installed games, then recent playtime, then title order.
    picked.sort_by(|a, b| {
        b.is_installed
            .cmp(&a.is_installed)
            .then_with(|| {
                let pa = latest_play_ts(library, user, a);
                let pb = latest_play_ts(library, user, b);
                pb.cmp(&pa)
            })
            .then_with(|| a.title.to_lowercase().cmp(&b.title.to_lowercase()))
    });
    picked
}

fn latest_play_ts<'a>(
    library: &LibrarySnapshot,
    user: &'a UserData,
    game: &GameInfo,
) -> Option<&'a str> {
    let id = game.id();
    let copies = library.copies_of(&id);
    if copies.is_empty() {
        user.playtime
            .get(&id.key())
            .and_then(|r| r.last_played.as_deref())
    } else {
        copies
            .iter()
            .filter_map(|c| user.playtime.get(&c.key())?.last_played.as_deref())
            .max()
    }
}

/// Deduplicate games resolving to the same Steam app ID into one fetch target.
struct ResolvedTarget {
    game: Arc<GameInfo>,
    app_id: u32,
    via_steam: bool,
}

impl Backend {
    /// Synchronously resolve a game's Steam app ID from its own runner,
    /// existing SteamLink, or a sibling copy in the library.
    pub fn resolved_steam_app_id(&self, game: &GameInfo) -> Option<(u32, bool)> {
        if game.runner() == Runner::Emulation {
            return None;
        }
        let link = self.steam_link(game);
        if let Some(id) = link.app_id() {
            return Some((id, game.runner() != Runner::Steam));
        }
        if matches!(link, SteamLink::NoneChosen) {
            return None;
        }
        let lib = self.library();
        for copy_id in lib.copies_of(&game.id()) {
            let Some(copy) = lib.get(copy_id) else {
                continue;
            };
            if let Some(id) = self.steam_link(copy).app_id() {
                return Some((id, game.runner() != Runner::Steam));
            }
        }
        None
    }

    /// Resolve a non-Steam game's Steam app ID via GamesDB store ID on demand.
    async fn resolve_via_gamesdb(&self, game: &GameInfo, now: u64) -> Option<u32> {
        if !gamesdb_supported(game.runner()) || self.working_offline() || self.isolated {
            return None;
        }
        if matches!(self.steam_link(game), SteamLink::NoneChosen) {
            return None;
        }
        let key = game.id().key();
        if is_gamesdb_miss_fresh(&key, now) {
            return None;
        }
        match irori_extras::wiki::sources::gamesdb_steam_id(game.runner(), &game.app_name).await {
            Ok(Some(raw)) => {
                if let Ok(app_id) = raw.trim().parse::<u32>()
                    && app_id > 0
                {
                    self.steam_links.note_wiki(&key, Some(app_id));
                    return Some(app_id);
                }
                note_gamesdb_miss(key, now);
                None
            }
            Ok(None) => {
                note_gamesdb_miss(key, now);
                None
            }
            Err(e) => {
                tracing::debug!("GamesDB Steam lookup skipped for {key}: {e:#}");
                None
            }
        }
    }

    /// Whether a game is actively followed in News (installed, or manually followed, and not muted).
    pub fn is_news_followed(&self, game: &GameInfo) -> bool {
        let user = self.userdata.snapshot();
        let lib = self.library();
        if entry_in_set(&lib, &user.news_muted, game) {
            return false;
        }
        game.is_installed || entry_in_set(&lib, &user.news_followed, game)
    }

    /// Follow or unfollow a game in News.
    pub fn follow_news(&self, id: &GameId, on: bool) {
        self.userdata.set_news_followed(id, on);
    }

    /// Mute or unmute a game in the unified News feed.
    pub fn mute_news(&self, id: &GameId, on: bool) {
        self.userdata.set_news_muted(id, on);
    }

    /// Unix timestamp of the last time the user visited News.
    pub fn news_last_visited(&self) -> u64 {
        json_store::read_or_default::<NewsVisitState>(&state_file()).last_visited_at
    }

    /// Record the current time as the last News visit and return the previous timestamp.
    pub fn mark_news_visited(&self) -> u64 {
        let prev = self.news_last_visited();
        let now = now_unix();
        if let Err(e) = json_store::write(
            &state_file(),
            &NewsVisitState {
                last_visited_at: now,
            },
        ) {
            tracing::warn!("saving news visit timestamp: {e:#}");
        }
        prev
    }

    /// Fetch (or read cached) Steam announcements for one game's `News` tab.
    pub async fn news_for_game(
        &self,
        game: &GameInfo,
        force: bool,
    ) -> Result<Option<GameNewsResult>> {
        if !self.settings.app().news {
            return Ok(None);
        }
        let user = self.userdata.snapshot();
        let lib = self.library();
        if self.hidden_locked() && entry_in_set(&lib, &user.hidden, game) {
            return Ok(None);
        }
        let now = now_unix();
        let resolved = match self.resolved_steam_app_id(game) {
            Some(found) => Some(found),
            None => self
                .resolve_via_gamesdb(game, now)
                .await
                .map(|id| (id, game.runner() != Runner::Steam)),
        };
        let Some((app_id, via_steam)) = resolved else {
            return Ok(None);
        };

        let cached = read_cached_app(app_id);
        if !force
            && let Some(c) = cached.as_ref()
            && is_fresh(c, now)
        {
            return Ok(Some(GameNewsResult {
                app_id,
                via_steam,
                items: c.items.clone(),
                offline: false,
                rate_limited: false,
            }));
        }

        if self.working_offline() {
            return Ok(Some(GameNewsResult {
                app_id,
                via_steam,
                items: cached.map(|c| c.items).unwrap_or_default(),
                offline: true,
                rate_limited: false,
            }));
        }

        let client = self.steam_links.client()?;
        match client.app_news(app_id, GAME_ITEMS_PER_APP, 0).await {
            Ok(items) => {
                write_cached_app(app_id, &items, now);
                self.bus.emit(Event::NewsChanged);
                Ok(Some(GameNewsResult {
                    app_id,
                    via_steam,
                    items,
                    offline: false,
                    rate_limited: false,
                }))
            }
            Err(e) if e.downcast_ref::<SteamError>() == Some(&SteamError::RateLimited) => {
                Ok(Some(GameNewsResult {
                    app_id,
                    via_steam,
                    items: cached.map(|c| c.items).unwrap_or_default(),
                    offline: false,
                    rate_limited: true,
                }))
            }
            Err(e) => {
                if let Some(c) = cached {
                    tracing::warn!("refreshing Steam news for app {app_id} failed: {e:#}");
                    Ok(Some(GameNewsResult {
                        app_id,
                        via_steam,
                        items: c.items,
                        offline: false,
                        rate_limited: false,
                    }))
                } else {
                    Err(e)
                }
            }
        }
    }

    /// Build an instant zero-network feed snapshot from cached disk files.
    pub fn cached_news_feed(
        &self,
        scope: &NewsScope,
        categories: &CategoryIndex,
    ) -> NewsFeedSnapshot {
        let lib = self.library();
        let user = self.userdata.snapshot();
        let games = scope_games(&lib, &user, categories, scope, self.hidden_locked());
        let total_games = games.len();
        let mut seen_apps = HashSet::new();
        let mut targets = Vec::new();
        for game in games {
            let Some((app_id, via_steam)) = self.resolved_steam_app_id(&game) else {
                continue;
            };
            if seen_apps.insert(app_id) {
                targets.push(ResolvedTarget {
                    game,
                    app_id,
                    via_steam,
                });
            }
        }
        let resolved_apps = targets.len();
        let entries = assemble_entries(&targets);
        NewsFeedSnapshot {
            entries,
            last_visited_at: self.news_last_visited(),
            offline: self.working_offline(),
            rate_limited: false,
            total_games,
            resolved_apps,
        }
    }

    /// Refresh stale apps for `scope` with paced batching, keeping cached posts
    /// intact offline or on HTTP 429.
    pub async fn refresh_news_feed(
        &self,
        scope: &NewsScope,
        categories: &CategoryIndex,
        force: bool,
    ) -> Result<NewsFeedSnapshot> {
        let lib = self.library();
        let user = self.userdata.snapshot();
        let games = scope_games(&lib, &user, categories, scope, self.hidden_locked());
        let total_games = games.len();
        let now = now_unix();
        let offline = self.working_offline();

        let mut seen_apps = HashSet::new();
        let mut targets = Vec::new();
        let mut gamesdb_budget = MAX_GAMESDB_LOOKUPS;

        for game in games {
            let resolved = match self.resolved_steam_app_id(&game) {
                Some(found) => Some(found),
                None if !offline && gamesdb_budget > 0 && gamesdb_supported(game.runner()) => {
                    gamesdb_budget -= 1;
                    self.resolve_via_gamesdb(&game, now)
                        .await
                        .map(|id| (id, game.runner() != Runner::Steam))
                }
                None => None,
            };
            let Some((app_id, via_steam)) = resolved else {
                continue;
            };
            if seen_apps.insert(app_id) {
                targets.push(ResolvedTarget {
                    game,
                    app_id,
                    via_steam,
                });
            }
        }

        let resolved_apps = targets.len();
        let mut rate_limited = false;

        if !offline {
            let stale_apps: Vec<u32> = targets
                .iter()
                .map(|t| t.app_id)
                .filter(|&app_id| {
                    force || !read_cached_app(app_id).is_some_and(|c| is_fresh(&c, now))
                })
                .take(MAX_REFRESH_APPS)
                .collect();

            if !stale_apps.is_empty() {
                let client = self.steam_links.client()?;
                let limited = Arc::new(std::sync::atomic::AtomicBool::new(false));
                let mut stream = stream::iter(stale_apps.into_iter().enumerate())
                    .map(|(idx, app_id)| {
                        let client = client.clone();
                        let limited = limited.clone();
                        async move {
                            if idx > 0 {
                                tokio::time::sleep(
                                    REQUEST_SPACING * u32::try_from(idx.min(4)).unwrap_or(1),
                                )
                                .await;
                            }
                            if limited.load(std::sync::atomic::Ordering::Relaxed) {
                                return;
                            }
                            match client.app_news(app_id, FEED_ITEMS_PER_APP, 0).await {
                                Ok(items) => write_cached_app(app_id, &items, now),
                                Err(e)
                                    if e.downcast_ref::<SteamError>()
                                        == Some(&SteamError::RateLimited) =>
                                {
                                    limited.store(true, std::sync::atomic::Ordering::Relaxed);
                                }
                                Err(e) => {
                                    tracing::warn!(
                                        "fetching Steam news for app {app_id} failed: {e:#}"
                                    );
                                }
                            }
                        }
                    })
                    .buffer_unordered(REFRESH_CONCURRENCY);
                while stream.next().await.is_some() {}
                rate_limited = limited.load(std::sync::atomic::Ordering::Relaxed);
                self.bus.emit(Event::NewsChanged);
            }
        }

        let entries = assemble_entries(&targets);
        Ok(NewsFeedSnapshot {
            entries,
            last_visited_at: self.news_last_visited(),
            offline,
            rate_limited,
            total_games,
            resolved_apps,
        })
    }
}

fn assemble_entries(targets: &[ResolvedTarget]) -> Vec<GameNewsEntry> {
    let mut entries = Vec::new();
    for target in targets {
        let Some(cached) = read_cached_app(target.app_id) else {
            continue;
        };
        for item in cached.items {
            entries.push(GameNewsEntry {
                game_id: target.game.id(),
                title: target.game.title.clone(),
                store: target.game.runner(),
                via_steam: target.via_steam,
                app_id: target.app_id,
                is_installed: target.game.is_installed,
                item,
            });
        }
    }
    entries.sort_by(|a, b| {
        b.item
            .date
            .cmp(&a.item.date)
            .then_with(|| b.item.gid.cmp(&a.item.gid))
    });
    entries
}

/// Filter helper shared by UI views for patch notes vs general news.
pub fn matches_news_kind(kind: NewsKind, filter: Option<NewsKind>) -> bool {
    filter.is_none_or(|wanted| wanted == kind)
}

#[cfg(test)]
mod tests {
    use super::*;
    use irori_core::{GameInfo, Paths, Runner};
    use irori_depot::test_server::{Resp, TestServer};
    use serde_json::json;

    fn sample_game(runner: Runner, app: &str, title: &str, installed: bool) -> Arc<GameInfo> {
        Arc::new(GameInfo {
            runner: Some(runner),
            app_name: app.into(),
            title: title.into(),
            is_installed: installed,
            ..GameInfo::default()
        })
    }

    #[test]
    fn scope_filters_installed_favorites_collections_and_muted() {
        let g1 = sample_game(Runner::Steam, "1086940", "Baldur's Gate 3", true);
        let g2 = sample_game(Runner::Steam, "1245620", "Elden Ring", false);
        let g3 = sample_game(Runner::Steam, "1145350", "Hades II", true);

        let lib = LibrarySnapshot::new(
            1,
            crate::copies::Grouped {
                entries: vec![g1.clone(), g2.clone(), g3.clone()],
                others: Vec::new(),
                groups: Vec::new(),
            },
            HashMap::new(),
        );
        let mut user = UserData::default();
        user.favourites.insert(g2.id().key());
        user.news_muted.insert(g3.id().key());
        user.categories
            .entry("RPGs".into())
            .or_default()
            .extend([g1.id().key(), g2.id().key()]);

        let cats = CategoryIndex::default();

        let installed = scope_games(&lib, &user, &cats, &NewsScope::Installed, false);
        assert_eq!(installed.len(), 1);
        assert_eq!(installed[0].id(), g1.id());

        let favs = scope_games(&lib, &user, &cats, &NewsScope::Favorites, false);
        assert_eq!(favs.len(), 1);
        assert_eq!(favs[0].id(), g2.id());

        let col = scope_games(
            &lib,
            &user,
            &cats,
            &NewsScope::Collection(CategoryRef::Manual("RPGs".into())),
            false,
        );
        assert_eq!(col.len(), 2);

        // Hidden lock hides hidden games from every scope.
        user.hidden.insert(g1.id().key());
        let locked = scope_games(&lib, &user, &cats, &NewsScope::Installed, true);
        assert!(locked.is_empty());
    }

    #[tokio::test]
    async fn caches_and_preserves_posts_on_rate_limit() {
        let tmp = tempfile::tempdir().unwrap();
        Paths::init(Paths::under(tmp.path()));

        let server = TestServer::start(|req| {
            if req.path.contains("appid=1086940") {
                Resp::json(json!({
                    "appnews": {
                        "appid": 1086940,
                        "newsitems": [{
                            "gid": "9001",
                            "title": "Hotfix #36 Now Live!",
                            "url": "https://store.steampowered.com/news/app/1086940",
                            "is_external_url": false,
                            "author": "Larian",
                            "contents": "Fixed crash",
                            "feedlabel": "Community Announcements",
                            "date": 1730000000,
                            "feedname": "steam_community_announcements",
                            "appid": 1086940,
                            "tags": ["patchnotes"]
                        }]
                    }
                }))
            } else {
                Resp::status(429)
            }
        });

        let api = irori_steam_api::Client::with_endpoints(irori_steam_api::Endpoints {
            api: server.base(),
            store: server.base(),
            login: server.base(),
            assets: server.base(),
        })
        .unwrap();

        let items = api.app_news(1086940, 10, 0).await.unwrap();
        write_cached_app(1086940, &items, now_unix());

        let cached = read_cached_app(1086940).unwrap();
        assert_eq!(cached.items.len(), 1);
        assert_eq!(cached.items[0].gid, "9001");
        assert!(is_fresh(&cached, now_unix()));
    }
}
