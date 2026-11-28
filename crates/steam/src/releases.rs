//! Keyless Steam new releases and popular upcoming games via
//! `IStoreQueryService/Query/v1`. Requires no API key or Steam sign-in.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::api::Client;
use crate::library::lenient_u64;

/// Maximum store items requested per release bucket.
pub const MAX_RELEASES_COUNT: u32 = 40;

/// Which Steam store release bucket to query.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReleaseFeedKind {
    /// New & Trending released games (`sort = 20`, `released_only = true`).
    NewThisWeek,
    /// Popular Upcoming games (`sort = 30`, `coming_soon_only = true`).
    ComingSoon,
}

/// One game entry returned by Steam's store query for weekly releases.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct StoreReleaseItem {
    pub appid: u32,
    pub name: String,
    pub short_description: String,
    pub store_url: String,
    pub header_image: Option<String>,
    pub release_date: Option<u64>,
    pub release_label: Option<String>,
    pub is_coming_soon: bool,
    pub is_free: bool,
    pub price_formatted: Option<String>,
    pub discount_pct: Option<i32>,
    pub windows: bool,
    pub linux: bool,
    pub macos: bool,
    pub developer: String,
    pub genres: Vec<String>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct QueryEnvelope {
    response: QueryResponse,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct QueryResponse {
    store_items: Vec<RawStoreItem>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RawStoreItem {
    #[serde(deserialize_with = "lenient_u64")]
    appid: u64,
    #[serde(deserialize_with = "lenient_u64")]
    id: u64,
    name: String,
    store_url_path: String,
    is_free: Option<bool>,
    tagids: Vec<u32>,
    basic_info: RawBasicInfo,
    assets: RawAssets,
    release: RawRelease,
    platforms: RawPlatforms,
    best_purchase_option: Option<RawPurchaseOption>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RawBasicInfo {
    short_description: String,
    developers: Vec<RawNamed>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RawNamed {
    name: String,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RawAssets {
    asset_url_format: String,
    header: String,
    main_capsule: String,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RawRelease {
    #[serde(deserialize_with = "lenient_u64")]
    steam_release_date: u64,
    is_coming_soon: bool,
    custom_release_date_message: String,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RawPlatforms {
    windows: bool,
    steamos_linux: bool,
    mac: bool,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RawPurchaseOption {
    formatted_final_price: String,
    discount_pct: Option<i32>,
}

/// Map high-signal Steam tag IDs to clean human-readable genre labels.
fn tag_id_to_genre(tag_id: u32) -> Option<&'static str> {
    match tag_id {
        19 => Some("Action"),
        122 => Some("RPG"),
        9 => Some("Strategy"),
        492 => Some("Indie"),
        21 => Some("Adventure"),
        599 => Some("Simulation"),
        1663 => Some("FPS"),
        4231 => Some("Action RPG"),
        3839 | 1667 => Some("Horror"),
        1716 | 3959 => Some("Roguelike"),
        699 => Some("Racing"),
        701 => Some("Sports"),
        1664 => Some("Puzzle"),
        1625 => Some("Platformer"),
        1695 => Some("Open World"),
        1685 => Some("Co-op"),
        3859 => Some("Multiplayer"),
        1662 => Some("Survival"),
        _ => None,
    }
}

fn resolve_asset_url(assets_base: &str, assets: &RawAssets) -> Option<String> {
    let file = if !assets.header.is_empty() {
        assets.header.as_str()
    } else if !assets.main_capsule.is_empty() {
        assets.main_capsule.as_str()
    } else {
        return None;
    };
    if assets.asset_url_format.is_empty() {
        return None;
    }
    let rel = assets.asset_url_format.replace("${FILENAME}", file);
    let base = assets_base.trim_end_matches('/');
    let rel = rel.trim_start_matches('/');
    Some(format!("{base}/{rel}"))
}

impl Client {
    /// Fetch keyless New & Trending or Popular Upcoming games from Steam.
    pub async fn store_releases(
        &self,
        kind: ReleaseFeedKind,
        country: &str,
        language: &str,
        count: u32,
    ) -> Result<Vec<StoreReleaseItem>> {
        let count = count.clamp(1, MAX_RELEASES_COUNT);
        let cc = if country.trim().len() == 2 {
            country.trim().to_ascii_uppercase()
        } else {
            "US".to_string()
        };
        let lang = if language.trim().is_empty() {
            "english"
        } else {
            language.trim()
        };
        let (sort, filters) = match kind {
            ReleaseFeedKind::NewThisWeek => (
                20,
                json!({
                    "released_only": true,
                    "type_filters": { "include_games": true },
                    "content_descriptors_excluded": [1, 3, 4],
                }),
            ),
            ReleaseFeedKind::ComingSoon => (
                30,
                json!({
                    "coming_soon_only": true,
                    "type_filters": { "include_games": true },
                    "content_descriptors_excluded": [1, 3, 4],
                }),
            ),
        };
        let payload = json!({
            "query": {
                "start": 0,
                "count": count,
                "sort": sort,
                "filters": filters,
            },
            "context": {
                "language": lang,
                "country_code": cc,
                "steam_realm": 1,
            },
            "data_request": {
                "include_assets": true,
                "include_release": true,
                "include_platforms": true,
                "include_basic_info": true,
                "include_tag_count": 6,
            },
        });
        let url = format!("{}/IStoreQueryService/Query/v1/", self.endpoints().api);
        let query = [("input_json", payload.to_string())];
        let env: QueryEnvelope = self.get_json(url, &query, "Steam store releases").await?;
        let store_base = self.endpoints().store.trim_end_matches('/');
        let assets_base = self.endpoints().assets.as_str();

        let items = env
            .response
            .store_items
            .into_iter()
            .filter_map(|raw| {
                let appid_u64 = if raw.appid > 0 { raw.appid } else { raw.id };
                let appid = u32::try_from(appid_u64).ok().filter(|&n| n > 0)?;
                let name = raw.name.trim().to_string();
                if name.is_empty() {
                    return None;
                }
                let store_url = if !raw.store_url_path.trim().is_empty() {
                    format!(
                        "{store_base}/{}",
                        raw.store_url_path.trim_start_matches('/')
                    )
                } else {
                    format!("{store_base}/app/{appid}/")
                };
                let mut genres = Vec::new();
                for tag_id in raw.tagids {
                    if let Some(g) = tag_id_to_genre(tag_id)
                        && !genres.iter().any(|existing| existing == g)
                    {
                        genres.push(g.to_string());
                        if genres.len() >= 3 {
                            break;
                        }
                    }
                }
                let price_formatted = raw
                    .best_purchase_option
                    .as_ref()
                    .map(|p| p.formatted_final_price.trim().to_string())
                    .filter(|s| !s.is_empty());
                let discount_pct = raw
                    .best_purchase_option
                    .as_ref()
                    .and_then(|p| p.discount_pct)
                    .filter(|&pct| pct > 0);
                let release_label =
                    Some(raw.release.custom_release_date_message.trim().to_string())
                        .filter(|s| !s.is_empty());
                let developer = raw
                    .basic_info
                    .developers
                    .into_iter()
                    .next()
                    .map(|d| d.name)
                    .unwrap_or_default();

                Some(StoreReleaseItem {
                    appid,
                    name,
                    short_description: raw.basic_info.short_description.trim().to_string(),
                    store_url,
                    header_image: resolve_asset_url(assets_base, &raw.assets),
                    release_date: (raw.release.steam_release_date > 0)
                        .then_some(raw.release.steam_release_date),
                    release_label,
                    is_coming_soon: raw.release.is_coming_soon,
                    is_free: raw.is_free.unwrap_or(false),
                    price_formatted,
                    discount_pct,
                    windows: raw.platforms.windows,
                    linux: raw.platforms.steamos_linux,
                    macos: raw.platforms.mac,
                    developer,
                    genres,
                })
            })
            .collect();
        Ok(items)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SteamError;
    use crate::api::Endpoints;
    use irori_depot::test_server::{Resp, TestServer};

    #[tokio::test]
    async fn fetches_new_and_upcoming_releases_and_maps_fields() {
        let server = TestServer::start(|req| {
            if req.path.contains("sort%22%3A20") || req.path.contains("\"sort\":20") {
                Resp::json(json!({
                    "response": {
                        "store_items": [{
                            "appid": 1799840,
                            "name": "Queen's Domain",
                            "store_url_path": "app/1799840/Queens_Domain",
                            "tagids": [4231, 492, 19],
                            "basic_info": {
                                "short_description": "Retro-fantasy action RPG.",
                                "developers": [{"name": "Freshly Baked Games"}]
                            },
                            "assets": {
                                "asset_url_format": "steam/apps/1799840/${FILENAME}?t=123",
                                "header": "abc/header.jpg"
                            },
                            "release": { "steam_release_date": 1793379348 },
                            "platforms": { "windows": true, "steamos_linux": true, "mac": false },
                            "best_purchase_option": {
                                "formatted_final_price": "$19.99",
                                "discount_pct": 10
                            }
                        }]
                    }
                }))
            } else if req.path.contains("ZZ") {
                Resp::status(429)
            } else {
                Resp::json(json!({
                    "response": {
                        "store_items": [{
                            "appid": 1422450,
                            "name": "Deadlock",
                            "tagids": [1663, 3859],
                            "release": {
                                "is_coming_soon": true,
                                "custom_release_date_message": "To be announced"
                            },
                            "platforms": { "windows": true }
                        }]
                    }
                }))
            }
        });
        let api = Client::with_endpoints(Endpoints {
            api: server.base(),
            store: "https://store.steampowered.com".into(),
            login: server.base(),
            assets: "https://cdn.example.com".into(),
        })
        .unwrap();

        let new_games = api
            .store_releases(ReleaseFeedKind::NewThisWeek, "US", "english", 10)
            .await
            .unwrap();
        assert_eq!(new_games.len(), 1);
        assert_eq!(new_games[0].appid, 1799840);
        assert_eq!(
            new_games[0].header_image.as_deref(),
            Some("https://cdn.example.com/steam/apps/1799840/abc/header.jpg?t=123")
        );
        assert_eq!(new_games[0].price_formatted.as_deref(), Some("$19.99"));
        assert_eq!(new_games[0].discount_pct, Some(10));
        assert!(new_games[0].windows && new_games[0].linux && !new_games[0].macos);
        assert_eq!(new_games[0].genres, vec!["Action RPG", "Indie", "Action"]);

        let soon = api
            .store_releases(ReleaseFeedKind::ComingSoon, "US", "english", 10)
            .await
            .unwrap();
        assert_eq!(soon.len(), 1);
        assert!(soon[0].is_coming_soon);
        assert_eq!(soon[0].release_label.as_deref(), Some("To be announced"));

        let err = api
            .store_releases(ReleaseFeedKind::ComingSoon, "ZZ", "english", 10)
            .await
            .unwrap_err();
        assert_eq!(
            err.downcast_ref::<SteamError>(),
            Some(&SteamError::RateLimited)
        );
    }
}
