//! Which umu database id (`GAMEID`) a game runs with when umu starts it, and
//! where that id came from. umu applies the protonfixes published for the
//! id, so a wrong one is worse than none: an id is taken only from the
//! user's choice, the store's own record in the database, the game's Steam
//! app id, or a title the database lists under one id.
//!
//! Title matches read the database's published CSV, kept in Irori's cache
//! and fetched again at most once a week; a failed fetch keeps using the
//! copy on disk.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use irori_core::{GameId, Paths, Runner};
use once_cell::sync::Lazy;
use parking_lot::Mutex;

/// The umu database as published (title, store, codename, umu id, …).
const DATABASE_URL: &str =
    "https://raw.githubusercontent.com/Open-Wine-Components/umu-database/main/umu-database.csv";

/// How long the downloaded database is used before it is fetched again.
const DATABASE_MAX_AGE: Duration = Duration::from_secs(7 * 24 * 3600);

/// Where a game's umu id came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UmuSource {
    /// The user's choice (`iroriUmuId`).
    Setting,
    /// The database's entry for the game's store and id.
    Store,
    /// The game's Steam app id (`iroriSteamAppId`): umu names Steam games
    /// `umu-<app id>`.
    SteamApp,
    /// The database lists the game's title under one id.
    Title,
}

/// A game's umu id and its source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UmuMatch {
    pub id: String,
    pub source: UmuSource,
}

/// One row of the database.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UmuEntry {
    pub title: String,
    pub store: String,
    pub umu_id: String,
}

/// An id as the user typed it: `umu-1091500`, or a bare Steam app id
/// (`1091500`), which umu spells with the prefix. `None` for nothing typed
/// or text with blanks in it.
pub fn normalize_id(value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty() || value.contains(char::is_whitespace) {
        return None;
    }
    if value.chars().all(|c| c.is_ascii_digit()) {
        return Some(format!("umu-{value}"));
    }
    Some(value.to_string())
}

/// The title as compared: lower case letters and digits only.
fn fold(title: &str) -> String {
    title
        .chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

/// Read the database's CSV (quoted fields may hold commas and doubled
/// quotes). Rows without a title or id are left out, as is the header.
pub fn parse_database(csv: &str) -> Vec<UmuEntry> {
    let mut rows = Vec::new();
    let mut lines = csv.lines();
    let header = lines.next().map(split_csv_line).unwrap_or_default();
    let column = |name: &str| {
        header
            .iter()
            .position(|h| h.trim().eq_ignore_ascii_case(name))
    };
    let (Some(title), Some(store), Some(id)) = (column("TITLE"), column("STORE"), column("UMU_ID"))
    else {
        return rows;
    };
    for line in lines {
        let fields = split_csv_line(line);
        let get = |i: usize| {
            fields
                .get(i)
                .map(|f| f.trim().to_string())
                .unwrap_or_default()
        };
        let entry = UmuEntry {
            title: get(title),
            store: get(store),
            umu_id: get(id),
        };
        if !entry.title.is_empty() && entry.umu_id.starts_with("umu-") {
            rows.push(entry);
        }
    }
    rows
}

fn split_csv_line(line: &str) -> Vec<String> {
    let mut fields = Vec::new();
    let mut field = String::new();
    let mut quoted = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match (quoted, c) {
            (true, '"') if chars.peek() == Some(&'"') => {
                field.push('"');
                chars.next();
            }
            (true, '"') => quoted = false,
            (false, '"') if field.is_empty() => quoted = true,
            (false, ',') => fields.push(std::mem::take(&mut field)),
            _ => field.push(c),
        }
    }
    fields.push(field);
    fields
}

/// The id the database lists `title` under, when every entry of that title
/// agrees on one.
pub fn id_for_title(entries: &[UmuEntry], title: &str) -> Option<String> {
    let wanted = fold(title);
    if wanted.is_empty() {
        return None;
    }
    let mut ids = entries
        .iter()
        .filter(|e| fold(&e.title) == wanted)
        .map(|e| e.umu_id.as_str());
    let first = ids.next()?;
    ids.all(|id| id == first).then(|| first.to_string())
}

/// Database titles containing every word of `query`, one per id, exact
/// matches first, then shorter titles; at most `limit`.
pub fn suggestions(entries: &[UmuEntry], query: &str, limit: usize) -> Vec<UmuEntry> {
    let words: Vec<String> = query
        .split_whitespace()
        .map(fold)
        .filter(|w| !w.is_empty())
        .collect();
    if words.is_empty() {
        return Vec::new();
    }
    let whole = fold(query);
    let mut found: Vec<&UmuEntry> = entries
        .iter()
        .filter(|e| {
            let title = fold(&e.title);
            words.iter().all(|w| title.contains(w.as_str()))
        })
        .collect();
    found.sort_by_key(|e| {
        (
            fold(&e.title) != whole,
            e.title.len(),
            e.title.to_lowercase(),
        )
    });
    let mut seen = std::collections::HashSet::new();
    found
        .into_iter()
        .filter(|e| seen.insert(e.umu_id.clone()))
        .take(limit)
        .cloned()
        .collect()
}

/// Pick a game's id from what is known, in order of trust: the user's
/// choice, the store's record, the Steam app id, the title.
pub fn pick(
    setting: &str,
    store_id: Option<String>,
    steam_app_id: &str,
    title: &str,
    entries: Option<&[UmuEntry]>,
) -> Option<UmuMatch> {
    let found = |id: String, source| Some(UmuMatch { id, source });
    if let Some(id) = normalize_id(setting) {
        return found(id, UmuSource::Setting);
    }
    if let Some(id) = store_id.filter(|i| !i.trim().is_empty()) {
        return found(id, UmuSource::Store);
    }
    let steam = steam_app_id.trim();
    if !steam.is_empty() && steam.chars().all(|c| c.is_ascii_digit()) {
        return found(format!("umu-{steam}"), UmuSource::SteamApp);
    }
    let id = id_for_title(entries?, title)?;
    found(id, UmuSource::Title)
}

fn database_file() -> PathBuf {
    Paths::get().cache.join("umu-database.csv")
}

/// A database read from its file, with the time the file was written.
type Loaded = (SystemTime, Arc<Vec<UmuEntry>>);

/// The database read in this run.
static DATABASE: Lazy<Mutex<Option<Loaded>>> = Lazy::new(|| Mutex::new(None));

/// Whether a database written at `modified` is still fresh at `now`.
fn is_fresh(modified: SystemTime, now: SystemTime) -> bool {
    now.duration_since(modified)
        .map(|age| age < DATABASE_MAX_AGE)
        .unwrap_or(true)
}

/// The database from `file` when it is fresh enough, else `None`.
async fn read_cached(file: &Path, fresh_only: bool) -> Option<Loaded> {
    let modified = tokio::fs::metadata(file).await.ok()?.modified().ok()?;
    if fresh_only && !is_fresh(modified, SystemTime::now()) {
        return None;
    }
    if let Some((at, rows)) = DATABASE.lock().as_ref()
        && *at == modified
    {
        return Some((*at, rows.clone()));
    }
    let text = tokio::fs::read_to_string(file).await.ok()?;
    let rows = Arc::new(parse_database(&text));
    *DATABASE.lock() = Some((modified, rows.clone()));
    Some((modified, rows))
}

/// The umu database: the copy in the cache while it is under a week old,
/// else a fresh download, else the old copy. `None` when there is none.
pub async fn database() -> Option<Arc<Vec<UmuEntry>>> {
    let file = database_file();
    if let Some((_, rows)) = read_cached(&file, true).await {
        return Some(rows);
    }
    match crate::download::get_text(DATABASE_URL).await {
        Ok(text) if !parse_database(&text).is_empty() => {
            if let Err(e) = irori_core::json_store::write_bytes(&file, text.as_bytes()) {
                tracing::debug!("saving the umu database: {e:#}");
            }
        }
        Ok(_) => tracing::debug!("the umu database download held no entries"),
        Err(e) => tracing::debug!("downloading the umu database: {e:#}"),
    }
    read_cached(&file, false).await.map(|(_, rows)| rows)
}

/// The id `id` runs with under umu, and where it came from. Store games
/// ask the database by their store id first; a title match is looked for
/// only when nothing else gives one, so the download happens only for
/// games that need it.
pub async fn resolve(
    id: &GameId,
    title: &str,
    settings: &irori_core::GameSettings,
) -> Option<UmuMatch> {
    if let Some(found) = pick(&settings.umu_id, None, &settings.steam_app_id, title, None) {
        return Some(found);
    }
    let store_id = match id.runner {
        Runner::Sideload | Runner::Emulation | Runner::Steam => None,
        _ => crate::hooks::hooks().umu_id(id).await,
    };
    if store_id.is_some() {
        return pick("", store_id, "", title, None);
    }
    let entries = database().await;
    pick("", None, "", title, entries.as_deref().map(Vec::as_slice))
}

#[cfg(test)]
mod tests {
    use super::*;

    const CSV: &str = "TITLE,STORE,CODENAME,UMU_ID,COMMON ACRONYM (Optional),NOTE (Optional)\n\
Age of Wonders,gog,1207658883,umu-61500,aow,\n\
Age of Wonders,humble,ageofwonders,umu-61500,aow,\n\
\"Warhammer 40,000: Space Marine\",egs,abc,umu-55150,,\n\
\"The \"\"Quoted\"\" Game\",none,none,umu-1,,\n\
Twin,gog,1,umu-2,,\n\
Twin,egs,2,umu-3,,\n\
Broken,gog,1,,,\n";

    #[test]
    fn reads_the_published_database() {
        let rows = parse_database(CSV);
        assert_eq!(rows.len(), 6);
        assert_eq!(rows[2].title, "Warhammer 40,000: Space Marine");
        assert_eq!(rows[2].umu_id, "umu-55150");
        assert_eq!(rows[3].title, "The \"Quoted\" Game");
        assert!(parse_database("nothing,here\n1,2").is_empty());
    }

    #[test]
    fn titles_match_only_when_the_database_agrees() {
        let rows = parse_database(CSV);
        assert_eq!(
            id_for_title(&rows, "age of wonders").as_deref(),
            Some("umu-61500")
        );
        assert_eq!(
            id_for_title(&rows, "Warhammer 40000 Space Marine").as_deref(),
            Some("umu-55150")
        );
        // Two entries of one title with different ids: no guess.
        assert_eq!(id_for_title(&rows, "Twin"), None);
        assert_eq!(id_for_title(&rows, "Age of"), None);
        assert_eq!(id_for_title(&rows, "  "), None);
    }

    #[test]
    fn suggests_titles_by_their_words() {
        let rows = parse_database(CSV);
        let found = suggestions(&rows, "wonders", 5);
        assert_eq!(found.len(), 1, "one per id");
        assert_eq!(found[0].umu_id, "umu-61500");
        let found = suggestions(&rows, "space marine", 5);
        assert_eq!(found[0].title, "Warhammer 40,000: Space Marine");
        assert!(suggestions(&rows, "", 5).is_empty());
        assert_eq!(suggestions(&rows, "twin", 1).len(), 1);
    }

    #[test]
    fn the_users_choice_wins_then_the_store_then_steam_then_the_title() {
        let rows = parse_database(CSV);
        let rows = Some(rows.as_slice());
        let pick = |setting, store: Option<&str>, steam, title| {
            pick(setting, store.map(str::to_string), steam, title, rows)
        };
        assert_eq!(
            pick(" 1091500 ", Some("umu-9"), "7", "Age of Wonders"),
            Some(UmuMatch {
                id: "umu-1091500".into(),
                source: UmuSource::Setting
            })
        );
        assert_eq!(
            pick("", Some("umu-9"), "7", "Age of Wonders")
                .unwrap()
                .source,
            UmuSource::Store
        );
        assert_eq!(
            pick("", None, "7", "Age of Wonders"),
            Some(UmuMatch {
                id: "umu-7".into(),
                source: UmuSource::SteamApp
            })
        );
        assert_eq!(
            pick("", None, "", "Age of Wonders"),
            Some(UmuMatch {
                id: "umu-61500".into(),
                source: UmuSource::Title
            })
        );
        assert_eq!(pick("", None, "x", "Unknown"), None);
        assert_eq!(normalize_id("umu-abc").as_deref(), Some("umu-abc"));
        assert_eq!(normalize_id("two words"), None);
    }

    #[test]
    fn the_database_is_kept_a_week() {
        let now = SystemTime::now();
        assert!(is_fresh(now - Duration::from_secs(3600), now));
        assert!(!is_fresh(now - Duration::from_secs(8 * 24 * 3600), now));
        // A clock set back keeps the copy.
        assert!(is_fresh(now + Duration::from_secs(60), now));
    }

    #[tokio::test]
    async fn a_cached_database_is_read_without_the_network() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("umu-database.csv");
        std::fs::write(&file, CSV).unwrap();
        let (_, rows) = read_cached(&file, true).await.unwrap();
        assert_eq!(rows.len(), 6);
        assert!(
            read_cached(&dir.path().join("none.csv"), false)
                .await
                .is_none()
        );
    }
}
