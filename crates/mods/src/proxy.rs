//! Proxy DLLs: mod loaders (BepInEx's `winhttp.dll`, MelonLoader's
//! `version.dll`, ASI loaders' `dinput8.dll`, UE4SS's `dwmapi.dll`…) put a
//! DLL named like a system one beside the game's program, and Windows loads
//! it from there first. Wine loads its own built-in copy instead unless it is
//! told to prefer the game's, so under Wine each such DLL a mod brings needs
//! `<dll>=n,b` in `WINEDLLOVERRIDES` while the mods are applied.

use std::collections::{BTreeSet, HashMap};
use std::path::Path;

use anyhow::Result;

use crate::fsutil::CaseAligner;
use crate::library::ModLibrary;

/// System DLLs mod loaders stand in for, without `.dll`. Only names Wine has
/// a built-in copy of matter: for any other DLL Wine already loads the
/// game's.
pub const PROXY_DLLS: &[&str] = &[
    "d3d8",
    "d3d9",
    "d3d10",
    "d3d10_1",
    "d3d11",
    "d3d12",
    "ddraw",
    "dinput",
    "dinput8",
    "dsound",
    "dwmapi",
    "dxgi",
    "iphlpapi",
    "msimg32",
    "opengl32",
    "version",
    "winhttp",
    "wininet",
    "winmm",
    "xinput1_3",
    "xinput1_4",
    "xinput9_1_0",
];

/// The proxy DLLs among `files` (paths relative to the game folder, `/`
/// separated) that sit in a folder holding a program, as base names in
/// lower case, sorted. `has_program` tells whether a folder (relative, `""`
/// for the game folder) holds an `.exe`. A DLL elsewhere (a redistributable
/// folder, a mod's tools) is no proxy: the game never loads it.
pub fn proxy_dlls(files: &[String], mut has_program: impl FnMut(&str) -> bool) -> Vec<String> {
    let mut found = BTreeSet::new();
    let mut checked: HashMap<String, bool> = HashMap::new();
    for rel in files {
        let (dir, name) = rel.rsplit_once('/').unwrap_or(("", rel.as_str()));
        let lower = name.to_ascii_lowercase();
        let Some(base) = lower.strip_suffix(".dll") else {
            continue;
        };
        if !PROXY_DLLS.contains(&base) {
            continue;
        }
        let key = dir.to_lowercase();
        let beside = *checked.entry(key).or_insert_with(|| has_program(dir));
        if beside {
            found.insert(base.to_string());
        }
    }
    found.into_iter().collect()
}

/// The proxy DLLs the enabled mods of `lib` put beside a program of the game
/// in `game_dir` or of a mod. Reads the mods' indexes and lists the game's
/// folders; blocking.
pub fn enabled_proxy_dlls(lib: &ModLibrary, game_dir: &Path) -> Result<Vec<String>> {
    let list = lib.load()?;
    let mut files = Vec::new();
    for m in list.mods.iter().filter(|m| m.enabled) {
        files.extend(lib.index(&m.id)?);
    }
    let is_exe = |name: &str| name.to_ascii_lowercase().ends_with(".exe");
    let mod_programs: BTreeSet<String> = files
        .iter()
        .filter(|f| is_exe(f))
        .map(|f| f.rsplit_once('/').map_or("", |(d, _)| d).to_lowercase())
        .collect();
    let mut aligner = CaseAligner::new(game_dir);
    Ok(proxy_dlls(&files, |dir| {
        if mod_programs.contains(&dir.to_lowercase()) {
            return true;
        }
        let path = game_dir.join(aligner.align(dir));
        std::fs::read_dir(path).is_ok_and(|entries| {
            entries
                .flatten()
                .any(|e| is_exe(&e.file_name().to_string_lossy()) && e.path().is_file())
        })
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_known_dlls_beside_a_program_count() {
        let files: Vec<String> = [
            "winhttp.dll",
            "BepInEx/core/BepInEx.dll",
            "Binaries/Win64/dwmapi.dll",
            "Binaries/Win64/UE4SS.dll",
            "_CommonRedist/d3d11.dll",
            "Version.DLL",
            "doorstop_config.ini",
        ]
        .map(String::from)
        .to_vec();
        let programs = ["", "binaries/win64"];
        let found = proxy_dlls(&files, |dir| {
            programs.contains(&dir.to_lowercase().as_str())
        });
        assert_eq!(found, ["dwmapi", "version", "winhttp"]);
        assert!(proxy_dlls(&files, |_| false).is_empty());
    }

    #[test]
    fn programs_come_from_the_game_or_a_mod() {
        let tmp = tempfile::tempdir().unwrap();
        let game = tmp.path().join("game");
        std::fs::create_dir_all(game.join("Bin")).unwrap();
        std::fs::write(game.join("Bin/Game.exe"), b"MZ").unwrap();
        let lib = ModLibrary::new(tmp.path().join("mods"));
        let mut list = crate::ModList::default();
        let add = |list: &mut crate::ModList, name: &str, files: &[&str]| {
            let staging = lib.staging().unwrap();
            let tree = staging.path().join("tree");
            for f in files {
                let path = tree.join(f);
                std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                std::fs::write(path, b"x").unwrap();
            }
            let entry = crate::ModEntry {
                id: String::new(),
                name: name.into(),
                version: None,
                author: None,
                summary: None,
                source: crate::ModSource::Archive {
                    file_name: format!("{name}.zip"),
                },
                enabled: true,
                installed_at: String::new(),
                size: 0,
                files: 0,
                dependencies: Vec::new(),
            };
            lib.add(
                list,
                &tree,
                entry,
                None,
                &irori_core::AppSettings::default(),
            )
            .unwrap()
        };
        add(&mut list, "loader", &["bin/dinput8.dll"]);
        let tool = add(&mut list, "tool", &["Tool/tool.exe", "Tool/version.dll"]);
        add(&mut list, "stray", &["Docs/winmm.dll"]);
        assert_eq!(
            enabled_proxy_dlls(&lib, &game).unwrap(),
            ["dinput8", "version"]
        );
        list.get_mut(&tool).unwrap().enabled = false;
        lib.save(&list).unwrap();
        assert_eq!(enabled_proxy_dlls(&lib, &game).unwrap(), ["dinput8"]);
    }
}
