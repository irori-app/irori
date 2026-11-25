//! Supported country/currency regions for store deals and GG.deals URLs.

use crate::util;

/// A country and currency pair used across Steam, GOG, Epic, GMG and GG.deals.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DealsRegion {
    /// Two-letter ISO 3166-1 alpha-2 country code (`BR`, `US`, `DE`…).
    pub code: &'static str,
    /// Currency code (`BRL`, `USD`, `EUR`…).
    pub currency_code: &'static str,
    /// GOG/Epic locale string (`pt-BR`, `en-US`, `de-DE`…).
    pub locale: &'static str,
    /// GG.deals path segment (`br`, `us`, `eu`, `uk`…).
    pub gg_deals_slug: &'static str,
    /// Compact display label (`Brazil (BRL)`).
    pub label: &'static str,
}

pub const DEALS_REGIONS: &[DealsRegion] = &[
    DealsRegion {
        code: "BR",
        currency_code: "BRL",
        locale: "pt-BR",
        gg_deals_slug: "br",
        label: "Brazil (BRL)",
    },
    DealsRegion {
        code: "US",
        currency_code: "USD",
        locale: "en-US",
        gg_deals_slug: "us",
        label: "United States (USD)",
    },
    DealsRegion {
        code: "GB",
        currency_code: "GBP",
        locale: "en-GB",
        gg_deals_slug: "uk",
        label: "United Kingdom (GBP)",
    },
    DealsRegion {
        code: "DE",
        currency_code: "EUR",
        locale: "de-DE",
        gg_deals_slug: "de",
        label: "Germany (EUR)",
    },
    DealsRegion {
        code: "FR",
        currency_code: "EUR",
        locale: "fr-FR",
        gg_deals_slug: "fr",
        label: "France (EUR)",
    },
    DealsRegion {
        code: "ES",
        currency_code: "EUR",
        locale: "es-ES",
        gg_deals_slug: "es",
        label: "Spain (EUR)",
    },
    DealsRegion {
        code: "IT",
        currency_code: "EUR",
        locale: "it-IT",
        gg_deals_slug: "it",
        label: "Italy (EUR)",
    },
    DealsRegion {
        code: "PT",
        currency_code: "EUR",
        locale: "pt-PT",
        gg_deals_slug: "pt",
        label: "Portugal (EUR)",
    },
    DealsRegion {
        code: "NL",
        currency_code: "EUR",
        locale: "nl-NL",
        gg_deals_slug: "nl",
        label: "Netherlands (EUR)",
    },
    DealsRegion {
        code: "CA",
        currency_code: "CAD",
        locale: "en-CA",
        gg_deals_slug: "ca",
        label: "Canada (CAD)",
    },
    DealsRegion {
        code: "AU",
        currency_code: "AUD",
        locale: "en-AU",
        gg_deals_slug: "au",
        label: "Australia (AUD)",
    },
    DealsRegion {
        code: "PL",
        currency_code: "PLN",
        locale: "pl-PL",
        gg_deals_slug: "pl",
        label: "Poland (PLN)",
    },
    DealsRegion {
        code: "JP",
        currency_code: "JPY",
        locale: "ja-JP",
        gg_deals_slug: "jp",
        label: "Japan (JPY)",
    },
    DealsRegion {
        code: "KR",
        currency_code: "KRW",
        locale: "ko-KR",
        gg_deals_slug: "kr",
        label: "South Korea (KRW)",
    },
    DealsRegion {
        code: "TR",
        currency_code: "USD",
        locale: "tr-TR",
        gg_deals_slug: "tr",
        label: "Turkey (USD)",
    },
    DealsRegion {
        code: "SE",
        currency_code: "SEK",
        locale: "sv-SE",
        gg_deals_slug: "se",
        label: "Sweden (SEK)",
    },
    DealsRegion {
        code: "CN",
        currency_code: "CNY",
        locale: "zh-CN",
        gg_deals_slug: "cn",
        label: "China (CNY)",
    },
];

fn by_code(code: &str) -> Option<&'static DealsRegion> {
    let norm = code.trim();
    DEALS_REGIONS
        .iter()
        .find(|r| r.code.eq_ignore_ascii_case(norm))
}

fn default_region() -> &'static DealsRegion {
    by_code("US").expect("US region exists")
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
        && let Some(r) = by_code(clean)
    {
        return r;
    }
    if let Some(locale) = system_locale
        && let Some(country) = country_from_locale(locale)
        && let Some(r) = by_code(country)
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
