//! Steam Specials, Steam Store Search, GOG Search, CheapShark global deal
//! search (~30 official stores without an API key), and regional price formatting.

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use anyhow::{Context as _, Result};
use serde::{Deserialize, Serialize};

use super::{
    CatalogPrice, CatalogProduct, DealStore, DealsRegion, Money, dedupe, format_money,
    gog_catalog_url, user_agent,
};
use crate::util;

const STEAM_FEATURED_URL: &str = "https://store.steampowered.com/api/featuredcategories";
const STEAM_SEARCH_URL: &str = "https://store.steampowered.com/api/storesearch/";
const CHEAPSHARK_STORES_URL: &str = "https://www.cheapshark.com/api/1.0/stores";
const CHEAPSHARK_DEALS_URL: &str = "https://www.cheapshark.com/api/1.0/deals";
const STEAM_CACHE_TTL: Duration = Duration::from_secs(3600);
const STORES_CACHE_TTL: Duration = Duration::from_secs(24 * 3600);

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

/// Steam featured regional specials (no API key required).
pub async fn steam_discounts(region: &DealsRegion) -> Result<Vec<CatalogProduct>> {
    let cache = util::cache_dir("discounts").join(format!("steam-{}.json", region.code));
    if let Some(hit) = util::cache_read::<Vec<CatalogProduct>>(&cache, STEAM_CACHE_TTL).await {
        return Ok(hit);
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
    let deals = parse_steam_featured(&bytes, region.currency_code)?;
    if let Err(e) = util::cache_write(&cache, &deals).await {
        tracing::debug!("caching Steam deals: {e}");
    }
    Ok(deals)
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
