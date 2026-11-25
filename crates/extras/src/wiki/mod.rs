//! Extra game information for the game page: PCGamingWiki (scores, Steam
//! and HLTB IDs, Direct3D versions, genres, release dates), HowLongToBeat
//! times, ProtonDB tier, Steam Deck compatibility, the GOG GamesDB Steam ID
//! and the umu ID.
//!
//! Results are cached on disk under `<cache>/wiki` for 30 days, keyed by
//! runner and app name (not by title, so same-titled games in different
//! stores do not share an entry). A lookup in which a source failed with a
//! network error is returned but not cached, so it is retried next time.
//! AppleGamingWiki is macOS-only in Heroic and not queried.

pub mod hltb;
pub mod pcgamingwiki;
pub mod sources;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Result, bail};
use irori_core::{GameInfo, Runner};
use once_cell::sync::Lazy;
use regex::Regex;
use serde::{Deserialize, Serialize};

pub use hltb::HowLongToBeat;
pub use pcgamingwiki::{GameScore, PcGamingWikiInfo};
pub use sources::{ProtonDbTier, SteamDeckCompat, protondb_batch};

use crate::util;

/// Lifetime of a cached [`WikiInfo`].
pub const WIKI_TTL: Duration = Duration::from_secs(30 * 24 * 3600);
/// Lifetime of a cached "no umu ID" answer. Found IDs never expire.
pub const UMU_MISS_TTL: Duration = Duration::from_secs(6 * 3600);

static STEAM_SEARCH_SEM: Lazy<tokio::sync::Semaphore> =
    Lazy::new(|| tokio::sync::Semaphore::new(2));

/// Which store entry to look up.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoreIds {
    pub runner: Runner,
    pub app_name: String,
    /// Store product page; used for GOG's HowLongToBeat box.
    pub store_url: Option<String>,
    /// The HowLongToBeat game the user chose ("Change match"): an HLTB id,
    /// or [`HLTB_NONE`] for "no match". `None` finds one on its own.
    pub hltb_id: Option<String>,
    /// The release year, to tell remakes apart in HLTB's search.
    pub release_year: Option<u32>,
    /// The Steam app the user linked the game to (`iroriSteamAppId`), or
    /// [`STEAM_NONE`] for none: it replaces the one GamesDB or PCGamingWiki
    /// give for ProtonDB and the Steam Deck. `None` uses theirs.
    pub steam_link: Option<String>,
    /// A Steam app found by the game's exact title, used only when neither
    /// GamesDB nor PCGamingWiki names one.
    pub steam_guess: Option<String>,
}

/// [`StoreIds::hltb_id`] for a game the user said HowLongToBeat doesn't have.
pub const HLTB_NONE: &str = "none";

/// [`StoreIds::steam_link`] for a game the user said Steam doesn't sell.
pub const STEAM_NONE: &str = "none";

impl StoreIds {
    pub fn of(game: &GameInfo) -> Self {
        Self {
            runner: game.runner(),
            app_name: game.app_name.clone(),
            store_url: game
                .store_url
                .clone()
                .or_else(|| game.extra.as_ref().and_then(|e| e.store_url.clone()))
                .filter(|s| !s.is_empty()),
            hltb_id: None,
            release_year: game
                .release_date
                .as_deref()
                .and_then(|d| d.get(..4))
                .and_then(|y| y.parse().ok()),
            steam_link: None,
            steam_guess: None,
        }
    }
}

/// Steam-side compatibility data.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SteamCompat {
    pub protondb: Option<ProtonDbTier>,
    pub steam_deck: Option<SteamDeckCompat>,
}

/// Everything known about a game from third-party sources. Each part is
/// `None` when the source has nothing (or was unreachable).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct WikiInfo {
    pub pcgamingwiki: Option<PcGamingWikiInfo>,
    pub howlongtobeat: Option<HowLongToBeat>,
    /// Steam app ID according to GOG GamesDB.
    pub gamesdb_steam_id: Option<String>,
    pub steam: Option<SteamCompat>,
    pub umu_id: Option<String>,
    /// The Steam app the lookup used instead of the sources' (see
    /// [`StoreIds::steam_link`]); [`STEAM_NONE`] for none.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub steam_link: Option<String>,
}

impl WikiInfo {
    /// The game's Steam app ID: the one linked, else GamesDB's, else
    /// PCGamingWiki's.
    pub fn steam_app_id(&self) -> Option<&str> {
        match self.steam_link.as_deref().map(str::trim) {
            Some(STEAM_NONE) => None,
            Some(id) if !id.is_empty() => Some(id),
            _ => self.found_steam_app_id(),
        }
    }

    /// The Steam app ID GamesDB, else PCGamingWiki, give the game, whatever
    /// was linked.
    pub fn found_steam_app_id(&self) -> Option<&str> {
        self.gamesdb_steam_id
            .as_deref()
            .filter(|s| !s.is_empty())
            .or_else(|| {
                self.pcgamingwiki
                    .as_ref()
                    .map(|p| p.steam_id.as_str())
                    .filter(|s| !s.is_empty())
            })
    }
}

static SPECIAL_CHARS: Lazy<Regex> =
    Lazy::new(|| Regex::new(r#"[:|/*?<>\\&{}%$@`!™+'"®]"#).unwrap());

/// Title with characters that break wiki searches removed (`: | / * ? < > \
/// & { } % $ @ \` ! ™ + ' " ®`).
pub fn clean_title(title: &str) -> String {
    SPECIAL_CHARS.replace_all(title, "").into_owned()
}

fn cache_file(ids: &StoreIds) -> PathBuf {
    // The user's HLTB choice is part of the name, so changing it reads the
    // game again instead of the answer cached for the old one.
    let part = |tag: &str, value: &Option<String>| {
        value
            .as_deref()
            .map(|v| format!("-{tag}{}", irori_core::paths::sanitize(v)))
            .unwrap_or_default()
    };
    let choice = format!(
        "{}{}{}",
        part("hltb", &ids.hltb_id),
        part("steam", &ids.steam_link),
        part("guess", &ids.steam_guess)
    );
    util::cache_dir("wiki").join(format!(
        "{}-{}{choice}.json",
        ids.runner.id(),
        irori_core::paths::sanitize(&ids.app_name)
    ))
}

/// Forget the cached info for a store entry (and the user's choices in
/// `ids`), so the next lookup asks every source again.
pub async fn forget(ids: &StoreIds) {
    match tokio::fs::remove_file(cache_file(ids)).await {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
            tracing::warn!("forgetting game information: {e}");
        }
        _ => {}
    }
}

/// Cached info for a store entry, without network access.
pub async fn cached(ids: &StoreIds) -> Option<Arc<WikiInfo>> {
    util::cache_read::<WikiInfo>(&cache_file(ids), WIKI_TTL)
        .await
        .filter(is_current)
        .map(Arc::new)
}

/// Whether a cached answer has every field this version reads from the
/// wiki's page (an older one is asked again).
fn is_current(info: &WikiInfo) -> bool {
    info.pcgamingwiki
        .as_ref()
        .is_none_or(|p| p.version >= pcgamingwiki::PARSE_VERSION)
}

/// Look a game up in every source (cached for 30 days). Fails only when
/// every source was unreachable.
pub async fn game_info(title: &str, ids: &StoreIds) -> Result<Arc<WikiInfo>> {
    if let Some(info) = cached(ids).await {
        return Ok(info);
    }
    let title = clean_title(title);
    let gog_id = (ids.runner == Runner::Gog).then_some(ids.app_name.as_str());
    let (pcgw, gamesdb, umu) = futures::join!(
        pcgamingwiki::fetch(&title, gog_id),
        sources::gamesdb_steam_id(ids.runner, &ids.app_name),
        umu_id(ids.runner, &ids.app_name),
    );

    let mut failures: Vec<String> = Vec::new();
    let pcgamingwiki = take("PCGamingWiki", &title, &mut failures, pcgw);
    let gamesdb_steam_id = take("GamesDB", &title, &mut failures, gamesdb);
    let umu_id = take("umu", &title, &mut failures, umu);

    let hltb = match ids.hltb_id.as_deref() {
        Some(HLTB_NONE) => Ok(None),
        Some(id) => hltb::fetch_by_id(id).await,
        None => find_hltb(&title, ids, pcgamingwiki.as_ref()).await,
    };
    let howlongtobeat = take("HowLongToBeat", &title, &mut failures, hltb);

    let mut info = WikiInfo {
        pcgamingwiki,
        howlongtobeat,
        gamesdb_steam_id,
        steam: None,
        umu_id,
        steam_link: None,
    };
    info.steam_link = ids.steam_link.clone().or_else(|| {
        info.found_steam_app_id()
            .is_none()
            .then(|| ids.steam_guess.clone())
            .flatten()
    });
    if let Some(steam_id) = info.steam_app_id().map(str::to_string) {
        let (proton, deck) =
            futures::join!(sources::protondb(&steam_id), sources::steam_deck(&steam_id));
        let protondb = take("ProtonDB", &title, &mut failures, proton);
        let steam_deck = take("Steam Deck", &title, &mut failures, deck);
        if protondb.is_some() || steam_deck.is_some() {
            info.steam = Some(SteamCompat {
                protondb,
                steam_deck,
            });
        }
    }

    let total_sources = if info.steam_app_id().is_some() { 6 } else { 4 };
    if failures.len() >= total_sources {
        bail!(
            "Game information is unavailable (no connection to {}).",
            failures.join(", ")
        );
    }
    if failures.is_empty()
        && let Err(e) = util::cache_write(&cache_file(ids), &info).await
    {
        tracing::warn!("caching game information: {e:#}");
    }
    Ok(Arc::new(info))
}

/// HowLongToBeat's times without a choice from the user: GOG's store page,
/// else the HLTB id PCGamingWiki gives, else HLTB's title search when its
/// result is the game.
async fn find_hltb(
    title: &str,
    ids: &StoreIds,
    pcgamingwiki: Option<&PcGamingWikiInfo>,
) -> Result<Option<HowLongToBeat>> {
    let found = if ids.runner == Runner::Gog
        && let Some(url) = &ids.store_url
    {
        hltb::fetch_from_gog_store(url, title).await?
    } else if let Some(id) = pcgamingwiki
        .map(|p| p.how_long_to_beat_id.as_str())
        .filter(|s| !s.is_empty())
    {
        hltb::fetch_by_id(id).await?
    } else {
        None
    };
    if found.is_some() {
        return Ok(found);
    }
    let results = hltb::search(title).await?;
    Ok(hltb::best_match(&results, title, ids.release_year).cloned())
}

/// Unwrap a source result, recording a failure.
fn take<T>(name: &str, title: &str, failures: &mut Vec<String>, r: Result<Option<T>>) -> Option<T> {
    match r {
        Ok(v) => v,
        Err(e) => {
            tracing::debug!("{name} lookup for {title} failed: {e:#}");
            failures.push(name.to_string());
            None
        }
    }
}

/// Steam app ID of a game (from cached or freshly fetched wiki data).
pub async fn steam_app_id(game: &GameInfo) -> Option<String> {
    let info = game_info(&game.title, &StoreIds::of(game)).await.ok()?;
    info.steam_app_id().map(str::to_string)
}

/// Steam's cover and hero art for a game known only by its title (one
/// added manually or from a folder), through PCGamingWiki or Steam's store
/// search and `IStoreBrowseService/GetItems/v1/` (so newer games with
/// hashed asset URLs also resolve).
pub async fn steam_art_for_title(title: &str) -> Option<(String, String)> {
    const STEAM_CDN: &str = "https://cdn.cloudflare.steamstatic.com/steam/apps";
    let game = GameInfo {
        title: title.to_string(),
        runner: Some(Runner::Sideload),
        ..Default::default()
    };
    let id = match steam_app_id(&game).await {
        Some(id) => id,
        None => steam_search_app_id(title).await.ok().flatten()?,
    };
    if let Some(hashed) = steam_store_hashed_art(&id).await {
        return Some(hashed);
    }
    Some((
        format!("{STEAM_CDN}/{id}/library_600x900_2x.jpg"),
        format!("{STEAM_CDN}/{id}/library_hero.jpg"),
    ))
}

/// Clean a game title for Steam's `storesearch` endpoint: Windows folder
/// names replace `:` with ` - ` (e.g. `Shambles - Sons of Apocalypse`),
/// which Steam's literal search returns 0 results for unless punctuation is
/// normalised to spaces.
fn steam_search_query(title: &str) -> String {
    let cleaned: String = title
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '\'' || c == '&' {
                c
            } else {
                ' '
            }
        })
        .collect();
    cleaned.split_whitespace().collect::<Vec<_>>().join(" ")
}

async fn steam_storesearch_items(query: &str) -> Result<Vec<serde_json::Value>> {
    if query.is_empty() {
        return Ok(Vec::new());
    }
    let _permit = STEAM_SEARCH_SEM.acquire().await;
    let mut url = url::Url::parse("https://store.steampowered.com/api/storesearch/")?;
    url.query_pairs_mut()
        .append_pair("term", query)
        .append_pair("l", "english")
        .append_pair("cc", "US");
    let json: serde_json::Value =
        util::get_json(url.as_str(), std::time::Duration::from_secs(6)).await?;
    Ok(json
        .get("items")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default())
}

/// Find a game's Steam app ID via Steam's public `storesearch` endpoint.
async fn steam_search_app_id(title: &str) -> Result<Option<String>> {
    let query = steam_search_query(title);
    if query.is_empty() {
        return Ok(None);
    }
    let mut items = steam_storesearch_items(&query).await?;
    if items.is_empty()
        && let Some((prefix, _)) = title.split_once(" - ").or_else(|| title.split_once(": "))
    {
        let prefix_query = steam_search_query(prefix);
        if !prefix_query.is_empty() && prefix_query != query {
            items = steam_storesearch_items(&prefix_query).await?;
        }
    }
    let squash = |s: &str| -> String {
        s.chars()
            .filter(|c| c.is_alphanumeric())
            .flat_map(char::to_lowercase)
            .collect()
    };
    let wanted = squash(title);
    if wanted.is_empty() {
        return Ok(None);
    }
    let picked = items
        .iter()
        .find(|item| {
            item.get("name")
                .and_then(|n| n.as_str())
                .is_some_and(|n| squash(n) == wanted)
        })
        .or_else(|| {
            items.iter().find(|item| {
                item.get("name").and_then(|n| n.as_str()).is_some_and(|n| {
                    let s = squash(n);
                    let lower = n.to_ascii_lowercase();
                    !lower.contains("soundtrack")
                        && !lower.contains(" demo")
                        && (s.contains(&wanted) || wanted.contains(&s))
                })
            })
        });
    Ok(picked.and_then(|v| {
        v.get("id")
            .and_then(|v| v.as_u64())
            .map(|id| id.to_string())
    }))
}

/// Resolve the hashed library capsule and hero URLs of a Steam app from
/// `IStoreBrowseService/GetItems/v1/`.
async fn steam_store_hashed_art(app_id: &str) -> Option<(String, String)> {
    const ASSETS_BASE: &str = "https://shared.akamai.steamstatic.com/store_item_assets";
    let id: u64 = app_id.parse().ok()?;
    let input = serde_json::json!({
        "ids": [{"appid": id}],
        "context": {"language": "english", "country_code": "US"},
        "data_request": {"include_assets": true}
    });
    let mut url =
        url::Url::parse("https://api.steampowered.com/IStoreBrowseService/GetItems/v1/").ok()?;
    url.query_pairs_mut()
        .append_pair("input_json", &input.to_string());
    let json: serde_json::Value = util::get_json(url.as_str(), std::time::Duration::from_secs(6))
        .await
        .ok()?;
    let assets = json.pointer("/response/store_items/0/assets")?;
    let fmt = assets.get("asset_url_format")?.as_str()?;
    let build = |key_2x: &str, key_1x: &str| -> Option<String> {
        let file = assets
            .get(key_2x)
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .or_else(|| {
                assets
                    .get(key_1x)
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.is_empty())
            })?;
        let rel = fmt.replace("${FILENAME}", file);
        Some(format!("{ASSETS_BASE}/{}", rel.trim_start_matches('/')))
    };
    let cover = build("library_capsule_2x", "library_capsule")?;
    let hero = build("library_hero_2x", "library_hero")
        .or_else(|| build("header_2x", "header"))
        .unwrap_or_else(|| cover.clone());
    Some((cover, hero))
}

/// umu ID of a store game, for `GAMEID`. Found IDs are cached forever;
/// "not found" answers for 6 hours. Sideloaded apps have none.
pub async fn umu_id(runner: Runner, app_name: &str) -> Result<Option<String>> {
    if runner == Runner::Sideload {
        return Ok(None);
    }
    let path = util::cache_dir("wiki").join(format!(
        "umu-{}-{}.json",
        runner.id(),
        irori_core::paths::sanitize(app_name)
    ));
    match util::cache_read_entry::<Option<String>>(&path).await {
        Some((Some(id), _)) => return Ok(Some(id)),
        Some((None, age)) if age <= UMU_MISS_TTL => return Ok(None),
        _ => {}
    }
    let id = sources::umu_lookup(runner, app_name).await?;
    if let Err(e) = util::cache_write(&path, &id).await {
        tracing::warn!("caching umu ID: {e:#}");
    }
    Ok(id)
}

/// Input item for batch ProtonDB resolution across store deals and giveaways.
#[derive(Debug, Clone)]
pub struct ProtonDbLookup {
    pub key: String,
    pub title: String,
    pub gog_id: Option<String>,
    pub steam_app_id: Option<String>,
}

/// Batch output mapping deal keys to resolved Steam app IDs and those IDs to
/// non-pending ProtonDB compatibility tiers, plus keys that failed transiently.
#[derive(Debug, Default)]
pub struct ProtonDbBatchResult {
    pub resolved_steam_ids: HashMap<String, String>,
    pub tiers: HashMap<String, ProtonDbTier>,
    pub retry_keys: Vec<String>,
}

const EDITION_SUFFIXES: &[&str] = &[
    "(steam) key",
    "(gog) key",
    "(epic) key",
    "steam key",
    "game of the year edition deluxe",
    "game of the year edition",
    "goty edition",
    "digital deluxe edition",
    "collector's edition",
    "collectors edition",
    "legendary edition",
    "anniversary edition",
    "definitive edition",
    "enhanced edition",
    "ultimate edition",
    "complete edition",
    "standard edition",
    "premium edition",
    "special edition",
    "master edition",
    "royal edition",
    "deluxe edition",
    "gold edition",
    "director's cut",
    "directors cut",
    "complete bundle",
    "franchise bundle",
    "complete collection",
    "digital deluxe",
    "remastered",
    "collection",
    "franchise",
    "enhanced",
    "ultimate",
    "complete",
    "legendary",
    "deluxe",
    "bundle",
    "classic",
    "pack",
    "goty",
    "gold",
    "(steam)",
    "(pc)",
    "- pc",
    "steam",
    "key",
];

pub fn strip_edition_suffix(title: &str) -> String {
    let mut s = title.trim().to_string();
    loop {
        let trimmed = s
            .trim_end_matches([' ', '-', ':', '–', '—', '™', '®'])
            .trim();
        let lower = trimmed.to_ascii_lowercase();
        let matched = EDITION_SUFFIXES
            .iter()
            .find(|&&suf| {
                if !lower.ends_with(suf) {
                    return false;
                }
                let idx = trimmed.len() - suf.len();
                idx == 0
                    || !trimmed[..idx]
                        .chars()
                        .next_back()
                        .is_some_and(char::is_alphanumeric)
            })
            .copied();
        let Some(suf) = matched else {
            return trimmed.to_string();
        };
        let next = trimmed[..trimmed.len() - suf.len()]
            .trim_end_matches([' ', '-', ':', '–', '—', '™', '®'])
            .trim();
        if next.is_empty() {
            return trimmed.to_string();
        }
        s = next.to_string();
    }
}

fn base_game_candidates(title: &str) -> Vec<String> {
    let cleaned = clean_title(title);
    let mut out = Vec::new();
    let stripped = strip_edition_suffix(&cleaned);
    if !stripped.is_empty() && stripped != cleaned {
        out.push(stripped);
    }
    if let Some((prefix, _)) = cleaned.split_once(" - ") {
        let p = prefix.trim();
        if p.chars().count() >= 4 && p != cleaned && !out.iter().any(|existing| existing == p) {
            out.push(p.to_string());
        }
    }
    out
}

async fn resolve_deal_steam_app_id(title: &str, gog_id: Option<&str>) -> Result<Option<String>> {
    let key = match gog_id {
        Some(gid) => format!("gog-{gid}"),
        None => {
            let cleaned = clean_title(title).to_ascii_lowercase();
            let stripped = strip_edition_suffix(&cleaned);
            if stripped.is_empty() {
                cleaned
            } else {
                stripped
            }
        }
    };
    let path =
        util::cache_dir("steam-ids-v2").join(format!("{}.json", irori_core::paths::sanitize(&key)));
    match util::cache_read_entry::<Option<String>>(&path).await {
        Some((Some(id), _)) => return Ok(Some(id)),
        Some((None, age)) if age <= UMU_MISS_TTL => return Ok(None),
        _ => {}
    }

    let mut had_transient_error = false;
    let mut resolved = if let Some(gid) = gog_id {
        match sources::gamesdb_steam_id(Runner::Gog, gid).await {
            Ok(v) => v,
            Err(e) => {
                tracing::debug!("GamesDB lookup for GOG {gid} failed: {e:#}");
                had_transient_error = true;
                None
            }
        }
    } else {
        None
    };

    if resolved.is_none() {
        let cleaned = clean_title(title);
        resolved = steam_search_app_id(&cleaned).await?;
        if resolved.is_none() {
            let stripped = strip_edition_suffix(&cleaned);
            if !stripped.is_empty() && stripped != cleaned {
                resolved = steam_search_app_id(&stripped).await?;
            }
        }
    }

    if (resolved.is_some() || !had_transient_error)
        && let Err(e) = util::cache_write(&path, &resolved).await
    {
        tracing::debug!("caching deal Steam app ID for {key}: {e}");
    }
    Ok(resolved)
}

enum ResolveOutcome {
    Found(String, String, Option<ProtonDbTier>),
    Miss,
    Retry(String),
}

async fn protondb_tier_valid(id: &str) -> Option<ProtonDbTier> {
    sources::protondb_cached(id)
        .await
        .ok()
        .flatten()
        .filter(|t| !matches!(t, ProtonDbTier::Unknown | ProtonDbTier::Pending))
}

/// Resolve Steam app IDs (for non-Steam deals/giveaways via GOG GamesDB or
/// Steam Store Search, cached on disk) and fetch ProtonDB tiers with bounded
/// concurrency.
pub async fn resolve_protondb_batch(items: Vec<ProtonDbLookup>) -> ProtonDbBatchResult {
    use futures::stream::{self, StreamExt as _};

    let outcomes: Vec<ResolveOutcome> = stream::iter(items)
        .map(|item| async move {
            let had_explicit_id = item.steam_app_id.as_deref().is_some_and(|s| {
                !s.trim().is_empty() && s.trim().chars().all(|c| c.is_ascii_digit())
            });
            let mut id = if had_explicit_id {
                item.steam_app_id.clone().unwrap()
            } else {
                match resolve_deal_steam_app_id(&item.title, item.gog_id.as_deref()).await {
                    Ok(Some(id)) => id,
                    Ok(None) => return ResolveOutcome::Miss,
                    Err(e) => {
                        tracing::debug!(
                            "Steam app ID lookup for {} failed transiently: {e:#}",
                            item.title
                        );
                        return ResolveOutcome::Retry(item.key);
                    }
                }
            };
            let mut tier = protondb_tier_valid(&id).await;
            if tier.is_none() && !had_explicit_id {
                for cand in base_game_candidates(&item.title) {
                    if let Ok(Some(base_id)) = resolve_deal_steam_app_id(&cand, None).await
                        && base_id != id
                        && let Some(base_tier) = protondb_tier_valid(&base_id).await
                    {
                        id = base_id;
                        tier = Some(base_tier);
                        break;
                    }
                }
            }
            ResolveOutcome::Found(item.key, id, tier)
        })
        .buffer_unordered(4)
        .collect()
        .await;

    let mut out = ProtonDbBatchResult::default();
    for outcome in outcomes {
        match outcome {
            ResolveOutcome::Found(key, id, tier) => {
                if let Some(t) = tier {
                    out.tiers.insert(id.clone(), t);
                }
                out.resolved_steam_ids.insert(key, id);
            }
            ResolveOutcome::Miss => {}
            ResolveOutcome::Retry(key) => out.retry_keys.push(key),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn title_cleaning() {
        assert_eq!(
            clean_title("Tom Clancy's: R6™ <Siege>!"),
            "Tom Clancys R6 Siege"
        );
        assert_eq!(
            clean_title("S.T.A.L.K.E.R. - Shadow"),
            "S.T.A.L.K.E.R. - Shadow"
        );
    }

    #[test]
    fn steam_id_preference() {
        let mut info = WikiInfo {
            pcgamingwiki: Some(PcGamingWikiInfo {
                steam_id: "2".into(),
                ..Default::default()
            }),
            ..Default::default()
        };
        assert_eq!(info.steam_app_id(), Some("2"));
        info.gamesdb_steam_id = Some("1".into());
        assert_eq!(info.steam_app_id(), Some("1"));
        info.gamesdb_steam_id = Some(String::new());
        assert_eq!(info.steam_app_id(), Some("2"));
        // The user's link wins, and "none" means none.
        info.steam_link = Some("620".into());
        assert_eq!(info.steam_app_id(), Some("620"));
        assert_eq!(info.found_steam_app_id(), Some("2"));
        info.steam_link = Some(STEAM_NONE.into());
        assert_eq!(info.steam_app_id(), None);
    }

    #[test]
    fn answers_read_before_pc_facts_are_asked_again() {
        let old: WikiInfo =
            serde_json::from_value(serde_json::json!({"pcgamingwiki": {"page_id": 1}})).unwrap();
        assert!(!is_current(&old));
        let new = WikiInfo {
            pcgamingwiki: Some(pcgamingwiki::parse_wikitext(1, "")),
            ..Default::default()
        };
        assert!(is_current(&new));
        assert!(is_current(&WikiInfo::default()), "no page at all");
    }

    #[test]
    fn wiki_info_json_roundtrip() {
        let info = WikiInfo {
            steam: Some(SteamCompat {
                protondb: Some(ProtonDbTier::Gold),
                steam_deck: Some(SteamDeckCompat::Playable),
            }),
            umu_id: Some("umu-1".into()),
            ..Default::default()
        };
        let json = serde_json::to_value(&info).unwrap();
        assert_eq!(json["steam"]["protondb"], "gold");
        assert_eq!(json["steam"]["steam_deck"], "playable");
        assert_eq!(serde_json::from_value::<WikiInfo>(json).unwrap(), info);
    }

    #[test]
    fn store_ids_from_game() {
        let game = GameInfo {
            runner: Some(Runner::Gog),
            app_name: "1".into(),
            extra: Some(irori_core::ExtraInfo {
                store_url: Some("https://www.gog.com/game/x".into()),
                ..Default::default()
            }),
            ..Default::default()
        };
        let ids = StoreIds::of(&game);
        assert_eq!(ids.store_url.as_deref(), Some("https://www.gog.com/game/x"));
        assert!(cache_file(&ids).ends_with("wiki/gog-1.json"));
        // Each choice reads its own answer.
        let linked = StoreIds {
            steam_link: Some("620".into()),
            ..ids.clone()
        };
        assert!(cache_file(&linked).ends_with("wiki/gog-1-steam620.json"));
        let guessed = StoreIds {
            steam_guess: Some("400".into()),
            ..ids
        };
        assert!(cache_file(&guessed).ends_with("wiki/gog-1-guess400.json"));
    }

    #[test]
    fn steam_search_normalizes_windows_folder_separators() {
        assert_eq!(
            steam_search_query("Shambles - Sons of Apocalypse"),
            "Shambles Sons of Apocalypse"
        );
    }

    #[test]
    fn strips_edition_and_key_suffixes() {
        assert_eq!(
            strip_edition_suffix("Fallout 4: Game of the Year Edition"),
            "Fallout 4"
        );
        assert_eq!(
            strip_edition_suffix("The Elder Scrolls III: Morrowind GOTY Edition"),
            "The Elder Scrolls III: Morrowind"
        );
        assert_eq!(
            strip_edition_suffix("Dwarven Realms (Steam) Key"),
            "Dwarven Realms"
        );
        assert_eq!(
            strip_edition_suffix("The Elder Scrolls V: Skyrim Special Edition"),
            "The Elder Scrolls V: Skyrim"
        );
        assert_eq!(
            strip_edition_suffix(
                "The Elder Scrolls IV: Oblivion - Game of the Year Edition Deluxe"
            ),
            "The Elder Scrolls IV: Oblivion"
        );
        assert_eq!(
            strip_edition_suffix("Heroes of Might and Magic 3: Complete"),
            "Heroes of Might and Magic 3"
        );
        assert_eq!(
            strip_edition_suffix("The Wardrobe - Complete Bundle"),
            "The Wardrobe"
        );
        assert_eq!(strip_edition_suffix("Fallout Franchise Bundle"), "Fallout");
    }
}
