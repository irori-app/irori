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

fn parse_money_field(val: Option<&serde_json::Value>, currency: &str) -> Option<String> {
    let val = val?;
    let amount = match val {
        serde_json::Value::String(s) => s.trim().parse::<f64>().ok()?,
        serde_json::Value::Number(n) => n.as_f64()?,
        _ => return None,
    };
    (amount.is_finite() && amount >= 0.0).then(|| format_money(amount, currency))
}

fn api_error_message(bytes: &[u8]) -> Option<String> {
    let v: serde_json::Value = serde_json::from_slice(bytes).ok()?;
    v.pointer("/data/message")
        .or_else(|| v.get("message"))
        .and_then(|m| m.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// Parse GG.deals `/v1/prices/by-steam-app-id/` JSON response, tolerating
/// per-entry `null` or PHP empty `[]` placeholders returned for unindexed IDs.
pub fn parse_ggdeals_prices(
    bytes: &[u8],
    default_currency: &str,
) -> Result<HashMap<String, GgDealsPrice>> {
    let root: serde_json::Value =
        serde_json::from_slice(bytes).context("parsing GG.deals API response")?;
    if !root
        .get("success")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
    {
        let msg = api_error_message(bytes).unwrap_or_else(|| "success=false".into());
        bail!("GG.deals API error: {msg}");
    }
    let Some(data) = root.get("data").and_then(|d| d.as_object()) else {
        return Ok(HashMap::new());
    };
    let mut out = HashMap::new();
    for (app_id, entry_val) in data {
        let Some(entry) = entry_val.as_object() else {
            continue;
        };
        let Some(prices) = entry.get("prices").and_then(|p| p.as_object()) else {
            continue;
        };
        let currency = prices
            .get("currency")
            .and_then(|c| c.as_str())
            .map(str::trim)
            .filter(|c| !c.is_empty())
            .map(str::to_uppercase)
            .unwrap_or_else(|| default_currency.to_string());
        let url = entry
            .get("url")
            .and_then(|u| u.as_str())
            .map(str::trim)
            .filter(|u| !u.is_empty())
            .map(str::to_string);
        out.insert(
            app_id.clone(),
            GgDealsPrice {
                retail: parse_money_field(prices.get("currentRetail"), &currency),
                keyshop: parse_money_field(prices.get("currentKeyshops"), &currency),
                historical_retail: parse_money_field(prices.get("historicalRetail"), &currency),
                historical_keyshop: parse_money_field(prices.get("historicalKeyshops"), &currency),
                currency,
                url,
            },
        );
    }
    Ok(out)
}

async fn fetch_ggdeals_chunk(
    ids: &str,
    key: &str,
    slug: &str,
    default_currency: &str,
) -> Result<HashMap<String, GgDealsPrice>> {
    let resp = irori_core::http::client()
        .get(GGDEALS_PRICES_URL)
        .query(&[("ids", ids), ("key", key), ("region", slug)])
        .header(reqwest::header::USER_AGENT, user_agent())
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("GG.deals request failed: {}", e.without_url()))?;
    let status = resp.status();
    let bytes = resp.bytes().await?;
    if !status.is_success() {
        let detail = api_error_message(&bytes).unwrap_or_else(|| format!("HTTP {status}"));
        bail!("GG.deals API ({slug}): {detail}");
    }
    parse_ggdeals_prices(&bytes, default_currency)
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
        let parsed = match fetch_ggdeals_chunk(
            &ids,
            key,
            region.gg_deals_slug,
            region.currency_code,
        )
        .await
        {
            Ok(p) => p,
            Err(e)
                if region.gg_deals_slug != "us"
                    && e.to_string().to_ascii_lowercase().contains("region") =>
            {
                tracing::info!(
                    "GG.deals region '{}' rejected, falling back to 'us'",
                    region.gg_deals_slug
                );
                fetch_ggdeals_chunk(&ids, key, "us", "USD").await?
            }
            Err(e) => return Err(e),
        };
        for (id, entry) in parsed {
            let cache = dir.join(format!("{id}.json"));
            let _ = util::cache_write(&cache, &entry).await;
            out.insert(id, entry);
        }
    }
    Ok(out)
}
