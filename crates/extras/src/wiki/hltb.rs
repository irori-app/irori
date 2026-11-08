//! HowLongToBeat times, from the HLTB game page (by PCGamingWiki's HLTB ID
//! or the one the user chose), for GOG games from the GOG store page's HLTB
//! box, and otherwise from HLTB's own title search.
//!
//! HLTB has no public API. Its site search asks `/api/search/site/init` for
//! a short-lived token (tied to the caller's address and user agent) and
//! posts the query with it; that is what [`search`] does, once per lookup.
//! A search result is only taken when its name is the game's title (folded),
//! never the nearest guess: a wrong match is worse than none, and the user
//! can pick one with "Change match".

use std::time::Duration;

use anyhow::Result;
use once_cell::sync::Lazy;
use regex::Regex;
use reqwest::header::{ACCEPT, ACCEPT_LANGUAGE, HeaderMap, HeaderValue, REFERER, USER_AGENT};
use serde::{Deserialize, Serialize};

/// Completion times in hours (rounded).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct HowLongToBeat {
    pub main_story: u32,
    pub main_extra: u32,
    pub completionist: u32,
    pub game_id: Option<u64>,
    pub game_name: Option<String>,
    pub game_image_url: Option<String>,
    pub game_web_link: Option<String>,
    /// The year HLTB gives for the release, from a search result.
    pub release_year: Option<u32>,
}

const BROWSER_UA: &str = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36";

fn browser_headers(referer: Option<&'static str>) -> HeaderMap {
    let mut h = HeaderMap::new();
    h.insert(USER_AGENT, HeaderValue::from_static(BROWSER_UA));
    h.insert(
        ACCEPT,
        HeaderValue::from_static(
            "text/html,application/xhtml+xml,application/xml;q=0.9,image/avif,image/webp,image/apng,*/*;q=0.8",
        ),
    );
    h.insert(ACCEPT_LANGUAGE, HeaderValue::from_static("en-US,en;q=0.9"));
    h.insert("Upgrade-Insecure-Requests", HeaderValue::from_static("1"));
    if let Some(r) = referer {
        h.insert(REFERER, HeaderValue::from_static(r));
    }
    h
}

/// GET an HTML page. `Ok(None)` for 4xx responses (page missing or blocked).
async fn get_page(url: &str, referer: Option<&'static str>) -> Result<Option<String>> {
    let resp = irori_core::http::client()
        .get(url)
        .headers(browser_headers(referer))
        .timeout(Duration::from_secs(10))
        .send()
        .await?;
    let status = resp.status();
    if status.is_server_error() {
        anyhow::bail!("{url} returned {status}");
    }
    if !status.is_success() {
        return Ok(None);
    }
    Ok(Some(resp.text().await?))
}

static NEXT_DATA: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"(?s)<script id="__NEXT_DATA__" type="application/json">(.*?)</script>"#).unwrap()
});

fn hours(seconds: Option<f64>) -> u32 {
    seconds
        .map(|s| (s / 3600.0).round().max(0.0) as u32)
        .unwrap_or(0)
}

/// Parse an HLTB game page (`__NEXT_DATA__` JSON).
pub fn parse_hltb_page(html: &str) -> Option<HowLongToBeat> {
    let json: serde_json::Value =
        serde_json::from_str(NEXT_DATA.captures(html)?.get(1)?.as_str()).ok()?;
    let game = json.pointer("/props/pageProps/game/data/game/0")?;
    let game_id = game.get("game_id")?.as_u64()?;
    let num = |k: &str| game.get(k).and_then(serde_json::Value::as_f64);
    let text = |k: &str| {
        game.get(k)
            .and_then(serde_json::Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    Some(HowLongToBeat {
        main_story: hours(num("comp_main")),
        main_extra: hours(num("comp_plus")),
        completionist: hours(num("comp_100")),
        game_id: Some(game_id),
        game_name: text("game_name"),
        game_image_url: text("game_image").map(|i| format!("https://howlongtobeat.com/games/{i}")),
        game_web_link: Some(format!("https://howlongtobeat.com/game/{game_id}")),
        release_year: None,
    })
}

static GOG_TIME: Lazy<[Regex; 3]> = Lazy::new(|| {
    ["Main", r"Main \+ Sides", "Completionist"].map(|label| {
        Regex::new(&format!(
            r#"<span class="howlongtobeat-box__time"> ?([\d.]+) h ?</span><span class="howlongtobeat-box__name">{label}</span>"#
        ))
        .unwrap()
    })
});
static GOG_ID: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r#"<a target="_blank" href="https://howlongtobeat.com/game/(\d+)">HowLongToBeat</a>"#,
    )
    .unwrap()
});

/// Parse the HLTB box of a GOG store page. All three times and the link are
/// required.
pub fn parse_gog_store_page(html: &str, title: &str) -> Option<HowLongToBeat> {
    let time = |re: &Regex| -> Option<u32> {
        let v: f64 = re.captures(html)?.get(1)?.as_str().parse().ok()?;
        Some(v.round().max(0.0) as u32)
    };
    let [main, extra, full] = &*GOG_TIME;
    let game_id: u64 = GOG_ID.captures(html)?.get(1)?.as_str().parse().ok()?;
    Some(HowLongToBeat {
        main_story: time(main)?,
        main_extra: time(extra)?,
        completionist: time(full)?,
        game_id: Some(game_id),
        game_name: Some(title.to_string()),
        game_image_url: None,
        game_web_link: Some(format!("https://howlongtobeat.com/game/{game_id}")),
        release_year: None,
    })
}

/// Times from HLTB by ID.
pub async fn fetch_by_id(hltb_id: &str) -> Result<Option<HowLongToBeat>> {
    let url = format!("https://howlongtobeat.com/game/{hltb_id}");
    Ok(get_page(&url, Some("https://howlongtobeat.com"))
        .await?
        .and_then(|html| parse_hltb_page(&html)))
}

/// Times from a GOG store page.
pub async fn fetch_from_gog_store(store_url: &str, title: &str) -> Result<Option<HowLongToBeat>> {
    Ok(get_page(store_url, None)
        .await?
        .and_then(|html| parse_gog_store_page(&html, title)))
}

const SITE: &str = "https://howlongtobeat.com";
/// Results asked for per search.
const RESULTS: usize = 10;

/// The games of a site search answer.
pub fn parse_search(json: &serde_json::Value) -> Vec<HowLongToBeat> {
    let Some(data) = json.get("data").and_then(|d| d.as_array()) else {
        return Vec::new();
    };
    data.iter()
        .filter_map(|game| {
            let game_id = game.get("game_id")?.as_u64()?;
            let num = |k: &str| game.get(k).and_then(serde_json::Value::as_f64);
            let text = |k: &str| {
                game.get(k)
                    .and_then(serde_json::Value::as_str)
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
            };
            Some(HowLongToBeat {
                main_story: hours(num("comp_main")),
                main_extra: hours(num("comp_plus")),
                completionist: hours(num("comp_100")),
                game_id: Some(game_id),
                game_name: text("game_name"),
                game_image_url: text("game_image").map(|i| format!("{SITE}/games/{i}")),
                game_web_link: Some(format!("{SITE}/game/{game_id}")),
                release_year: game
                    .get("release_world")
                    .and_then(serde_json::Value::as_u64)
                    .and_then(|y| u32::try_from(y).ok())
                    .filter(|y| *y > 0),
            })
        })
        .collect()
}

/// Letters and digits only, lower case, Roman numerals II–X as digits, so
/// "DOOM II: Hell on Earth" and "Doom 2 - Hell on Earth" compare equal.
fn fold(title: &str) -> String {
    let words: Vec<String> = title
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(|w| {
            let lower = w.to_lowercase();
            match lower.as_str() {
                "ii" => "2".into(),
                "iii" => "3".into(),
                "iv" => "4".into(),
                "v" => "5".into(),
                "vi" => "6".into(),
                "vii" => "7".into(),
                "viii" => "8".into(),
                "ix" => "9".into(),
                "x" => "10".into(),
                _ => lower,
            }
        })
        .collect();
    words.concat()
}

/// The result that is `title`, if one is: the same folded name, and of
/// several (remakes share names), the one released in `year` or else the
/// first (HLTB ranks by popularity). `None` rather than a near miss.
pub fn best_match<'a>(
    results: &'a [HowLongToBeat],
    title: &str,
    year: Option<u32>,
) -> Option<&'a HowLongToBeat> {
    let wanted = fold(title);
    if wanted.is_empty() {
        return None;
    }
    let same: Vec<&HowLongToBeat> = results
        .iter()
        .filter(|r| r.game_name.as_deref().is_some_and(|n| fold(n) == wanted))
        .collect();
    year.and_then(|y| same.iter().find(|r| r.release_year == Some(y)).copied())
        .or_else(|| same.first().copied())
}

/// HLTB's site search for `query`, most popular first.
pub async fn search(query: &str) -> Result<Vec<HowLongToBeat>> {
    let terms: Vec<&str> = query.split_whitespace().collect();
    if terms.is_empty() {
        return Ok(Vec::new());
    }
    let client = irori_core::http::client();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or_default();
    let init: serde_json::Value = client
        .get(format!("{SITE}/api/search/site/init?t={now}"))
        .headers(browser_headers(Some("https://howlongtobeat.com/")))
        .timeout(Duration::from_secs(10))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let token = init
        .get("token")
        .and_then(|t| t.as_str())
        .filter(|t| !t.is_empty())
        .ok_or_else(|| anyhow::anyhow!("HowLongToBeat gave no search token"))?;
    let body = serde_json::json!({
        "searchType": "games",
        "searchTerms": terms,
        "searchPage": 1,
        "size": RESULTS,
        "searchOptions": {
            "games": {
                "userId": 0, "platform": "", "sortCategory": "popular",
                "rangeCategory": "main", "rangeTime": {"min": null, "max": null},
                "gameplay": {"perspective": "", "flow": "", "genre": "", "difficulty": ""},
                "rangeYear": {"min": "", "max": ""}, "modifier": ""
            },
            "users": {"sortCategory": "postcount"},
            "lists": {"sortCategory": "follows"},
            "filter": "", "sort": 0, "randomizer": 0
        },
        "useCache": true
    });
    let answer: serde_json::Value = client
        .post(format!("{SITE}/api/search/site"))
        .headers(browser_headers(Some("https://howlongtobeat.com/")))
        .header("Origin", SITE)
        .header("x-auth-token", token)
        .json(&body)
        .timeout(Duration::from_secs(10))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    Ok(parse_search(&answer))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hltb_page() {
        let html = r#"<html><script id="__NEXT_DATA__" type="application/json">{"props":{"pageProps":{"game":{"data":{"game":[{"game_id":2127,"game_name":"Cyberpunk 2077","game_image":"x.jpg","comp_main":91800,"comp_plus":217000,"comp_100":0}]}}}}}</script></html>"#;
        let h = parse_hltb_page(html).unwrap();
        assert_eq!((h.main_story, h.main_extra, h.completionist), (26, 60, 0));
        assert_eq!(h.game_id, Some(2127));
        assert_eq!(
            h.game_image_url.as_deref(),
            Some("https://howlongtobeat.com/games/x.jpg")
        );
        assert_eq!(
            h.game_web_link.as_deref(),
            Some("https://howlongtobeat.com/game/2127")
        );
        assert!(parse_hltb_page("<html></html>").is_none());
    }

    #[test]
    fn gog_store_page() {
        let html = r#"<span class="howlongtobeat-box__time">25.5 h</span><span class="howlongtobeat-box__name">Main</span>
<span class="howlongtobeat-box__time"> 60 h </span><span class="howlongtobeat-box__name">Main + Sides</span>
<span class="howlongtobeat-box__time">104 h</span><span class="howlongtobeat-box__name">Completionist</span>
<a target="_blank" href="https://howlongtobeat.com/game/2127">HowLongToBeat</a>"#;
        let h = parse_gog_store_page(html, "Cyberpunk").unwrap();
        assert_eq!((h.main_story, h.main_extra, h.completionist), (26, 60, 104));
        assert_eq!(h.game_name.as_deref(), Some("Cyberpunk"));
        assert!(parse_gog_store_page("<span></span>", "x").is_none());
    }

    #[test]
    fn search_results_and_safe_matches() {
        let json = serde_json::json!({"data": [
            {"game_id": 62941, "game_name": "Hades", "game_image": "62941_Hades.jpg",
             "comp_main": 84888, "comp_plus": 175294, "comp_100": 343112, "release_world": 2020},
            {"game_id": 118218, "game_name": "Hades II", "comp_main": 0, "release_world": 2025},
            {"game_id": 1, "game_name": "DOOM", "release_world": 1993},
            {"game_id": 2, "game_name": "DOOM", "release_world": 2016},
            {"game_name": "no id"}
        ]});
        let results = parse_search(&json);
        assert_eq!(results.len(), 4);
        assert_eq!(results[0].main_story, 24);
        assert_eq!(results[0].release_year, Some(2020));
        assert_eq!(
            results[0].game_web_link.as_deref(),
            Some("https://howlongtobeat.com/game/62941")
        );
        let id = |r: Option<&HowLongToBeat>| r.and_then(|r| r.game_id);
        assert_eq!(id(best_match(&results, "Hades", None)), Some(62941));
        assert_eq!(id(best_match(&results, "HADES 2", None)), Some(118218));
        assert_eq!(id(best_match(&results, "Doom", Some(2016))), Some(2));
        assert_eq!(id(best_match(&results, "Doom", None)), Some(1));
        // Near misses are no match.
        assert_eq!(
            id(best_match(&results, "Hades: Battle out of Hell", None)),
            None
        );
        assert_eq!(id(best_match(&results, "", None)), None);
        assert!(parse_search(&serde_json::json!({})).is_empty());
    }
}
