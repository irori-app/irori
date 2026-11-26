//! Supported country/currency regions for store deals and GG.deals URLs.

use crate::util;

/// A country and currency pair used across Steam, GOG, Epic, GMG and GG.deals.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DealsRegion {
    /// Two-letter ISO 3166-1 alpha-2 country code (`BR`, `US`, `EU`…).
    pub code: &'static str,
    /// Two-letter ISO 3166-1 alpha-2 country code sent to store APIs (Steam, GOG, Epic).
    pub store_country: &'static str,
    /// Currency code (`BRL`, `USD`, `EUR`…).
    pub currency_code: &'static str,
    /// GOG/Epic locale string (`pt-BR`, `en-US`, `pl-PL`…).
    pub locale: &'static str,
    /// GG.deals path segment (`br`, `us`, `eu`, `uk`…).
    pub gg_deals_slug: &'static str,
    /// Compact display label (`Brazil (BRL)`).
    pub label: &'static str,
}

const EUROZONE_COUNTRIES: &[&str] = &[
    "AT", "BE", "CY", "DE", "EE", "ES", "FI", "FR", "GR", "HR", "IE", "IT", "LT", "LU", "LV", "MT",
    "NL", "PT", "SI", "SK",
];

pub const DEALS_REGIONS: &[DealsRegion] = &[
    DealsRegion {
        code: "BR",
        store_country: "BR",
        currency_code: "BRL",
        locale: "pt-BR",
        gg_deals_slug: "br",
        label: "Brazil (BRL)",
    },
    DealsRegion {
        code: "US",
        store_country: "US",
        currency_code: "USD",
        locale: "en-US",
        gg_deals_slug: "us",
        label: "United States (USD)",
    },
    DealsRegion {
        code: "GB",
        store_country: "GB",
        currency_code: "GBP",
        locale: "en-GB",
        gg_deals_slug: "uk",
        label: "United Kingdom (GBP)",
    },
    DealsRegion {
        code: "EU",
        store_country: "NL",
        currency_code: "EUR",
        locale: "en-US",
        gg_deals_slug: "eu",
        label: "Europe (EUR)",
    },
    DealsRegion {
        code: "CA",
        store_country: "CA",
        currency_code: "CAD",
        locale: "en-CA",
        gg_deals_slug: "ca",
        label: "Canada (CAD)",
    },
    DealsRegion {
        code: "AU",
        store_country: "AU",
        currency_code: "AUD",
        locale: "en-AU",
        gg_deals_slug: "au",
        label: "Australia (AUD)",
    },
    DealsRegion {
        code: "PL",
        store_country: "PL",
        currency_code: "PLN",
        locale: "pl-PL",
        gg_deals_slug: "pl",
        label: "Poland (PLN)",
    },
    DealsRegion {
        code: "JP",
        store_country: "JP",
        currency_code: "JPY",
        locale: "ja-JP",
        gg_deals_slug: "jp",
        label: "Japan (JPY)",
    },
    DealsRegion {
        code: "KR",
        store_country: "KR",
        currency_code: "KRW",
        locale: "ko-KR",
        gg_deals_slug: "kr",
        label: "South Korea (KRW)",
    },
    DealsRegion {
        code: "TR",
        store_country: "TR",
        currency_code: "USD",
        locale: "tr-TR",
        gg_deals_slug: "tr",
        label: "Turkey (USD)",
    },
    DealsRegion {
        code: "SE",
        store_country: "SE",
        currency_code: "SEK",
        locale: "sv-SE",
        gg_deals_slug: "se",
        label: "Sweden (SEK)",
    },
    DealsRegion {
        code: "CN",
        store_country: "CN",
        currency_code: "CNY",
        locale: "zh-CN",
        gg_deals_slug: "cn",
        label: "China (CNY)",
    },
];

/// Find a deals region by explicit region code or Eurozone country code.
pub fn find_region(code: &str) -> Option<&'static DealsRegion> {
    let norm = code.trim();
    if let Some(region) = DEALS_REGIONS
        .iter()
        .find(|r| r.code.eq_ignore_ascii_case(norm))
    {
        return Some(region);
    }
    if EUROZONE_COUNTRIES
        .iter()
        .any(|c| c.eq_ignore_ascii_case(norm))
    {
        return DEALS_REGIONS.iter().find(|r| r.code == "EU");
    }
    None
}

fn default_region() -> &'static DealsRegion {
    find_region("US").expect("US region exists")
}

/// Extract a 2-letter country code from locale tags like `"pt-BR"` or `"en_GB.UTF-8"`.
fn country_from_locale(locale: &str) -> Option<&str> {
    let base = locale.split(['.', '@']).next()?.trim();
    let (_, region) = base.split_once(['-', '_'])?;
    (region.len() == 2 && region.chars().all(|c| c.is_ascii_alphabetic())).then_some(region)
}

/// Resolve the active deals region from `iroriDealsRegion` (`""`/`"auto"` uses
/// the system locale, falling back to `US`).
pub fn resolve_region(setting: &str, system_locale: Option<&str>) -> &'static DealsRegion {
    let clean = setting.trim();
    if !clean.is_empty()
        && !clean.eq_ignore_ascii_case("auto")
        && let Some(r) = find_region(clean)
    {
        return r;
    }
    if let Some(locale) = system_locale
        && let Some(country) = country_from_locale(locale)
        && let Some(r) = find_region(country)
    {
        return r;
    }
    default_region()
}

/// Regional deals browse URL on GG.deals.
pub fn gg_deals_browse_url(region: &DealsRegion) -> String {
    format!("https://gg.deals/{}/deals/", region.gg_deals_slug)
}

/// Regional search URL on GG.deals for comparing official stores and keyshops.
pub fn gg_deals_search_url(title: &str, region: &DealsRegion) -> String {
    format!(
        "https://gg.deals/{}/games/?title={}",
        region.gg_deals_slug,
        util::encode_component(title.trim())
    )
}

/// ProtonDB app report URL for a Steam app ID.
pub fn protondb_app_url(steam_app_id: &str) -> String {
    format!("https://www.protondb.com/app/{}", steam_app_id.trim())
}

/// ProtonDB search URL when a Steam app ID is unavailable.
pub fn protondb_search_url(title: &str) -> String {
    format!(
        "https://www.protondb.com/search?q={}",
        util::encode_component(title.trim())
    )
}
