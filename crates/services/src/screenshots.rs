//! Screenshots of the user's own play, found where they already land:
//! Steam's screenshot folders, one per app, and the desktop's screenshot
//! folders (Spectacle, GNOME, Game Bar, macOS). A Steam folder names its
//! game; any other screenshot belongs to the play session it was taken in.
//! Files are only read: Irori never moves, renames or removes them.
//!
//! Each folder's listing is cached in `<cache>/screenshots.json` with the
//! folder's modified time, so a folder nothing was added to is not listed
//! again. Matching to sessions is redone every time, because a screenshot is
//! found before the session it was taken in has ended.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use chrono::{Local, NaiveDate, NaiveDateTime, TimeZone as _};
use irori_core::settings::AppSettings;
use serde::{Deserialize, Serialize};

use crate::sessions::Session;

/// Image formats the art store can show.
const EXTENSIONS: [&str; 4] = ["png", "jpg", "jpeg", "webp"];
/// A screenshot belongs to a session from this long before it started...
const LEAD_SECS: i64 = 5;
/// ...until this long after it ended (a last look at the game's final frame).
const TRAIL_SECS: i64 = 30;
/// Steam gives its shortcuts to other programs app ids from here up; their
/// folders say nothing about which game was shown.
const STEAM_SHORTCUT_IDS: u64 = 1 << 31;
/// A folder changed this recently may still be written to within the same
/// tick of its modified time, so its listing is not reused.
const SETTLE_SECS: i64 = 2;
const CACHE_VERSION: u32 = 1;

/// One screenshot the user took while playing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Screenshot {
    pub path: PathBuf,
    /// Unix time it was taken.
    pub taken: i64,
    /// `GameId::key()` of the game.
    pub game: String,
}

/// Every screenshot found, and where Irori looked.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScreenshotIndex {
    /// Newest first.
    pub shots: Vec<Screenshot>,
    /// The desktop's screenshot folders looked in (Steam's are not listed),
    /// whether they exist or not, so the user can be told where to save.
    pub folders: Vec<PathBuf>,
}

impl ScreenshotIndex {
    /// The screenshots of one game, newest first.
    pub fn for_game<'a>(&'a self, key: &'a str) -> impl Iterator<Item = &'a Screenshot> {
        self.shots.iter().filter(move |s| s.game == key)
    }
}

/// A folder to look in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Folder {
    pub path: PathBuf,
    /// The game every screenshot in it shows (a Steam app's folder); `None`
    /// when screenshots are matched to sessions by time.
    pub game: Option<String>,
}

/// Every folder screenshots land in on this system: Steam's (for every
/// account on it) and the desktop's.
pub fn folders(app: &AppSettings) -> Vec<Folder> {
    let steam = irori_runtime::locations::steam_path(app);
    let mut out = steam_folders(&steam);
    out.extend(
        desktop_folders()
            .into_iter()
            .map(|path| Folder { path, game: None }),
    );
    out
}

/// `<steam>/userdata/<account>/760/remote/<appid>/screenshots`, for every
/// account and app.
fn steam_folders(steam: &Path) -> Vec<Folder> {
    let mut out = Vec::new();
    let Ok(accounts) = std::fs::read_dir(steam.join("userdata")) else {
        return out;
    };
    for account in accounts.flatten() {
        let Ok(apps) = std::fs::read_dir(account.path().join("760").join("remote")) else {
            continue;
        };
        for app in apps.flatten() {
            let Some(id) = app.file_name().to_str().and_then(|n| n.parse::<u64>().ok()) else {
                continue;
            };
            let path = app.path().join("screenshots");
            if path.is_dir() {
                let game = (id < STEAM_SHORTCUT_IDS).then(|| format!("steam:{id}"));
                out.push(Folder { path, game });
            }
        }
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    out
}

/// Where the desktop saves screenshots, without duplicates.
fn desktop_folders() -> Vec<PathBuf> {
    let home = dirs::home_dir().unwrap_or_else(std::env::temp_dir);
    let pictures = dirs::picture_dir().unwrap_or_else(|| home.join("Pictures"));
    let mut out = Vec::new();
    if cfg!(target_os = "linux") {
        // Spectacle's chosen folder, then GNOME's and Spectacle's default,
        // then Pictures itself, where GNOME saved before version 42.
        let spectacle = dirs::config_dir()
            .and_then(|c| std::fs::read_to_string(c.join("spectaclerc")).ok())
            .and_then(|text| spectacle_folder(&text));
        out.extend(spectacle);
        out.push(pictures.join("Screenshots"));
        out.push(pictures);
    } else if cfg!(windows) {
        // Game Bar saves to Videos\Captures, Win+PrtScn to Pictures\Screenshots.
        let videos = dirs::video_dir().unwrap_or_else(|| home.join("Videos"));
        out.push(videos.join("Captures"));
        out.push(pictures.join("Screenshots"));
    } else {
        out.push(
            mac_capture_folder(&home)
                .unwrap_or_else(|| dirs::desktop_dir().unwrap_or_else(|| home.join("Desktop"))),
        );
        out.push(pictures.join("Screenshots"));
    }
    let mut seen = Vec::new();
    out.retain(|p| {
        let key = std::fs::canonicalize(p).unwrap_or_else(|_| p.clone());
        let new = !seen.contains(&key);
        seen.push(key);
        new
    });
    out
}

/// The folder Spectacle saves to, from its `spectaclerc`: `imageSaveLocation`
/// in `[ImageSave]` (Plasma 6), else `defaultSaveLocation` in `[General]`.
fn spectacle_folder(text: &str) -> Option<PathBuf> {
    let mut section = "";
    let mut found = None;
    for line in text.lines().map(str::trim) {
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            section = name;
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let wanted = matches!(
            (section, key.trim()),
            ("ImageSave", "imageSaveLocation") | ("General", "defaultSaveLocation")
        );
        if wanted && !value.trim().is_empty() {
            let value = value.trim();
            let path = value.strip_prefix("file://").unwrap_or(value);
            let path = irori_core::paths::expand_home(path);
            // The Plasma 6 key wins over the old one wherever it appears.
            if section == "ImageSave" || found.is_none() {
                found = Some(path);
            }
        }
    }
    found
}

/// The folder macOS saves screenshots to when the user moved it
/// (`com.apple.screencapture` `location`).
fn mac_capture_folder(home: &Path) -> Option<PathBuf> {
    let plist = home.join("Library/Preferences/com.apple.screencapture.plist");
    let value = plist::Value::from_file(plist).ok()?;
    let location = value.as_dictionary()?.get("location")?.as_string()?;
    Some(irori_core::paths::expand_home(location))
}

/// The cached listing of every folder.
#[derive(Debug, Default, Serialize, Deserialize)]
struct Cache {
    version: u32,
    #[serde(default)]
    folders: BTreeMap<String, Listing>,
}

/// One folder's screenshots, as of its modified time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Listing {
    /// The folder's modified time: seconds and nanoseconds since 1970.
    modified: (i64, u32),
    /// File names with the time each was taken.
    files: Vec<(String, i64)>,
}

/// Build the index from `folders`, matching to `sessions`, with the
/// listings cached in `cache_file`. Reads the disk; call off the UI thread.
pub fn build(folders: &[Folder], sessions: &[Session], cache_file: &Path) -> ScreenshotIndex {
    let mut cache: Cache = irori_core::json_store::read_or_default(cache_file);
    if cache.version != CACHE_VERSION {
        cache = Cache {
            version: CACHE_VERSION,
            folders: BTreeMap::new(),
        };
    }
    let now = unix(SystemTime::now());
    let played = Played::new(sessions);
    let mut fresh = BTreeMap::new();
    let mut shots = Vec::new();
    for folder in folders {
        let key = folder.path.to_string_lossy().into_owned();
        let Some(listing) = list(&folder.path, cache.folders.get(&key), now) else {
            continue;
        };
        for (name, taken) in &listing.files {
            let game = match &folder.game {
                Some(game) => Some(game.as_str()),
                None => played.game_at(*taken),
            };
            if let Some(game) = game {
                shots.push(Screenshot {
                    path: folder.path.join(name),
                    taken: *taken,
                    game: game.to_owned(),
                });
            }
        }
        fresh.insert(key, listing);
    }
    if fresh != cache.folders {
        let cache = Cache {
            version: CACHE_VERSION,
            folders: fresh,
        };
        if let Err(e) = irori_core::json_store::write(cache_file, &cache) {
            tracing::debug!("could not cache the screenshot index: {e:#}");
        }
    }
    shots.sort_by(|a, b| b.taken.cmp(&a.taken).then_with(|| a.path.cmp(&b.path)));
    ScreenshotIndex {
        shots,
        folders: folders
            .iter()
            .filter(|f| f.game.is_none() && !is_steam_folder(&f.path))
            .map(|f| f.path.clone())
            .collect(),
    }
}

/// A Steam shortcut's folder, which is matched by time like the desktop's
/// but is no place to tell the user about.
fn is_steam_folder(path: &Path) -> bool {
    path.ends_with("screenshots") && path.ancestors().nth(3).is_some_and(|p| p.ends_with("760"))
}

/// The screenshots in `dir`, reusing `cached` when the folder is unchanged.
/// `None` when the folder cannot be read (it does not exist).
fn list(dir: &Path, cached: Option<&Listing>, now: i64) -> Option<Listing> {
    let modified = modified_of(&std::fs::metadata(dir).ok()?)?;
    if let Some(cached) = cached.filter(|c| c.modified == modified) {
        return Some(cached.clone());
    }
    let mut files = Vec::new();
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        let path = Path::new(name);
        let is_image = path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| EXTENSIONS.iter().any(|x| e.eq_ignore_ascii_case(x)));
        if !is_image || name.starts_with('.') {
            continue;
        }
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        if !meta.is_file() {
            continue;
        }
        let from_name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .and_then(time_in_name)
            .and_then(|t| Local.from_local_datetime(&t).earliest())
            .map(|t| t.timestamp());
        let taken = from_name.or_else(|| meta.modified().ok().map(unix));
        if let Some(taken) = taken {
            files.push((name.to_owned(), taken));
        }
    }
    files.sort();
    // Another file could still arrive within the same tick of the folder's
    // time; list such a folder again next time.
    let modified = if now - modified.0 < SETTLE_SECS {
        (i64::MIN, 0)
    } else {
        modified
    };
    Some(Listing { modified, files })
}

fn modified_of(meta: &std::fs::Metadata) -> Option<(i64, u32)> {
    let d = meta.modified().ok()?.duration_since(UNIX_EPOCH).ok()?;
    Some((d.as_secs() as i64, d.subsec_nanos()))
}

fn unix(t: SystemTime) -> i64 {
    t.duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

/// The local date and time in a screenshot's file name, when it holds one
/// in full: `Screenshot_20261103_191505`, `Screenshot From 2026-11-03
/// 19-15-05`, Steam's `20261103191505_1`, Game Bar's `Hades 2026-11-03
/// 19-15-05`, macOS's `Screenshot 2026-11-03 at 19.15.05`. Digits from the
/// title before it are skipped by trying each run of digits as the start.
fn time_in_name(name: &str) -> Option<NaiveDateTime> {
    let runs: Vec<&str> = name
        .split(|c: char| !c.is_ascii_digit())
        .filter(|r| !r.is_empty())
        .collect();
    (0..runs.len()).find_map(|start| {
        let mut digits = String::new();
        for run in &runs[start..] {
            digits.push_str(run);
            if digits.len() >= 14 {
                break;
            }
        }
        if digits.len() != 14 {
            return None;
        }
        let n = |range: std::ops::Range<usize>| digits[range].parse::<u32>().ok();
        let year = i32::try_from(n(0..4)?).ok()?;
        if !(2000..2100).contains(&year) {
            return None;
        }
        NaiveDate::from_ymd_opt(year, n(4..6)?, n(6..8)?)?.and_hms_opt(
            n(8..10)?,
            n(10..12)?,
            n(12..14)?,
        )
    })
}

/// The sessions as time windows, for finding the game a screenshot shows.
struct Played<'a> {
    /// Start, end and game key, by start.
    spans: Vec<(i64, i64, &'a str)>,
    longest: i64,
}

impl<'a> Played<'a> {
    fn new(sessions: &'a [Session]) -> Self {
        let mut spans: Vec<(i64, i64, &str)> = sessions
            .iter()
            .filter_map(|s| {
                let start = irori_core::util::parse_rfc3339(&s.started)?;
                let seconds = i64::try_from(s.seconds).ok()?;
                Some((start, start.saturating_add(seconds), s.game.as_str()))
            })
            .collect();
        spans.sort_by_key(|(start, _, _)| *start);
        let longest = spans.iter().map(|(s, e, _)| e - s).max().unwrap_or(0);
        Self { spans, longest }
    }

    /// The game of the session `taken` falls in; of two at once, the one
    /// started last (it is the one on screen).
    fn game_at(&self, taken: i64) -> Option<&'a str> {
        let after = self
            .spans
            .partition_point(|(start, _, _)| start - LEAD_SECS <= taken);
        self.spans[..after]
            .iter()
            .rev()
            .take_while(|(start, _, _)| start + self.longest + TRAIL_SECS >= taken)
            .find(|(_, end, _)| taken <= end + TRAIL_SECS)
            .map(|(_, _, game)| *game)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(game: &str, started: &str, seconds: u64) -> Session {
        Session {
            game: game.into(),
            started: started.into(),
            seconds,
            crashed: false,
            exit_code: None,
        }
    }

    fn local(text: &str) -> i64 {
        let t = NaiveDateTime::parse_from_str(text, "%Y-%m-%d %H:%M:%S").unwrap();
        Local
            .from_local_datetime(&t)
            .earliest()
            .unwrap()
            .timestamp()
    }

    fn rfc3339(unix: i64) -> String {
        chrono::DateTime::from_timestamp(unix, 0)
            .unwrap()
            .to_rfc3339()
    }

    fn touch(path: &Path, modified: Option<i64>) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let file = std::fs::File::create(path).unwrap();
        if let Some(t) = modified {
            let t = UNIX_EPOCH + std::time::Duration::from_secs(t as u64);
            file.set_modified(t).unwrap();
        }
    }

    #[test]
    fn reads_the_time_in_common_screenshot_names() {
        let at = |s: &str| time_in_name(s).map(|t| t.format("%F %T").to_string());
        let want = Some("2026-11-03 19:15:05".to_string());
        assert_eq!(at("Screenshot_20261103_191505"), want);
        assert_eq!(at("Screenshot From 2026-11-03 19-15-05"), want);
        assert_eq!(at("20261103191505_1"), want);
        assert_eq!(at("Cyberpunk 2077 2026-11-03 19-15-05"), want);
        assert_eq!(at("Screenshot 2026-11-03 at 19.15.05"), want);
        assert_eq!(at("gamescope_2026-11-03_19-15-05"), want);
        // A 12-hour clock or no time at all leaves the file's own time.
        assert_eq!(at("Screenshot 2026-11-03 at 7.15.05 PM"), None);
        assert_eq!(at("wallpaper"), None);
        assert_eq!(at("Screenshot_20261340_191505"), None);
    }

    #[test]
    fn reads_spectacles_folder() {
        let plasma6 = "[General]\ndefaultSaveLocation=file:///old\n\n[ImageSave]\nimageSaveLocation=file:///home/u/Shots\ntranslatedScreenshotsFolder=Screenshots\n";
        assert_eq!(
            spectacle_folder(plasma6),
            Some(PathBuf::from("/home/u/Shots"))
        );
        let plasma5 = "[General]\ndefaultSaveLocation=file:///home/u/Old\n";
        assert_eq!(
            spectacle_folder(plasma5),
            Some(PathBuf::from("/home/u/Old"))
        );
        // The last file chosen with "Save as" is not the folder.
        let last = "[ImageSave]\nlastImageSaveLocation=file:///tmp/x.png\n";
        assert_eq!(spectacle_folder(last), None);
    }

    #[test]
    fn finds_steam_app_folders_and_leaves_shortcuts_to_time() {
        let dir = tempfile::tempdir().unwrap();
        let remote = dir.path().join("userdata/1234/760/remote");
        std::fs::create_dir_all(remote.join("1245620/screenshots")).unwrap();
        std::fs::create_dir_all(remote.join("3228583970/screenshots")).unwrap();
        std::fs::create_dir_all(remote.join("7/no-screenshots-here")).unwrap();
        let found = steam_folders(dir.path());
        assert_eq!(
            found,
            vec![
                Folder {
                    path: remote.join("1245620/screenshots"),
                    game: Some("steam:1245620".into()),
                },
                Folder {
                    path: remote.join("3228583970/screenshots"),
                    game: None,
                },
            ]
        );
        assert!(is_steam_folder(&found[1].path));
        assert!(!is_steam_folder(&dir.path().join("Pictures/Screenshots")));
    }

    #[test]
    fn matches_screenshots_to_the_session_they_were_taken_in() {
        let dir = tempfile::tempdir().unwrap();
        let shots = dir.path().join("Screenshots");
        let steam = dir.path().join("steam/screenshots");
        let start = local("2026-11-03 19:00:00");
        let sessions = vec![
            session("gog:hades", &rfc3339(start), 3600),
            // Started later and still overlapping: it is the one on screen.
            session("legendary:Fowl", &rfc3339(start + 1800), 600),
        ];
        touch(&shots.join("Screenshot_20261103_191505.png"), None);
        touch(&shots.join("Screenshot_20261103_193500.png"), None);
        // Thirty seconds after the end still counts; an hour later does not.
        touch(&shots.join("Screenshot_20261103_200020.jpg"), None);
        touch(&shots.join("Screenshot_20261103_210000.png"), None);
        // No time in the name: the file's own.
        touch(&shots.join("capture.webp"), Some(start + 60));
        // Not a picture, hidden, or a folder.
        touch(&shots.join("notes_20261103_191505.txt"), None);
        touch(&shots.join(".Screenshot_20261103_191505.png"), None);
        std::fs::create_dir_all(shots.join("Screenshot_20261103_191506.png")).unwrap();
        // Steam's folder names its game whenever the screenshot was taken.
        touch(&steam.join("20250101120000_1.jpg"), None);

        let folders = vec![
            Folder {
                path: shots.clone(),
                game: None,
            },
            Folder {
                path: steam.clone(),
                game: Some("steam:1245620".into()),
            },
            Folder {
                path: dir.path().join("missing"),
                game: None,
            },
        ];
        let cache = dir.path().join("cache/screenshots.json");
        let index = build(&folders, &sessions, &cache);
        let got: Vec<(String, &str)> = index
            .shots
            .iter()
            .map(|s| {
                (
                    s.path.file_name().unwrap().to_string_lossy().into_owned(),
                    s.game.as_str(),
                )
            })
            .collect();
        assert_eq!(
            got,
            vec![
                ("Screenshot_20261103_200020.jpg".into(), "gog:hades"),
                ("Screenshot_20261103_193500.png".into(), "legendary:Fowl"),
                ("Screenshot_20261103_191505.png".into(), "gog:hades"),
                ("capture.webp".into(), "gog:hades"),
                ("20250101120000_1.jpg".into(), "steam:1245620"),
            ]
        );
        assert_eq!(index.for_game("gog:hades").count(), 3);
        assert_eq!(
            index.folders,
            vec![shots.clone(), dir.path().join("missing")]
        );
        assert!(cache.is_file());
    }

    // Windows sets a folder's time only through a handle opened for backup.
    #[cfg(unix)]
    #[test]
    fn reuses_the_listing_of_an_unchanged_folder() {
        let dir = tempfile::tempdir().unwrap();
        let shots = dir.path().join("Screenshots");
        let start = local("2026-11-03 19:00:00");
        let sessions = vec![session("gog:hades", &rfc3339(start), 3600)];
        touch(&shots.join("Screenshot_20261103_191505.png"), None);
        let old = UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
        let set_dir_time = || {
            std::fs::File::open(&shots)
                .unwrap()
                .set_modified(old)
                .unwrap()
        };
        set_dir_time();
        let folders = vec![Folder {
            path: shots.clone(),
            game: None,
        }];
        let cache = dir.path().join("screenshots.json");
        assert_eq!(build(&folders, &sessions, &cache).shots.len(), 1);

        // A file that appears without the folder's time changing is not seen:
        // the cached listing is used.
        touch(&shots.join("Screenshot_20261103_192000.png"), None);
        set_dir_time();
        assert_eq!(build(&folders, &sessions, &cache).shots.len(), 1);

        // Once the folder changes it is listed again.
        std::fs::File::open(&shots)
            .unwrap()
            .set_modified(old + std::time::Duration::from_secs(60))
            .unwrap();
        assert_eq!(build(&folders, &sessions, &cache).shots.len(), 2);
    }

    #[test]
    fn a_screenshot_found_before_its_session_ends_is_matched_later() {
        let dir = tempfile::tempdir().unwrap();
        let shots = dir.path().join("Screenshots");
        touch(&shots.join("Screenshot_20261103_191505.png"), None);
        let folders = vec![Folder {
            path: shots,
            game: None,
        }];
        let cache = dir.path().join("screenshots.json");
        assert!(build(&folders, &[], &cache).shots.is_empty());
        let start = local("2026-11-03 19:00:00");
        let sessions = vec![session("gog:hades", &rfc3339(start), 3600)];
        assert_eq!(build(&folders, &sessions, &cache).shots.len(), 1);
    }

    #[test]
    fn a_long_session_is_found_behind_later_short_ones() {
        let start = 1_000_000;
        let sessions = vec![
            session("gog:long", &rfc3339(start), 10_000),
            session("gog:short", &rfc3339(start + 100), 10),
            session("gog:short", &rfc3339(start + 200), 10),
        ];
        let w = Played::new(&sessions);
        assert_eq!(w.game_at(start + 105), Some("gog:short"));
        assert_eq!(w.game_at(start + 5000), Some("gog:long"));
        assert_eq!(w.game_at(start - 4), Some("gog:long"));
        assert_eq!(w.game_at(start - 60), None);
        assert_eq!(w.game_at(start + 10_031), None);
    }
}
