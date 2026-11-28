//! Gaming news RSS/Atom aggregator and on-demand pure-Rust Reader Mode.
//! Loads only when the user opens `Stores -> News -> Gaming news`, uses bounded
//! HTTP downloads and disk caches, and requires no web engine or API keys.

mod reader;

use std::collections::HashSet;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Result, bail};
use futures::stream::{self, StreamExt as _};
use irori_core::{Paths, json_store};
use md5::{Digest as _, Md5};
use once_cell::sync::Lazy;
use quick_xml::Reader;
use quick_xml::events::{BytesStart, Event};
use regex::Regex;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use time::format_description::well_known::{Rfc2822, Rfc3339};
use url::Url;

use crate::util::{cache_dir, cache_read, cache_read_entry, cache_write};

pub use reader::{ArticleContent, fetch_reader_article};

/// Feed cache freshness window (30 minutes).
const FEED_TTL: Duration = Duration::from_secs(30 * 60);
/// Per-feed HTTP request timeout.
const FEED_TIMEOUT: Duration = Duration::from_secs(15);
/// Hard byte cap per RSS/Atom response so malformed feeds cannot exhaust RAM.
const MAX_FEED_BYTES: usize = 2_500_000;
/// Maximum parallel feed requests during a refresh.
const FEED_CONCURRENCY: usize = 4;
/// Maximum items retained per feed source.
const MAX_ITEMS_PER_FEED: usize = 30;
/// Maximum characters kept in a plain-text card excerpt.
const EXCERPT_CHARS: usize = 190;

/// Built-in gaming-only RSS preset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PresetFeed {
    pub id: &'static str,
    pub name: &'static str,
    pub url: &'static str,
    pub site_url: &'static str,
}

/// Curated gaming-only English RSS feeds (live-verified editorial sources, no
/// mixed hardware-deal feeds).
pub const PRESET_FEEDS: &[PresetFeed] = &[
    PresetFeed {
        id: "rps",
        name: "Rock Paper Shotgun",
        url: "https://www.rockpapershotgun.com/feed/news",
        site_url: "https://www.rockpapershotgun.com/",
    },
    PresetFeed {
        id: "eurogamer",
        name: "Eurogamer",
        url: "https://www.eurogamer.net/feed/news",
        site_url: "https://www.eurogamer.net/",
    },
    PresetFeed {
        id: "gamingonlinux",
        name: "GamingOnLinux",
        url: "https://www.gamingonlinux.com/article_rss.php",
        site_url: "https://www.gamingonlinux.com/",
    },
    PresetFeed {
        id: "pcgamesn",
        name: "PCGamesN",
        url: "https://www.pcgamesn.com/mainrss.xml",
        site_url: "https://www.pcgamesn.com/",
    },
    PresetFeed {
        id: "polygon",
        name: "Polygon",
        url: "https://www.polygon.com/rss/index.xml",
        site_url: "https://www.polygon.com/",
    },
];

/// A user-added custom RSS or Atom feed.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CustomFeed {
    pub id: String,
    pub name: String,
    pub url: String,
}

/// Saved feed preferences in `<config>/news-feeds.json`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct NewsFeedsConfig {
    pub disabled_presets: Vec<String>,
    pub custom_feeds: Vec<CustomFeed>,
}

/// Resolved active feed source (preset or custom).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeedSource {
    pub id: String,
    pub name: String,
    pub url: String,
}

/// One normalized article parsed from an RSS 2.0 or Atom feed.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct FeedArticle {
    pub id: String,
    pub source_id: String,
    pub source_name: String,
    pub title: String,
    pub url: String,
    pub published_at: u64,
    pub author: String,
    pub summary: String,
    pub body_html: String,
    pub image_url: Option<String>,
}

/// Result of refreshing all enabled RSS/Atom sources.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FeedFetchResult {
    pub articles: Vec<FeedArticle>,
    pub failed_sources: Vec<String>,
}

fn config_path() -> PathBuf {
    Paths::get().config.join("news-feeds.json")
}

/// Read the user's saved RSS/Atom feed preferences.
pub fn load_config() -> NewsFeedsConfig {
    json_store::read_or_default(&config_path())
}

/// Persist the user's RSS/Atom feed preferences atomically.
pub fn save_config(cfg: &NewsFeedsConfig) -> Result<()> {
    json_store::write(&config_path(), cfg)
}

/// Validate and normalize a user-supplied RSS/Atom URL (`http://` or `https://`).
pub fn validate_feed_url(raw: &str) -> Result<String> {
    let trimmed = raw.trim();
    let parsed = Url::parse(trimmed).map_err(|_| anyhow::anyhow!("invalid feed URL"))?;
    if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
        bail!("feed URL must start with http:// or https://");
    }
    Ok(parsed.to_string())
}

/// Active feed sources in display order (enabled presets followed by custom feeds).
pub fn enabled_sources(cfg: &NewsFeedsConfig) -> Vec<FeedSource> {
    let disabled: HashSet<&str> = cfg.disabled_presets.iter().map(String::as_str).collect();
    let mut out: Vec<FeedSource> = PRESET_FEEDS
        .iter()
        .filter(|p| !disabled.contains(p.id))
        .map(|p| FeedSource {
            id: p.id.to_string(),
            name: p.name.to_string(),
            url: p.url.to_string(),
        })
        .collect();
    for custom in &cfg.custom_feeds {
        if !custom.url.trim().is_empty() {
            out.push(FeedSource {
                id: custom.id.clone(),
                name: custom.name.clone(),
                url: custom.url.clone(),
            });
        }
    }
    out
}

pub(crate) fn url_hash(url: &str) -> String {
    let digest = Md5::digest(url.as_bytes());
    format!("{digest:x}")
}

fn feed_cache_path(url: &str) -> PathBuf {
    cache_dir("news")
        .join("feeds")
        .join(format!("{}.json", url_hash(url)))
}

/// Read cached articles for all enabled feeds without touching the network.
pub async fn fetch_cached_feeds(cfg: &NewsFeedsConfig) -> Vec<FeedArticle> {
    let sources = enabled_sources(cfg);
    let mut all = Vec::new();
    for source in sources {
        if let Some((items, _)) =
            cache_read_entry::<Vec<FeedArticle>>(&feed_cache_path(&source.url)).await
        {
            all.extend(items);
        }
    }
    dedup_and_sort(all)
}

/// Refresh enabled RSS/Atom feeds in parallel, falling back to stale cache per
/// source if a server fails.
pub async fn fetch_feeds(cfg: &NewsFeedsConfig, force: bool) -> FeedFetchResult {
    let sources = enabled_sources(cfg);
    let results: Vec<(String, Vec<FeedArticle>, bool)> = stream::iter(sources)
        .map(|source| async move {
            let path = feed_cache_path(&source.url);
            if !force && let Some(cached) = cache_read::<Vec<FeedArticle>>(&path, FEED_TTL).await {
                return (source.name, cached, false);
            }
            match download_text_bounded(&source.url, FEED_TIMEOUT, MAX_FEED_BYTES).await {
                Ok(xml) => {
                    let parsed = parse_feed_xml(&xml, &source);
                    if !parsed.is_empty() {
                        let _ = cache_write(&path, &parsed).await;
                        return (source.name, parsed, false);
                    }
                    let stale = cache_read_entry::<Vec<FeedArticle>>(&path)
                        .await
                        .map(|(v, _)| v)
                        .unwrap_or_default();
                    (source.name, stale, true)
                }
                Err(e) => {
                    tracing::warn!("fetching feed {} ({}): {e:#}", source.name, source.url);
                    let stale = cache_read_entry::<Vec<FeedArticle>>(&path)
                        .await
                        .map(|(v, _)| v)
                        .unwrap_or_default();
                    (source.name, stale, true)
                }
            }
        })
        .buffer_unordered(FEED_CONCURRENCY)
        .collect()
        .await;

    let mut articles = Vec::new();
    let mut failed_sources = Vec::new();
    for (name, items, failed) in results {
        if failed {
            failed_sources.push(name);
        }
        articles.extend(items);
    }
    failed_sources.sort();
    FeedFetchResult {
        articles: dedup_and_sort(articles),
        failed_sources,
    }
}

pub(crate) async fn download_text_bounded(
    url: &str,
    timeout: Duration,
    max_bytes: usize,
) -> Result<String> {
    let mut resp = irori_core::http::client()
        .get(url)
        .timeout(timeout)
        .send()
        .await?
        .error_for_status()?;
    if resp.content_length().is_some_and(|n| n > max_bytes as u64) {
        bail!("response exceeds {max_bytes} bytes");
    }
    let mut buf = Vec::new();
    while let Some(chunk) = resp.chunk().await? {
        if buf.len().saturating_add(chunk.len()) > max_bytes {
            bail!("response exceeds {max_bytes} bytes");
        }
        buf.extend_from_slice(&chunk);
    }
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

fn dedup_and_sort(mut articles: Vec<FeedArticle>) -> Vec<FeedArticle> {
    articles.sort_by(|a, b| {
        b.published_at
            .cmp(&a.published_at)
            .then_with(|| a.title.cmp(&b.title))
    });
    let mut seen = HashSet::new();
    articles.retain(|a| {
        let key = if !a.url.is_empty() {
            a.url.as_str()
        } else {
            a.id.as_str()
        };
        seen.insert(key.to_string())
    });
    articles
}

#[derive(Default)]
struct RawEntry {
    id: String,
    title: String,
    url: String,
    date_raw: String,
    author: String,
    description: String,
    content: String,
    image_url: Option<String>,
}

/// Parse RSS 2.0 (`<item>`) or Atom (`<entry>`) XML into normalized articles.
pub fn parse_feed_xml(xml: &str, source: &FeedSource) -> Vec<FeedArticle> {
    let mut reader = Reader::from_str(xml);
    let mut in_entry = false;
    let mut in_author = false;
    let mut field: Option<&'static str> = None;
    let mut text = String::new();
    let mut current = RawEntry::default();
    let mut out = Vec::new();

    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => {
                let local = local_name_lower(&e);
                let full = full_name_lower(&e);
                if !in_entry && matches!(local.as_str(), "item" | "entry") {
                    in_entry = true;
                    in_author = false;
                    current = RawEntry::default();
                } else if in_entry {
                    inspect_media_attrs(&e, &local, &mut current);
                    if local == "link" {
                        extract_atom_link(&e, &mut current);
                    }
                    if local == "author" {
                        in_author = true;
                    }
                    field = match_entry_field(&local, &full, in_author);
                    text.clear();
                }
            }
            Ok(Event::Empty(e)) if in_entry => {
                let local = local_name_lower(&e);
                inspect_media_attrs(&e, &local, &mut current);
                if local == "link" {
                    extract_atom_link(&e, &mut current);
                }
            }
            Ok(Event::Text(t)) if in_entry && field.is_some() => {
                text.push_str(&t.xml10_content());
            }
            Ok(Event::CData(t)) if in_entry && field.is_some() => {
                text.push_str(t.as_ref());
            }
            Ok(Event::GeneralRef(r)) if in_entry && field.is_some() => {
                if let Ok(Some(c)) = r.resolve_char_ref() {
                    text.push(c);
                } else if let Some(resolved) =
                    quick_xml::escape::resolve_predefined_entity(&r.xml10_content())
                {
                    text.push_str(resolved);
                }
            }
            Ok(Event::End(e)) => {
                let local = e.local_name().as_ref().to_ascii_lowercase();
                if in_entry && matches!(local.as_str(), "item" | "entry") {
                    in_entry = false;
                    in_author = false;
                    field = None;
                    if let Some(article) = finalize_entry(std::mem::take(&mut current), source) {
                        out.push(article);
                        if out.len() >= MAX_ITEMS_PER_FEED {
                            break;
                        }
                    }
                } else if in_entry {
                    if local == "author" {
                        in_author = false;
                    }
                    if let Some(f) = field.take() {
                        apply_field(&mut current, f, text.trim());
                        text.clear();
                    }
                }
            }
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
    }
    out
}

fn local_name_lower(e: &BytesStart<'_>) -> String {
    e.local_name().as_ref().to_ascii_lowercase()
}

fn full_name_lower(e: &BytesStart<'_>) -> String {
    e.name().as_ref().to_ascii_lowercase()
}

fn match_entry_field(local: &str, full: &str, in_author: bool) -> Option<&'static str> {
    if in_author && local == "name" {
        return Some("author");
    }
    match (local, full) {
        ("title", _) => Some("title"),
        ("link", _) => Some("link"),
        ("guid" | "id", _) => Some("id"),
        ("pubdate" | "published" | "updated", _) => Some("date"),
        ("creator" | "author", _) => Some("author"),
        ("encoded", f) if f.contains("content") => Some("content"),
        ("content", _) => Some("content"),
        ("description" | "summary", _) => Some("description"),
        _ => None,
    }
}

fn apply_field(entry: &mut RawEntry, field: &str, value: &str) {
    if value.is_empty() {
        return;
    }
    match field {
        "title" if entry.title.is_empty() => entry.title = decode_entities(value),
        "link" if entry.url.is_empty() => entry.url = value.to_string(),
        "id" if entry.id.is_empty() => entry.id = value.to_string(),
        "date" if entry.date_raw.is_empty() => entry.date_raw = value.to_string(),
        "author" if entry.author.is_empty() => entry.author = decode_entities(value),
        "content" if value.len() > entry.content.len() => entry.content = value.to_string(),
        "description" if entry.description.is_empty() => entry.description = value.to_string(),
        _ => {}
    }
}

fn inspect_media_attrs(e: &BytesStart<'_>, local: &str, entry: &mut RawEntry) {
    if entry.image_url.is_some() {
        return;
    }
    if !matches!(local, "content" | "thumbnail" | "enclosure") {
        return;
    }
    let mut url = None;
    let mut mime = None;
    let mut medium = None;
    for attr in e.attributes().flatten() {
        let key = attr.key.local_name().as_ref().to_ascii_lowercase();
        let val = decode_entities(attr.value.trim());
        match key.as_str() {
            "url" => url = Some(val),
            "type" => mime = Some(val.to_ascii_lowercase()),
            "medium" => medium = Some(val.to_ascii_lowercase()),
            _ => {}
        }
    }
    let Some(candidate) = url.filter(|u| is_http_url(u)) else {
        return;
    };
    let is_image = local == "thumbnail"
        || medium.as_deref() == Some("image")
        || mime.as_deref().is_some_and(|m| m.starts_with("image/"))
        || (local == "content" && mime.is_none() && medium.is_none());
    if is_image {
        entry.image_url = Some(candidate);
    }
}

fn extract_atom_link(e: &BytesStart<'_>, entry: &mut RawEntry) {
    if !entry.url.is_empty() {
        return;
    }
    let mut href = None;
    let mut rel = None;
    for attr in e.attributes().flatten() {
        let key = attr.key.local_name().as_ref().to_ascii_lowercase();
        let val = decode_entities(attr.value.trim());
        match key.as_str() {
            "href" => href = Some(val),
            "rel" => rel = Some(val.to_ascii_lowercase()),
            _ => {}
        }
    }
    if let Some(u) = href.filter(|u| is_http_url(u))
        && matches!(rel.as_deref(), None | Some("alternate"))
    {
        entry.url = u;
    }
}

fn finalize_entry(raw: RawEntry, source: &FeedSource) -> Option<FeedArticle> {
    let title = raw.title.trim().to_string();
    if title.is_empty() {
        return None;
    }
    let body_html = if !raw.content.is_empty() {
        raw.content
    } else {
        raw.description.clone()
    };
    let image_url = raw
        .image_url
        .or_else(|| first_img_src(&body_html))
        .or_else(|| first_img_src(&raw.description));
    let summary_src = if !raw.description.is_empty() {
        &raw.description
    } else {
        &body_html
    };
    let summary = plain_excerpt(summary_src, EXCERPT_CHARS);
    let url = if is_http_url(&raw.url) {
        raw.url
    } else if is_http_url(&raw.id) {
        raw.id.clone()
    } else {
        String::new()
    };
    let id = if !raw.id.is_empty() {
        raw.id
    } else if !url.is_empty() {
        url.clone()
    } else {
        format!("{}:{}", source.id, url_hash(&title))
    };
    Some(FeedArticle {
        id,
        source_id: source.id.clone(),
        source_name: source.name.clone(),
        title,
        url,
        published_at: parse_feed_timestamp(&raw.date_raw),
        author: raw.author.trim().to_string(),
        summary,
        body_html,
        image_url,
    })
}

pub(crate) fn is_http_url(s: &str) -> bool {
    let t = s.trim();
    t.starts_with("https://") || t.starts_with("http://")
}

fn parse_feed_timestamp(raw: &str) -> u64 {
    let s = raw.trim();
    if s.is_empty() {
        return 0;
    }
    if let Ok(dt) = OffsetDateTime::parse(s, &Rfc2822) {
        return u64::try_from(dt.unix_timestamp()).unwrap_or(0);
    }
    if let Ok(dt) = OffsetDateTime::parse(s, &Rfc3339) {
        return u64::try_from(dt.unix_timestamp()).unwrap_or(0);
    }
    // Common RSS tweak: trailing "GMT" or "UTC" instead of "+0000".
    if let Some(prefix) = s.strip_suffix(" GMT").or_else(|| s.strip_suffix(" UTC")) {
        let fixed = format!("{prefix} +0000");
        if let Ok(dt) = OffsetDateTime::parse(&fixed, &Rfc2822) {
            return u64::try_from(dt.unix_timestamp()).unwrap_or(0);
        }
    }
    0
}

static RE_IMG_SRC: Lazy<Option<Regex>> =
    Lazy::new(|| Regex::new(r#"(?is)<img\b[^>]*\bsrc\s*=\s*["'](https?://[^"']+)["']"#).ok());
static RE_TAGS: Lazy<Option<Regex>> = Lazy::new(|| Regex::new(r"(?s)<[^>]+>").ok());

pub(crate) fn first_img_src(html: &str) -> Option<String> {
    RE_IMG_SRC
        .as_ref()?
        .captures(html)
        .and_then(|c| c.get(1))
        .map(|m| decode_entities(m.as_str()))
        .filter(|u| is_http_url(u))
}

pub(crate) fn decode_entities(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
        .replace("&nbsp;", " ")
        .replace("&#8217;", "'")
        .replace("&#8216;", "'")
        .replace("&#8220;", "\"")
        .replace("&#8221;", "\"")
        .replace("&#8211;", "-")
        .replace("&#8212;", "-")
        .replace("&rsquo;", "'")
        .replace("&lsquo;", "'")
        .replace("&rdquo;", "\"")
        .replace("&ldquo;", "\"")
        .replace("&hellip;", "...")
}

pub(crate) fn strip_tags_plain(html: &str) -> String {
    let no_tags = match RE_TAGS.as_ref() {
        Some(re) => re.replace_all(html, " "),
        None => std::borrow::Cow::Borrowed(html),
    };
    let decoded = decode_entities(&no_tags);
    decoded.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn plain_excerpt(html: &str, max_chars: usize) -> String {
    let clean = strip_tags_plain(html);
    if clean.chars().count() <= max_chars {
        return clean;
    }
    let mut out: String = clean.chars().take(max_chars).collect();
    if let Some(pos) = out.rfind(' ')
        && pos > max_chars / 2
    {
        out.truncate(pos);
    }
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use irori_depot::test_server::{Resp, TestServer};

    fn test_source(url: &str) -> FeedSource {
        FeedSource {
            id: "test".to_string(),
            name: "Test News".to_string(),
            url: url.to_string(),
        }
    }

    #[test]
    fn parses_rss2_and_atom_feeds() {
        let rss = r#"<?xml version="1.0" encoding="UTF-8"?>
        <rss version="2.0" xmlns:media="http://search.yahoo.com/mrss/" xmlns:content="http://purl.org/rss/1.0/modules/content/">
          <channel>
            <title>Test Gaming</title>
            <item>
              <title>Hades II &amp; Major Update</title>
              <link>https://example.com/hades-2</link>
              <guid>rss-1</guid>
              <pubDate>Fri, 27 Nov 2026 12:00:00 GMT</pubDate>
              <media:content url="https://example.com/cover.jpg" medium="image"/>
              <description><![CDATA[<p>Big patch arrives today.</p>]]></description>
              <content:encoded><![CDATA[<p>Full notes here.</p>]]></content:encoded>
            </item>
          </channel>
        </rss>"#;
        let items = parse_feed_xml(rss, &test_source("https://example.com/rss"));
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].title, "Hades II & Major Update");
        assert_eq!(items[0].url, "https://example.com/hades-2");
        assert_eq!(
            items[0].image_url.as_deref(),
            Some("https://example.com/cover.jpg")
        );
        assert!(items[0].published_at > 0);
        assert_eq!(items[0].summary, "Big patch arrives today.");

        let atom = r#"<?xml version="1.0" encoding="utf-8"?>
        <feed xmlns="http://www.w3.org/2005/Atom">
          <entry>
            <id>atom-1</id>
            <title>Linux Gaming Roundup</title>
            <link rel="alternate" href="https://example.com/linux-roundup"/>
            <updated>2026-11-27T15:30:00Z</updated>
            <author><name>Liam</name></author>
            <content type="html">&lt;p&gt;&lt;img src="https://example.com/thumb.png"/&gt;Proton update released.&lt;/p&gt;</content>
          </entry>
        </feed>"#;
        let atom_items = parse_feed_xml(atom, &test_source("https://example.com/atom"));
        assert_eq!(atom_items.len(), 1);
        assert_eq!(atom_items[0].author, "Liam");
        assert_eq!(
            atom_items[0].image_url.as_deref(),
            Some("https://example.com/thumb.png")
        );
        assert!(atom_items[0].published_at > 0);
    }

    #[test]
    fn validates_feed_urls() {
        assert!(validate_feed_url("https://example.com/rss.xml").is_ok());
        assert!(validate_feed_url("javascript:alert(1)").is_err());
        assert!(validate_feed_url("file:///etc/passwd").is_err());
    }

    #[tokio::test]
    async fn reader_mode_extracts_article_and_falls_back_on_403() {
        let Some(_paths) = crate::util::test_paths() else {
            return;
        };
        let long_body = "Detailed review paragraph discussing performance, gameplay mechanics, level design, and controller support across desktop and handheld Linux PCs. ".repeat(4);
        let html = format!(
            r#"<html><head><meta property="og:image" content="https://example.com/og.jpg"></head>
            <body><nav>Menu</nav><article><h1>Review</h1><script>evil()</script><p>{long_body}</p></article><footer>Copyright</footer></body></html>"#
        );
        let server = TestServer::start(move |req| {
            if req.path.contains("/ok") {
                Resp {
                    status: 200,
                    headers: vec![("Content-Type".into(), "text/html".into())],
                    body: html.clone().into_bytes(),
                    ..Default::default()
                }
            } else {
                Resp::status(403)
            }
        });

        let ok = fetch_reader_article(&format!("{}/ok", server.base()), "<p>Short</p>", None).await;
        assert!(ok.used_reader_mode);
        assert!(!ok.body_html.contains("evil()"));
        assert!(!ok.body_html.contains("Menu"));
        assert!(ok.body_html.contains("Detailed review paragraph"));
        assert_eq!(ok.image_url.as_deref(), Some("https://example.com/og.jpg"));

        let fallback = fetch_reader_article(
            &format!("{}/blocked", server.base()),
            "<p>RSS summary fallback</p>",
            Some("https://example.com/rss.jpg"),
        )
        .await;
        assert!(!fallback.used_reader_mode);
        assert_eq!(fallback.body_html, "<p>RSS summary fallback</p>");
        assert_eq!(
            fallback.image_url.as_deref(),
            Some("https://example.com/rss.jpg")
        );
    }
}
