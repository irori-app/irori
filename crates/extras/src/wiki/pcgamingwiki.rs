//! PCGamingWiki: page lookup (by GOG ID, else search) and wikitext scraping.

use std::time::Duration;

use anyhow::Result;
use irori_core::GameMode;
use once_cell::sync::Lazy;
use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::util;

const API: &str = "https://www.pcgamingwiki.com/w/api.php";
const TIMEOUT: Duration = Duration::from_secs(10);

/// A review score with its site-specific ID.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct GameScore {
    pub score: String,
    pub url_id: String,
}

/// Data scraped from a PCGamingWiki page.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PcGamingWikiInfo {
    pub page_id: u64,
    pub steam_id: String,
    pub how_long_to_beat_id: String,
    pub metacritic: GameScore,
    pub opencritic: GameScore,
    pub igdb: GameScore,
    pub direct3d_versions: Vec<String>,
    pub genres: Vec<String>,
    /// `"<platform>: <date>"` entries.
    pub release_dates: Vec<String>,
    /// From the infobox's modes and the multiplayer table (see
    /// [`game_modes`]).
    #[serde(
        skip_serializing_if = "Vec::is_empty",
        deserialize_with = "irori_core::game_mode::lenient"
    )]
    pub game_modes: Vec<GameMode>,
}

impl PcGamingWikiInfo {
    /// Link to the wiki page.
    pub fn page_url(&self) -> String {
        format!("https://www.pcgamingwiki.com/wiki/?curid={}", self.page_id)
    }
}

static METACRITIC: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?m)game/row/reception\|Metacritic\|(\S+)\|([0-9]+)").unwrap());
static OPENCRITIC: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?m)game/row/reception\|OpenCritic\|(\S+)\|([0-9]+)").unwrap());
static IGDB: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?m)game/row/reception\|IGDB\|(\S+)\|([0-9]+)").unwrap());
static STEAM_ID: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?m)steam appid {2}= ([0-9]+)").unwrap());
static HLTB_ID: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?m)hltb {9}= ([0-9]+)").unwrap());
static D3D: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?m)direct3d versions {6}= (.+)").unwrap());
static GENRES: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"game/row/taxonomy/genres\s*\|\s*(.*?)\s*\}\}").unwrap());
static MODES: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"game/row/taxonomy/modes\s*\|\s*(.*?)\s*\}\}").unwrap());
/// One field of the `{{Network/Multiplayer}}` table (`|online play modes = Co-op, Versus`).
static NETWORK_FIELD: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?m)^\|\s*(local play|lan play|online play)( modes)?\s*=[ \t]*(.*?)\s*$").unwrap()
});

/// Game modes of a page: "Singleplayer" in the infobox's modes, local,
/// LAN and online play from the multiplayer table (`true` or `limited`;
/// `false`, `unknown`, `hackable` and `n/a` don't count), co-op when one of
/// those plays has a "Co-op" mode, and MMO genres. LAN play counts as
/// online: both are network multiplayer.
pub fn game_modes(text: &str) -> Vec<GameMode> {
    let mut modes = Vec::new();
    if list(&MODES, text)
        .iter()
        .any(|m| m.eq_ignore_ascii_case("singleplayer"))
    {
        modes.push(GameMode::Single);
    }
    let mut played = Vec::new();
    let mut coop = Vec::new();
    for c in NETWORK_FIELD.captures_iter(text) {
        let venue = match &c[1] {
            "local play" => GameMode::Local,
            _ => GameMode::Online,
        };
        let value = c[3].to_ascii_lowercase();
        if c.get(2).is_some() {
            if value.split(',').any(|m| m.trim() == "co-op") {
                coop.push(venue);
            }
        } else if matches!(value.as_str(), "true" | "limited") {
            played.push(venue);
        }
    }
    modes.extend(played.iter().copied());
    if coop.iter().any(|v| played.contains(v)) {
        modes.push(GameMode::Coop);
    }
    if list(&GENRES, text)
        .iter()
        .any(|g| g.to_ascii_uppercase().starts_with("MMO"))
    {
        modes.extend([GameMode::Mmo, GameMode::Online]);
    }
    irori_core::game_mode::normalized(modes)
}

static RELEASE: Lazy<Regex> = Lazy::new(|| Regex::new(r"game/row/date\|([^|]+)\|([^}]*)").unwrap());

fn score(re: &Regex, text: &str) -> GameScore {
    re.captures(text)
        .map(|c| GameScore {
            url_id: c[1].to_string(),
            score: c[2].to_string(),
        })
        .unwrap_or_default()
}

fn list(re: &Regex, text: &str) -> Vec<String> {
    re.captures(text)
        .map(|c| {
            c[1].split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// Extract the fields Irori shows from page wikitext.
pub fn parse_wikitext(page_id: u64, text: &str) -> PcGamingWikiInfo {
    let group = |re: &Regex| {
        re.captures(text)
            .map(|c| c[1].to_string())
            .unwrap_or_default()
    };
    PcGamingWikiInfo {
        page_id,
        steam_id: group(&STEAM_ID),
        how_long_to_beat_id: group(&HLTB_ID),
        metacritic: score(&METACRITIC, text),
        opencritic: score(&OPENCRITIC, text),
        igdb: score(&IGDB, text),
        direct3d_versions: list(&D3D, text),
        genres: list(&GENRES, text),
        release_dates: RELEASE
            .captures_iter(text)
            .map(|c| format!("{}: {}", &c[1], &c[2]))
            .collect(),
        game_modes: game_modes(text),
    }
}

#[derive(Deserialize)]
struct SearchResponse {
    query: Option<SearchQuery>,
}

#[derive(Deserialize)]
struct SearchQuery {
    #[serde(default)]
    search: Vec<SearchHit>,
}

#[derive(Deserialize)]
struct SearchHit {
    pageid: u64,
    #[serde(default)]
    title: String,
}

/// Page ID from a search response: the exact (case-insensitive) title match,
/// else the first hit.
pub fn pick_search_result(json: &serde_json::Value, title: &str) -> Option<u64> {
    let resp: SearchResponse = serde_json::from_value(json.clone()).ok()?;
    let hits = resp.query?.search;
    hits.iter()
        .find(|h| h.title.to_lowercase() == title.to_lowercase())
        .or(hits.first())
        .map(|h| h.pageid)
}

/// Whether a page's wikitext names `gog_id` among its GOG.com ids
/// (`gogcom id`, and `gogcom id side` for editions and packs).
pub fn names_gog_id(wikitext: &str, gog_id: &str) -> bool {
    GOG_IDS
        .captures_iter(wikitext)
        .any(|c| c[1].split(',').any(|id| id.trim() == gog_id))
}

static GOG_IDS: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?m)^\|gogcom id(?: side)? *= *([0-9, ]+)").unwrap());

/// The pages of a `prop=revisions` answer (`formatversion=2`), as
/// (page id, wikitext), in the order asked.
pub fn parse_revisions(json: &serde_json::Value, order: &[u64]) -> Vec<(u64, String)> {
    let mut pages: Vec<(u64, String)> = json
        .pointer("/query/pages")
        .and_then(|p| p.as_array())
        .into_iter()
        .flatten()
        .filter_map(|page| {
            let id = page.get("pageid")?.as_u64()?;
            let text = page.pointer("/revisions/0/slots/main/content")?.as_str()?;
            Some((id, text.to_owned()))
        })
        .collect();
    pages.sort_by_key(|(id, _)| order.iter().position(|o| o == id).unwrap_or(usize::MAX));
    pages
}

/// Look the game up. `Ok(None)` when the wiki has no page for it.
///
/// The wiki refuses anonymous Cargo queries ("permissiondenied"), so a GOG
/// game is found by searching its title and keeping the hit whose infobox
/// lists its GOG id; the hits' wikitext comes in one request, so this costs
/// what the lookup by id did.
pub async fn fetch(title: &str, gog_id: Option<&str>) -> Result<Option<PcGamingWikiInfo>> {
    let title = title.replacen(" -", ":", 1);
    let url = format!(
        "{API}?action=query&list=search&srsearch={}&srlimit={SEARCH_HITS}&format=json",
        util::encode_component(&title)
    );
    let json: serde_json::Value = util::get_json(&url, TIMEOUT).await?;
    let Some(best) = pick_search_result(&json, &title) else {
        return Ok(None);
    };
    if let Some(gog) = gog_id.filter(|s| !s.is_empty()) {
        let hits = search_hits(&json);
        let ids = hits
            .iter()
            .map(u64::to_string)
            .collect::<Vec<_>>()
            .join("|");
        let url = format!(
            "{API}?action=query&prop=revisions&rvprop=content&rvslots=main&pageids={}&format=json&formatversion=2",
            util::encode_component(&ids)
        );
        let json: serde_json::Value = util::get_json(&url, TIMEOUT).await?;
        let pages = parse_revisions(&json, &hits);
        let page = pages
            .iter()
            .find(|(_, text)| names_gog_id(text, gog))
            .or_else(|| pages.iter().find(|(id, _)| *id == best));
        return Ok(page
            .filter(|(_, text)| !text.is_empty())
            .map(|(id, text)| parse_wikitext(*id, text)));
    }
    let url = format!("{API}?action=parse&format=json&pageid={best}&redirects=true&prop=wikitext");
    let json: serde_json::Value = util::get_json(&url, TIMEOUT).await?;
    let text = json
        .pointer("/parse/wikitext/*")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if text.is_empty() {
        return Ok(None);
    }
    Ok(Some(parse_wikitext(best, text)))
}

/// How many search hits a GOG game's id is checked against.
const SEARCH_HITS: usize = 5;

/// Page ids of a search response, in rank order.
fn search_hits(json: &serde_json::Value) -> Vec<u64> {
    serde_json::from_value::<SearchResponse>(json.clone())
        .ok()
        .and_then(|r| r.query)
        .map(|q| q.search.iter().map(|h| h.pageid).collect())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    const WIKITEXT: &str = "{{Infobox game\n|steam appid  = 1091500\n|hltb         = 2127\n\
|direct3d versions      = 12, 11\n}}\n{{Infobox game/row/reception|Metacritic|cyberpunk-2077|86}}\n\
{{Infobox game/row/reception|OpenCritic|8525/cyberpunk-2077|89}}\n{{Infobox game/row/reception|IGDB|cyberpunk-2077|84}}\n\
{{Infobox game/row/taxonomy/genres | Action, RPG, Open world }}\n\
{{Infobox game/row/date|Windows|December 10, 2020}}\n{{Infobox game/row/date|OS X|TBA}}\n";

    #[test]
    fn parses_wikitext() {
        let info = parse_wikitext(7, WIKITEXT);
        assert_eq!(info.steam_id, "1091500");
        assert_eq!(info.how_long_to_beat_id, "2127");
        assert_eq!(info.direct3d_versions, ["12", "11"]);
        assert_eq!(info.genres, ["Action", "RPG", "Open world"]);
        assert_eq!(
            info.metacritic,
            GameScore {
                score: "86".into(),
                url_id: "cyberpunk-2077".into()
            }
        );
        assert_eq!(info.opencritic.url_id, "8525/cyberpunk-2077");
        assert_eq!(info.igdb.score, "84");
        assert_eq!(
            info.release_dates,
            ["Windows: December 10, 2020", "OS X: TBA"]
        );
        assert!(info.page_url().ends_with("curid=7"));
    }

    /// Portal 2's infobox modes and multiplayer table, as its page has them.
    const PORTAL_2: &str = "{{Infobox game/row/taxonomy/modes             | Singleplayer, Multiplayer}}\n\
{{Infobox game/row/taxonomy/genres            | Platform, Puzzle}}\n\
{{Network/Multiplayer\n\
|local play           = true\n\
|local play players   = 2\n\
|local play modes     = Co-op\n\
|local play notes     = Split-screen.\n\
|lan play             = limited\n\
|lan play players     = 2\n\
|lan play modes       = Co-op\n\
|online play          = true\n\
|online play players  = 2\n\
|online play modes    = Co-op\n\
|asynchronous         = false\n\
}}\n";

    #[test]
    fn reads_game_modes() {
        assert_eq!(
            parse_wikitext(1, PORTAL_2).game_modes,
            [
                GameMode::Single,
                GameMode::Coop,
                GameMode::Local,
                GameMode::Online
            ]
        );
        // Versus over the network only; local play is false.
        let versus = "{{Network/Multiplayer\n|local play           = false\n\
|local play modes     = Co-op\n|online play          = true\n|online play modes    = Versus\n}}";
        assert_eq!(game_modes(versus), [GameMode::Online]);
        let mmo = "{{Infobox game/row/taxonomy/genres | MMORPG, Fantasy }}";
        assert_eq!(game_modes(mmo), [GameMode::Online, GameMode::Mmo]);
        assert!(game_modes(WIKITEXT).is_empty());
    }

    #[test]
    fn empty_wikitext_gives_empty_lists() {
        let info = parse_wikitext(1, "nothing here");
        assert!(info.genres.is_empty());
        assert!(info.direct3d_versions.is_empty());
        assert_eq!(info.steam_id, "");
    }

    #[test]
    fn search_hits_rank() {
        let json = serde_json::json!({"query": {"search": [
            {"pageid": 1, "title": "Doom Eternal"}, {"pageid": 2, "title": "DOOM"}]}});
        assert_eq!(pick_search_result(&json, "doom"), Some(2));
        assert_eq!(pick_search_result(&json, "Doom 3"), Some(1));
        assert_eq!(
            pick_search_result(&serde_json::json!({"query": {"search": []}}), "x"),
            None
        );
        assert_eq!(search_hits(&json), [1, 2]);
    }

    #[test]
    fn a_gog_game_is_the_hit_listing_its_id() {
        let witcher = "{{Infobox game\n|gogcom id    = 1495134320\n\
|gogcom id side = 1207664663,1207664643\n|hltb = 1}}";
        assert!(names_gog_id(witcher, "1495134320"));
        assert!(names_gog_id(witcher, "1207664643"));
        assert!(!names_gog_id(witcher, "120766466"));
        assert!(!names_gog_id("|steam appid  = 1207664663", "1207664663"));

        let json = serde_json::json!({"query": {"pages": [
            {"pageid": 9, "title": "B", "revisions": [{"slots": {"main": {"content": "b"}}}]},
            {"pageid": 3, "title": "A", "revisions": [{"slots": {"main": {"content": witcher}}}]},
            {"pageid": 4, "title": "Missing", "missing": true}
        ]}});
        let pages = parse_revisions(&json, &[3, 9]);
        assert_eq!(pages.iter().map(|p| p.0).collect::<Vec<_>>(), [3, 9]);
        assert!(names_gog_id(&pages[0].1, "1207664663"));
    }
}
