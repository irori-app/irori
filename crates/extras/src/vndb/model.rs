//! VNDB's visual novel records as Irori keeps them: the queries that ask
//! for them, the answers read into [`VndbNovel`] (descriptions as plain
//! text, only images VNDB's users flagged safe), matching a library title
//! to a search result and filling a store page's gaps.

use irori_core::ExtraInfo;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::wiki::hltb;

/// What [`fill_extra`] names in the game page.
pub const SOURCE_NAME: &str = "VNDB";
/// VNDB's site, where a novel's page is `<SITE>/<id>`.
pub const SITE: &str = "https://vndb.org";

/// The fields every query asks for: what the game page, the art picker and
/// matching use, nothing more.
pub(crate) const FIELDS: &str = "title,alttitle,titles{title,latin},aliases,released,\
     description,developers{name},image{url,thumbnail,sexual,violence},\
     screenshots{url,thumbnail,sexual,violence}";

/// Search results asked for at once.
pub(crate) const SEARCH_LIMIT: u32 = 10;
/// Screenshots kept per novel; VNDB lists dozens for some.
const MAX_SCREENSHOTS: usize = 20;

/// An image VNDB's users rated safe to show: no sexual content at all and
/// no more than mild violence.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct VndbImage {
    pub url: String,
    pub thumbnail: String,
}

/// One visual novel as VNDB describes it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct VndbNovel {
    /// VNDB's id, `v17`.
    pub id: String,
    pub title: String,
    /// Its other names: the original script's, romanized ones, aliases.
    pub other_titles: Vec<String>,
    /// `YYYY-MM-DD`, or as much of it as is known (`YYYY-MM`, `YYYY`).
    pub release_date: Option<String>,
    /// Plain text, spoilers left out.
    pub description: Option<String>,
    pub developers: Vec<String>,
    pub cover: Option<VndbImage>,
    pub screenshots: Vec<VndbImage>,
}

impl VndbNovel {
    pub fn release_year(&self) -> Option<u32> {
        self.release_date.as_deref()?.get(..4)?.parse().ok()
    }

    /// The novel's page on vndb.org.
    pub fn url(&self) -> String {
        format!("{SITE}/{}", self.id)
    }

    /// The number in the id (`17` for `v17`).
    pub fn number(&self) -> Option<u64> {
        self.id.strip_prefix('v')?.parse().ok()
    }
}

/// `text` as a VNDB novel id (`v17`, or `17`), if it is one.
pub fn novel_id(text: &str) -> Option<String> {
    let text = text.trim();
    let digits = text.strip_prefix(['v', 'V']).unwrap_or(text);
    let valid =
        !digits.is_empty() && digits.len() <= 12 && digits.bytes().all(|b| b.is_ascii_digit());
    valid
        .then(|| format!("v{}", digits.trim_start_matches('0')))
        .filter(|id| id.len() > 1)
}

/// The search for novels named like `title`, VNDB's best matches first.
pub(crate) fn search_query(title: &str) -> serde_json::Value {
    json!({
        "filters": ["search", "=", title.trim()],
        "fields": FIELDS,
        "sort": "searchrank",
        "results": SEARCH_LIMIT,
    })
}

/// The query for novel `id` (already checked with [`novel_id`]).
pub(crate) fn novel_query(id: &str) -> serde_json::Value {
    json!({
        "filters": ["id", "=", id],
        "fields": FIELDS,
        "results": 1,
    })
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RawTitle {
    title: Option<String>,
    latin: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RawProducer {
    name: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RawImage {
    url: Option<String>,
    thumbnail: Option<String>,
    sexual: Option<f64>,
    violence: Option<f64>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RawNovel {
    id: String,
    title: Option<String>,
    alttitle: Option<String>,
    titles: Vec<RawTitle>,
    aliases: Vec<String>,
    released: Option<String>,
    description: Option<String>,
    developers: Vec<RawProducer>,
    image: Option<RawImage>,
    screenshots: Vec<RawImage>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Answer {
    results: Vec<RawNovel>,
}

/// An address VNDB serves its images from: HTTPS on vndb.org or one of its
/// hosts (`t.vndb.org`).
fn image_address(url: Option<String>) -> Option<String> {
    let url = url?.trim().to_string();
    let parsed = url::Url::parse(&url).ok()?;
    let host = parsed.host_str()?;
    (parsed.scheme() == "https" && (host == "vndb.org" || host.ends_with(".vndb.org")))
        .then_some(url)
}

impl RawImage {
    /// The image, when its flags say it is safe to show: sexual content
    /// rated 0 and violence at most 1. An image without ratings isn't.
    fn safe(self) -> Option<VndbImage> {
        let sexual = self.sexual?;
        let violence = self.violence?;
        if sexual > 0.0 || violence > 1.0 {
            return None;
        }
        let url = image_address(self.url)?;
        let thumbnail = image_address(self.thumbnail).unwrap_or_else(|| url.clone());
        Some(VndbImage { url, thumbnail })
    }
}

fn nonempty(s: Option<String>) -> Option<String> {
    s.map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

/// VNDB's date when it names at least a year (not `TBA` or `unknown`).
fn release_date(released: Option<String>) -> Option<String> {
    let date = nonempty(released)?;
    let year_known = date.len() >= 4 && date.as_bytes()[..4].iter().all(u8::is_ascii_digit);
    year_known.then_some(date)
}

impl From<RawNovel> for VndbNovel {
    fn from(raw: RawNovel) -> Self {
        let title = nonempty(raw.title).unwrap_or_default();
        let mut other_titles: Vec<String> = Vec::new();
        let names = raw
            .alttitle
            .into_iter()
            .chain(
                raw.titles
                    .into_iter()
                    .flat_map(|t| [t.title, t.latin])
                    .flatten(),
            )
            .chain(raw.aliases);
        for name in names {
            let name = name.trim();
            if !name.is_empty() && name != title && !other_titles.iter().any(|n| n == name) {
                other_titles.push(name.to_string());
            }
        }
        let mut developers: Vec<String> = Vec::new();
        for name in raw.developers.into_iter().filter_map(|p| nonempty(p.name)) {
            if !developers.contains(&name) {
                developers.push(name);
            }
        }
        Self {
            id: novel_id(&raw.id).unwrap_or_default(),
            title,
            other_titles,
            release_date: release_date(raw.released),
            description: raw
                .description
                .map(|d| plain_text(&d))
                .filter(|d| !d.is_empty()),
            developers,
            cover: raw.image.and_then(RawImage::safe),
            screenshots: raw
                .screenshots
                .into_iter()
                .filter_map(RawImage::safe)
                .take(MAX_SCREENSHOTS)
                .collect(),
        }
    }
}

/// The novels in an API answer.
pub(crate) fn parse_answer(body: &[u8]) -> serde_json::Result<Vec<VndbNovel>> {
    let answer: Answer = serde_json::from_slice(body)?;
    Ok(answer
        .results
        .into_iter()
        .map(VndbNovel::from)
        .filter(|n| !n.id.is_empty())
        .collect())
}

/// A description's formatting codes as plain text: a link's text without
/// its address, spoilers left out, other codes (`[b]`, `[quote]`, `[raw]`…)
/// dropped around what they hold. Blank lines left by removed spoilers
/// close up.
pub fn plain_text(description: &str) -> String {
    let mut out = String::with_capacity(description.len());
    let mut rest = description;
    let mut spoilers = 0usize;
    while let Some(open) = rest.find('[') {
        let (before, tag_start) = rest.split_at(open);
        if spoilers == 0 {
            out.push_str(before);
        }
        let Some(close) = tag_start.find(']') else {
            if spoilers == 0 {
                out.push_str(tag_start);
            }
            rest = "";
            break;
        };
        let tag = tag_start[1..close].to_ascii_lowercase();
        let after = &tag_start[close + 1..];
        match tag.as_str() {
            "spoiler" => spoilers += 1,
            "/spoiler" => spoilers = spoilers.saturating_sub(1),
            "b" | "/b" | "i" | "/i" | "u" | "/u" | "s" | "/s" | "quote" | "/quote" | "raw"
            | "/raw" | "code" | "/code" | "/url" => {}
            t if t.starts_with("url=") => {}
            _ => {
                // Not a code: the bracket is part of the text.
                if spoilers == 0 {
                    out.push('[');
                }
                rest = &tag_start[1..];
                continue;
            }
        }
        rest = after;
    }
    if spoilers == 0 {
        out.push_str(rest);
    }
    let lines: Vec<&str> = out.lines().map(str::trim_end).collect();
    let mut text = String::new();
    let mut blank = false;
    for line in lines {
        if line.trim().is_empty() {
            blank = !text.is_empty();
            continue;
        }
        if blank {
            text.push_str("\n\n");
        } else if !text.is_empty() {
            text.push('\n');
        }
        blank = false;
        text.push_str(line);
    }
    text
}

/// The search result that is the game titled `title`: its title or one of
/// its other names folded like the title (case, punctuation and Roman
/// numerals aside), the one from `year` when several are. Nothing that
/// only looks alike is taken.
pub fn best_match<'a>(
    found: &'a [VndbNovel],
    title: &str,
    year: Option<u32>,
) -> Option<&'a VndbNovel> {
    let wanted = hltb::fold(title);
    if wanted.is_empty() {
        return None;
    }
    let same: Vec<&VndbNovel> = found
        .iter()
        .filter(|n| {
            std::iter::once(&n.title)
                .chain(&n.other_titles)
                .any(|name| hltb::fold(name) == wanted)
        })
        .collect();
    year.and_then(|y| same.iter().find(|n| n.release_year() == Some(y)).copied())
        .or_else(|| same.first().copied())
}

/// Fill what the store page (and IGDB) left empty with what VNDB knows:
/// the description, release date, developers and screenshots. Values
/// already there stay. Returns whether anything was added.
pub fn fill_extra(info: &mut ExtraInfo, novel: &VndbNovel) -> bool {
    let mut filled = false;
    let empty = |s: &Option<String>| s.as_deref().is_none_or(|s| s.trim().is_empty());
    if empty(&info.description)
        && empty(&info.short_description)
        && let Some(text) = &novel.description
    {
        info.description = Some(text.clone());
        filled = true;
    }
    if empty(&info.release_date)
        && let Some(date) = &novel.release_date
    {
        info.release_date = Some(date.clone());
        filled = true;
    }
    if info.developers.is_empty() && !novel.developers.is_empty() {
        info.developers = novel.developers.clone();
        filled = true;
    }
    if info.screenshots.is_empty() && !novel.screenshots.is_empty() {
        info.screenshots = novel.screenshots.iter().map(|s| s.url.clone()).collect();
        filled = true;
    }
    if filled {
        info.note_source(SOURCE_NAME);
    }
    filled
}

#[cfg(test)]
mod tests {
    use super::*;

    const ANSWER: &str = r#"{"more": true, "results": [{
        "id": "v17",
        "title": "Ever17 -the out of infinity-",
        "alttitle": "Ever17 -the out of infinity-",
        "titles": [
            {"title": "Ever17 -the out of infinity-", "latin": null},
            {"title": "エバー17", "latin": "Ever Seventeen"}
        ],
        "aliases": ["E17", "Ever Seventeen"],
        "released": "2002-08-29",
        "description": "Takeshi visits [url=https://example.com/lemu]LeMU[/url].\n\n[spoiler]Coco is [b]not[/b] who she seems.[/spoiler]\n\n[From [url=https://example.com]the publisher[/url]]",
        "developers": [{"id": "p8", "name": "KID"}, {"id": "p9", "name": " "}, {"id": "p8", "name": "KID"}],
        "image": {"url": "https://t.vndb.org/cv/57/102657.jpg", "thumbnail": "https://t.vndb.org/cv.t/57/102657.jpg", "sexual": 0, "violence": 0},
        "screenshots": [
            {"url": "https://t.vndb.org/sf/1.jpg", "thumbnail": "https://t.vndb.org/st/1.jpg", "sexual": 0.0, "violence": 1.0},
            {"url": "https://t.vndb.org/sf/2.jpg", "thumbnail": "https://t.vndb.org/st/2.jpg", "sexual": 0.4, "violence": 0},
            {"url": "https://t.vndb.org/sf/3.jpg", "thumbnail": "https://t.vndb.org/st/3.jpg", "sexual": 0, "violence": 1.6},
            {"url": "https://t.vndb.org/sf/4.jpg", "thumbnail": "https://t.vndb.org/st/4.jpg"},
            {"url": "http://t.vndb.org/sf/5.jpg", "sexual": 0, "violence": 0},
            {"url": "https://evil.example/vndb.org/6.jpg", "sexual": 0, "violence": 0}
        ],
        "aNewerField": 1
    }, {"id": ""}]}"#;

    #[test]
    fn parses_an_answer() {
        let novels = parse_answer(ANSWER.as_bytes()).unwrap();
        assert_eq!(novels.len(), 1);
        let n = &novels[0];
        assert_eq!(n.id, "v17");
        assert_eq!(n.number(), Some(17));
        assert_eq!(n.url(), "https://vndb.org/v17");
        assert_eq!(n.other_titles, ["エバー17", "Ever Seventeen", "E17"]);
        assert_eq!(n.release_year(), Some(2002));
        assert_eq!(n.developers, ["KID"]);
        assert_eq!(
            n.description.as_deref(),
            Some("Takeshi visits LeMU.\n\n[From the publisher]")
        );
        assert_eq!(
            n.cover.as_ref().map(|c| c.thumbnail.as_str()),
            Some("https://t.vndb.org/cv.t/57/102657.jpg")
        );
        let json = serde_json::to_value(n).unwrap();
        assert_eq!(serde_json::from_value::<VndbNovel>(json).unwrap(), *n);
    }

    #[test]
    fn shows_only_images_flagged_safe() {
        let n = &parse_answer(ANSWER.as_bytes()).unwrap()[0];
        let urls: Vec<&str> = n.screenshots.iter().map(|s| s.url.as_str()).collect();
        // Sexual above 0, violence above 1, unrated, plain HTTP and other
        // hosts are all left out.
        assert_eq!(urls, ["https://t.vndb.org/sf/1.jpg"]);
        let flagged = parse_answer(
            br#"{"results": [{"id": "v2", "title": "X",
                "image": {"url": "https://t.vndb.org/cv/1.jpg", "sexual": 2, "violence": 0}}]}"#,
        )
        .unwrap();
        assert_eq!(flagged[0].cover, None);
    }

    #[test]
    fn reads_dates_only_with_a_year() {
        let date = |d: &str| release_date(Some(d.to_string()));
        assert_eq!(date("2004").as_deref(), Some("2004"));
        assert_eq!(date("2004-05").as_deref(), Some("2004-05"));
        assert_eq!(date("TBA"), None);
        assert_eq!(date("unknown"), None);
    }

    #[test]
    fn turns_markup_into_plain_text() {
        assert_eq!(plain_text("[b]Bold[/b] and [i]it[/i]"), "Bold and it");
        assert_eq!(
            plain_text("Before.\n\n[spoiler]Hidden [url=x]link[/url].[/spoiler]\n\nAfter."),
            "Before.\n\nAfter."
        );
        assert_eq!(
            plain_text("A [note] stays [ and so"),
            "A [note] stays [ and so"
        );
        assert_eq!(plain_text("[SPOILER]all[/SPOILER]"), "");
        assert_eq!(plain_text("Unclosed [spoiler]secret"), "Unclosed");
        assert_eq!(plain_text("[quote]Said[/quote]\n\n\n"), "Said");
    }

    #[test]
    fn reads_novel_ids() {
        assert_eq!(novel_id("v17").as_deref(), Some("v17"));
        assert_eq!(novel_id(" V0042 ").as_deref(), Some("v42"));
        assert_eq!(novel_id("17").as_deref(), Some("v17"));
        assert_eq!(novel_id("v"), None);
        assert_eq!(novel_id("v0"), None);
        assert_eq!(novel_id("r17"), None);
        assert_eq!(novel_id("none"), None);
        assert_eq!(novel_query("v17")["filters"], json!(["id", "=", "v17"]));
        assert_eq!(
            search_query(" Ever17 ")["filters"],
            json!(["search", "=", "Ever17"])
        );
    }

    #[test]
    fn matches_titles_and_other_names_only() {
        let novel = |id: &str, title: &str, others: &[&str], date: Option<&str>| VndbNovel {
            id: id.into(),
            title: title.into(),
            other_titles: others.iter().map(|s| s.to_string()).collect(),
            release_date: date.map(str::to_string),
            ..Default::default()
        };
        let found = [
            novel(
                "v1",
                "Ever17 -the out of infinity-",
                &["エバー17"],
                Some("2002"),
            ),
            novel("v2", "Ever17", &[], Some("2011-06-30")),
            novel("v3", "Ever17 -the out of infinity- Premium", &[], None),
            novel("v4", "Saya no Uta", &["沙耶の唄", "The Song of Saya"], None),
        ];
        assert_eq!(
            best_match(&found, "Ever17: The Out of Infinity", None)
                .unwrap()
                .id,
            "v1"
        );
        assert_eq!(best_match(&found, "エバー17", None).unwrap().id, "v1");
        assert_eq!(
            best_match(&found, "The Song of Saya", None).unwrap().id,
            "v4"
        );
        assert_eq!(best_match(&found, "Ever17", Some(2011)).unwrap().id, "v2");
        assert_eq!(best_match(&found, "Ever 18", None), None);
        assert_eq!(best_match(&found, "™", None), None);
    }

    #[test]
    fn fills_only_what_others_left_empty() {
        let novel = VndbNovel {
            id: "v1".into(),
            description: Some("From VNDB".into()),
            release_date: Some("2002-08-29".into()),
            developers: vec!["KID".into()],
            screenshots: vec![VndbImage {
                url: "https://t.vndb.org/sf/1.jpg".into(),
                thumbnail: String::new(),
            }],
            ..Default::default()
        };
        let mut store = ExtraInfo {
            description: Some("The store's own".into()),
            details_from: Some("IGDB".into()),
            ..Default::default()
        };
        assert!(fill_extra(&mut store, &novel));
        assert_eq!(store.description.as_deref(), Some("The store's own"));
        assert_eq!(store.release_date.as_deref(), Some("2002-08-29"));
        assert_eq!(store.developers, ["KID"]);
        assert_eq!(store.screenshots, ["https://t.vndb.org/sf/1.jpg"]);
        assert_eq!(store.details_from.as_deref(), Some("IGDB, VNDB"));

        let mut full = store.clone();
        full.details_from = None;
        assert!(!fill_extra(&mut full, &novel));
        assert_eq!(full.details_from, None);
    }
}
