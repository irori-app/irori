//! Where an unpacked mod's files go in the game folder.
//!
//! Archives wrap their files in all sorts of ways: a folder named after the
//! mod, a version folder, the game's own folders, or loose files meant for
//! the game's data folder (Bethesda games load mods from `Data`, Unreal
//! Engine games load `.pak` mods from `<Project>/Content/Paks/~mods`). The
//! rules here turn what an archive holds into paths relative to the game
//! folder.

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
        let unreal = unreal_project(game_dir).is_some() && UNREAL_FOLDERS.contains(&lower.as_str());
        if game_has(game_dir, name)
            || in_data
            || unreal
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

/// Folders of an Unreal Engine project a mod may start with.
const UNREAL_FOLDERS: &[&str] = &["content", "paks", "~mods", "logicmods", "binaries"];

/// Files Unreal Engine loads as packages: `.pak`, and the I/O store pair
/// (`.ucas`, `.utoc`) with its signature that newer games use beside it.
fn is_unreal_package(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    [".pak", ".ucas", ".utoc", ".sig"]
        .iter()
        .any(|ext| lower.ends_with(ext))
}

/// The Unreal Engine project folder of the game (`Pal`, `Stalker2`…): the
/// folder beside `Engine` holding `Content/Paks`, as the game folder spells
/// it.
pub fn unreal_project(game_dir: &Path) -> Option<String> {
    let top = entries(game_dir);
    if !top
        .iter()
        .any(|(n, dir)| *dir && n.eq_ignore_ascii_case("engine"))
    {
        return None;
    }
    top.into_iter()
        .filter(|(n, dir)| *dir && !n.eq_ignore_ascii_case("engine"))
        .map(|(n, _)| n)
        .find(|n| {
            let content = entries(&game_dir.join(n))
                .into_iter()
                .find(|(c, dir)| *dir && c.eq_ignore_ascii_case("content"));
            content.is_some_and(|(c, _)| {
                entries(&game_dir.join(n).join(c))
                    .iter()
                    .any(|(p, dir)| *dir && p.eq_ignore_ascii_case("paks"))
            })
        })
}

/// Where an Unreal Engine mod goes, by what it starts with: packages into
/// `Content/Paks/~mods`, `~mods` and `LogicMods` (UE4SS' Blueprint mods)
/// into `Content/Paks`, `Paks` into `Content`, `Content` and `Binaries`
/// into the project folder. `None` when it is none of these.
fn unreal_prefix(top: &[(String, bool)], game_dir: &Path) -> Option<String> {
    let project = unreal_project(game_dir)?;
    if top.is_empty() {
        return None;
    }
    if top.iter().all(|(n, dir)| !dir && is_unreal_package(n)) {
        return Some(format!("{project}/Content/Paks/~mods"));
    }
    let dirs: Vec<String> = top
        .iter()
        .filter(|(_, dir)| *dir)
        .map(|(n, _)| n.to_ascii_lowercase())
        .collect();
    let only =
        |names: &[&str]| !dirs.is_empty() && dirs.iter().all(|d| names.contains(&d.as_str()));
    if only(&["~mods", "logicmods"]) {
        Some(format!("{project}/Content/Paks"))
    } else if only(&["paks"]) {
        Some(format!("{project}/Content"))
    } else if only(&["content", "binaries"]) {
        Some(project)
    } else {
        None
    }
}

/// The one folder of the game holding every folder `top` starts with, when
/// the game folder itself holds none of them: a mod made for a game whose
/// files sit one level down (`<Game>/Mods`, `bin/plugins`).
fn marker_prefix(top: &[(String, bool)], game_dir: &Path) -> Option<String> {
    let dirs: Vec<&str> = top
        .iter()
        .filter(|(_, dir)| *dir)
        .map(|(n, _)| n.as_str())
        .collect();
    if dirs.is_empty() || dirs.iter().any(|d| game_has(game_dir, d)) {
        return None;
    }
    let mut holders = entries(game_dir).into_iter().filter(|(n, dir)| {
        *dir && {
            let inside = entries(&game_dir.join(n));
            dirs.iter().all(|d| {
                inside
                    .iter()
                    .any(|(i, is_dir)| *is_dir && i.eq_ignore_ascii_case(d))
            })
        }
    });
    let (first, _) = holders.next()?;
    // Two folders holding them: which one is meant can't be told.
    holders.next().is_none().then_some(first)
}

/// Where the files in `content` go, relative to the game folder: `""` when
/// they already mirror the game folder, the game's data folder for
/// Bethesda mods made of plugins and data folders, the project's package
/// folders for Unreal Engine mods, or the one folder of the game that holds
/// every folder the mod starts with.
pub fn target_prefix(content: &Path, game_dir: &Path) -> String {
    let top = entries(content);
    if let Some(prefix) = unreal_prefix(&top, game_dir) {
        return prefix;
    }
    let Some(data) = data_dir(game_dir) else {
        return marker_prefix(&top, game_dir).unwrap_or_default();
    };
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

#[cfg(test)]
mod tests {
    use super::*;

    fn touch(root: &Path, rel: &str) {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, b"x").unwrap();
    }

    fn unreal_game(root: &Path) -> PathBuf {
        let game = root.join("game");
        touch(&game, "Engine/Binaries/x.dll");
        touch(&game, "Pal/Content/Paks/Pal-Windows.pak");
        touch(&game, "Pal/Binaries/Win64/Pal.exe");
        touch(&game, "Palworld.exe");
        game
    }

    /// Where an archive unpacked into `name` under `root` goes.
    fn place(root: &Path, game: &Path, name: &str, files: &[&str]) -> String {
        let unpacked = root.join(name);
        for f in files {
            touch(&unpacked, f);
        }
        let content = content_root(&unpacked, game);
        target_prefix(&content, game)
    }

    #[test]
    fn unreal_packages_go_to_the_mods_folder() {
        let tmp = tempfile::tempdir().unwrap();
        let game = unreal_game(tmp.path());
        assert_eq!(unreal_project(&game).as_deref(), Some("Pal"));
        let p = |name: &str, files: &[&str]| place(tmp.path(), &game, name, files);
        assert_eq!(
            p(
                "a",
                &[
                    "MyMod/MyMod_P.pak",
                    "MyMod/MyMod_P.ucas",
                    "MyMod/MyMod_P.utoc"
                ]
            ),
            "Pal/Content/Paks/~mods"
        );
        assert_eq!(p("b", &["~mods/x.pak"]), "Pal/Content/Paks");
        assert_eq!(
            p("c", &["LogicMods/x.pak", "~mods/y.pak"]),
            "Pal/Content/Paks"
        );
        assert_eq!(p("d", &["Paks/LogicMods/x.pak"]), "Pal/Content");
        assert_eq!(
            p(
                "e",
                &[
                    "Wrapper/Binaries/Win64/dwmapi.dll",
                    "Wrapper/Binaries/Win64/ue4ss/UE4SS.dll"
                ]
            ),
            "Pal"
        );
        // Mirroring the game folder already.
        assert_eq!(p("f", &["Pal/Content/Paks/~mods/x.pak", "readme.txt"]), "");
    }

    #[test]
    fn folders_one_level_down_are_found() {
        let tmp = tempfile::tempdir().unwrap();
        let game = tmp.path().join("game");
        touch(&game, "Game/Mods/readme.txt");
        touch(&game, "Game.exe");
        let p = |name: &str, files: &[&str]| place(tmp.path(), &game, name, files);
        assert_eq!(p("a", &["Mods/Cool/mod.lua", "notes.txt"]), "Game");
        assert_eq!(p("b", &["NewFolder/x.txt", "y.txt"]), "");
        // Two folders could take it: left at the root.
        touch(&game, "Other/Mods/readme.txt");
        assert_eq!(p("c", &["Mods/Cool/mod.lua", "notes.txt"]), "");
        assert!(unreal_project(&game).is_none());
    }
}
