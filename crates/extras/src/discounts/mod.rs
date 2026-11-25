//! Store deals across Steam, GOG, GreenManGaming, Humble and CheapShark,
//! keyless free-to-claim giveaways (Epic + GamerPower), regional currency
//! selection, global live search and optional GG.deals comparison.

mod free;
mod ggdeals;
mod regions;
mod search;

use std::collections::HashSet;
use std::time::Duration;

use anyhow::{Context as _, Result};
use futures::future::join_all;
use once_cell::sync::Lazy;
use regex::Regex;
use serde::{Deserialize, Deserializer, Serialize};
use tokio::sync::Mutex;

pub use free::{
    FreeGame, FreeStore, epic_free_games, free_games, gamerpower_free_games, parse_epic_free_games,
    parse_gamerpower_free_games,
};
pub use ggdeals::{GgDealsPrice, ggdeals_prices, parse_ggdeals_prices};
pub use regions::{
    DEALS_REGIONS, DealsRegion, find_region, gg_deals_browse_url, gg_deals_search_url,
    protondb_app_url, protondb_search_url, resolve_region,
};
pub use search::{
    cheapshark_search, gog_search, parse_cheapshark_deals, parse_steam_featured,
    parse_steam_query_discounts, parse_steam_search, search_global, steam_discounts, steam_search,
};

use crate::util;

const CATALOG_URL: &str = "https://catalog.gog.com/v1/catalog";
const WISHLIST_URL: &str = "https://embed.gog.com/user/wishlist.json";
const USER_DATA_URL: &str = "https://embed.gog.com/userData.json";
const PAGE_LIMIT: u32 = 48;
const MAX_PAGES: u32 = 10;
/// Lifetime of cached GMG/Humble feeds.
pub const FEED_TTL: Duration = Duration::from_secs(24 * 3600);

/// Which shop a deal is from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DealStore {
    #[default]
    Gog,
    Steam,
    Gmg,
    Humble,
    Other,
}

/// An amount in a currency, as decimal text (`"2.40"`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Money {
    pub amount: String,
    pub currency: String,
}

/// Prices of a deal; the strings are formatted for display.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct CatalogPrice {
    #[serde(deserialize_with = "string_or_null")]
    pub r#final: String,
    #[serde(deserialize_with = "string_or_null")]
    pub base: String,
    /// `"-70%"`.
    #[serde(deserialize_with = "string_or_null")]
    pub discount: String,
    pub final_money: Option<Money>,
    pub base_money: Option<Money>,
}

/// A name/slug pair (genre, feature).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Tag {
    pub name: String,
    pub slug: String,
}

/// An age rating.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct CatalogRating {
    pub name: String,
    pub age_rating: String,
}

/// One discounted or searched store product.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct CatalogProduct {
    #[serde(deserialize_with = "string_or_number")]
    pub id: String,
    pub title: String,
    pub store: DealStore,
    /// Display name when `store` is [`DealStore::Other`] (e.g. `Fanatical`).
    pub store_name: Option<String>,
    /// Steam app ID when known (Steam directly, or CheapShark cross-reference).
    pub steam_app_id: Option<String>,
    /// Optional relevance/rating score (e.g. CheapShark deal rating).
    pub deal_rating: Option<f32>,
    pub cover_horizontal: Option<String>,
    pub cover_vertical: Option<String>,
    pub price: CatalogPrice,
    pub product_type: String,
    pub store_link: String,
    pub release_date: Option<String>,
    pub developers: Vec<String>,
    pub publishers: Vec<String>,
    pub operating_systems: Vec<String>,
    pub genres: Vec<Tag>,
    pub features: Vec<Tag>,
    pub reviews_rating: Option<f64>,
    pub ratings: Vec<CatalogRating>,
}

fn string_or_number<'de, D: Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    Ok(match serde_json::Value::deserialize(d)? {
        serde_json::Value::String(s) => s,
        serde_json::Value::Number(n) => n.to_string(),
        _ => String::new(),
    })
}

fn string_or_null<'de, D: Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    Ok(Option::<String>::deserialize(d)?.unwrap_or_default())
}

/// Country, locale and currency for the GOG catalog.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogLocale {
    pub country_code: String,
    pub locale: String,
    pub currency_code: String,
}

impl Default for CatalogLocale {
    /// `US` / `en-US` / `USD`.
    fn default() -> Self {
        Self {
            country_code: "US".into(),
            locale: "en-US".into(),
            currency_code: "USD".into(),
        }
    }
}

fn gog_catalog_locale(locale: &str) -> &'static str {
    match locale {
        "de-DE" => "de-DE",
        "fr-FR" => "fr-FR",
        "pl-PL" => "pl-PL",
        "ru-RU" => "ru-RU",
        "zh-Hans" => "zh-Hans",
        _ => "en-US",
    }
}

impl From<&DealsRegion> for CatalogLocale {
    fn from(r: &DealsRegion) -> Self {
        Self {
            country_code: r.store_country.to_string(),
            // GOG's catalog returns [] for locales it doesn't serve (e.g. pt-BR);
            // keep the user's country and currency while requesting a valid UI locale.
            locale: gog_catalog_locale(r.locale).to_string(),
            currency_code: r.currency_code.to_string(),
        }
    }
}

/// The GOG account's country and currency.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GogDealsRegion {
    pub country_code: String,
    pub currency_code: String,
}

pub(crate) fn user_agent() -> String {
    format!("Irori/{}", env!("CARGO_PKG_VERSION"))
}

pub(crate) fn gog_catalog_url(
    locale: &CatalogLocale,
    page: u32,
    hide_owned: bool,
    query: Option<&str>,
    limit: u32,
) -> String {
    let mut url = format!(
        "{CATALOG_URL}?limit={limit}&order=desc%3Atrending&productType=in%3Agame%2Cpack%2Cdlc&page={page}&countryCode={}&locale={}&currencyCode={}",
        util::encode_component(&locale.country_code),
        util::encode_component(&locale.locale),
        util::encode_component(&locale.currency_code),
    );
    if let Some(q) = query.map(str::trim).filter(|q| !q.is_empty()) {
        url.push_str("&query=like%3A");
        url.push_str(&util::encode_component(q));
    } else {
        url.push_str("&discounted=eq%3Atrue");
    }
    if hide_owned {
        url.push_str("&hideOwned=true");
    }
    url
}

#[derive(Deserialize)]
pub(crate) struct CatalogPage {
    #[serde(default)]
    pub(crate) pages: u32,
    #[serde(default)]
    pub(crate) products: Vec<CatalogProduct>,
}

pub(crate) async fn fetch_page(url: &str, token: Option<&str>) -> Result<CatalogPage> {
    let mut req = irori_core::http::client()
        .get(url)
        .header(reqwest::header::USER_AGENT, user_agent())
        .timeout(Duration::from_secs(15));
    if let Some(t) = token {
        req = req.bearer_auth(t);
    }
    Ok(req.send().await?.error_for_status()?.json().await?)
}

pub(crate) fn dedupe(products: Vec<CatalogProduct>) -> Vec<CatalogProduct> {
    let mut seen = HashSet::new();
    products
        .into_iter()
        .filter(|p| seen.insert(p.id.clone()))
        .collect()
}

async fn fetch_all(locale: &CatalogLocale, token: Option<&str>) -> Result<Vec<CatalogProduct>> {
    let hide_owned = token.is_some();
    let first = fetch_page(
        &gog_catalog_url(locale, 1, hide_owned, None, PAGE_LIMIT),
        token,
    )
    .await?;
    let total = first.pages.min(MAX_PAGES);
    let mut all = first.products;
    if total > 1 {
        let pages = join_all((2..=total).map(|n| async move {
            match fetch_page(
                &gog_catalog_url(locale, n, hide_owned, None, PAGE_LIMIT),
                token,
            )
            .await
            {
                Ok(p) => p.products,
                Err(e) => {
                    tracing::warn!("Failed to fetch discounts page {n}: {}", e);
                    Vec::new()
                }
            }
        }))
        .await;
        all.extend(pages.into_iter().flatten());
    }
    Ok(dedupe(all))
}

/// GOG's discounted catalog. With `access_token`, owned games are hidden.
/// An empty result retries with `en-US` in the user's country/currency first,
/// and only falls back to `US/en-US/USD` if still empty.
pub async fn gog_discounts(
    locale: &CatalogLocale,
    access_token: Option<&str>,
) -> Result<Vec<CatalogProduct>> {
    let mut products = fetch_all(locale, access_token)
        .await
        .context("Failed to fetch GOG discounts")?;
    if products.is_empty() && locale.locale != "en-US" {
        let same_region = CatalogLocale {
            country_code: locale.country_code.clone(),
            locale: "en-US".into(),
            currency_code: locale.currency_code.clone(),
        };
        products = fetch_all(&same_region, access_token)
            .await
            .context("Failed to fetch GOG discounts")?;
    }
    let fallback = CatalogLocale::default();
    if products.is_empty() && *locale != fallback {
        tracing::info!(
            "no GOG discounts for {}, retrying with US",
            locale.country_code
        );
        return fetch_all(&fallback, access_token)
            .await
            .context("Failed to fetch GOG discounts");
    }
    Ok(products)
}

/// Parse `userData.json` into a region.
pub fn parse_region(json: &serde_json::Value) -> Option<GogDealsRegion> {
    let country = json.get("country")?.as_str()?;
    let currency = json.pointer("/selectedCurrency/code")?.as_str()?;
    Some(GogDealsRegion {
        country_code: country.to_uppercase(),
        currency_code: currency.to_uppercase(),
    })
}

/// The GOG account's region (retried without the token if that fails).
pub async fn gog_deals_region(access_token: Option<&str>) -> Option<GogDealsRegion> {
    let attempt = |token: Option<&str>| {
        let mut req = irori_core::http::client()
            .get(USER_DATA_URL)
            .header(reqwest::header::USER_AGENT, user_agent())
            .timeout(Duration::from_secs(10));
        if let Some(t) = token {
            req = req.bearer_auth(t);
        }
        async move {
            let json: serde_json::Value = req
                .send()
                .await
                .ok()?
                .error_for_status()
                .ok()?
                .json()
                .await
                .ok()?;
            parse_region(&json)
        }
    };
    if let Some(r) = attempt(access_token).await {
        return Some(r);
    }
    if access_token.is_some() {
        return attempt(None).await;
    }
    None
}

/// Product IDs on the user's GOG wishlist.
pub async fn gog_wishlist(access_token: &str) -> Result<Vec<String>> {
    let json: serde_json::Value = irori_core::http::client()
        .get(WISHLIST_URL)
        .bearer_auth(access_token)
        .header(reqwest::header::USER_AGENT, user_agent())
        .timeout(Duration::from_secs(15))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await
        .context("reading the GOG wishlist")?;
    Ok(json
        .get("wishlist")
        .and_then(|w| w.as_object())
        .map(|m| m.keys().cloned().collect())
        .unwrap_or_default())
}

/// Currencies GMG publishes feeds for.
pub const GMG_CURRENCIES: [&str; 9] = [
    "GBP", "USD", "EUR", "CAD", "AUD", "BRL", "CNY", "TRL", "KRW",
];

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
struct FeedItem {
    #[serde(deserialize_with = "opt_string_or_number")]
    catalog_item_id: Option<String>,
    #[serde(deserialize_with = "opt_string_or_number")]
    id: Option<String>,
    name: Option<String>,
    url: Option<String>,
    image_url: Option<String>,
    #[serde(deserialize_with = "opt_string_or_number")]
    current_price: Option<String>,
    #[serde(deserialize_with = "opt_string_or_number")]
    original_price: Option<String>,
    currency: Option<String>,
    text1: Option<String>,
}

fn opt_string_or_number<'de, D: Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    Ok(match serde_json::Value::deserialize(d)? {
        serde_json::Value::String(s) => Some(s),
        serde_json::Value::Number(n) => Some(n.to_string()),
        _ => None,
    })
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
struct Feed {
    items: Vec<FeedItem>,
}

static SKU_PLATFORM: Lazy<Regex> = Lazy::new(|| Regex::new(r" - ([A-Za-z0-9 +()]+)$").unwrap());

fn platform_from_sku(sku: &str) -> Vec<String> {
    let Some(c) = SKU_PLATFORM.captures(sku) else {
        return Vec::new();
    };
    let suffix = c[1].to_lowercase();
    let os = if suffix.starts_with("xbox") {
        "xbox"
    } else if suffix == "mac" {
        "osx"
    } else if suffix == "pc" || suffix.starts_with("windows") {
        "windows"
    } else {
        return Vec::new();
    };
    vec![os.to_string()]
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().chain(c).collect(),
        None => String::new(),
    }
}

/// Format an amount for display.
pub fn format_money(amount: f64, currency: &str) -> String {
    let (symbol, decimals) = match currency {
        "USD" => ("$", 2),
        "GBP" => ("£", 2),
        "EUR" => ("€", 2),
        "CAD" => ("CA$", 2),
        "AUD" => ("A$", 2),
        "BRL" => ("R$", 2),
        "CNY" => ("CN¥", 2),
        "PLN" => ("zł ", 2),
        "SEK" => ("kr ", 2),
        "JPY" => ("¥", 0),
        "KRW" => ("₩", 0),
        other => return format!("{other} {amount:.2}"),
    };
    let text = format!("{amount:.decimals$}");
    let (int, frac) = text.split_once('.').unwrap_or((&text, ""));
    let mut grouped = String::new();
    for (i, ch) in int.chars().enumerate() {
        if i > 0 && (int.len() - i) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(ch);
    }
    if frac.is_empty() {
        format!("{symbol}{grouped}")
    } else {
        format!("{symbol}{grouped}.{frac}")
    }
}

fn normalize_feed(feed: Feed, store: DealStore) -> Vec<CatalogProduct> {
    let prefix = match store {
        DealStore::Gmg => "gmg-",
        DealStore::Humble => "humble-",
        DealStore::Gog | DealStore::Steam | DealStore::Other => "",
    };
    let items = feed.items.into_iter().filter_map(|item| {
        let id = item.catalog_item_id.clone().or(item.id)?;
        let title = item.name?;
        let link = item.url?;
        let current: f64 = item.current_price?.trim().parse().ok()?;
        if !current.is_finite() || current < 0.0 {
            return None;
        }
        let base = item
            .original_price
            .and_then(|p| p.trim().parse::<f64>().ok())
            .filter(|b| b.is_finite() && *b > 0.0)
            .unwrap_or(current);
        if base <= current {
            return None;
        }
        let pct = ((1.0 - current / base) * 100.0).round();
        let currency = item.currency.unwrap_or_else(|| "USD".into());
        let (operating_systems, features) = match store {
            DealStore::Humble => (vec!["windows".to_string()], Vec::new()),
            _ => (
                platform_from_sku(item.catalog_item_id.as_deref().unwrap_or("")),
                item.text1
                    .filter(|t| !t.is_empty())
                    .map(|t| {
                        vec![Tag {
                            name: capitalize(&t),
                            slug: t.to_lowercase(),
                        }]
                    })
                    .unwrap_or_default(),
            ),
        };
        if store == DealStore::Gmg && operating_systems.iter().any(|os| os == "xbox") {
            return None;
        }
        Some(CatalogProduct {
            id: format!("{prefix}{id}"),
            title,
            store,
            cover_horizontal: item.image_url,
            price: CatalogPrice {
                r#final: format_money(current, &currency),
                base: format_money(base, &currency),
                discount: format!("-{pct}%"),
                final_money: Some(Money {
                    amount: format!("{current:.2}"),
                    currency: currency.clone(),
                }),
                base_money: Some(Money {
                    amount: format!("{base:.2}"),
                    currency,
                }),
            },
            product_type: "game".into(),
            store_link: link,
            operating_systems,
            features,
            ..Default::default()
        })
    });
    dedupe(items.collect())
}

static GMG_LOCK: Mutex<()> = Mutex::const_new(());
static HUMBLE_LOCK: Mutex<()> = Mutex::const_new(());

async fn feed_deals(store: DealStore, currency: &str) -> Result<Vec<CatalogProduct>> {
    let (lock, url, name) = match store {
        DealStore::Gmg => (
            &GMG_LOCK,
            format!(
                "https://raw.githubusercontent.com/Heroic-Games-Launcher/deals-listing/gmg-feed/gmg-discounts-{currency}.json"
            ),
            "GMG",
        ),
        _ => (
            &HUMBLE_LOCK,
            format!(
                "https://raw.githubusercontent.com/Heroic-Games-Launcher/deals-listing/humble-feed/humble-discounts-{currency}.json"
            ),
            "Humble",
        ),
    };
    let _guard = lock.lock().await;
    let cache =
        util::cache_dir("discounts").join(format!("{}-{currency}.json", name.to_lowercase()));
    if let Some(deals) = util::cache_read::<Vec<CatalogProduct>>(&cache, FEED_TTL).await {
        return Ok(deals);
    }
    let resp = irori_core::http::client()
        .get(&url)
        .header(reqwest::header::USER_AGENT, user_agent())
        .timeout(Duration::from_secs(30))
        .send()
        .await
        .and_then(|r| r.error_for_status())
        .with_context(|| format!("Failed to fetch {name} discounts ({currency})"))?;
    let feed: Feed = resp
        .json()
        .await
        .with_context(|| format!("Failed to read {name} discounts ({currency})"))?;
    let deals = normalize_feed(feed, store);
    if let Err(e) = util::cache_write(&cache, &deals).await {
        tracing::warn!("caching {name} deals: {e:#}");
    }
    Ok(deals)
}

fn deal_base_amount(p: &CatalogProduct) -> f64 {
    p.price
        .base_money
        .as_ref()
        .or(p.price.final_money.as_ref())
        .and_then(|m| m.amount.trim().parse::<f64>().ok())
        .filter(|v| v.is_finite() && *v > 0.0)
        .unwrap_or(0.0)
}

fn deal_discount_pct(p: &CatalogProduct) -> u32 {
    p.price
        .discount
        .trim()
        .trim_start_matches('-')
        .trim_end_matches('%')
        .trim()
        .parse()
        .unwrap_or(0)
}

fn featured_fallback_score(p: &CatalogProduct) -> f64 {
    let base = deal_base_amount(p);
    let pct = deal_discount_pct(p) as f64;
    let tier = if base >= 20.0 {
        3.0
    } else if base >= 15.0 {
        2.0
    } else if base >= 10.0 {
        1.0
    } else {
        0.0
    };
    let discount_bonus = if pct >= 20.0 { pct * 0.4 } else { pct * 0.05 };
    tier * 100.0 + base.min(120.0) + discount_bonus
}

fn curate_gmg_deals(
    deals: &mut [CatalogProduct],
    index: &std::collections::HashMap<String, search::CuratedEntry>,
) {
    for deal in deals.iter_mut() {
        let key = irori_core::sort_key(&deal.title);
        let matched = index.get(&key).or_else(|| {
            let stripped = crate::wiki::strip_edition_suffix(&deal.title);
            let skey = irori_core::sort_key(&stripped);
            (skey != key).then(|| index.get(&skey)).flatten()
        });
        if let Some(entry) = matched {
            if deal.steam_app_id.is_none() {
                deal.steam_app_id = entry.steam_app_id.clone();
            }
            deal.deal_rating = Some(entry.score);
        }
    }
    deals.sort_by(|a, b| match (a.deal_rating, b.deal_rating) {
        (Some(sa), Some(sb)) => sb.partial_cmp(&sa).unwrap_or(std::cmp::Ordering::Equal),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => featured_fallback_score(b)
            .partial_cmp(&featured_fallback_score(a))
            .unwrap_or(std::cmp::Ordering::Equal),
    });
}

fn rank_humble_featured(deals: &mut [CatalogProduct]) {
    deals.sort_by(|a, b| {
        featured_fallback_score(b)
            .partial_cmp(&featured_fallback_score(a))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
}

/// GreenManGaming deals in a currency (unsupported currencies use USD).
pub async fn gmg_discounts(currency: Option<&str>) -> Result<Vec<CatalogProduct>> {
    let cur = currency.unwrap_or("USD").to_uppercase();
    let cur = if GMG_CURRENCIES.contains(&cur.as_str()) {
        cur
    } else {
        "USD".to_string()
    };
    let (deals_res, index) = futures::join!(
        feed_deals(DealStore::Gmg, &cur),
        search::gmg_curated_index()
    );
    let mut deals = deals_res?;
    curate_gmg_deals(&mut deals, &index);
    Ok(deals)
}

/// Delete cached discounts data so a manual reload fetches fresh store prices.
pub async fn clear_cache() {
    let dir = util::cache_dir("discounts");
    if let Err(e) = tokio::fs::remove_dir_all(&dir).await
        && e.kind() != std::io::ErrorKind::NotFound
    {
        tracing::debug!("clearing discounts cache: {e}");
    }
}

/// Humble deals (USD only).
pub async fn humble_discounts() -> Result<Vec<CatalogProduct>> {
    let mut deals = feed_deals(DealStore::Humble, "USD").await?;
    rank_humble_featured(&mut deals);
    Ok(deals)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    #[test]
    fn gog_catalog_product() {
        let json = r#"{"pages": 3, "products": [{"id": "1207658924", "slug": "x", "title": "Witcher",
            "coverHorizontal": "https://a/h.jpg", "price": {"final": "$4.99", "base": "$9.99", "discount": "-50%",
            "finalMoney": {"amount": "4.99", "currency": "USD", "discount": "5.00"},
            "baseMoney": {"amount": "9.99", "currency": "USD"}}, "productType": "game",
            "storeLink": "https://www.gog.com/en/game/x", "developers": ["CDPR"], "operatingSystems": ["windows"],
            "genres": [{"name": "RPG", "slug": "rpg"}], "reviewsRating": 45, "ratings": [{"name": "PEGI", "ageRating": "18"}]},
            {"id": 5, "title": "N", "price": {"final": "$1", "base": "$1", "discount": null}}]}"#;
        let page: CatalogPage = serde_json::from_str(json).unwrap();
        assert_eq!(page.pages, 3);
        let p = &page.products[0];
        assert_eq!(p.store, DealStore::Gog);
        assert_eq!(p.price.discount, "-50%");
        assert_eq!(p.price.final_money.as_ref().unwrap().amount, "4.99");
        assert_eq!(p.ratings[0].age_rating, "18");
        assert_eq!(page.products[1].id, "5");
        assert_eq!(page.products[1].price.discount, "");
    }

    #[test]
    fn catalog_url_params() {
        let url = gog_catalog_url(&CatalogLocale::default(), 2, true, None, PAGE_LIMIT);
        assert!(url.ends_with(
            "limit=48&order=desc%3Atrending&productType=in%3Agame%2Cpack%2Cdlc&page=2&countryCode=US&locale=en-US&currencyCode=USD&discounted=eq%3Atrue&hideOwned=true"
        ));
    }

    #[test]
    fn resolves_regions_and_builds_urls() {
        assert_eq!(resolve_region("BR", Some("en-US")).code, "BR");
        assert_eq!(resolve_region("", Some("pt_BR.UTF-8")).code, "BR");
        let eu = resolve_region("auto", Some("de-DE"));
        assert_eq!(eu.code, "EU");
        assert_eq!(eu.store_country, "NL");
        assert_eq!(eu.currency_code, "EUR");
        assert_eq!(eu.gg_deals_slug, "eu");
        assert_eq!(resolve_region("PT", None).code, "EU");
        assert_eq!(resolve_region("fr", None).code, "EU");
        let eu_cat = CatalogLocale::from(eu);
        assert_eq!(eu_cat.country_code, "NL");
        assert_eq!(eu_cat.currency_code, "EUR");
        assert_eq!(eu_cat.locale, "en-US");
        assert_eq!(gg_deals_browse_url(eu), "https://gg.deals/eu/deals/");
        assert_eq!(resolve_region("unknown", None).code, "US");
        let br = resolve_region("BR", None);
        let br_cat = CatalogLocale::from(br);
        assert_eq!(br_cat.country_code, "BR");
        assert_eq!(br_cat.currency_code, "BRL");
        assert_eq!(br_cat.locale, "en-US");
        assert_eq!(gg_deals_browse_url(br), "https://gg.deals/br/deals/");
        assert_eq!(
            gg_deals_search_url("Shadow of Mordor", br),
            "https://gg.deals/br/games/?title=Shadow%20of%20Mordor"
        );
        assert_eq!(
            protondb_app_url("241930"),
            "https://www.protondb.com/app/241930"
        );
    }

    #[test]
    fn parses_steam_featured_and_search() {
        let featured = r#"{
            "specials": {"items": [
                {"id": 241930, "name": "Middle-earth: Shadow of Mordor", "discounted": true, "discount_percent": 80,
                 "original_price": 8999, "final_price": 1799, "currency": "BRL",
                 "header_image": "https://cdn/241930/header.jpg", "windows_available": true, "linux_available": true},
                {"id": 999, "name": "Full Price", "discounted": false, "discount_percent": 0, "final_price": 1000}
            ]},
            "top_sellers": {"items": [
                {"id": 241930, "name": "Middle-earth: Shadow of Mordor", "discounted": true, "discount_percent": 80,
                 "original_price": 8999, "final_price": 1799, "currency": "BRL"},
                {"id": 1145360, "name": "Hades", "discounted": true, "discount_percent": 60,
                 "original_price": 7399, "final_price": 2959, "currency": "BRL"},
                {"id": 888, "name": "Unreduced Hit", "discounted": false, "discount_percent": 0, "final_price": 19900}
            ]}
        }"#;
        let deals = parse_steam_featured(featured.as_bytes(), "BRL").unwrap();
        assert_eq!(deals.len(), 2);
        assert_eq!(deals[0].store, DealStore::Steam);
        assert_eq!(deals[0].steam_app_id.as_deref(), Some("241930"));
        assert_eq!(deals[0].price.discount, "-80%");
        assert_eq!(deals[0].price.r#final, "R$17.99");
        assert_eq!(deals[0].operating_systems, ["windows", "linux"]);
        assert_eq!(deals[1].title, "Hades");
        assert_eq!(deals[1].price.discount, "-60%");

        let search = r#"{"items": [
            {"id": 241930, "name": "Middle-earth: Shadow of Mordor", "price": {"currency": "BRL", "initial": 8999, "final": 1799}}
        ]}"#;
        let found = parse_steam_search(search.as_bytes(), "BRL").unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].price.discount, "-80%");
        assert_eq!(found[0].price.r#final, "R$17.99");

        let query_json = r#"{"response": {"store_items": [
            {"appid": 1174180, "name": "Red Dead Redemption 2",
             "best_purchase_option": {"final_price_in_cents": "7497", "original_price_in_cents": "29990", "discount_pct": 75},
             "assets": {"asset_url_format": "steam/apps/1174180/${FILENAME}?t=123", "header": "header.jpg"},
             "platforms": {"windows": true, "mac": false, "steamos_linux": true}},
            {"appid": 730, "name": "Counter-Strike 2",
             "best_purchase_option": {"final_price_in_cents": "0", "original_price_in_cents": "0", "discount_pct": 0}}
        ]}}"#;
        let query_deals = parse_steam_query_discounts(query_json.as_bytes(), "BRL").unwrap();
        assert_eq!(query_deals.len(), 1);
        assert_eq!(query_deals[0].steam_app_id.as_deref(), Some("1174180"));
        assert_eq!(query_deals[0].price.r#final, "R$74.97");
        assert_eq!(query_deals[0].price.base, "R$299.90");
        assert_eq!(query_deals[0].price.discount, "-75%");
        assert_eq!(
            query_deals[0].cover_horizontal.as_deref(),
            Some(
                "https://shared.fastly.steamstatic.com/store_item_assets/steam/apps/1174180/header.jpg?t=123"
            )
        );
        assert_eq!(query_deals[0].operating_systems, ["windows", "linux"]);
    }

    #[test]
    fn curates_gmg_and_ranks_humble_featured() {
        let make = |id: &str, title: &str, store: DealStore, cur: &str, base: &str, disc: &str| {
            CatalogProduct {
                id: id.into(),
                title: title.into(),
                store,
                price: CatalogPrice {
                    r#final: format!("${cur}"),
                    base: format!("${base}"),
                    discount: disc.into(),
                    final_money: Some(Money {
                        amount: cur.into(),
                        currency: "USD".into(),
                    }),
                    base_money: Some(Money {
                        amount: base.into(),
                        currency: "USD".into(),
                    }),
                },
                ..Default::default()
            }
        };
        let mut gmg = vec![
            make("g1", "Tiny Jigsaws", DealStore::Gmg, "1.00", "2.99", "-67%"),
            make(
                "g2",
                "Disco Elysium - The Final Cut",
                DealStore::Gmg,
                "10.00",
                "39.99",
                "-75%",
            ),
            make("g3", "God of War", DealStore::Gmg, "20.00", "49.99", "-60%"),
        ];
        let mut idx = HashMap::new();
        idx.insert(
            irori_core::sort_key("Disco Elysium - The Final Cut"),
            search::CuratedEntry {
                score: 520.0,
                steam_app_id: Some("632470".into()),
            },
        );
        idx.insert(
            irori_core::sort_key("God of War"),
            search::CuratedEntry {
                score: 490.0,
                steam_app_id: Some("1593500".into()),
            },
        );
        curate_gmg_deals(&mut gmg, &idx);
        assert_eq!(gmg[0].id, "g2");
        assert_eq!(gmg[0].steam_app_id.as_deref(), Some("632470"));
        assert_eq!(gmg[1].id, "g3");
        assert_eq!(gmg[1].steam_app_id.as_deref(), Some("1593500"));
        assert_eq!(gmg[2].id, "g1");

        let mut humble = vec![
            make(
                "h1",
                "Cheap Shovelware",
                DealStore::Humble,
                "0.51",
                "3.99",
                "-87%",
            ),
            make(
                "h2",
                "Full Priced RPG",
                DealStore::Humble,
                "14.99",
                "59.99",
                "-75%",
            ),
            make(
                "h3",
                "Mid Indie",
                DealStore::Humble,
                "9.99",
                "19.99",
                "-50%",
            ),
        ];
        rank_humble_featured(&mut humble);
        assert_eq!(humble[0].id, "h2");
        assert_eq!(humble[1].id, "h3");
        assert_eq!(humble[2].id, "h1");
    }

    #[test]
    fn parses_epic_and_gamerpower_freebies() {
        let epic = r#"{"data":{"Catalog":{"searchStore":{"elements":[
            {"id":"e1","title":"Free Now","keyImages":[{"type":"OfferImageWide","url":"https://e/1.jpg"}],
             "catalogNs":{"mappings":[{"pageSlug":"free-now"}]},
             "price":{"totalPrice":{"discountPrice":0,"originalPrice":1999,"fmtPrice":{"originalPrice":"$19.99"}}},
             "promotions":{"promotionalOffers":[{"promotionalOffers":[{"endDate":"2026-12-03T16:00:00Z","discountSetting":{"discountPercentage":0}}]}]}},
            {"id":"e2","title":"Next Week","keyImages":[],
             "productSlug":"next-week",
             "price":{"totalPrice":{"discountPrice":1999,"originalPrice":1999,"fmtPrice":{"originalPrice":"$19.99"}}},
             "promotions":{"upcomingPromotionalOffers":[{"promotionalOffers":[{"startDate":"2026-12-03T16:00:00Z","discountSetting":{"discountPercentage":0}}]}]}}
        ]}}}}"#;
        let epic_list = parse_epic_free_games(epic.as_bytes()).unwrap();
        assert_eq!(epic_list.len(), 2);
        assert!(!epic_list[0].upcoming);
        assert_eq!(
            epic_list[0].claim_url,
            "https://store.epicgames.com/en-US/p/free-now"
        );
        assert!(epic_list[1].upcoming);

        let gp = r#"[{"id": 42, "title": "Some Indie (Steam) Giveaway", "worth": "$9.99",
            "image": "https://gp/img.jpg", "platforms": "PC, Steam", "end_date": "2026-12-01 23:59:00",
            "open_giveaway_url": "https://store.steampowered.com/app/12345/Indie/", "status": "Active"}]"#;
        let gp_list = parse_gamerpower_free_games(gp.as_bytes()).unwrap();
        assert_eq!(gp_list.len(), 1);
        assert_eq!(gp_list[0].store, FreeStore::Steam);
        assert_eq!(gp_list[0].steam_app_id.as_deref(), Some("12345"));
        assert!(gp_list[0].from_gamerpower);
    }

    #[test]
    fn parses_cheapshark_and_ggdeals() {
        let mut stores = HashMap::new();
        stores.insert("15".into(), "Fanatical".into());
        let cs = r#"[{"title":"Shadow of Mordor","dealID":"abc123","storeID":"15",
            "salePrice":"3.99","normalPrice":"19.99","savings":"80.04","steamAppID":"241930","dealRating":"9.4"}]"#;
        let deals = parse_cheapshark_deals(cs.as_bytes(), &stores).unwrap();
        assert_eq!(deals.len(), 1);
        assert_eq!(deals[0].store, DealStore::Other);
        assert_eq!(deals[0].store_name.as_deref(), Some("Fanatical"));
        assert_eq!(deals[0].steam_app_id.as_deref(), Some("241930"));
        assert_eq!(deals[0].price.r#final, "$3.99");

        let gg = r#"{"success": true, "data": {
            "241930": {"url": "https://gg.deals/game/shadow/",
                "prices": {"currentRetail": "17.99", "currentKeyshops": "11.50", "historicalRetail": "8.99", "currency": "BRL"}},
            "999999": [],
            "888888": {"url": "https://gg.deals/game/other/", "prices": []},
            "777777": {"url": "https://gg.deals/game/no-cur/", "prices": {"currentRetail": 10, "currency": null}}
        }}"#;
        let map = parse_ggdeals_prices(gg.as_bytes(), "BRL").unwrap();
        assert_eq!(map.len(), 2);
        let p = &map["241930"];
        assert_eq!(p.retail.as_deref(), Some("R$17.99"));
        assert_eq!(p.keyshop.as_deref(), Some("R$11.50"));
        assert_eq!(p.historical_retail.as_deref(), Some("R$8.99"));
        assert_eq!(p.retail_cents, Some(1799));
        assert_eq!(p.keyshop_cents, Some(1150));
        assert_eq!(p.historical_retail_cents, Some(899));
        assert_eq!(map["777777"].retail.as_deref(), Some("R$10.00"));
        assert_eq!(map["777777"].currency, "BRL");
    }

    #[test]
    fn gmg_feed_normalization() {
        let feed: Feed = serde_json::from_str(r#"{"Items": [
            {"CatalogItemId": "Mega Sudoku - PC", "Name": "Mega Sudoku", "Url": "https://gmg/x",
             "ImageUrl": "https://img", "CurrentPrice": "2.40", "OriginalPrice": "7.99", "Currency": "USD", "Text1": "steam"},
            {"CatalogItemId": "Mega Sudoku - PC", "Name": "Dup", "Url": "u", "CurrentPrice": "1", "OriginalPrice": "2"},
            {"Id": 7, "Name": "Full price", "Url": "u", "CurrentPrice": "5", "OriginalPrice": "5"},
            {"Id": 8, "Name": "No url", "CurrentPrice": "1", "OriginalPrice": "5"},
            {"Id": 9, "Name": "Big", "Url": "u", "CurrentPrice": "1500", "OriginalPrice": 3000, "Currency": "KRW"},
            {"CatalogItemId": "Thing - Xbox One", "Name": "X", "Url": "u", "CurrentPrice": "1", "OriginalPrice": "4", "Currency": "TRL"}
        ]}"#).unwrap();
        let deals = normalize_feed(feed, DealStore::Gmg);
        assert_eq!(deals.len(), 2);
        assert_eq!(deals[0].id, "gmg-Mega Sudoku - PC");
        assert_eq!(deals[0].price.discount, "-70%");
        assert_eq!(deals[1].price.r#final, "₩1,500");
    }

    #[test]
    fn humble_feed_normalization() {
        let feed: Feed = serde_json::from_str(
            r#"{"Items": [
            {"CatalogItemId": "blackbook_storefront", "Name": "Black Book", "Url": "u",
             "CurrentPrice": "7.49", "OriginalPrice": "24.99", "Text1": "steam"}]}"#,
        )
        .unwrap();
        let deals = normalize_feed(feed, DealStore::Humble);
        assert_eq!(deals[0].id, "humble-blackbook_storefront");
        assert_eq!(deals[0].store, DealStore::Humble);
        assert_eq!(deals[0].price.discount, "-70%");
    }

    #[test]
    fn money_formatting() {
        assert_eq!(format_money(1234567.5, "USD"), "$1,234,567.50");
        assert_eq!(format_money(0.5, "EUR"), "€0.50");
        assert_eq!(format_money(999.0, "GBP"), "£999.00");
        assert_eq!(format_money(3.0, "XYZ"), "XYZ 3.00");
    }
}
