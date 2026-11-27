//! On-demand web image search for the Art Picker.
//!
//! Only runs when the user explicitly selects the "Web" tab in the Art Picker
//! (never automatically or in the background). Queries DuckDuckGo Images
//! (`i.js`), falling back to Bing Images HTML (`a.iusc` metadata) and Google
//! Images HTML when an upstream blocks or returns no results. If all engines
//! fail, [`browser_search_url`] gives a direct link so the user can search in
//! their browser instead.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow, bail};
use once_cell::sync::Lazy;
use regex::Regex;
use serde_json::Value;

use crate::util;

const DEFAULT_DDG: &str = "https://duckduckgo.com";
const DEFAULT_BING: &str = "https://www.bing.com";
const DEFAULT_GOOGLE: &str = "https://www.google.com";

const SEARCH_TIMEOUT: Duration = Duration::from_secs(20);
const MAX_BODY_BYTES: usize = 8 * 1024 * 1024;
const BROWSER_UA: &str = "Mozilla/5.0 (X11; Linux x86_64; rv:136.0) Gecko/20100101 Firefox/136.0";

/// Serializes concurrent web image searches so rapid clicks do not burst
/// upstream search engines.
static SEARCH_LOCK: Lazy<tokio::sync::Mutex<()>> = Lazy::new(|| tokio::sync::Mutex::new(()));

static RE_WHITESPACE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"\s+").expect("valid whitespace regex"));
static RE_GOOGLE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"\["(https://encrypted-[^,]+?)",\d+,\d+\],\["(http.+?)",(\d+),(\d+)\]"#)
        .expect("valid google image regex")
});
static RE_BING_TAG: Lazy<Regex> =
    Lazy::new(|| Regex::new(r#"<a\s[^>]*class="iusc"[^>]*>"#).expect("valid bing tag regex"));
static RE_BING_ATTR: Lazy<Regex> =
    Lazy::new(|| Regex::new(r#"\sm="([^"]*)""#).expect("valid bing attr regex"));
static RE_HEX_ENTITY: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"&#x([0-9a-fA-F]+);").expect("valid hex entity regex"));
static RE_DEC_ENTITY: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"&#([0-9]+);").expect("valid dec entity regex"));
static RE_VQD: Lazy<[Regex; 4]> = Lazy::new(|| {
    [
        Regex::new(r#"vqd="([\d-]+)""#).expect("valid vqd regex"),
        Regex::new(r#"vqd=([\d-]+)&"#).expect("valid vqd regex"),
        Regex::new(r#"vqd='([\d-]+)'"#).expect("valid vqd regex"),
        Regex::new(r#"vqd=([\d-]+)"#).expect("valid vqd regex"),
    ]
});

/// One web image result returned by the on-demand search.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebImage {
    pub image_url: String,
    pub thumb_url: String,
    pub width: u32,
    pub height: u32,
}

/// Optional aspect filter passed to upstream image search engines.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WebShape {
    Square,
    Wide,
}

impl WebShape {
    fn ddg(self) -> &'static str {
        match self {
            Self::Square => "layout:Square",
            Self::Wide => "layout:Wide",
        }
    }

    fn bing(self) -> &'static str {
        match self {
            Self::Square => "+filterui:aspect-square",
            Self::Wide => "+filterui:aspect-wide",
        }
    }

    fn google(self) -> &'static str {
        match self {
            Self::Square => "iar:s",
            Self::Wide => "iar:w",
        }
    }
}

/// Base URLs for the web image cascade (overridden in offline tests).
#[derive(Debug, Clone)]
pub struct Endpoints {
    pub duckduckgo: String,
    pub bing: String,
    pub google: String,
}

impl Default for Endpoints {
    fn default() -> Self {
        Self {
            duckduckgo: DEFAULT_DDG.to_owned(),
            bing: DEFAULT_BING.to_owned(),
            google: DEFAULT_GOOGLE.to_owned(),
        }
    }
}

/// Direct browser URL for image search when automated scraping is blocked or
/// unavailable.
pub fn browser_search_url(query: &str) -> String {
    let clean = normalize_query(query);
    format!(
        "{DEFAULT_DDG}/?ia=images&iax=images&q={}",
        util::encode_component(&clean)
    )
}

fn normalize_query(query: &str) -> String {
    RE_WHITESPACE.replace_all(query, " ").trim().to_owned()
}

fn as_http_url(value: &str) -> Option<String> {
    let decoded = value.replace("\\u003d", "=").replace("\\u0026", "&");
    let url = url::Url::parse(decoded.trim()).ok()?;
    matches!(url.scheme(), "http" | "https").then(|| url.to_string())
}

fn json_u32(v: Option<&Value>) -> u32 {
    match v {
        Some(Value::Number(n)) => n.as_f64().map(|f| f.max(0.0) as u32).unwrap_or(0),
        Some(Value::String(s)) => s
            .trim()
            .parse::<f64>()
            .map(|f| f.max(0.0) as u32)
            .unwrap_or(0),
        _ => 0,
    }
}

fn map_images(rows: impl IntoIterator<Item = (String, String, u32, u32)>) -> Vec<WebImage> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for (image, thumb, width, height) in rows {
        let Some(image_url) = as_http_url(&image) else {
            continue;
        };
        let thumb_url = as_http_url(&thumb).unwrap_or_else(|| image_url.clone());
        if !seen.insert(image_url.clone()) {
            continue;
        }
        out.push(WebImage {
            image_url,
            thumb_url,
            width,
            height,
        });
    }
    out
}

/// Parse the JSON payload of DuckDuckGo's `i.js` endpoint.
pub fn parse_ddg_images(body: &Value) -> Vec<WebImage> {
    let rows = body
        .get("results")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|r| {
            (
                r.get("image")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_owned(),
                r.get("thumbnail")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_owned(),
                json_u32(r.get("width")),
                json_u32(r.get("height")),
            )
        });
    map_images(rows)
}

/// Extract DuckDuckGo's `vqd` search token from an HTML page.
pub fn extract_vqd(html: &str) -> Option<String> {
    for rx in RE_VQD.iter() {
        if let Some(c) = rx.captures(html)
            && let Some(m) = c.get(1)
        {
            return Some(m.as_str().to_owned());
        }
    }
    None
}

fn html_unescape(s: &str) -> String {
    let s = RE_HEX_ENTITY.replace_all(s, |c: &regex::Captures| {
        u32::from_str_radix(&c[1], 16)
            .ok()
            .and_then(char::from_u32)
            .map(String::from)
            .unwrap_or_default()
    });
    let s = RE_DEC_ENTITY.replace_all(&s, |c: &regex::Captures| {
        c[1].parse::<u32>()
            .ok()
            .and_then(char::from_u32)
            .map(String::from)
            .unwrap_or_default()
    });
    s.replace("&quot;", "\"")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&#39;", "'")
        .replace("&amp;", "&")
}

/// Parse Bing image search HTML (`<a class="iusc" m="{...}">`).
pub fn parse_bing_html(html: &str) -> Vec<WebImage> {
    let rows = RE_BING_TAG.find_iter(html).filter_map(|tag| {
        let raw = RE_BING_ATTR.captures(tag.as_str())?.get(1)?.as_str();
        let meta: Value = serde_json::from_str(&html_unescape(raw)).ok()?;
        let w = json_u32(meta.get("ow")).max(json_u32(meta.get("width")));
        let h = json_u32(meta.get("oh")).max(json_u32(meta.get("height")));
        Some((
            meta.get("murl")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned(),
            meta.get("turl")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned(),
            w,
            h,
        ))
    });
    map_images(rows)
}

/// Parse Google Images HTML entries (thumbnail + full-size image tuples).
pub fn parse_google_html(html: &str) -> Vec<WebImage> {
    let flat = html.replace("\r\n", "").replace(['\r', '\n'], "");
    let rows = RE_GOOGLE.captures_iter(&flat).map(|c| {
        (
            c[2].to_owned(),
            c[1].to_owned(),
            c[4].parse().unwrap_or(0),
            c[3].parse().unwrap_or(0),
        )
    });
    map_images(rows)
}

fn ddg_filters(transparent: bool, shape: Option<WebShape>) -> String {
    match (transparent, shape) {
        (false, None) => ",,,,,".into(),
        (true, None) => ",type:transparent,,,,".into(),
        (t, Some(s)) => format!(
            ",{},,{},,",
            if t { "type:transparent" } else { "" },
            s.ddg()
        ),
    }
}

fn remaining_timeout(deadline: Instant) -> Duration {
    let secs = deadline
        .saturating_duration_since(Instant::now())
        .as_secs()
        .clamp(1, 10);
    Duration::from_secs(secs)
}

async fn read_text_bounded(mut resp: reqwest::Response) -> Result<String> {
    if resp
        .content_length()
        .is_some_and(|n| n > MAX_BODY_BYTES as u64)
    {
        bail!("response exceeds {MAX_BODY_BYTES} bytes");
    }
    let mut buf = Vec::new();
    while let Some(chunk) = resp
        .chunk()
        .await
        .map_err(|e| anyhow!("reading web image response: {}", e.without_url()))?
    {
        if buf.len().saturating_add(chunk.len()) > MAX_BODY_BYTES {
            bail!("response exceeds {MAX_BODY_BYTES} bytes");
        }
        buf.extend_from_slice(&chunk);
    }
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

async fn ddg_images(
    base: &str,
    query: &str,
    transparent: bool,
    shape: Option<WebShape>,
    deadline: Instant,
) -> Result<Vec<WebImage>> {
    let client = irori_core::http::client();
    let mut params = vec![("ia", "images"), ("iax", "images"), ("q", query)];
    if transparent {
        params.push(("iaf", "type:transparent"));
    } else if let Some(s) = shape {
        params.push(("iaf", s.ddg()));
    }
    let page_resp = client
        .get(format!("{base}/"))
        .query(&params)
        .header(reqwest::header::USER_AGENT, BROWSER_UA)
        .header(reqwest::header::ACCEPT_LANGUAGE, "en-US,en;q=0.9")
        .timeout(remaining_timeout(deadline))
        .send()
        .await
        .map_err(|e| anyhow!("DuckDuckGo request failed: {}", e.without_url()))?
        .error_for_status()
        .map_err(|e| anyhow!("DuckDuckGo HTTP error: {}", e.without_url()))?;
    let page = read_text_bounded(page_resp).await?;
    let vqd = extract_vqd(&page).ok_or_else(|| anyhow!("DuckDuckGo returned no vqd token"))?;
    let filters = ddg_filters(transparent, shape);
    let json_resp = client
        .get(format!("{base}/i.js"))
        .query(&[
            ("l", "us-en"),
            ("o", "json"),
            ("q", query),
            ("vqd", &vqd),
            ("f", &filters),
            ("p", "1"),
        ])
        .header(reqwest::header::USER_AGENT, BROWSER_UA)
        .header(reqwest::header::REFERER, format!("{DEFAULT_DDG}/"))
        .header(
            reqwest::header::ACCEPT,
            "application/json, text/javascript, */*; q=0.01",
        )
        .header(reqwest::header::ACCEPT_LANGUAGE, "en-US,en;q=0.9")
        .header("X-Requested-With", "XMLHttpRequest")
        .timeout(remaining_timeout(deadline))
        .send()
        .await
        .map_err(|e| anyhow!("DuckDuckGo image list failed: {}", e.without_url()))?
        .error_for_status()
        .map_err(|e| anyhow!("DuckDuckGo image list HTTP error: {}", e.without_url()))?;
    let body_text = read_text_bounded(json_resp).await?;
    let body: Value = serde_json::from_str(&body_text)?;
    Ok(parse_ddg_images(&body))
}

async fn bing_images(
    base: &str,
    query: &str,
    shape: Option<WebShape>,
    deadline: Instant,
) -> Result<Vec<WebImage>> {
    let mut params = vec![("q", query), ("form", "HDRSC2")];
    if let Some(s) = shape {
        params.push(("qft", s.bing()));
    }
    let resp = irori_core::http::client()
        .get(format!("{base}/images/search"))
        .query(&params)
        .header(reqwest::header::USER_AGENT, BROWSER_UA)
        .header(reqwest::header::ACCEPT_LANGUAGE, "en-US,en;q=0.9")
        .timeout(remaining_timeout(deadline))
        .send()
        .await
        .map_err(|e| anyhow!("Bing request failed: {}", e.without_url()))?
        .error_for_status()
        .map_err(|e| anyhow!("Bing HTTP error: {}", e.without_url()))?;
    let html = read_text_bounded(resp).await?;
    Ok(parse_bing_html(&html))
}

async fn google_images(
    base: &str,
    query: &str,
    transparent: bool,
    shape: Option<WebShape>,
    deadline: Instant,
) -> Result<Vec<WebImage>> {
    let mut params = vec![
        ("tbm", "isch"),
        ("client", "firefox-b-d"),
        ("source", "lnt"),
        ("q", query),
    ];
    let tbs = match (transparent, shape) {
        (true, Some(s)) => Some(format!("ic:trans,{}", s.google())),
        (true, None) => Some("ic:trans".to_owned()),
        (false, Some(s)) => Some(s.google().to_owned()),
        (false, None) => None,
    };
    if let Some(tbs) = &tbs {
        params.push(("tbs", tbs.as_str()));
    }
    let resp = irori_core::http::client()
        .get(format!("{base}/search"))
        .query(&params)
        .header(reqwest::header::USER_AGENT, BROWSER_UA)
        .header(reqwest::header::ACCEPT_LANGUAGE, "en-US,en;q=0.9")
        // Avoids cookie-consent redirect pages in EU regions.
        .header(
            reqwest::header::COOKIE,
            "CONSENT=YES+cb; SOCS=CAESEwgDEgk0ODE3Nzk3MjQaAmVuIAEaBgiA_LyaBg",
        )
        .timeout(remaining_timeout(deadline))
        .send()
        .await
        .map_err(|e| anyhow!("Google request failed: {}", e.without_url()))?
        .error_for_status()
        .map_err(|e| anyhow!("Google HTTP error: {}", e.without_url()))?;
    let html = read_text_bounded(resp).await?;
    Ok(parse_google_html(&html))
}

#[derive(Clone, Copy)]
enum Engine {
    DuckDuckGo,
    Bing,
    Google,
}

impl Engine {
    fn label(self) -> &'static str {
        match self {
            Self::DuckDuckGo => "DuckDuckGo",
            Self::Bing => "Bing",
            Self::Google => "Google",
        }
    }
}

async fn fetch_cascade(
    endpoints: &Endpoints,
    query: &str,
    transparent: bool,
    shape: Option<WebShape>,
    deadline: Instant,
) -> Result<Vec<WebImage>> {
    let mut last_error: Option<anyhow::Error> = None;
    for engine in [Engine::DuckDuckGo, Engine::Bing, Engine::Google] {
        if Instant::now() >= deadline {
            bail!("Web image search timed out");
        }
        let outcome = match engine {
            Engine::DuckDuckGo => {
                ddg_images(&endpoints.duckduckgo, query, transparent, shape, deadline).await
            }
            Engine::Bing => bing_images(&endpoints.bing, query, shape, deadline).await,
            Engine::Google => {
                google_images(&endpoints.google, query, transparent, shape, deadline).await
            }
        };
        match outcome {
            Ok(images) if !images.is_empty() => return Ok(images),
            Ok(_) => tracing::debug!("Web image search: {} returned no images", engine.label()),
            Err(e) => {
                tracing::info!("Web image search: {} failed: {e:#}", engine.label());
                last_error = Some(e);
            }
        }
    }
    match last_error {
        Some(e) => Err(e),
        None => Ok(Vec::new()),
    }
}

/// On-demand web image search (DuckDuckGo -> Bing -> Google).
pub async fn search(
    query: &str,
    transparent: bool,
    shape: Option<WebShape>,
) -> Result<Vec<WebImage>> {
    irori_core::net::ensure_online()?;
    search_with_endpoints(&Endpoints::default(), query, transparent, shape).await
}

/// [`search`] against custom endpoints (for offline tests).
pub async fn search_with_endpoints(
    endpoints: &Endpoints,
    query: &str,
    transparent: bool,
    shape: Option<WebShape>,
) -> Result<Vec<WebImage>> {
    let query = normalize_query(query);
    if query.is_empty() {
        return Ok(Vec::new());
    }
    let _guard = SEARCH_LOCK.lock().await;
    let mut result = fetch_cascade(
        endpoints,
        &query,
        transparent,
        shape,
        Instant::now() + SEARCH_TIMEOUT,
    )
    .await;
    if shape.is_some() && result.as_ref().is_ok_and(Vec::is_empty) {
        result = fetch_cascade(
            endpoints,
            &query,
            transparent,
            None,
            Instant::now() + SEARCH_TIMEOUT,
        )
        .await;
    }
    if result.is_err() && transparent {
        result = fetch_cascade(
            endpoints,
            &query,
            false,
            shape,
            Instant::now() + SEARCH_TIMEOUT,
        )
        .await;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use irori_depot::test_server::{Resp, TestServer};

    #[test]
    fn parses_and_dedupes_ddg_payload() {
        let body = serde_json::json!({
            "results": [
                {"image": "https://a.example/cover.jpg", "thumbnail": "https://a.example/t.jpg", "width": 600, "height": "900"},
                {"image": "https://a.example/cover.jpg", "thumbnail": "https://a.example/dup.jpg"},
                {"image": "ftp://a.example/skip.jpg"},
                {"image": "javascript:alert(1)"},
                {"image": "https://a.example/logo.png", "thumbnail": "data:image/png;base64,xx", "width": 400, "height": 200}
            ]
        });
        let images = parse_ddg_images(&body);
        assert_eq!(images.len(), 2);
        assert_eq!(images[0].image_url, "https://a.example/cover.jpg");
        assert_eq!(images[0].thumb_url, "https://a.example/t.jpg");
        assert_eq!((images[0].width, images[0].height), (600, 900));
        assert_eq!(images[1].thumb_url, "https://a.example/logo.png");
    }

    #[test]
    fn extracts_vqd_variants_and_builds_filters() {
        assert_eq!(
            extract_vqd(r#"<script>nrje('/d.js?q=x&vqd="4-1234"&p=1')</script>"#).as_deref(),
            Some("4-1234")
        );
        assert_eq!(
            extract_vqd("<a href=\"/?vqd=4-99&x=1\">").as_deref(),
            Some("4-99")
        );
        assert_eq!(extract_vqd("vqd='4-55'").as_deref(), Some("4-55"));
        assert_eq!(extract_vqd("no token here"), None);
        assert_eq!(ddg_filters(false, None), ",,,,,");
        assert_eq!(ddg_filters(true, None), ",type:transparent,,,,");
        assert_eq!(
            ddg_filters(false, Some(WebShape::Square)),
            ",,,layout:Square,,"
        );
        assert_eq!(
            ddg_filters(true, Some(WebShape::Wide)),
            ",type:transparent,,layout:Wide,,"
        );
    }

    #[test]
    fn parses_bing_and_google_html() {
        let bing = r#"<a class="iusc" m="{&quot;murl&quot;:&quot;https://cdn.example/wide.jpg&quot;,&quot;turl&quot;:&quot;https://ts.example/th?id=1&amp;pid=15.1&quot;,&quot;ow&quot;:920,&quot;oh&quot;:430}"></a>"#;
        let bing_imgs = parse_bing_html(bing);
        assert_eq!(bing_imgs.len(), 1);
        assert_eq!(bing_imgs[0].image_url, "https://cdn.example/wide.jpg");
        assert_eq!(
            bing_imgs[0].thumb_url,
            "https://ts.example/th?id=1&pid=15.1"
        );
        assert_eq!((bing_imgs[0].width, bing_imgs[0].height), (920, 430));

        let google = r#"["https://encrypted-tbn0.gstatic.com/images?q=tbn:AAA\u003d",225,225],["https://cdn.example/art.jpg?a=1\u0026b=2",1000,1500]"#;
        let google_imgs = parse_google_html(google);
        assert_eq!(google_imgs.len(), 1);
        assert_eq!(
            google_imgs[0].image_url,
            "https://cdn.example/art.jpg?a=1&b=2"
        );
        assert_eq!(
            google_imgs[0].thumb_url,
            "https://encrypted-tbn0.gstatic.com/images?q=tbn:AAA="
        );
        assert_eq!((google_imgs[0].width, google_imgs[0].height), (1500, 1000));
    }

    #[test]
    fn browser_search_url_encodes_query() {
        assert_eq!(
            browser_search_url("  Hades II   cover "),
            "https://duckduckgo.com/?ia=images&iax=images&q=Hades%20II%20cover"
        );
    }

    fn text_resp(body: &str) -> Resp {
        Resp {
            status: 200,
            body: body.as_bytes().to_vec(),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn cascade_falls_back_from_ddg_and_bing_to_google() {
        let ddg = TestServer::start(|_| Resp::status(403));
        let bing = TestServer::start(|_| Resp::status(503));
        let google = TestServer::start(|_| {
            text_resp(
                r#"["https://encrypted-tbn0.gstatic.com/images?q=tbn:OK",100,100],["https://img.example/g.jpg",430,920]"#,
            )
        });
        let endpoints = Endpoints {
            duckduckgo: ddg.base(),
            bing: bing.base(),
            google: google.base(),
        };
        let images = search_with_endpoints(&endpoints, "Portal 2", false, Some(WebShape::Wide))
            .await
            .unwrap();
        assert_eq!(images.len(), 1);
        assert_eq!(images[0].image_url, "https://img.example/g.jpg");
        assert_eq!((images[0].width, images[0].height), (920, 430));
    }

    #[tokio::test]
    async fn ddg_vqd_then_json_succeeds_without_hitting_fallbacks() {
        let ddg = TestServer::start(|req| {
            if req.path.starts_with("/i.js") {
                Resp::json(serde_json::json!({
                    "results": [{
                        "image": "https://img.example/ddg.png",
                        "thumbnail": "https://img.example/t.png",
                        "width": 600,
                        "height": 900
                    }]
                }))
            } else {
                text_resp(r#"<script>vqd="4-777"</script>"#)
            }
        });
        let fail = TestServer::start(|_| Resp::status(500));
        let endpoints = Endpoints {
            duckduckgo: ddg.base(),
            bing: fail.base(),
            google: fail.base(),
        };
        let images = search_with_endpoints(&endpoints, "Hades", false, None)
            .await
            .unwrap();
        assert_eq!(images.len(), 1);
        assert_eq!(images[0].image_url, "https://img.example/ddg.png");
    }

    #[tokio::test]
    async fn all_engines_failing_returns_error() {
        let blocked = TestServer::start(|_| Resp::status(429));
        let endpoints = Endpoints {
            duckduckgo: blocked.base(),
            bing: blocked.base(),
            google: blocked.base(),
        };
        assert!(
            search_with_endpoints(&endpoints, "Hades", false, None)
                .await
                .is_err()
        );
    }
}
