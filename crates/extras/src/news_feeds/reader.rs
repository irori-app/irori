//! On-demand pure-Rust Reader Mode for RSS articles whose feed only ships a
//! short summary. Strips scripts, navigation, and boilerplate, extracts the
//! `<article>` or `<main>` HTML, caches for 7 days, and falls back gracefully
//! to the RSS summary on HTTP errors or paywalls.

use std::path::PathBuf;
use std::time::Duration;

use once_cell::sync::Lazy;
use regex::Regex;
use serde::{Deserialize, Serialize};

use super::{decode_entities, download_text_bounded, is_http_url, strip_tags_plain, url_hash};
use crate::util::{cache_dir, cache_read, cache_write};

/// Cached Reader Mode articles stay valid for 7 days.
const ARTICLE_TTL: Duration = Duration::from_secs(7 * 24 * 60 * 60);
/// Per-article HTTP timeout.
const ARTICLE_TIMEOUT: Duration = Duration::from_secs(15);
/// Hard byte cap per HTML page fetched for Reader Mode.
const MAX_ARTICLE_BYTES: usize = 1_500_000;
/// If the RSS feed already provides at least this many characters of plain
/// prose, skip fetching the web page entirely.
const RICH_RSS_CHARS: usize = 600;
/// Minimum extracted prose characters required to prefer Reader Mode output
/// over the RSS summary.
const MIN_EXTRACTED_CHARS: usize = 220;

/// Resolved article HTML ready for Irori's native `rich_text` renderer.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ArticleContent {
    pub body_html: String,
    pub image_url: Option<String>,
    pub used_reader_mode: bool,
}

fn article_cache_path(url: &str) -> PathBuf {
    cache_dir("news")
        .join("articles")
        .join(format!("{}.json", url_hash(url)))
}

/// Fetch readable article HTML for `url` on demand, returning cached or RSS
/// fallback content whenever the feed already has full text or the publisher
/// blocks scraping.
pub async fn fetch_reader_article(
    url: &str,
    fallback_html: &str,
    fallback_image: Option<&str>,
) -> ArticleContent {
    let fallback = ArticleContent {
        body_html: fallback_html.to_string(),
        image_url: fallback_image.map(str::to_string),
        used_reader_mode: false,
    };

    if strip_tags_plain(fallback_html).chars().count() >= RICH_RSS_CHARS || !is_http_url(url) {
        return fallback;
    }

    let path = article_cache_path(url);
    if let Some(cached) = cache_read::<ArticleContent>(&path, ARTICLE_TTL).await {
        return cached;
    }

    let Ok(page_html) = download_text_bounded(url, ARTICLE_TIMEOUT, MAX_ARTICLE_BYTES).await else {
        return fallback;
    };

    let Some((extracted_html, og_image)) = extract_readable_html(&page_html) else {
        return fallback;
    };

    let content = ArticleContent {
        body_html: extracted_html,
        image_url: fallback.image_url.or(og_image),
        used_reader_mode: true,
    };
    let _ = cache_write(&path, &content).await;
    content
}

static RE_OG_IMAGE_A: Lazy<Option<Regex>> = Lazy::new(|| {
    Regex::new(
        r#"(?is)<meta\b[^>]*\b(?:property|name)\s*=\s*["']og:image["'][^>]*\bcontent\s*=\s*["'](https?://[^"']+)["']"#,
    )
    .ok()
});
static RE_OG_IMAGE_B: Lazy<Option<Regex>> = Lazy::new(|| {
    Regex::new(
        r#"(?is)<meta\b[^>]*\bcontent\s*=\s*["'](https?://[^"']+)["'][^>]*\b(?:property|name)\s*=\s*["']og:image["']"#,
    )
    .ok()
});
static RE_STRIP_NOISE: Lazy<Option<Regex>> = Lazy::new(|| {
    Regex::new(
        r"(?is)<(script|style|noscript|svg|nav|aside|footer|header|form|iframe|button)\b[^>]*>.*?</(script|style|noscript|svg|nav|aside|footer|header|form|iframe|button)>",
    )
    .ok()
});
static RE_COMMENTS: Lazy<Option<Regex>> = Lazy::new(|| Regex::new(r"(?s)<!--.*?-->").ok());
static RE_ARTICLE: Lazy<Option<Regex>> =
    Lazy::new(|| Regex::new(r"(?is)<article\b[^>]*>(.*?)</article>").ok());
static RE_MAIN: Lazy<Option<Regex>> =
    Lazy::new(|| Regex::new(r"(?is)<main\b[^>]*>(.*?)</main>").ok());

/// Extract clean `<article>` or `<main>` HTML and optional `og:image` from raw
/// HTML without spawning any web engine.
pub(crate) fn extract_readable_html(page_html: &str) -> Option<(String, Option<String>)> {
    let og_image = RE_OG_IMAGE_A
        .as_ref()
        .and_then(|re| re.captures(page_html))
        .or_else(|| RE_OG_IMAGE_B.as_ref().and_then(|re| re.captures(page_html)))
        .and_then(|c| c.get(1))
        .map(|m| decode_entities(m.as_str()))
        .filter(|u| is_http_url(u));

    let without_comments = match RE_COMMENTS.as_ref() {
        Some(re) => re.replace_all(page_html, ""),
        None => std::borrow::Cow::Borrowed(page_html),
    };
    let raw_fragment = RE_ARTICLE
        .as_ref()
        .and_then(|re| {
            re.captures_iter(&without_comments)
                .filter_map(|c| c.get(1).map(|m| m.as_str()))
                .max_by_key(|frag| frag.len())
        })
        .or_else(|| {
            RE_MAIN
                .as_ref()
                .and_then(|re| re.captures(&without_comments))
                .and_then(|c| c.get(1))
                .map(|m| m.as_str())
        })?;

    // Two passes handle one level of nested noisy wrappers (e.g. `<aside><form>`).
    let cleaned = if let Some(re) = RE_STRIP_NOISE.as_ref() {
        let once = re.replace_all(raw_fragment, "");
        re.replace_all(&once, "").into_owned()
    } else {
        raw_fragment.to_string()
    };
    let plain_len = strip_tags_plain(&cleaned).chars().count();
    if plain_len < MIN_EXTRACTED_CHARS {
        return None;
    }
    Some((cleaned, og_image))
}
