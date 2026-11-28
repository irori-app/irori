//! Local artwork and manual resolution for eXo game collections.
//!
//! Reads the user's local `Images/<Platform>/` directories once per scan so
//! matching thousands of titles costs O(1) hash lookups instead of per-game
//! filesystem probing. Nothing here touches the network.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use irori_core::archive_guard;

/// Maximum local screenshots attached to a single game's `ExtraInfo`.
const MAX_SCREENSHOTS: usize = 8;

/// Local media paths discovered inside the user's eXo collection folder.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LocalMedia {
    pub cover: Option<PathBuf>,
    pub logo: Option<PathBuf>,
    pub background: Option<PathBuf>,
    pub screenshots: Vec<PathBuf>,
}

/// Pre-indexed local images for one eXo LaunchBox platform folder.
#[derive(Debug, Default)]
pub struct MediaIndex {
    box_front: HashMap<String, Vec<PathBuf>>,
    box_3d: HashMap<String, Vec<PathBuf>>,
    clear_logo: HashMap<String, Vec<PathBuf>>,
    fanart: HashMap<String, Vec<PathBuf>>,
    gameplay: HashMap<String, Vec<PathBuf>>,
    title_screen: HashMap<String, Vec<PathBuf>>,
}

impl MediaIndex {
    /// Index `<exo_root>/Images/<platform>/` once in memory.
    pub fn load(exo_root: &Path, platform: &str) -> Self {
        let base = exo_root.join("Images").join(platform);
        if !base.is_dir() {
            return Self::default();
        }
        Self {
            box_front: index_folder(&base.join("Box - Front")),
            box_3d: index_folder(&base.join("Box - 3D")),
            clear_logo: index_folder(&base.join("Clear Logo")),
            fanart: index_folder(&base.join("Fanart - Background")),
            gameplay: index_folder(&base.join("Screenshot - Gameplay")),
            title_screen: index_folder(&base.join("Screenshot - Game Title")),
        }
    }

    /// Look up local artwork for a game titled `title`.
    pub fn lookup(&self, title: &str) -> LocalMedia {
        let key = media_key(title);
        if key.is_empty() {
            return LocalMedia::default();
        }
        let first =
            |map: &HashMap<String, Vec<PathBuf>>| map.get(&key).and_then(|v| v.first()).cloned();
        let cover = first(&self.box_front).or_else(|| first(&self.box_3d));
        let logo = first(&self.clear_logo);
        let background = first(&self.fanart);

        let mut screenshots = Vec::new();
        for map in [&self.gameplay, &self.title_screen] {
            if let Some(paths) = map.get(&key) {
                for p in paths {
                    if screenshots.len() >= MAX_SCREENSHOTS {
                        break;
                    }
                    screenshots.push(p.clone());
                }
            }
        }
        LocalMedia {
            cover,
            logo,
            background,
            screenshots,
        }
    }
}

/// Convert a LaunchBox game title or image stem into a normalized lookup key.
///
/// LaunchBox replaces characters such as `'`, `;`, `:`, `?`, `/` with `_` and
/// appends numeric suffixes (`-01`, `-02`) to image files.
pub fn media_key(name: &str) -> String {
    let trimmed = strip_launchbox_index(name.trim());
    let mut out = String::with_capacity(trimmed.len());
    let mut prev_sep = false;
    for ch in trimmed.chars() {
        if matches!(
            ch,
            '\'' | ';' | ':' | '?' | '/' | '\\' | '*' | '"' | '<' | '>' | '|' | '_'
        ) {
            if !prev_sep && !out.is_empty() {
                out.push('_');
                prev_sep = true;
            }
        } else {
            for lower in ch.to_lowercase() {
                out.push(lower);
            }
            prev_sep = false;
        }
    }
    out.trim_matches('_').to_string()
}

/// Strip a trailing `-01` / `-02` style LaunchBox image numbering suffix.
fn strip_launchbox_index(stem: &str) -> &str {
    let Some((prefix, suffix)) = stem.rsplit_once('-') else {
        return stem;
    };
    if !prefix.is_empty()
        && (1..=3).contains(&suffix.len())
        && suffix.bytes().all(|b| b.is_ascii_digit())
    {
        prefix
    } else {
        stem
    }
}

fn is_image_ext(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|ext| {
            matches!(
                ext.to_ascii_lowercase().as_str(),
                "png" | "jpg" | "jpeg" | "webp" | "gif" | "bmp"
            )
        })
}

/// Index image files in `dir` and one level of region subfolders (e.g.
/// `Box - Front/North America/DOOM-01.jpg`).
fn index_folder(dir: &Path) -> HashMap<String, Vec<PathBuf>> {
    let mut out: HashMap<String, Vec<PathBuf>> = HashMap::new();
    let Ok(read) = std::fs::read_dir(dir) else {
        return out;
    };
    let mut dirs_to_scan = vec![dir.to_path_buf()];
    let mut entries: Vec<PathBuf> = read.flatten().map(|e| e.path()).collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            dirs_to_scan.push(path);
        }
    }
    for folder in dirs_to_scan {
        let Ok(read) = std::fs::read_dir(&folder) else {
            continue;
        };
        let mut files: Vec<PathBuf> = read
            .flatten()
            .filter_map(|e| {
                let p = e.path();
                (e.file_type().is_ok_and(|t| t.is_file()) && is_image_ext(&p)).then_some(p)
            })
            .collect();
        files.sort();
        for file in files {
            let Some(stem) = file.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            let key = media_key(stem);
            if !key.is_empty() {
                out.entry(key).or_default().push(file);
            }
        }
    }
    out
}

/// Resolve an XML `<ManualPath>` relative to `exo_root`, rejecting path traversal.
pub fn resolve_manual(exo_root: &Path, manual_path: Option<&str>) -> Option<PathBuf> {
    let raw = manual_path?.trim();
    if raw.is_empty() {
        return None;
    }
    let normalized = raw.replace('\\', "/");
    let rel = archive_guard::relative_entry_path(Path::new(&normalized))?;
    let candidate = exo_root.join(rel);
    candidate.is_file().then_some(candidate)
}

/// Format a local filesystem path as the `file://` URL expected by `irori-ui::art`.
pub fn path_to_file_url(path: &Path) -> String {
    format!("file://{}", path.display())
}
