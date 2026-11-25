//! Keyless free-to-claim giveaways from Epic Games' public catalog and
//! GamerPower (Steam, GOG and itch.io).

use std::collections::HashSet;
use std::time::Duration;

use anyhow::{Context as _, Result};
use once_cell::sync::Lazy;
use regex::Regex;
use serde::{Deserialize, Serialize};

use super::DealsRegion;
use crate::util;

const EPIC_FREE_URL: &str =
    "https://store-site-backend-static.ak.epicgames.com/freeGamesPromotions";
const GAMERPOWER_URL: &str =
    "https://www.gamerpower.com/api/filter?platform=steam.gog.itchio&type=game";
const CACHE_TTL: Duration = Duration::from_secs(3600);

static STEAM_APP_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"store\.steampowered\.com/app/(\d+)").expect("valid regex"));

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FreeStore {
    Epic,
    Steam,
    Gog,
    Itch,
}

impl FreeStore {
    pub fn display_name(self) -> &'static str {
        match self {
            FreeStore::Epic => "Epic Games",
            FreeStore::Steam => "Steam",
            FreeStore::Gog => "GOG",
            FreeStore::Itch => "itch.io",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FreeGame {
    pub id: String,
    pub title: String,
    pub store: FreeStore,
    pub image_url: Option<String>,
    pub claim_url: String,
    pub original_price: Option<String>,
    pub ends_at: Option<String>,
    pub upcoming: bool,
    pub steam_app_id: Option<String>,
    pub from_gamerpower: bool,
}

#[derive(Deserialize)]
struct EpicResponse {
    data: Option<EpicData>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct EpicData {
    catalog: Option<EpicCatalog>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct EpicCatalog {
    search_store: Option<EpicSearchStore>,
}

#[derive(Deserialize)]
struct EpicSearchStore {
    #[serde(default)]
    elements: Vec<EpicElement>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct EpicElement {
    #[serde(default)]
    id: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    product_slug: Option<String>,
    #[serde(default)]
    url_slug: Option<String>,
    #[serde(default)]
    key_images: Vec<EpicImage>,
    #[serde(default)]
    catalog_ns: Option<EpicCatalogNs>,
    #[serde(default)]
    price: Option<EpicPrice>,
    #[serde(default)]
    promotions: Option<EpicPromotions>,
}

#[derive(Deserialize)]
struct EpicCatalogNs {
    #[serde(default)]
    mappings: Vec<EpicMapping>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct EpicMapping {
    page_slug: Option<String>,
}

#[derive(Deserialize)]
struct EpicImage {
    #[serde(rename = "type", default)]
    kind: String,
    #[serde(default)]
    url: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct EpicPrice {
    total_price: Option<EpicTotalPrice>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct EpicTotalPrice {
    #[serde(default)]
    discount_price: i64,
    #[serde(default)]
    original_price: i64,
    fmt_price: Option<EpicFmtPrice>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct EpicFmtPrice {
    original_price: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct EpicPromotions {
    #[serde(default)]
    promotional_offers: Vec<EpicOfferGroup>,
    #[serde(default)]
    upcoming_promotional_offers: Vec<EpicOfferGroup>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct EpicOfferGroup {
    #[serde(default)]
    promotional_offers: Vec<EpicOffer>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct EpicOffer {
    start_date: Option<String>,
    end_date: Option<String>,
    discount_setting: Option<EpicDiscountSetting>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct EpicDiscountSetting {
    discount_percentage: Option<i64>,
}

fn pick_epic_image(images: &[EpicImage]) -> Option<String> {
    for preferred in [
        "OfferImageWide",
        "DieselStoreFrontWide",
        "Thumbnail",
        "DieselGameBox",
        "DieselGameBoxTall",
    ] {
        if let Some(img) = images
            .iter()
            .find(|i| i.kind == preferred && !i.url.is_empty())
        {
            return Some(img.url.clone());
        }
    }
    images
        .iter()
        .find(|i| !i.url.is_empty())
        .map(|i| i.url.clone())
}

fn epic_claim_url(el: &EpicElement) -> String {
    let slug = el
        .catalog_ns
        .as_ref()
        .and_then(|ns| ns.mappings.iter().find_map(|m| m.page_slug.as_deref()))
        .or(el.product_slug.as_deref())
        .or(el.url_slug.as_deref())
        .map(|s| s.trim_matches('/'))
        .filter(|s| !s.is_empty());
    match slug {
        Some(slug) => format!("https://store.epicgames.com/en-US/p/{slug}"),
        None => "https://store.epicgames.com/en-US/free-games".to_string(),
    }
}

/// Parse Epic Games' `freeGamesPromotions` JSON payload into active and
/// upcoming freebies.
pub fn parse_epic_free_games(bytes: &[u8]) -> Result<Vec<FreeGame>> {
    let parsed: EpicResponse =
        serde_json::from_slice(bytes).context("parsing Epic freeGamesPromotions")?;
    let elements = parsed
        .data
        .and_then(|d| d.catalog)
        .and_then(|c| c.search_store)
        .map(|s| s.elements)
        .unwrap_or_default();

    let mut out = Vec::new();
    for el in elements {
        let Some(promos) = &el.promotions else {
            continue;
        };
        let active_offer = promos.promotional_offers.iter().find_map(|g| {
            g.promotional_offers.iter().find(|o| {
                o.discount_setting
                    .as_ref()
                    .and_then(|d| d.discount_percentage)
                    == Some(0)
            })
        });
        let upcoming_offer = promos.upcoming_promotional_offers.iter().find_map(|g| {
            g.promotional_offers.iter().find(|o| {
                o.discount_setting
                    .as_ref()
                    .and_then(|d| d.discount_percentage)
                    == Some(0)
            })
        });

        let total = el.price.as_ref().and_then(|p| p.total_price.as_ref());
        let is_free_now = active_offer.is_some()
            && total.is_some_and(|p| p.discount_price == 0 && p.original_price > 0);
        let (upcoming, date) = if is_free_now {
            (false, active_offer.and_then(|o| o.end_date.clone()))
        } else if let Some(up) = upcoming_offer {
            (true, up.start_date.clone().or_else(|| up.end_date.clone()))
        } else {
            continue;
        };
        if el.title.trim().is_empty() {
            continue;
        }
        let original_price = total
            .filter(|p| p.original_price > 0)
            .and_then(|p| p.fmt_price.as_ref())
            .and_then(|f| f.original_price.clone())
            .filter(|s| !s.trim().is_empty());

        out.push(FreeGame {
            id: format!("epic-free-{}", el.id),
            title: el.title.clone(),
            store: FreeStore::Epic,
            image_url: pick_epic_image(&el.key_images),
            claim_url: epic_claim_url(&el),
            original_price,
            ends_at: date,
            upcoming,
            steam_app_id: None,
            from_gamerpower: false,
        });
    }
    Ok(out)
}

pub async fn epic_free_games(region: &DealsRegion) -> Result<Vec<FreeGame>> {
    let cache = util::cache_dir("discounts").join(format!("epic-free-{}.json", region.code));
    if let Some(hit) = util::cache_read::<Vec<FreeGame>>(&cache, CACHE_TTL).await {
        return Ok(hit);
    }
    let url = format!(
        "{EPIC_FREE_URL}?locale={}&country={}&allowCountries={}",
        util::encode_component(region.locale),
        util::encode_component(region.code),
        util::encode_component(region.code),
    );
    let resp = irori_core::http::client()
        .get(&url)
        .header(
            reqwest::header::USER_AGENT,
            format!("Irori/{}", env!("CARGO_PKG_VERSION")),
        )
        .timeout(Duration::from_secs(15))
        .send()
        .await?
        .error_for_status()?;
    let bytes = resp.bytes().await?;
    let games = parse_epic_free_games(&bytes)?;
    if let Err(e) = util::cache_write(&cache, &games).await {
        tracing::debug!("caching Epic free games: {e}");
    }
    Ok(games)
}

#[derive(Deserialize)]
struct GamerPowerItem {
    id: u64,
    #[serde(default)]
    title: String,
    #[serde(default)]
    worth: String,
    #[serde(default)]
    image: String,
    #[serde(default)]
    thumbnail: String,
    #[serde(default)]
    platforms: String,
    #[serde(default)]
    end_date: String,
    #[serde(default)]
    open_giveaway_url: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    status: String,
}

fn map_gamerpower_store(platforms: &str) -> Option<FreeStore> {
    let lower = platforms.to_ascii_lowercase();
    if lower.contains("steam") {
        Some(FreeStore::Steam)
    } else if lower.contains("gog") {
        Some(FreeStore::Gog)
    } else if lower.contains("itch") {
        Some(FreeStore::Itch)
    } else {
        None
    }
}

fn clean_gamerpower_title(title: &str) -> String {
    let t = title.trim();
    t.strip_suffix("Giveaway")
        .or_else(|| t.strip_suffix("(Steam)"))
        .or_else(|| t.strip_suffix("(GOG)"))
        .unwrap_or(t)
        .trim()
        .to_string()
}

fn extract_steam_id(text: &str) -> Option<String> {
    STEAM_APP_RE.captures(text).map(|c| c[1].to_string())
}

/// Parse GamerPower `/api/filter` JSON response for Steam, GOG and itch.io
/// full-game giveaways.
pub fn parse_gamerpower_free_games(bytes: &[u8]) -> Result<Vec<FreeGame>> {
    let items: Vec<GamerPowerItem> =
        serde_json::from_slice(bytes).context("parsing GamerPower giveaways")?;
    let mut out = Vec::new();
    for item in items {
        if !item.status.is_empty() && !item.status.eq_ignore_ascii_case("Active") {
            continue;
        }
        let Some(store) = map_gamerpower_store(&item.platforms) else {
            continue;
        };
        let title = clean_gamerpower_title(&item.title);
        if title.is_empty() || item.open_giveaway_url.trim().is_empty() {
            continue;
        }
        let worth = item.worth.trim();
        let original_price =
            (!worth.is_empty() && !worth.eq_ignore_ascii_case("N/A")).then(|| worth.to_string());
        let ends_at = (!item.end_date.trim().is_empty()
            && !item.end_date.trim().eq_ignore_ascii_case("N/A"))
        .then(|| item.end_date.trim().to_string());
        let image_url = (!item.image.trim().is_empty())
            .then(|| item.image.trim().to_string())
            .or_else(|| {
                (!item.thumbnail.trim().is_empty()).then(|| item.thumbnail.trim().to_string())
            });
        let steam_app_id = extract_steam_id(&item.open_giveaway_url)
            .or_else(|| extract_steam_id(&item.description))
            .or_else(|| image_url.as_deref().and_then(extract_steam_id));
        out.push(FreeGame {
            id: format!("gp-{}", item.id),
            title,
            store,
            image_url,
            claim_url: item.open_giveaway_url,
            original_price,
            ends_at,
            upcoming: false,
            steam_app_id,
            from_gamerpower: true,
        });
    }
    Ok(out)
}

pub async fn gamerpower_free_games() -> Result<Vec<FreeGame>> {
    let cache = util::cache_dir("discounts").join("gamerpower-free.json");
    if let Some(hit) = util::cache_read::<Vec<FreeGame>>(&cache, CACHE_TTL).await {
        return Ok(hit);
    }
    let resp = irori_core::http::client()
        .get(GAMERPOWER_URL)
        .header(
            reqwest::header::USER_AGENT,
            format!("Irori/{}", env!("CARGO_PKG_VERSION")),
        )
        .timeout(Duration::from_secs(15))
        .send()
        .await?
        .error_for_status()?;
    let bytes = resp.bytes().await?;
    let games = parse_gamerpower_free_games(&bytes)?;
    if let Err(e) = util::cache_write(&cache, &games).await {
        tracing::debug!("caching GamerPower freebies: {e}");
    }
    Ok(games)
}

/// Fetch Epic official free games + Steam/GOG/itch.io freebies in parallel.
/// Active giveaways come before upcoming Epic entries; store+title duplicates
/// are dropped.
pub async fn free_games(region: &DealsRegion) -> (Vec<FreeGame>, Vec<&'static str>) {
    let (epic, gp) = futures::join!(epic_free_games(region), gamerpower_free_games());
    let mut combined = Vec::new();
    let mut failed = Vec::new();
    match epic {
        Ok(list) => combined.extend(list),
        Err(e) => {
            tracing::warn!("free games from Epic: {e:#}");
            failed.push("Epic Games");
        }
    }
    match gp {
        Ok(list) => combined.extend(list),
        Err(e) => {
            tracing::warn!("free games from GamerPower: {e:#}");
            failed.push("GamerPower");
        }
    }
    combined.sort_by_key(|g| g.upcoming);
    let mut seen = HashSet::new();
    combined.retain(|g| {
        let key = (g.store, irori_core::sort_key(&g.title));
        seen.insert(key)
    });
    (combined, failed)
}
