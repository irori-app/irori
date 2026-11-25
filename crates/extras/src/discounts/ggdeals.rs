//! Optional GG.deals API v1 integration for comparing Official stores vs
//! Keyshops and historical lows by Steam app ID.
//!
//! Security: `api_key` and URLs containing `?key=...` are never logged.

use std::collections::HashMap;
use std::time::Duration;

use anyhow::{Context as _, Result, bail};
use serde::{Deserialize, Serialize};

use super::{DealsRegion, format_money, user_agent};
use crate::util;

const GGDEALS_PRICES_URL: &str = "https://api.gg.deals/v1/prices/by-steam-app-id/";
const CACHE_TTL: Duration = Duration::from_secs(3600);

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GgDealsPrice {
    pub retail: Option<String>,
    pub keyshop: Option<String>,
    pub historical_retail: Option<String>,
    pub historical_keyshop: Option<String>,
    pub currency: String,
    pub url: Option<String>,
}

#[derive(Deserialize)]
struct ApiResponse {
    #[serde(default)]
    success: bool,
    #[serde(default)]
    data: HashMap<String, Option<ApiEntry>>,
}

#[derive(Deserialize)]
struct ApiEntry {
    url: Option<String>,
    prices: Option<ApiPrices>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ApiPrices {
    current_retail: Option<serde_json::Value>,
    current_keyshops: Option<serde_json::Value>,
    historical_retail: Option<serde_json::Value>,
    historical_keyshops: Option<serde_json::Value>,
    #[serde(default)]
    currency: String,
}

fn parse_money_field(val: Option<&serde_json::Value>, currency: &str) -> Option<String> {
    let val = val?;
    let amount = match val {
        serde_json::Value::String(s) => s.trim().parse::<f64>().ok()?,
        serde_json::Value::Number(n) => n.as_f64()?,
        _ => return None,
    };
    (amount.is_finite() && amount >= 0.0).then(|| format_money(amount, currency))
}

/// Parse GG.deals `/v1/prices/by-steam-app-id/` JSON response.
pub fn parse_ggdeals_prices(
    bytes: &[u8],
    default_currency: &str,
) -> Result<HashMap<String, GgDealsPrice>> {
    let parsed: ApiResponse =
        serde_json::from_slice(bytes).context("parsing GG.deals API response")?;
    if !parsed.success {
        bail!("GG.deals API returned success=false");
    }
    let mut out = HashMap::new();
    for (app_id, entry) in parsed.data {
        let Some(entry) = entry else {
            continue;
        };
        let Some(prices) = entry.prices else {
            continue;
        };
        let currency = if prices.currency.trim().is_empty() {
            default_currency.to_string()
        } else {
            prices.currency.to_uppercase()
        };
        out.insert(
            app_id,
            GgDealsPrice {
                retail: parse_money_field(prices.current_retail.as_ref(), &currency),
                keyshop: parse_money_field(prices.current_keyshops.as_ref(), &currency),
                historical_retail: parse_money_field(prices.historical_retail.as_ref(), &currency),
                historical_keyshop: parse_money_field(
                    prices.historical_keyshops.as_ref(),
                    &currency,
                ),
                currency,
                url: entry.url.filter(|u| !u.trim().is_empty()),
            },
        );
    }
    Ok(out)
}

/// Fetch Official vs Keyshop prices and historical lows for a batch of Steam
/// app IDs, cached per app+region on disk for [`CACHE_TTL`].
pub async fn ggdeals_prices(
    api_key: &str,
    region: &DealsRegion,
    app_ids: &[String],
) -> Result<HashMap<String, GgDealsPrice>> {
    let key = api_key.trim();
    if key.is_empty() {
        return Ok(HashMap::new());
    }
    let mut unique: Vec<String> = app_ids
        .iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty() && s.chars().all(|c| c.is_ascii_digit()))
        .collect();
    unique.sort();
    unique.dedup();

    let mut out = HashMap::new();
    let mut missing = Vec::new();
    let dir = util::cache_dir("discounts").join(format!("ggdeals-{}", region.gg_deals_slug));
    for id in unique {
        let cache = dir.join(format!("{id}.json"));
        if let Some(hit) = util::cache_read::<GgDealsPrice>(&cache, CACHE_TTL).await {
            out.insert(id, hit);
        } else {
            missing.push(id);
        }
    }

    for chunk in missing.chunks(100) {
        let ids = chunk.join(",");
        let resp = irori_core::http::client()
            .get(GGDEALS_PRICES_URL)
            .query(&[
                ("ids", ids.as_str()),
                ("key", key),
                ("region", region.gg_deals_slug),
            ])
            .header(reqwest::header::USER_AGENT, user_agent())
            .timeout(Duration::from_secs(15))
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("GG.deals request failed: {}", e.without_url()))?;
        let status = resp.status();
        if !status.is_success() {
            bail!("GG.deals API returned HTTP {status}");
        }
        let bytes = resp.bytes().await?;
        let parsed = parse_ggdeals_prices(&bytes, region.currency_code)?;
        for (id, entry) in parsed {
            let cache = dir.join(format!("{id}.json"));
            let _ = util::cache_write(&cache, &entry).await;
            out.insert(id, entry);
        }
    }
    Ok(out)
}
