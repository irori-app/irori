//! IGDB's game records as Irori keeps them, the queries that ask for them,
//! matching a library title to a search result and filling a store page's
//! gaps with what IGDB knows.

use irori_core::ExtraInfo;
use serde::{Deserialize, Serialize};

use crate::wiki::hltb;

/// Where IGDB serves its images.
const IMAGE_BASE: &str = "https://images.igdb.com/igdb/image/upload";

/// The fields every game query asks for: what the game page and the art
/// picker use, nothing more.
pub(crate) const GAME_FIELDS: &str = "name,summary,storyline,first_release_date,genres.name,\
     involved_companies.company.name,involved_companies.developer,\
     involved_companies.publisher,cover.image_id,artworks.image_id,screenshots.image_id,url";

/// Search results asked for at once.
pub(crate) const SEARCH_LIMIT: u32 = 10;

/// What [`IgdbGame::source`] names in the game page.
pub const SOURCE_NAME: &str = "IGDB";

/// One game as IGDB describes it. Images are IGDB's image ids; build their
/// addresses with [`image_url`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct IgdbGame {
    pub id: u64,
    pub name: String,
    pub summary: Option<String>,
    /// `YYYY-MM-DD`, in UTC.
    pub release_date: Option<String>,
    pub genres: Vec<String>,
    pub developers: Vec<String>,
    pub publishers: Vec<String>,
    pub cover: Option<String>,
    pub artworks: Vec<String>,
    pub screenshots: Vec<String>,
    /// The game's page on igdb.com.
    pub url: Option<String>,
}

impl IgdbGame {
    pub fn release_year(&self) -> Option<u32> {
        self.release_date.as_deref()?.get(..4)?.parse().ok()
    }

    /// The cover at `size`.
    pub fn cover_url(&self, size: ImageSize) -> Option<String> {
        image_url(self.cover.as_deref()?, size)
    }
}

/// IGDB's image sizes Irori uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageSize {
    /// 264 × 374, for lists.
    CoverBig,
    /// 569 × 320, a screenshot thumbnail.
    ScreenshotMed,
    /// 1280 × 720.
    HD,
    /// 1920 × 1080.
    FullHD,
    /// As uploaded.
    Original,
}

impl ImageSize {
    fn name(self) -> &'static str {
        match self {
            ImageSize::CoverBig => "cover_big",
            ImageSize::ScreenshotMed => "screenshot_med",
            ImageSize::HD => "720p",
            ImageSize::FullHD => "1080p",
            ImageSize::Original => "original",
        }
    }
}

/// The address of IGDB image `image_id` at `size`. Ids are letters, digits,
/// `_` and `-`; anything else names no image.
pub fn image_url(image_id: &str, size: ImageSize) -> Option<String> {
    let valid = !image_id.is_empty()
        && image_id.len() <= 64
        && image_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    valid.then(|| format!("{IMAGE_BASE}/t_{}/{image_id}.jpg", size.name()))
}

/// `text` as an Apicalypse string literal's contents: quotes and
/// backslashes escaped, control characters (which would end the clause)
/// dropped.
pub(crate) fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '"' | '\\' => {
                out.push('\\');
                out.push(c);
            }
            c if c.is_control() => out.push(' '),
            c => out.push(c),
        }
    }
    out.trim().to_string()
}

/// The search for games named like `title`.
pub(crate) fn search_query(title: &str) -> String {
    format!(
        "search \"{}\"; fields {GAME_FIELDS}; limit {SEARCH_LIMIT};",
        escape(title)
    )
}

/// The query for game `id`.
pub(crate) fn game_query(id: u64) -> String {
    format!("fields {GAME_FIELDS}; where id = {id}; limit 1;")
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Named {
    name: String,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Image {
    image_id: String,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Involved {
    company: Option<Named>,
    developer: bool,
    publisher: bool,
}

/// A game as the API sends it: expanded fields arrive as objects.
#[derive(Deserialize, Default)]
#[serde(default)]
struct RawGame {
    id: u64,
    name: String,
    summary: Option<String>,
    storyline: Option<String>,
    first_release_date: Option<i64>,
    genres: Vec<Named>,
    involved_companies: Vec<Involved>,
    cover: Option<Image>,
    artworks: Vec<Image>,
    screenshots: Vec<Image>,
    url: Option<String>,
}

fn nonempty(s: Option<String>) -> Option<String> {
    s.map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

fn date(unix: i64) -> Option<String> {
    let t = time::OffsetDateTime::from_unix_timestamp(unix).ok()?;
    Some(format!(
        "{:04}-{:02}-{:02}",
        t.year(),
        u8::from(t.month()),
        t.day()
    ))
}

impl From<RawGame> for IgdbGame {
    fn from(raw: RawGame) -> Self {
        let companies = |pick: fn(&Involved) -> bool| -> Vec<String> {
            let mut names: Vec<String> = raw
                .involved_companies
                .iter()
                .filter(|i| pick(i))
                .filter_map(|i| i.company.as_ref())
                .map(|c| c.name.trim().to_string())
                .filter(|n| !n.is_empty())
                .collect();
            names.dedup();
            names
        };
        let images = |list: &[Image]| -> Vec<String> {
            list.iter()
                .map(|i| i.image_id.clone())
                .filter(|id| image_url(id, ImageSize::Original).is_some())
                .collect()
        };
        Self {
            id: raw.id,
            name: raw.name.trim().to_string(),
            developers: companies(|i| i.developer),
            publishers: companies(|i| i.publisher),
            artworks: images(&raw.artworks),
            screenshots: images(&raw.screenshots),
            cover: raw
                .cover
                .map(|c| c.image_id)
                .filter(|id| image_url(id, ImageSize::Original).is_some()),
            summary: nonempty(raw.summary).or(nonempty(raw.storyline)),
            release_date: raw.first_release_date.and_then(date),
            genres: raw
                .genres
                .into_iter()
                .map(|g| g.name.trim().to_string())
                .filter(|g| !g.is_empty())
                .collect(),
            url: nonempty(raw.url).filter(|u| u.starts_with("https://")),
        }
    }
}

/// The games in an API answer.
pub(crate) fn parse_games(body: &[u8]) -> serde_json::Result<Vec<IgdbGame>> {
    let raw: Vec<RawGame> = serde_json::from_slice(body)?;
    Ok(raw
        .into_iter()
        .filter(|g| g.id != 0)
        .map(IgdbGame::from)
        .collect())
}

/// The search result that is the game titled `title`: its folded name is
/// the title's (case, punctuation and Roman numerals aside), the one from
/// `year` when several are. Nothing that only looks alike is taken.
pub fn best_match<'a>(
    found: &'a [IgdbGame],
    title: &str,
    year: Option<u32>,
) -> Option<&'a IgdbGame> {
    let wanted = hltb::fold(title);
    if wanted.is_empty() {
        return None;
    }
    let same: Vec<&IgdbGame> = found
        .iter()
        .filter(|g| hltb::fold(&g.name) == wanted)
        .collect();
    year.and_then(|y| same.iter().find(|g| g.release_year() == Some(y)).copied())
        .or_else(|| same.first().copied())
}

/// Fill what the store page leaves empty with what IGDB knows: the
/// description, genres, release date and screenshots. The store's own
/// values stay. Returns whether anything was added.
pub fn fill_extra(info: &mut ExtraInfo, game: &IgdbGame) -> bool {
    let mut filled = false;
    let empty = |s: &Option<String>| s.as_deref().is_none_or(|s| s.trim().is_empty());
    if empty(&info.description)
        && empty(&info.short_description)
        && let Some(summary) = &game.summary
    {
        info.description = Some(summary.clone());
        filled = true;
    }
    if info.genres.is_empty() && !game.genres.is_empty() {
        info.genres = game.genres.clone();
        filled = true;
    }
    if empty(&info.release_date)
        && let Some(date) = &game.release_date
    {
        info.release_date = Some(date.clone());
        filled = true;
    }
    if info.screenshots.is_empty() && !game.screenshots.is_empty() {
        info.screenshots = game
            .screenshots
            .iter()
            .filter_map(|id| image_url(id, ImageSize::FullHD))
            .collect();
        filled = !info.screenshots.is_empty() || filled;
    }
    if filled {
        info.details_from = Some(SOURCE_NAME.to_string());
    }
    filled
}

#[cfg(test)]
mod tests {
    use super::*;

    const ANSWER: &str = r#"[{
        "id": 1942, "name": "The Witcher 3: Wild Hunt",
        "summary": "  RPG in a world at war.  ",
        "first_release_date": 1431993600,
        "genres": [{"id": 12, "name": "Role-playing (RPG)"}, {"id": 31, "name": "Adventure"}],
        "involved_companies": [
            {"id": 1, "company": {"id": 908, "name": "CD Projekt RED"}, "developer": true, "publisher": false},
            {"id": 2, "company": {"id": 909, "name": "CD Projekt"}, "developer": false, "publisher": true},
            {"id": 3, "company": {"id": 910, "name": "Bandai Namco"}, "developer": false, "publisher": true}
        ],
        "cover": {"id": 89386, "image_id": "coaarl"},
        "artworks": [{"id": 1, "image_id": "ar5x1"}, {"id": 2, "image_id": "../etc"}],
        "screenshots": [{"id": 3, "image_id": "em1y2ugcwy2myuhvb9db"}],
        "url": "https://www.igdb.com/games/the-witcher-3-wild-hunt",
        "rating": 93.2
    }, {"id": 0}]"#;

    #[test]
    fn parses_an_answer() {
        let games = parse_games(ANSWER.as_bytes()).unwrap();
        assert_eq!(games.len(), 1);
        let g = &games[0];
        assert_eq!(g.summary.as_deref(), Some("RPG in a world at war."));
        assert_eq!(g.release_date.as_deref(), Some("2015-05-19"));
        assert_eq!(g.release_year(), Some(2015));
        assert_eq!(g.developers, ["CD Projekt RED"]);
        assert_eq!(g.publishers, ["CD Projekt", "Bandai Namco"]);
        assert_eq!(g.artworks, ["ar5x1"]);
        assert_eq!(
            g.cover_url(ImageSize::CoverBig).as_deref(),
            Some("https://images.igdb.com/igdb/image/upload/t_cover_big/coaarl.jpg")
        );
        let json = serde_json::to_value(g).unwrap();
        assert_eq!(serde_json::from_value::<IgdbGame>(json).unwrap(), *g);
    }

    #[test]
    fn reads_records_with_unknown_and_missing_fields() {
        let g: IgdbGame =
            serde_json::from_str(r#"{"id": 5, "name": "X", "aNewerField": [1]}"#).unwrap();
        assert_eq!((g.id, g.name.as_str(), g.cover), (5, "X", None));
        let games =
            parse_games(br#"[{"id": 7, "storyline": "Story", "url": "http://x"}]"#).unwrap();
        assert_eq!(games[0].summary.as_deref(), Some("Story"));
        assert_eq!(games[0].url, None);
    }

    #[test]
    fn image_ids_cannot_name_another_address() {
        assert_eq!(image_url("a/../b", ImageSize::HD), None);
        assert_eq!(image_url("", ImageSize::HD), None);
        assert_eq!(image_url("x?y", ImageSize::HD), None);
        assert!(image_url("sc6m-x_1", ImageSize::HD).is_some());
    }

    #[test]
    fn escapes_search_text() {
        assert_eq!(
            search_query("Say \"Hi\"\\\n; fields *"),
            format!(
                "search \"Say \\\"Hi\\\"\\\\ ; fields *\"; fields {GAME_FIELDS}; limit {SEARCH_LIMIT};"
            )
        );
        assert_eq!(
            game_query(42),
            format!("fields {GAME_FIELDS}; where id = 42; limit 1;")
        );
    }

    #[test]
    fn matches_only_the_same_title() {
        let game = |id, name: &str, date: Option<&str>| IgdbGame {
            id,
            name: name.into(),
            release_date: date.map(str::to_string),
            ..Default::default()
        };
        let found = [
            game(1, "Doom Eternal", None),
            game(2, "DOOM", Some("1993-12-10")),
            game(3, "Doom", Some("2016-05-13")),
        ];
        assert_eq!(best_match(&found, "Doom", Some(2016)).unwrap().id, 3);
        assert_eq!(best_match(&found, "doom", None).unwrap().id, 2);
        assert_eq!(best_match(&found, "Doom 3", None), None);
        assert_eq!(best_match(&found, "™", None), None);
        let sequel = [game(9, "Final Fantasy VII", None)];
        assert_eq!(best_match(&sequel, "Final Fantasy 7", None).unwrap().id, 9);
    }

    #[test]
    fn fills_only_what_the_store_lacks() {
        let igdb = IgdbGame {
            summary: Some("From IGDB".into()),
            genres: vec!["Shooter".into()],
            release_date: Some("2016-05-13".into()),
            screenshots: vec!["sc1".into()],
            ..Default::default()
        };
        let mut store = ExtraInfo {
            description: Some("The store's own".into()),
            genres: vec!["Action".into()],
            ..Default::default()
        };
        assert!(fill_extra(&mut store, &igdb));
        assert_eq!(store.description.as_deref(), Some("The store's own"));
        assert_eq!(store.genres, ["Action"]);
        assert_eq!(store.release_date.as_deref(), Some("2016-05-13"));
        assert_eq!(
            store.screenshots,
            ["https://images.igdb.com/igdb/image/upload/t_1080p/sc1.jpg"]
        );
        assert_eq!(store.details_from.as_deref(), Some(SOURCE_NAME));

        let mut full = store.clone();
        full.details_from = None;
        assert!(!fill_extra(&mut full, &igdb));
        assert_eq!(full.details_from, None);

        let mut empty = ExtraInfo::default();
        assert!(fill_extra(&mut empty, &igdb));
        assert_eq!(empty.description.as_deref(), Some("From IGDB"));
    }
}
