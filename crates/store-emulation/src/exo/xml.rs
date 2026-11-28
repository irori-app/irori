//! Streaming LaunchBox XML parser for local eXo game collections.
//!
//! Reads `<LaunchBox><Game>...</Game></LaunchBox>` platform catalogs (such as
//! `Data/Platforms/MS-DOS.xml` or `Windows 3x.xml`) with a strict byte cap so
//! corrupt or oversized files cannot exhaust memory.

use std::fs::File;
use std::io::{BufReader, Read as _};
use std::path::Path;

use anyhow::{Context as _, Result, bail};
use quick_xml::Reader;
use quick_xml::events::Event;

/// Maximum bytes accepted for a single platform XML file (64 MiB).
/// Full eXoDOS `MS-DOS.xml` (~9,000 titles) is typically 15 to 25 MiB.
pub const MAX_XML_BYTES: u64 = 64 * 1024 * 1024;

/// Maximum UTF-8 bytes kept for a single XML field value.
const MAX_FIELD_BYTES: usize = 32 * 1024;

/// Parsed fields from one LaunchBox `<Game>` element.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct XmlGame {
    pub title: String,
    pub sort_title: Option<String>,
    pub platform: Option<String>,
    pub developer: Option<String>,
    pub publisher: Option<String>,
    pub release_date: Option<String>,
    pub genres: Vec<String>,
    pub notes: Option<String>,
    pub series: Option<String>,
    pub region: Option<String>,
    pub application_path: Option<String>,
    pub manual_path: Option<String>,
}

/// Parse all `<Game>` elements from an eXo LaunchBox XML file at `path`.
pub fn parse_platform_xml(path: &Path) -> Result<Vec<XmlGame>> {
    let meta = std::fs::metadata(path)
        .with_context(|| format!("reading metadata of {}", path.display()))?;
    if meta.len() > MAX_XML_BYTES {
        bail!(
            "{} exceeds the {} MiB XML size limit",
            path.display(),
            MAX_XML_BYTES / (1024 * 1024)
        );
    }
    let file = File::open(path).with_context(|| format!("opening {}", path.display()))?;
    parse_reader(BufReader::new(file.take(MAX_XML_BYTES)))
        .with_context(|| format!("parsing {}", path.display()))
}

/// Stream-parse LaunchBox `<Game>` entries from `reader`.
pub fn parse_reader<R: std::io::BufRead>(reader: R) -> Result<Vec<XmlGame>> {
    let mut xml = Reader::from_reader(reader);
    xml.config_mut().trim_text(false);

    let mut buf = Vec::with_capacity(4096);
    let mut games = Vec::new();
    let mut current: Option<XmlGame> = None;
    let mut field: Option<Field> = None;
    let mut text = String::new();
    let mut depth_in_game = 0usize;

    loop {
        buf.clear();
        match xml.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                let name = e.local_name();
                if current.is_none() {
                    if name.as_ref().eq_ignore_ascii_case("Game") {
                        current = Some(XmlGame::default());
                        depth_in_game = 0;
                    }
                } else {
                    depth_in_game += 1;
                    if depth_in_game == 1 {
                        field = Field::from_name(name.as_ref());
                        text.clear();
                    }
                }
            }
            Ok(Event::Text(e)) => {
                if field.is_some() && text.len() < MAX_FIELD_BYTES {
                    push_capped(&mut text, &e.xml10_content());
                }
            }
            Ok(Event::GeneralRef(e)) => {
                if field.is_some() && text.len() < MAX_FIELD_BYTES {
                    if let Ok(Some(c)) = e.resolve_char_ref() {
                        let mut tmp = [0u8; 4];
                        push_capped(&mut text, c.encode_utf8(&mut tmp));
                    } else if let Some(s) =
                        quick_xml::escape::resolve_predefined_entity(&e.xml10_content())
                    {
                        push_capped(&mut text, s);
                    }
                }
            }
            Ok(Event::CData(e)) => {
                if field.is_some() && text.len() < MAX_FIELD_BYTES {
                    push_capped(&mut text, e.as_ref());
                }
            }
            Ok(Event::End(e)) => {
                if let Some(game) = current.as_mut() {
                    if depth_in_game == 0 {
                        if e.local_name().as_ref().eq_ignore_ascii_case("Game")
                            && let Some(finished) = current.take()
                            && !finished.title.is_empty()
                        {
                            games.push(finished);
                        }
                    } else {
                        if depth_in_game == 1
                            && let Some(f) = field.take()
                        {
                            apply_field(game, f, &text);
                            text.clear();
                        }
                        depth_in_game -= 1;
                    }
                }
            }
            Ok(Event::Eof) => break,
            Ok(_) => {}
            Err(e) => return Err(e).context("malformed LaunchBox XML"),
        }
    }
    Ok(games)
}

#[derive(Debug, Clone, Copy)]
enum Field {
    Title,
    SortTitle,
    Platform,
    Developer,
    Publisher,
    ReleaseDate,
    Genre,
    Notes,
    Series,
    Region,
    ApplicationPath,
    ManualPath,
}

impl Field {
    fn from_name(tag: &str) -> Option<Self> {
        match tag {
            "Title" => Some(Self::Title),
            "SortTitle" => Some(Self::SortTitle),
            "Platform" => Some(Self::Platform),
            "Developer" => Some(Self::Developer),
            "Publisher" => Some(Self::Publisher),
            "ReleaseDate" => Some(Self::ReleaseDate),
            "Genre" => Some(Self::Genre),
            "Notes" => Some(Self::Notes),
            "Series" => Some(Self::Series),
            "Region" => Some(Self::Region),
            "ApplicationPath" => Some(Self::ApplicationPath),
            "ManualPath" => Some(Self::ManualPath),
            _ => None,
        }
    }
}

fn push_capped(dst: &mut String, src: &str) {
    let remaining = MAX_FIELD_BYTES.saturating_sub(dst.len());
    if src.len() <= remaining {
        dst.push_str(src);
        return;
    }
    let mut end = remaining;
    while end > 0 && !src.is_char_boundary(end) {
        end -= 1;
    }
    dst.push_str(&src[..end]);
}

fn non_empty(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

fn apply_field(game: &mut XmlGame, field: Field, raw: &str) {
    match field {
        Field::Title => {
            if let Some(t) = non_empty(raw) {
                game.title = t;
            }
        }
        Field::SortTitle => game.sort_title = non_empty(raw),
        Field::Platform => game.platform = non_empty(raw),
        Field::Developer => game.developer = non_empty(raw),
        Field::Publisher => game.publisher = non_empty(raw),
        Field::ReleaseDate => game.release_date = normalize_release_date(raw),
        Field::Genre => {
            game.genres = raw
                .split(';')
                .map(str::trim)
                .filter(|g| !g.is_empty())
                .map(str::to_string)
                .collect();
        }
        Field::Notes => game.notes = non_empty(raw),
        Field::Series => game.series = non_empty(raw),
        Field::Region => game.region = non_empty(raw),
        Field::ApplicationPath => game.application_path = non_empty(raw),
        Field::ManualPath => game.manual_path = non_empty(raw),
    }
}

/// Turn LaunchBox ISO timestamps (`1993-12-10T00:00:00+00:00`) into `YYYY-MM-DD`
/// (or a 4-digit year when no full date is present).
pub fn normalize_release_date(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.len() >= 10
        && trimmed.as_bytes()[4] == b'-'
        && trimmed.as_bytes()[7] == b'-'
        && trimmed[..4].bytes().all(|b| b.is_ascii_digit())
    {
        return Some(trimmed[..10].to_string());
    }
    if trimmed.len() >= 4 && trimmed[..4].bytes().all(|b| b.is_ascii_digit()) {
        return Some(trimmed[..4].to_string());
    }
    None
}

/// Extract `(shortcode, optional_lang_dir)` from an eXo LaunchBox `ApplicationPath`.
///
/// Examples:
/// - `eXo\eXoDOS\!dos\doom\DOOM (1993).bat` -> `("doom", None)`
/// - `eXo\eXoDOS\!dos\!german\doom\DOOM (1993).bat` -> `("doom", Some("!german"))`
pub fn extract_shortcode_and_lang(
    app_path: &str,
    config_segment: &str,
) -> Option<(String, Option<String>)> {
    let parts: Vec<&str> = app_path
        .split(['\\', '/'])
        .filter(|p| !p.is_empty() && *p != ".")
        .collect();
    let seg_idx = parts
        .iter()
        .position(|p| p.eq_ignore_ascii_case(config_segment))?;
    let after = parts.get(seg_idx + 1..)?;
    // Need at least `<shortcode>/<script>` after the segment.
    if after.len() < 2 {
        return None;
    }
    if after[0].starts_with('!') && after.len() >= 3 {
        Some((after[1].to_string(), Some(after[0].to_string())))
    } else {
        Some((after[0].to_string(), None))
    }
}

/// Extract a short language tag (`"DE"`, `"ES"`, `"PL"`, ...) for non-English
/// eXo language packs from LaunchBox's `Series` field or `!lang` folder name.
pub fn extract_language(series: Option<&str>, lang_dir: Option<&str>) -> Option<String> {
    if let Some(s) = series {
        for part in s.split(';') {
            if let Some(lang) = part.trim().strip_prefix("Language:")
                && let Some(code) = normalize_language(lang.trim())
            {
                return Some(code);
            }
        }
    }
    lang_dir.and_then(|d| normalize_language(d.trim_start_matches('!')))
}

fn normalize_language(raw: &str) -> Option<String> {
    let up = raw.trim().to_ascii_uppercase();
    if up.is_empty() || matches!(up.as_str(), "EN" | "ENGLISH") {
        return None;
    }
    Some(match up.as_str() {
        "GERMAN" | "DEUTSCH" | "DE" => "DE".into(),
        "SPANISH" | "ESPANOL" | "ESPAÑOL" | "ES" => "ES".into(),
        "POLISH" | "POLSKI" | "PL" => "PL".into(),
        "FRENCH" | "FRANCAIS" | "FRANÇAIS" | "FR" => "FR".into(),
        "ITALIAN" | "ITALIANO" | "IT" => "IT".into(),
        "PORTUGUESE" | "PORTUGUES" | "PT" => "PT".into(),
        other => other.to_string(),
    })
}
