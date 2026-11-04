//! Where an unpacked mod's files go in the game folder.
//!
//! Archives wrap their files in all sorts of ways: a folder named after the
//! mod, a version folder, the game's own folders, or loose files meant for
//! the game's data folder (Bethesda games load mods from `Data`). The rules
//! here turn what an archive holds into paths relative to the game folder.

use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::fsutil::{CaseAligner, join_rel, move_file, walk_files};

/// Entries archivers add that are never part of a mod.
fn is_junk(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower == "__macosx" || lower == ".ds_store" || lower == "thumbs.db" || lower == "desktop.ini"
}

fn entries(dir: &Path) -> Vec<(String, bool)> {
    let Ok(read) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<(String, bool)> = read
        .flatten()
        .map(|e| {
            (
                e.file_name().to_string_lossy().into_owned(),
                e.file_type().is_ok_and(|t| t.is_dir()),
            )
        })
        .filter(|(n, _)| !is_junk(n))
        .collect();
    out.sort();
    out
}

fn game_has(game_dir: &Path, name: &str) -> bool {
    let lower = name.to_lowercase();
    entries(game_dir)
        .iter()
        .any(|(n, _)| n.to_lowercase() == lower)
}

/// The folder inside `extracted` where the mod starts: single wrapping
/// folders are skipped, unless the folder is one the game has too (a mod
/// whose only folder is `BepInEx` or `Data` belongs at the game's root), a
/// folder of the game's data folder (`meshes`), or a FOMOD installer.
pub fn content_root(extracted: &Path, game_dir: &Path) -> PathBuf {
    let mut root = extracted.to_path_buf();
    for _ in 0..8 {
        let found = entries(&root);
        let [(name, true)] = found.as_slice() else {
            break;
        };
        let lower = name.to_ascii_lowercase();
        let in_data = data_dir(game_dir).is_some_and(|d| game_has(&game_dir.join(d), name));
        if game_has(game_dir, name)
            || in_data
            || lower == "fomod"
            || DATA_FOLDERS.contains(&lower.as_str())
        {
            break;
        }
        root.push(name);
    }
    root
}

/// The game's data folder for loose mod files, when it has one: Bethesda
/// games keep plugins and archives (`.esp`, `.esm`, `.bsa`, `.ba2`) in
/// `Data`. Returned as the game folder spells it.
pub fn data_dir(game_dir: &Path) -> Option<String> {
    let (name, _) = entries(game_dir)
        .into_iter()
        .find(|(n, dir)| *dir && n.eq_ignore_ascii_case("data"))?;
    let plugins = entries(&game_dir.join(&name)).iter().any(|(n, dir)| {
        let lower = n.to_ascii_lowercase();
        !dir && [".esm", ".esp", ".esl", ".bsa", ".ba2"]
            .iter()
            .any(|ext| lower.ends_with(ext))
    });
    plugins.then_some(name)
}

/// Folder names that only appear inside a Bethesda data folder.
const DATA_FOLDERS: &[&str] = &[
    "meshes",
    "textures",
    "scripts",
    "sound",
    "music",
    "interface",
    "materials",
    "seq",
    "skse",
    "f4se",
    "sksevr",
    "strings",
    "shadersfx",
    "lodsettings",
    "dialogueviews",
    "grass",
    "video",
    "nvse",
    "obse",
    "fose",
];

/// Where the files in `content` go, relative to the game folder: `""` when
/// they already mirror the game folder, else the game's data folder for
/// mods made of plugins and data folders.
pub fn target_prefix(content: &Path, game_dir: &Path) -> String {
    let Some(data) = data_dir(game_dir) else {
        return String::new();
    };
    let top = entries(content);
    if top.iter().any(|(n, _)| game_has(game_dir, n)) {
        return String::new();
    }
    let looks_like_data = top.iter().any(|(n, dir)| {
        let lower = n.to_ascii_lowercase();
        if *dir {
            DATA_FOLDERS.contains(&lower.as_str())
        } else {
            [".esm", ".esp", ".esl", ".bsa", ".ba2"]
                .iter()
                .any(|ext| lower.ends_with(ext))
        }
    });
    if looks_like_data { data } else { String::new() }
}

/// Move every file under `from` into `tree` at the path `map` gives it
/// (relative to the game folder; `None` leaves the file out), spelled as
/// the game folder spells its folders. Returns the number of files moved.
pub fn import(
    from: &Path,
    tree: &Path,
    game_dir: &Path,
    mut map: impl FnMut(&str) -> Option<String>,
) -> Result<usize> {
    let mut moved = 0;
    let mut aligner = CaseAligner::new(game_dir);
    for (rel, _) in walk_files(from)? {
        if rel.split('/').any(is_junk) {
            continue;
        }
        let Some(target) = map(&rel) else {
            continue;
        };
        let target = aligner.align(&target);
        if irori_core::archive_guard::relative_entry_path(Path::new(&target)).is_none() {
            continue;
        }
        move_file(&join_rel(from, &rel), &join_rel(tree, &target))?;
        moved += 1;
    }
    Ok(moved)
}

/// `prefix` joined to `rel` with `/`.
pub fn prefixed(prefix: &str, rel: &str) -> String {
    if prefix.is_empty() {
        rel.to_string()
    } else {
        format!("{}/{rel}", prefix.trim_end_matches('/'))
    }
}
