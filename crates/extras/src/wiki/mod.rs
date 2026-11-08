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
pub use sources::{ProtonDbTier, SteamDeckCompat};

use crate::util;

/// Lifetime of a cached [`WikiInfo`].
pub const WIKI_TTL: Duration = Duration::from_secs(30 * 24 * 3600);
/// Lifetime of a cached "no umu ID" answer. Found IDs never expire.
pub const UMU_MISS_TTL: Duration = Duration::from_secs(6 * 3600);

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
}

/// [`StoreIds::hltb_id`] for a game the user said HowLongToBeat doesn't have.
pub const HLTB_NONE: &str = "none";

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
}

impl WikiInfo {
    /// Steam app ID from GamesDB, else PCGamingWiki.
    pub fn steam_app_id(&self) -> Option<&str> {
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
    let choice = ids
        .hltb_id
        .as_deref()
        .map(|id| format!("-hltb{}", irori_core::paths::sanitize(id)))
        .unwrap_or_default();
    util::cache_dir("wiki").join(format!(
        "{}-{}{choice}.json",
        ids.runner.id(),
        irori_core::paths::sanitize(&ids.app_name)
    ))
}

/// Cached info for a store entry, without network access.
pub async fn cached(ids: &StoreIds) -> Option<Arc<WikiInfo>> {
    util::cache_read::<WikiInfo>(&cache_file(ids), WIKI_TTL)
        .await
        .map(Arc::new)
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
    };
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
/// added from a folder), through the Steam app ID PCGamingWiki gives it.
pub async fn steam_art_for_title(title: &str) -> Option<(String, String)> {
    const STEAM_CDN: &str = "https://cdn.cloudflare.steamstatic.com/steam/apps";
    let game = GameInfo {
        title: title.to_string(),
        runner: Some(Runner::Sideload),
        ..Default::default()
    };
    let id = steam_app_id(&game).await?;
    Some((
        format!("{STEAM_CDN}/{id}/library_600x900_2x.jpg"),
        format!("{STEAM_CDN}/{id}/library_hero.jpg"),
    ))
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
    }
}
