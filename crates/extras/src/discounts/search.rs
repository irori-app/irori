//! Steam Specials, Steam Store Search, GOG Search, CheapShark global deal
//! search (~30 official stores without an API key), and regional price formatting.

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use anyhow::{Context as _, Result};
use serde::{Deserialize, Deserializer, Serialize};

use super::{
    CatalogPrice, CatalogProduct, DealStore, DealsRegion, Money, dedupe, format_money,
    gog_catalog_url, user_agent,
};
use crate::util;

const STEAM_FEATURED_URL: &str = "https://store.steampowered.com/api/featuredcategories";
const STEAM_QUERY_URL: &str = "https://api.steampowered.com/IStoreQueryService/Query/v1/";
const STEAM_ASSETS_BASE: &str = "https://shared.fastly.steamstatic.com/store_item_assets";
const STEAM_SEARCH_URL: &str = "https://store.steampowered.com/api/storesearch/";
const CHEAPSHARK_STORES_URL: &str = "https://www.cheapshark.com/api/1.0/stores";
const CHEAPSHARK_DEALS_URL: &str = "https://www.cheapshark.com/api/1.0/deals";
const STEAM_CACHE_TTL: Duration = Duration::from_secs(3600);
const STORES_CACHE_TTL: Duration = Duration::from_secs(24 * 3600);
const GMG_CURATED_TTL: Duration = Duration::from_secs(6 * 3600);

#[derive(Deserialize)]
struct SteamFeatured {
    specials: Option<SteamCategory>,
    top_sellers: Option<SteamCategory>,
}

#[derive(Deserialize)]
struct SteamCategory {
    #[serde(default)]
    items: Vec<SteamFeaturedItem>,
}

#[derive(Deserialize)]
struct SteamFeaturedItem {
    id: u64,
    #[serde(default)]
    name: String,
    #[serde(default)]
    discounted: bool,
    #[serde(default)]
    discount_percent: u32,
    #[serde(default)]
    original_price: Option<u64>,
    #[serde(default)]
    final_price: Option<u64>,
    #[serde(default)]
    currency: Option<String>,
    #[serde(default)]
    large_capsule_image: Option<String>,
    #[serde(default)]
    header_image: Option<String>,
    #[serde(default)]
    windows_available: bool,
    #[serde(default)]
    mac_available: bool,
    #[serde(default)]
    linux_available: bool,
}

fn steam_os_list(win: bool, mac: bool, linux: bool) -> Vec<String> {
    let mut os = Vec::new();
    if win {
        os.push("windows".into());
    }
    if mac {
        os.push("osx".into());
    }
    if linux {
        os.push("linux".into());
    }
    os
}

/// Parse Steam's `/api/featuredcategories` JSON payload into normalized deals,
/// combining `specials` with discounted `top_sellers`.
pub fn parse_steam_featured(bytes: &[u8], default_currency: &str) -> Result<Vec<CatalogProduct>> {
    let parsed: SteamFeatured =
        serde_json::from_slice(bytes).context("parsing Steam featuredcategories")?;
    let items = parsed
        .specials
        .into_iter()
        .chain(parsed.top_sellers)
        .flat_map(|c| c.items);
    let mut out = Vec::new();
    for item in items {
        if (!item.discounted && item.discount_percent == 0) || item.name.trim().is_empty() {
            continue;
        }
        let Some(final_cents) = item.final_price else {
            continue;
        };
        let orig_cents = item.original_price.unwrap_or(final_cents);
        let currency = item
            .currency
            .as_deref()
            .filter(|c| !c.trim().is_empty())
            .unwrap_or(default_currency)
            .to_uppercase();
        let final_amt = final_cents as f64 / 100.0;
        let base_amt = orig_cents as f64 / 100.0;
        let app_id = item.id.to_string();
        out.push(CatalogProduct {
            id: format!("steam-{app_id}"),
            title: item.name,
            store: DealStore::Steam,
            store_name: None,
            steam_app_id: Some(app_id.clone()),
            deal_rating: None,
            cover_horizontal: item.header_image.or(item.large_capsule_image),
            price: CatalogPrice {
                r#final: format_money(final_amt, &currency),
                base: if orig_cents > final_cents {
                    format_money(base_amt, &currency)
                } else {
                    String::new()
                },
                discount: if item.discount_percent > 0 {
                    format!("-{}%", item.discount_percent)
                } else {
                    String::new()
                },
                final_money: Some(Money {
                    amount: format!("{final_amt:.2}"),
                    currency: currency.clone(),
                }),
                base_money: (orig_cents > final_cents).then(|| Money {
                    amount: format!("{base_amt:.2}"),
                    currency,
                }),
            },
            product_type: "game".into(),
            store_link: format!("https://store.steampowered.com/app/{app_id}/"),
            operating_systems: steam_os_list(
                item.windows_available,
                item.mac_available,
                item.linux_available,
            ),
            ..Default::default()
        });
    }
    Ok(dedupe(out))
}

#[derive(Deserialize)]
struct SteamQueryResponse {
    response: Option<SteamQueryBody>,
}

#[derive(Deserialize)]
struct SteamQueryBody {
    #[serde(default)]
    store_items: Vec<SteamQueryItem>,
}

#[derive(Deserialize)]
struct SteamQueryItem {
    #[serde(default)]
    appid: u64,
    #[serde(default)]
    name: String,
    best_purchase_option: Option<SteamQueryPurchaseOption>,
    assets: Option<SteamQueryAssets>,
    platforms: Option<SteamQueryPlatforms>,
}

fn opt_u64_or_string<'de, D: Deserializer<'de>>(d: D) -> Result<Option<u64>, D::Error> {
    Ok(match serde_json::Value::deserialize(d)? {
        serde_json::Value::Number(n) => n.as_u64(),
        serde_json::Value::String(s) => s.trim().parse().ok(),
        _ => None,
    })
}

#[derive(Deserialize)]
struct SteamQueryPurchaseOption {
    #[serde(default, deserialize_with = "opt_u64_or_string")]
    final_price_in_cents: Option<u64>,
    #[serde(default, deserialize_with = "opt_u64_or_string")]
    original_price_in_cents: Option<u64>,
    #[serde(default)]
    discount_pct: u32,
}

#[derive(Deserialize)]
struct SteamQueryAssets {
    #[serde(default)]
    asset_url_format: String,
    #[serde(default)]
    header: String,
    #[serde(default)]
    main_capsule: String,
}

#[derive(Deserialize)]
struct SteamQueryPlatforms {
    #[serde(default)]
    windows: bool,
    #[serde(default)]
    mac: bool,
    #[serde(default)]
    steamos_linux: bool,
}

fn steam_query_cover(app_id: &str, assets: Option<&SteamQueryAssets>) -> String {
    if let Some(a) = assets
        && !a.asset_url_format.trim().is_empty()
    {
        let file = if !a.header.trim().is_empty() {
            a.header.trim()
        } else {
            a.main_capsule.trim()
        };
        if !file.is_empty() {
            let rel = a.asset_url_format.replace("${FILENAME}", file);
            return format!("{STEAM_ASSETS_BASE}/{}", rel.trim_start_matches('/'));
        }
    }
    format!("{STEAM_ASSETS_BASE}/steam/apps/{app_id}/header.jpg")
}

/// Parse Steam `IStoreQueryService/Query/v1/` JSON response into discounted games.
pub fn parse_steam_query_discounts(
    bytes: &[u8],
    default_currency: &str,
) -> Result<Vec<CatalogProduct>> {
    let parsed: SteamQueryResponse =
        serde_json::from_slice(bytes).context("parsing Steam IStoreQueryService")?;
    let items = parsed.response.map(|r| r.store_items).unwrap_or_default();
    let currency = default_currency.to_uppercase();
    let mut out = Vec::new();
    for item in items {
        if item.appid == 0 || item.name.trim().is_empty() {
            continue;
        }
        let Some(opt) = item.best_purchase_option else {
            continue;
        };
        if opt.discount_pct == 0 {
            continue;
        }
        let Some(final_cents) = opt.final_price_in_cents else {
            continue;
        };
        let orig_cents = opt.original_price_in_cents.unwrap_or(final_cents);
        let final_amt = final_cents as f64 / 100.0;
        let base_amt = orig_cents as f64 / 100.0;
        let app_id = item.appid.to_string();
        let cover = steam_query_cover(&app_id, item.assets.as_ref());
        let os = item
            .platforms
            .as_ref()
            .map(|p| steam_os_list(p.windows, p.mac, p.steamos_linux))
            .unwrap_or_default();
        out.push(CatalogProduct {
            id: format!("steam-{app_id}"),
            title: item.name,
            store: DealStore::Steam,
            store_name: None,
            steam_app_id: Some(app_id.clone()),
            deal_rating: None,
            cover_horizontal: Some(cover),
            price: CatalogPrice {
                r#final: format_money(final_amt, &currency),
                base: if orig_cents > final_cents {
                    format_money(base_amt, &currency)
                } else {
                    String::new()
                },
                discount: format!("-{}%", opt.discount_pct),
                final_money: Some(Money {
                    amount: format!("{final_amt:.2}"),
                    currency: currency.clone(),
                }),
                base_money: (orig_cents > final_cents).then(|| Money {
                    amount: format!("{base_amt:.2}"),
                    currency: currency.clone(),
                }),
            },
            product_type: "game".into(),
            store_link: format!("https://store.steampowered.com/app/{app_id}/"),
            operating_systems: os,
            ..Default::default()
        });
    }
    Ok(dedupe(out))
}

async fn fetch_steam_query_discounts(region: &DealsRegion) -> Result<Vec<CatalogProduct>> {
    let input = serde_json::json!({
        "query": {
            "start": 0,
            "count": 100,
            "sort": 10,
            "filters": {
                "released_only": true,
                "type_filters": { "include_games": true },
                "price_filters": { "min_discount_percent": 1 }
            }
        },
        "context": {
            "language": "english",
            "country_code": region.store_country,
            "steam_realm": 1
        },
        "data_request": {
            "include_basic_info": true,
            "include_assets": true,
            "include_platforms": true
        }
    });
    let url = format!(
        "{STEAM_QUERY_URL}?input_json={}",
        util::encode_component(&input.to_string())
    );
    let resp = irori_core::http::client()
        .get(&url)
        .header(reqwest::header::USER_AGENT, user_agent())
        .timeout(Duration::from_secs(15))
        .send()
        .await?
        .error_for_status()?;
    let bytes = resp.bytes().await?;
    parse_steam_query_discounts(&bytes, region.currency_code)
}

/// Steam featured regional specials (no API key required).
/// Uses `IStoreQueryService/Query/v1/` (~100 top-selling discounted games) and
/// falls back to `/api/featuredcategories` if the query API fails or is empty.
pub async fn steam_discounts(region: &DealsRegion) -> Result<Vec<CatalogProduct>> {
    let cache = util::cache_dir("discounts").join(format!("steam-query-{}.json", region.code));
    if let Some(hit) = util::cache_read::<Vec<CatalogProduct>>(&cache, STEAM_CACHE_TTL).await {
        return Ok(hit);
    }
    match fetch_steam_query_discounts(region).await {
        Ok(deals) if !deals.is_empty() => {
            if let Err(e) = util::cache_write(&cache, &deals).await {
                tracing::debug!("caching Steam deals: {e}");
            }
            return Ok(deals);
        }
        Ok(_) => {
            tracing::debug!("Steam IStoreQueryService returned empty; using featuredcategories");
        }
        Err(e) => {
            tracing::warn!(
                "Steam IStoreQueryService failed ({e:#}); falling back to featuredcategories"
            );
        }
    }
    let url = format!(
        "{STEAM_FEATURED_URL}?cc={}&l=english",
        util::encode_component(region.store_country)
    );
    let resp = irori_core::http::client()
        .get(&url)
        .header(reqwest::header::USER_AGENT, user_agent())
        .timeout(Duration::from_secs(15))
        .send()
        .await?
        .error_for_status()?;
    let bytes = resp.bytes().await?;
    parse_steam_featured(&bytes, region.currency_code)
}

#[derive(Deserialize)]
struct SteamSearchResponse {
    #[serde(default)]
    items: Vec<SteamSearchItem>,
}

#[derive(Deserialize)]
struct SteamSearchItem {
    id: u64,
    #[serde(default)]
    name: String,
    #[serde(default)]
    tiny_image: Option<String>,
    #[serde(default)]
    price: Option<SteamSearchPrice>,
    #[serde(default)]
    platforms: Option<SteamSearchPlatforms>,
}

#[derive(Deserialize)]
struct SteamSearchPrice {
    #[serde(default)]
    currency: String,
    #[serde(default)]
    initial: u64,
    #[serde(default)]
    final_price_alias: Option<u64>,
    #[serde(rename = "final", default)]
    r#final: u64,
    #[serde(default)]
    discount_percent: u32,
}

#[derive(Deserialize)]
struct SteamSearchPlatforms {
    #[serde(default)]
    windows: bool,
    #[serde(default)]
    mac: bool,
    #[serde(default)]
    linux: bool,
}

/// Parse Steam `/api/storesearch` response.
pub fn parse_steam_search(bytes: &[u8], default_currency: &str) -> Result<Vec<CatalogProduct>> {
    let parsed: SteamSearchResponse =
        serde_json::from_slice(bytes).context("parsing Steam storesearch")?;
    let mut out = Vec::new();
    for item in parsed.items {
        let Some(p) = item.price else {
            continue;
        };
        let final_cents = p.final_price_alias.unwrap_or(p.r#final);
        let currency = if p.currency.trim().is_empty() {
            default_currency.to_string()
        } else {
            p.currency.to_uppercase()
        };
        let final_amt = final_cents as f64 / 100.0;
        let base_amt = p.initial as f64 / 100.0;
        let discount_pct = if p.discount_percent > 0 {
            p.discount_percent
        } else if p.initial > final_cents && p.initial > 0 {
            ((1.0 - final_amt / base_amt) * 100.0).round() as u32
        } else {
            0
        };
        let app_id = item.id.to_string();
        let cover = format!(
            "https://shared.fastly.steamstatic.com/store_item_assets/steam/apps/{app_id}/header.jpg"
        );
        let os = item
            .platforms
            .as_ref()
            .map(|pl| steam_os_list(pl.windows, pl.mac, pl.linux))
            .unwrap_or_default();
        out.push(CatalogProduct {
            id: format!("steam-{app_id}"),
            title: item.name,
            store: DealStore::Steam,
            store_name: None,
            steam_app_id: Some(app_id.clone()),
            deal_rating: None,
            cover_horizontal: Some(cover).or(item.tiny_image),
            price: CatalogPrice {
                r#final: format_money(final_amt, &currency),
                base: if p.initial > final_cents {
                    format_money(base_amt, &currency)
                } else {
                    String::new()
                },
                discount: if discount_pct > 0 {
                    format!("-{discount_pct}%")
                } else {
                    String::new()
                },
                final_money: Some(Money {
                    amount: format!("{final_amt:.2}"),
                    currency: currency.clone(),
                }),
                base_money: (p.initial > final_cents).then(|| Money {
                    amount: format!("{base_amt:.2}"),
                    currency,
                }),
            },
            product_type: "game".into(),
            store_link: format!("https://store.steampowered.com/app/{app_id}/"),
            operating_systems: os,
            ..Default::default()
        });
    }
    Ok(dedupe(out))
}

/// Live regional search on Steam Store.
pub async fn steam_search(query: &str, region: &DealsRegion) -> Result<Vec<CatalogProduct>> {
    let url = format!(
        "{STEAM_SEARCH_URL}?term={}&l=english&cc={}",
        util::encode_component(query.trim()),
        util::encode_component(region.store_country)
    );
    let resp = irori_core::http::client()
        .get(&url)
        .header(reqwest::header::USER_AGENT, user_agent())
        .timeout(Duration::from_secs(12))
        .send()
        .await?
        .error_for_status()?;
    let bytes = resp.bytes().await?;
    parse_steam_search(&bytes, region.currency_code)
}

/// Live regional search on GOG's catalog.
pub async fn gog_search(query: &str, region: &DealsRegion) -> Result<Vec<CatalogProduct>> {
    let locale = super::CatalogLocale::from(region);
    let url = gog_catalog_url(&locale, 1, false, Some(query.trim()), 12);
    let page = super::fetch_page(&url, None).await?;
    Ok(dedupe(page.products))
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct CheapSharkStoreInfo {
    #[serde(rename = "storeID", default)]
    store_id: String,
    #[serde(default)]
    store_name: String,
    #[serde(default)]
    is_active: u8,
}

async fn cheapshark_stores() -> Result<HashMap<String, String>> {
    let cache = util::cache_dir("discounts").join("cheapshark-stores.json");
    if let Some(hit) = util::cache_read::<HashMap<String, String>>(&cache, STORES_CACHE_TTL).await {
        return Ok(hit);
    }
    let list: Vec<CheapSharkStoreInfo> = irori_core::http::client()
        .get(CHEAPSHARK_STORES_URL)
        .header(reqwest::header::USER_AGENT, user_agent())
        .timeout(Duration::from_secs(15))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let map: HashMap<String, String> = list
        .into_iter()
        .filter(|s| s.is_active == 1 && !s.store_id.is_empty() && !s.store_name.is_empty())
        .map(|s| (s.store_id, s.store_name))
        .collect();
    if let Err(e) = util::cache_write(&cache, &map).await {
        tracing::debug!("caching CheapShark stores: {e}");
    }
    Ok(map)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CheapSharkDeal {
    #[serde(default)]
    title: String,
    #[serde(rename = "dealID", default)]
    deal_id: String,
    #[serde(rename = "storeID", default)]
    store_id: String,
    #[serde(default)]
    sale_price: String,
    #[serde(default)]
    normal_price: String,
    #[serde(default)]
    savings: String,
    #[serde(rename = "steamAppID", default)]
    steam_app_id: Option<String>,
    #[serde(default)]
    deal_rating: Option<String>,
    #[serde(default)]
    thumb: Option<String>,
}

fn map_cheapshark_store(
    store_id: &str,
    stores: &HashMap<String, String>,
) -> (DealStore, Option<String>) {
    match store_id {
        "1" => (DealStore::Steam, None),
        "3" => (DealStore::Gmg, None),
        "7" => (DealStore::Gog, None),
        "11" => (DealStore::Humble, None),
        other => (
            DealStore::Other,
            Some(
                stores
                    .get(other)
                    .cloned()
                    .unwrap_or_else(|| "Official store".to_string()),
            ),
        ),
    }
}

/// Parse CheapShark `/api/1.0/deals` response into `CatalogProduct` items.
pub fn parse_cheapshark_deals(
    bytes: &[u8],
    stores: &HashMap<String, String>,
) -> Result<Vec<CatalogProduct>> {
    let items: Vec<CheapSharkDeal> =
        serde_json::from_slice(bytes).context("parsing CheapShark deals")?;
    let mut out = Vec::new();
    for item in items {
        if item.title.trim().is_empty() || item.deal_id.trim().is_empty() {
            continue;
        }
        let Ok(sale) = item.sale_price.trim().parse::<f64>() else {
            continue;
        };
        let normal = item.normal_price.trim().parse::<f64>().unwrap_or(sale);
        let pct = item
            .savings
            .trim()
            .parse::<f64>()
            .map(|v| v.round().clamp(0.0, 100.0) as u32)
            .unwrap_or(0);
        let (store, store_name) = map_cheapshark_store(&item.store_id, stores);
        let steam_app_id = item
            .steam_app_id
            .filter(|id| !id.trim().is_empty() && id.chars().all(|c| c.is_ascii_digit()));
        let cover = steam_app_id
            .as_ref()
            .map(|id| {
                format!("https://shared.fastly.steamstatic.com/store_item_assets/steam/apps/{id}/header.jpg")
            })
            .or(item.thumb);
        out.push(CatalogProduct {
            id: format!("cs-{}", item.deal_id),
            title: item.title,
            store,
            store_name,
            steam_app_id,
            deal_rating: item.deal_rating.and_then(|r| r.trim().parse::<f32>().ok()),
            cover_horizontal: cover,
            price: CatalogPrice {
                r#final: format_money(sale, "USD"),
                base: if normal > sale {
                    format_money(normal, "USD")
                } else {
                    String::new()
                },
                discount: if pct > 0 {
                    format!("-{pct}%")
                } else {
                    String::new()
                },
                final_money: Some(Money {
                    amount: format!("{sale:.2}"),
                    currency: "USD".into(),
                }),
                base_money: (normal > sale).then(|| Money {
                    amount: format!("{normal:.2}"),
                    currency: "USD".into(),
                }),
            },
            product_type: "game".into(),
            store_link: format!(
                "https://www.cheapshark.com/redirect?dealID={}",
                util::encode_component(&item.deal_id)
            ),
            ..Default::default()
        });
    }
    Ok(dedupe(out))
}

/// Curated store entry derived from CheapShark rankings for a title.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct CuratedEntry {
    pub score: f32,
    pub steam_app_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CheapSharkCuratedDeal {
    #[serde(default)]
    title: String,
    #[serde(rename = "steamAppID", default)]
    steam_app_id: Option<String>,
    #[serde(default)]
    metacritic_score: serde_json::Value,
    #[serde(default)]
    steam_rating_percent: serde_json::Value,
    #[serde(default)]
    steam_rating_count: serde_json::Value,
    #[serde(default)]
    deal_rating: serde_json::Value,
}

fn json_as_f32(v: &serde_json::Value) -> f32 {
    match v {
        serde_json::Value::Number(n) => n.as_f64().unwrap_or(0.0) as f32,
        serde_json::Value::String(s) => s.trim().parse::<f32>().unwrap_or(0.0),
        _ => 0.0,
    }
}

async fn fetch_gmg_curated_sort(sort_by: &str) -> Result<Vec<CheapSharkCuratedDeal>> {
    let url = format!(
        "{CHEAPSHARK_DEALS_URL}?storeID=3&onSale=1&steamRating=70&pageSize=60&sortBy={sort_by}"
    );
    let resp = irori_core::http::client()
        .get(url)
        .header(reqwest::header::USER_AGENT, user_agent())
        .timeout(Duration::from_secs(12))
        .send()
        .await?
        .error_for_status()?;
    Ok(resp.json().await?)
}

/// Curated index of popular/highly-rated GMG deals from CheapShark (`storeID=3`),
/// cached on disk and resilient to upstream errors.
pub(crate) async fn gmg_curated_index() -> HashMap<String, CuratedEntry> {
    let cache = util::cache_dir("discounts").join("gmg-curated-index.json");
    if let Some(hit) =
        util::cache_read::<HashMap<String, CuratedEntry>>(&cache, GMG_CURATED_TTL).await
    {
        return hit;
    }
    let (mc, rev, dr) = futures::join!(
        fetch_gmg_curated_sort("Metacritic"),
        fetch_gmg_curated_sort("Reviews"),
        fetch_gmg_curated_sort("DealRating"),
    );
    let mut index: HashMap<String, CuratedEntry> = HashMap::new();
    for res in [mc, rev, dr] {
        let Ok(list) = res else { continue };
        for item in list {
            let key = irori_core::sort_key(&item.title);
            if key.is_empty() {
                continue;
            }
            let metacritic = json_as_f32(&item.metacritic_score).max(0.0);
            let steam_pct = json_as_f32(&item.steam_rating_percent).max(0.0);
            let steam_cnt = json_as_f32(&item.steam_rating_count).max(0.0);
            let deal_rating = json_as_f32(&item.deal_rating).max(0.0);
            let score = metacritic * 3.0
                + steam_pct * 1.5
                + (steam_cnt + 1.0).ln() * 25.0
                + deal_rating * 10.0;
            let steam_id = item
                .steam_app_id
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty() && s != "0" && s.chars().all(|c| c.is_ascii_digit()));
            index
                .entry(key)
                .and_modify(|e| {
                    if score > e.score {
                        e.score = score;
                    }
                    if e.steam_app_id.is_none() && steam_id.is_some() {
                        e.steam_app_id = steam_id.clone();
                    }
                })
                .or_insert(CuratedEntry {
                    score,
                    steam_app_id: steam_id,
                });
        }
    }
    if !index.is_empty()
        && let Err(e) = util::cache_write(&cache, &index).await
    {
        tracing::debug!("caching GMG curated index: {e}");
    }
    index
}

/// Live search across ~30 official storefronts on CheapShark (USD).
pub async fn cheapshark_search(query: &str) -> Result<Vec<CatalogProduct>> {
    let (stores, resp) = futures::join!(
        cheapshark_stores(),
        irori_core::http::client()
            .get(format!(
                "{CHEAPSHARK_DEALS_URL}?title={}&pageSize=24&sortBy=Deal%20Rating",
                util::encode_component(query.trim())
            ))
            .header(reqwest::header::USER_AGENT, user_agent())
            .timeout(Duration::from_secs(12))
            .send()
    );
    let stores = stores.unwrap_or_default();
    let bytes = resp?.error_for_status()?.bytes().await?;
    parse_cheapshark_deals(&bytes, &stores)
}

/// Global live search across Steam (regional), GOG (regional), CheapShark (~30
/// official stores in USD), and cached GMG/Humble feeds.
pub async fn search_global(
    query: &str,
    region: &DealsRegion,
    cached_feeds: &[CatalogProduct],
) -> (Vec<CatalogProduct>, Vec<&'static str>) {
    let q = query.trim();
    if q.is_empty() {
        return (Vec::new(), Vec::new());
    }
    let (steam, gog, cheapshark) = futures::join!(
        steam_search(q, region),
        gog_search(q, region),
        cheapshark_search(q),
    );
    let mut results = Vec::new();
    let mut failed = Vec::new();
    let mut seen_store_title: HashSet<(DealStore, String, String)> = HashSet::new();

    let push_unique = |list: Vec<CatalogProduct>,
                       out: &mut Vec<CatalogProduct>,
                       seen: &mut HashSet<(DealStore, String, String)>| {
        for p in list {
            let store_label = p.store_name.clone().unwrap_or_default();
            let key = (p.store, store_label, irori_core::sort_key(&p.title));
            if seen.insert(key) {
                out.push(p);
            }
        }
    };

    match steam {
        Ok(list) => push_unique(list, &mut results, &mut seen_store_title),
        Err(e) => {
            tracing::warn!("Steam search: {e:#}");
            failed.push("Steam");
        }
    }
    match gog {
        Ok(list) => push_unique(list, &mut results, &mut seen_store_title),
        Err(e) => {
            tracing::warn!("GOG search: {e:#}");
            failed.push("GOG");
        }
    }
    let needle = q.to_lowercase();
    let local_matches: Vec<CatalogProduct> = cached_feeds
        .iter()
        .filter(|p| {
            matches!(p.store, DealStore::Gmg | DealStore::Humble)
                && p.title.to_lowercase().contains(&needle)
        })
        .cloned()
        .collect();
    push_unique(local_matches, &mut results, &mut seen_store_title);

    match cheapshark {
        Ok(list) => push_unique(list, &mut results, &mut seen_store_title),
        Err(e) => {
            tracing::warn!("CheapShark search: {e:#}");
            failed.push("CheapShark");
        }
    }

    (results, failed)
}
