//! Local eXo game collection support (`eXoDOS`, `eXoWin3x`, `eXoScummVM`,
//! `eXoDemoscene`).
//!
//! Irori only reads user-owned collections already on disk: it never bundles
//! torrents, streams game archives from external swarms, or downloads ROMs.

pub mod conf;
pub mod extract;
pub mod media;
pub mod scummvm;
pub mod xml;

#[cfg(test)]
mod launch_tests;
#[cfg(test)]
mod tests;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::scan::{RomEntry, app_name_of};
use media::MediaIndex;
use xml::{extract_language, extract_shortcode_and_lang, parse_platform_xml};

/// Tier 1 eXo game collections supported natively via Irori's existing engines.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ExoCollectionKind {
    ExoDos,
    ExoWin3x,
    ExoScummVm,
    ExoDemoscene,
}

impl ExoCollectionKind {
    /// Human-readable collection identifier shown as a tag and metadata source.
    pub fn id(self) -> &'static str {
        match self {
            Self::ExoDos => "eXoDOS",
            Self::ExoWin3x => "eXoWin3x",
            Self::ExoScummVm => "eXoScummVM",
            Self::ExoDemoscene => "eXoDemoscene",
        }
    }

    /// Matching Irori `systems.json` id.
    pub fn system_id(self) -> &'static str {
        match self {
            Self::ExoDos | Self::ExoDemoscene => "dos",
            Self::ExoWin3x => "win3x",
            Self::ExoScummVm => "scummvm",
        }
    }

    /// Relative folder under `<exo_root>` holding game folders and `.zip` archives.
    pub fn game_subpath(self) -> &'static str {
        match self {
            Self::ExoDos => "eXo/eXoDOS",
            Self::ExoWin3x => "eXo/eXoWin3x",
            Self::ExoScummVm => "eXo/eXoScummVM",
            Self::ExoDemoscene => "eXo/eXoDemoscene",
        }
    }

    /// Config folder segment inside `game_subpath` (`!dos`, `!win3x`, `!ScummVM`, `!demos`).
    pub fn config_segment(self) -> &'static str {
        match self {
            Self::ExoDos => "!dos",
            Self::ExoWin3x => "!win3x",
            Self::ExoScummVm => "!ScummVM",
            Self::ExoDemoscene => "!demos",
        }
    }

    /// Map a LaunchBox `<Platform>` name (or XML file stem) to a supported Tier 1 collection.
    pub fn from_platform(platform: &str) -> Option<Self> {
        match platform.trim().to_ascii_lowercase().as_str() {
            "ms-dos" | "msdos" | "dos" | "exodos" => Some(Self::ExoDos),
            "windows 3x" | "windows 3.x" | "win3x" | "exowin3x" => Some(Self::ExoWin3x),
            "scummvm" | "scummvm svn" | "exoscummvm" => Some(Self::ExoScummVm),
            "exodemoscene" | "demoscene" | "ms-dos demoscene" => Some(Self::ExoDemoscene),
            _ => None,
        }
    }
}

/// eXo-specific metadata persisted alongside a [`RomEntry`] in `library.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExoEntryMeta {
    pub collection: ExoCollectionKind,
    /// Exact LaunchBox `<Platform>` name (used for `Images/<Platform>/`).
    pub platform: String,
    pub shortcode: String,
    pub exo_root: PathBuf,
    /// Where extracted game files live (or will be extracted from `archive_path`).
    pub game_dir: PathBuf,
    /// Per-game eXo config folder (`eXo/<Collection>/!<seg>/[<lang>/]<shortcode>`).
    pub config_dir: PathBuf,
    /// Path to `dosbox.conf` inside `config_dir`, when present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conf_path: Option<PathBuf>,
    /// Local compressed game archive on disk (e.g. eXoDOS Full `.zip`), when present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archive_path: Option<PathBuf>,
    /// Native eXo launch script (`.command`/`.bsh` on Unix, `.bat` on Windows), when present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub script_path: Option<PathBuf>,
    /// True when extracted game files are present on disk; false when only `archive_path` exists.
    pub installed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub developer: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub publisher: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release_date: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub genres: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manual_path: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_cover: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_logo: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_background: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub local_screenshots: Vec<PathBuf>,
}

/// Whether `dir` looks like the root folder of a local eXo collection.
pub fn is_exo_root(dir: &Path) -> bool {
    let has_exo_dir = dir.join("eXo").is_dir();
    if !has_exo_dir {
        return false;
    }
    dir.join("Data").join("Platforms").is_dir() || dir.join("xml").join("all").is_dir()
}

/// Lightweight watch directories for an eXo collection root so Linux `notify`
/// avoids burning thousands of inotify watches on `Images/` and `!dos/`.
pub fn watch_dirs(root: &Path) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    for rel in [
        "Data/Platforms",
        "xml/all",
        "eXo/eXoDOS",
        "eXo/eXoWin3x",
        "eXo/eXoScummVM",
        "eXo/eXoDemoscene",
    ] {
        let p = root.join(rel);
        if p.is_dir() {
            dirs.push(p);
        }
    }
    if dirs.is_empty() && root.is_dir() {
        dirs.push(root.to_path_buf());
    }
    dirs
}

/// Platform XML catalogs under `root` (`Data/Platforms/*.xml`, else `xml/all/*.xml`).
pub fn platform_xml_files(root: &Path) -> Vec<PathBuf> {
    for rel in ["Data/Platforms", "xml/all"] {
        let dir = root.join(rel);
        let Ok(read) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut xmls: Vec<PathBuf> = read
            .flatten()
            .filter_map(|e| {
                let p = e.path();
                let is_xml = e.file_type().is_ok_and(|t| t.is_file())
                    && p.extension()
                        .and_then(|x| x.to_str())
                        .is_some_and(|x| x.eq_ignore_ascii_case("xml"));
                is_xml.then_some(p)
            })
            .collect();
        xmls.sort();
        if !xmls.is_empty() {
            return xmls;
        }
    }
    Vec::new()
}

/// Scan a local eXo collection rooted at `exo_root` into [`RomEntry`] records.
///
/// Only titles with local files on disk (either an extracted game directory or
/// a local `.zip` archive ready for extraction) are included; un-downloaded
/// Lite catalog stubs are skipped cleanly.
pub fn scan_root(exo_root: &Path, rom_root: &Path) -> Vec<RomEntry> {
    let xml_files = platform_xml_files(exo_root);
    if xml_files.is_empty() {
        return Vec::new();
    }

    let mut media_cache: HashMap<String, MediaIndex> = HashMap::new();
    let mut zip_cache: HashMap<PathBuf, HashMap<String, (PathBuf, u64)>> = HashMap::new();
    let mut skipped_platforms: HashSet<String> = HashSet::new();
    let mut seen_apps: HashSet<String> = HashSet::new();
    let mut entries = Vec::new();

    for xml_path in xml_files {
        let fallback_platform = xml_path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let games = match parse_platform_xml(&xml_path) {
            Ok(g) => g,
            Err(e) => {
                tracing::warn!("reading eXo catalog {}: {e:#}", xml_path.display());
                continue;
            }
        };

        for game in games {
            let platform = game
                .platform
                .clone()
                .unwrap_or_else(|| fallback_platform.clone());
            let Some(kind) = ExoCollectionKind::from_platform(&platform) else {
                if skipped_platforms.insert(platform.clone()) {
                    tracing::info!(
                        "eXo collection: skipping unsupported platform '{platform}' in {}",
                        exo_root.display()
                    );
                }
                continue;
            };
            let Some(app_path) = game.application_path.as_deref() else {
                continue;
            };
            let Some((shortcode, lang_dir)) =
                extract_shortcode_and_lang(app_path, kind.config_segment())
            else {
                continue;
            };

            let base_dir = exo_root.join(kind.game_subpath());
            let config_dir = match &lang_dir {
                Some(lang) => base_dir
                    .join(kind.config_segment())
                    .join(lang)
                    .join(&shortcode),
                None => base_dir.join(kind.config_segment()).join(&shortcode),
            };
            let game_dir = match &lang_dir {
                Some(lang) => base_dir.join(lang).join(&shortcode),
                None => base_dir.join(&shortcode),
            };
            let archive_dir = match &lang_dir {
                Some(lang) => base_dir.join(lang),
                None => base_dir.clone(),
            };

            let installed = dir_has_files(&game_dir);
            let zip_map = zip_cache
                .entry(archive_dir.clone())
                .or_insert_with(|| index_zips(&archive_dir));
            let archive = find_game_zip(
                zip_map,
                app_path,
                &game.title,
                game.release_date.as_deref(),
                &shortcode,
            );

            // Skip un-downloaded eXo Lite stubs (neither extracted nor packed locally).
            if !installed && archive.is_none() {
                continue;
            }

            let conf_path = find_file_ci(&config_dir, "dosbox.conf");
            let script_path = resolve_native_script(exo_root, app_path, &config_dir);
            let (archive_path, archive_size) = match archive {
                Some((p, size)) => (Some(p), size),
                None => (None, 0),
            };

            let app_name = app_name_of(&format!("exo:{}", kind.id()), &config_dir);
            if !seen_apps.insert(app_name.clone()) {
                continue;
            }

            let media_idx = media_cache
                .entry(platform.clone())
                .or_insert_with(|| MediaIndex::load(exo_root, &platform));
            let local_media = media_idx.lookup(&game.title);
            let manual_path = media::resolve_manual(exo_root, game.manual_path.as_deref());
            let language = extract_language(game.series.as_deref(), lang_dir.as_deref());

            let mut tags = vec![kind.id().to_string()];
            if let Some(lang) = &language {
                tags.push(lang.clone());
            }
            if let Some(region) = &game.region
                && !tags.iter().any(|t| t.eq_ignore_ascii_case(region))
            {
                tags.push(region.clone());
            }

            let entry_path = if kind == ExoCollectionKind::ExoScummVm {
                if installed {
                    game_dir.clone()
                } else {
                    archive_path.clone().unwrap_or_else(|| game_dir.clone())
                }
            } else {
                conf_path
                    .clone()
                    .or_else(|| installed.then(|| game_dir.clone()))
                    .or_else(|| archive_path.clone())
                    .unwrap_or_else(|| game_dir.clone())
            };

            let stem = game.title.clone();
            entries.push(RomEntry {
                app_name,
                system: kind.system_id().to_string(),
                title: game.title,
                tags,
                path: entry_path,
                discs: Vec::new(),
                size: if installed { 0 } else { archive_size },
                stem,
                root: rom_root.to_path_buf(),
                exo: Some(ExoEntryMeta {
                    collection: kind,
                    platform,
                    shortcode,
                    exo_root: exo_root.to_path_buf(),
                    game_dir,
                    config_dir,
                    conf_path,
                    archive_path,
                    script_path,
                    installed,
                    language,
                    developer: game.developer,
                    publisher: game.publisher,
                    release_date: game.release_date,
                    genres: game.genres,
                    notes: game.notes,
                    manual_path,
                    local_cover: local_media.cover,
                    local_logo: local_media.logo,
                    local_background: local_media.background,
                    local_screenshots: local_media.screenshots,
                }),
            });
        }
    }

    entries
}

/// Whether `dir` exists and holds at least one non-hidden entry.
pub(crate) fn dir_has_files(dir: &Path) -> bool {
    let Ok(read) = std::fs::read_dir(dir) else {
        return false;
    };
    read.flatten()
        .any(|e| !e.file_name().to_string_lossy().starts_with('.'))
}

/// Index `*.zip` files directly inside `dir` by lowercased stem.
fn index_zips(dir: &Path) -> HashMap<String, (PathBuf, u64)> {
    let mut out = HashMap::new();
    let Ok(read) = std::fs::read_dir(dir) else {
        return out;
    };
    for entry in read.flatten() {
        let path = entry.path();
        let is_zip = entry.file_type().is_ok_and(|t| t.is_file())
            && path
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| e.eq_ignore_ascii_case("zip"));
        if !is_zip {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        let size = entry.metadata().map_or(0, |m| m.len());
        out.insert(stem.to_ascii_lowercase(), (path, size));
    }
    out
}

/// Match a game's local `.zip` archive in `zip_map` by script stem, `Title (Year)`,
/// `Title`, or `shortcode`.
fn find_game_zip(
    zip_map: &HashMap<String, (PathBuf, u64)>,
    app_path: &str,
    title: &str,
    release_date: Option<&str>,
    shortcode: &str,
) -> Option<(PathBuf, u64)> {
    if zip_map.is_empty() {
        return None;
    }
    // LaunchBox ApplicationPath ends with `<Game Name (Year)>.bat`, whose stem
    // matches eXo's archive name (`<Game Name (Year)>.zip`).
    let file_name = app_path.rsplit(['\\', '/']).next().unwrap_or("");
    let script_stem = file_name
        .rsplit_once('.')
        .map(|(s, _)| s)
        .unwrap_or(file_name);
    if !script_stem.is_empty()
        && let Some(found) = zip_map.get(&script_stem.to_ascii_lowercase())
    {
        return Some(found.clone());
    }
    if let Some(year) = release_date.and_then(|d| d.get(..4)) {
        let titled_year = format!("{title} ({year})").to_ascii_lowercase();
        if let Some(found) = zip_map.get(&titled_year) {
            return Some(found.clone());
        }
    }
    zip_map
        .get(&title.to_ascii_lowercase())
        .or_else(|| zip_map.get(&shortcode.to_ascii_lowercase()))
        .cloned()
}

/// Find a file directly inside `dir` matching `want` ASCII case-insensitively.
fn find_file_ci(dir: &Path, want: &str) -> Option<PathBuf> {
    let exact = dir.join(want);
    if exact.is_file() {
        return Some(exact);
    }
    let read = std::fs::read_dir(dir).ok()?;
    for entry in read.flatten() {
        if entry.file_type().is_ok_and(|t| t.is_file())
            && entry
                .file_name()
                .to_string_lossy()
                .eq_ignore_ascii_case(want)
        {
            return Some(entry.path());
        }
    }
    None
}

/// Locate the native eXo launch script corresponding to `ApplicationPath`
/// (`.command` then `.bsh` on Unix; `.bat` on Windows), when present on disk.
fn resolve_native_script(exo_root: &Path, app_path: &str, config_dir: &Path) -> Option<PathBuf> {
    let file_name = app_path.rsplit(['\\', '/']).next()?;
    let stem = file_name.rsplit_once('.').map_or(file_name, |(s, _)| s);
    if stem.is_empty() {
        return None;
    }
    let candidates: &[&str] = if cfg!(windows) {
        &["bat", "cmd"]
    } else {
        &["command", "bsh", "sh"]
    };
    for ext in candidates {
        let name = format!("{stem}.{ext}");
        if let Some(found) = find_file_ci(config_dir, &name) {
            return Some(found);
        }
    }
    // Fall back to the normalized relative ApplicationPath directly on Windows.
    if cfg!(windows) {
        let normalized = app_path.replace('\\', "/");
        if let Some(rel) = irori_core::archive_guard::relative_entry_path(Path::new(&normalized)) {
            let direct = exo_root.join(rel);
            if direct.is_file() {
                return Some(direct);
            }
        }
    }
    None
}
