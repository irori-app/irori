//! Which program of a game draws it, its architecture and its render API.
//!
//! The API comes from the program's imports, with the preferences of
//! ReShade's installer (DXGI over Direct3D 9, Vulkan over Direct3D 12, any
//! of them over OpenGL), unless ReShade's compatibility list names it.
//!
//! Many games start through a small launcher that imports no graphics API
//! itself. When the configured program is one, the game folder is searched
//! (a few levels deep, skipping installers, redistributables, crash
//! reporters, anticheat and updaters) and the program chosen is, in order:
//! one the compatibility list knows the API of, else the largest one whose
//! imports name an API. When an executable delegates rendering to a sibling
//! engine runtime DLL (such as Unity's `UnityPlayer.dll`), that DLL's imports
//! and size are counted toward the executable. The real game is nearly always
//! the largest program that draws anything; the user can still choose another.

use std::path::{Path, PathBuf};

use anyhow::Result;
use once_cell::sync::Lazy;
use regex::Regex;

use crate::compat::CompatList;
use crate::pe::{self, Machine};
use crate::{Api, ReshadeError};

/// What [`detect`] found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Detection {
    /// The program that draws the game.
    pub exe: PathBuf,
    pub machine: Machine,
    pub api: Option<Api>,
    /// The DLL name ReShade goes in as (`dxgi.dll`), when the API has one.
    pub dll: Option<String>,
    /// The folder ReShade goes to: the program's, or the folder the
    /// compatibility list names below it.
    pub dir: PathBuf,
    /// `exe` was found by searching the folder, not configured.
    pub searched: bool,
    /// The program (or the configured launcher) is banned from ReShade.
    pub banned: Option<String>,
}

/// The API ReShade's installer would pick for these imports.
pub fn api_from_imports(imports: &[String]) -> Option<Api> {
    let has = |prefix: &str| imports.iter().any(|m| m.starts_with(prefix));
    let d3d8 = has("d3d8");
    let mut d3d9 = d3d8 || has("d3d9");
    // GameWorks libraries mean DXGI, as in ReShade's installer.
    let mut dxgi = has("dxgi") || has("d3d1") || imports.iter().any(|m| m.contains("gfsdk"));
    let ddraw = has("ddraw");
    let mut opengl = has("opengl32");
    let vulkan = has("vulkan-1");
    if d3d9 && dxgi {
        d3d9 = false;
    }
    if dxgi && vulkan {
        dxgi = false;
    }
    if opengl && (d3d9 || dxgi || vulkan) {
        opengl = false;
    }
    if vulkan {
        Some(Api::Vulkan)
    } else if d3d9 {
        Some(if d3d8 && !has("d3d9") {
            Api::D3D8
        } else {
            Api::D3D9
        })
    } else if dxgi {
        Some(Api::Dxgi)
    } else if ddraw {
        Some(Api::DDraw)
    } else if opengl {
        Some(Api::OpenGl)
    } else {
        None
    }
}

/// Folders that never hold the game's program, including heavy asset trees.
const SKIP_DIRS: &[&str] = &[
    "_commonredist",
    "commonredist",
    "redist",
    "redists",
    "redistributables",
    "__installer",
    "_installer",
    "installer",
    "installers",
    "directx",
    "dotnet",
    "vcredist",
    "prereqs",
    "prerequisites",
    "support",
    "easyanticheat",
    "battleye",
    "crashreporter",
    "reshade-shaders",
    "content",
    "paks",
    "pak",
    "shaders",
    "movies",
    "audio",
    "sound",
    ".git",
];

/// Programs that are not the game.
static SKIP_EXE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r"(?i)^(unins\d*|setup.*|install.*|.*redist.*|dxsetup|dxwebsetup|.*crash.*|.*report.*|cef.*|.*helper.*|.*launcher.*|.*updater.*|.*_be|.*_eac|easyanticheat.*|start_protected_game|.*prereq.*|.*benchmark.*|.*config.*|.*settings.*|.*editor.*|.*server.*)\.exe$",
    )
    .expect("valid regex")
});

/// Engine runtime DLLs that carry the render API imports for a thin stub
/// executable (for example Unity 2018+ games, where `<Game>.exe` imports only
/// `UnityPlayer.dll` and `kernel32.dll`).
///
/// Stored as `(lowercase_import_name, canonical_filename)` so the common case
/// resolves directly on case-sensitive filesystems without scanning the folder.
const ENGINE_DLLS: &[(&str, &str)] = &[("unityplayer.dll", "UnityPlayer.dll")];

/// How deep the folder search goes, and how much it looks at.
/// Depth 4 reaches `<Game>/Binaries/Win64/*-Shipping.exe` (depth 3) and one
/// wrapper directory above it without traversing deep asset trees.
const MAX_DEPTH: usize = 4;
const MAX_ENTRIES: usize = 50_000;
const MAX_PARSED: usize = 40;

fn is_exe(path: &Path) -> bool {
    path.extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("exe"))
}

/// Skip Unreal Engine's shared `Engine/Binaries/` directory, which holds
/// tools and crash reporters rather than the game executable.
fn is_engine_binaries(parent: &Path, lower_name: &str) -> bool {
    lower_name == "binaries"
        && parent
            .file_name()
            .is_some_and(|p| p.eq_ignore_ascii_case("engine"))
}

struct Candidate {
    path: PathBuf,
    exe_size: u64,
    /// Total size of known engine runtime DLLs in the same folder, used as an
    /// upper bound when ordering candidates before reading PE headers.
    sibling_engine_size: u64,
}

impl Candidate {
    fn max_size(&self) -> u64 {
        self.exe_size.saturating_add(self.sibling_engine_size)
    }
}

/// Programs under `root` that might be the game, largest first.
fn candidates(root: &Path) -> Vec<Candidate> {
    let mut found = Vec::new();
    let mut stack = vec![(root.to_path_buf(), 0usize)];
    let mut seen = 0usize;
    while let Some((dir, depth)) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut dir_exes = Vec::new();
        let mut dir_engine_size = 0u64;
        for entry in entries.flatten() {
            seen += 1;
            if seen > MAX_ENTRIES {
                break;
            }
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_lowercase();
            if kind.is_dir() {
                if depth < MAX_DEPTH
                    && !SKIP_DIRS.contains(&name.as_str())
                    && !is_engine_binaries(&dir, &name)
                {
                    stack.push((path, depth + 1));
                }
            } else if kind.is_file() {
                if is_exe(&path) && !SKIP_EXE.is_match(&name) {
                    let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
                    dir_exes.push((path, size));
                } else if ENGINE_DLLS.iter().any(|&(lower, _)| name == lower) {
                    let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
                    dir_engine_size = dir_engine_size.saturating_add(size);
                }
            }
        }
        for (path, exe_size) in dir_exes {
            found.push(Candidate {
                path,
                exe_size,
                sibling_engine_size: dir_engine_size,
            });
        }
    }
    found.sort_by(|a, b| {
        b.max_size()
            .cmp(&a.max_size())
            .then_with(|| a.path.cmp(&b.path))
    });
    found
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn inside(root: &Path, path: &Path) -> Option<PathBuf> {
    std::fs::canonicalize(path)
        .ok()
        .filter(|p| p.starts_with(root))
}

fn find_sibling_engine_dll(
    root: &Path,
    dir: &Path,
    lower: &str,
    canonical: &str,
) -> Option<PathBuf> {
    let direct = dir.join(canonical);
    if let Some(path) = inside(root, &direct).filter(|p| p.is_file()) {
        return Some(path);
    }
    // Fall back to a case-insensitive scan on case-sensitive filesystems when
    // the DLL on disk uses different letter case.
    let entries = std::fs::read_dir(dir).ok()?;
    for entry in entries.flatten() {
        if entry
            .file_name()
            .to_str()
            .is_some_and(|name| name.eq_ignore_ascii_case(lower))
            && let Some(path) = inside(root, &entry.path()).filter(|p| p.is_file())
        {
            return Some(path);
        }
    }
    None
}

/// Resolve a render API from a sibling engine runtime DLL imported by `exe`,
/// keeping the lookup inside `root` and requiring the DLL's architecture to
/// match `exe`. Returns the API and the DLL's byte size.
fn api_from_engine_dlls(root: &Path, exe: &Path, info: &pe::PeInfo) -> Option<(Api, u64)> {
    let dir = exe.parent()?;
    for &(lower, canonical) in ENGINE_DLLS {
        if !info.imports.iter().any(|m| m == lower) {
            continue;
        }
        let Some(dll) = find_sibling_engine_dll(root, dir, lower, canonical) else {
            continue;
        };
        let Ok(dll_info) = pe::read_file(&dll) else {
            continue;
        };
        if dll_info.machine != info.machine {
            continue;
        }
        if let Some(api) = api_from_imports(&dll_info.imports) {
            let size = std::fs::metadata(&dll).map(|m| m.len()).unwrap_or(0);
            return Some((api, size));
        }
    }
    None
}

/// The API of `exe`: the list's, when it names one (forced or not), else the
/// imports of `exe`, else a sibling engine runtime DLL imported by `exe`.
/// Also returns the engine DLL's byte size when the API came from one.
fn api_of(root: &Path, exe: &Path, info: &pe::PeInfo, compat: &CompatList) -> Option<(Api, u64)> {
    compat
        .get(&file_name(exe))
        .and_then(|e| e.api())
        .or_else(|| api_from_imports(&info.imports))
        .map(|api| (api, 0))
        .or_else(|| api_from_engine_dlls(root, exe, info))
}

/// The DLL for `api`; a list entry forcing Direct3D 10, 11 or 12 gets that
/// API's own DLL instead of `dxgi.dll`, as ReShade's installer does.
fn dll_for(exe: &Path, api: Api, compat: &CompatList) -> Option<String> {
    let entry = compat.get(&file_name(exe));
    if api == Api::Dxgi
        && let Some(e) = entry.filter(|e| e.force_api)
    {
        match e.render_api.as_deref() {
            Some("D3D10") => return Some("d3d10.dll".into()),
            Some("D3D11") => return Some("d3d11.dll".into()),
            Some("D3D12") => return Some("d3d12.dll".into()),
            _ => {}
        }
    }
    api.dll().map(str::to_owned)
}

/// A usable program: a PE for x86 or x64 that draws with a known API.
/// Returns `(machine, api, engine_dll_size)`.
fn probe(root: &Path, exe: &Path, compat: &CompatList) -> Option<(Machine, Api, u64)> {
    let info = pe::read_file(exe).ok()?;
    if !matches!(info.machine, Machine::X86 | Machine::X64) {
        return None;
    }
    let (api, engine_size) = api_of(root, exe, &info, compat)?;
    Some((info.machine, api, engine_size))
}

/// Find the game's program under `root`. `chosen` is a program the user
/// picked and is used as it is; otherwise `configured` (the store's or the
/// user's executable setting) when it draws anything, else the folder is
/// searched. Blocking: run it off the async runtime.
pub fn detect(
    root: &Path,
    configured: Option<&Path>,
    chosen: Option<&Path>,
    compat: &CompatList,
) -> Result<Detection> {
    let root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let launcher_banned = configured
        .map(file_name)
        .filter(|name| compat.is_banned(name));

    let (exe, searched) = if let Some(chosen) = chosen {
        (
            inside(&root, chosen).ok_or(ReshadeError::OutsideGame)?,
            false,
        )
    } else if let Some(exe) = configured
        .and_then(|p| inside(&root, p))
        .filter(|e| e.is_file() && probe(&root, e, compat).is_some())
    {
        (exe, false)
    } else {
        let all = candidates(&root);
        let known = all
            .iter()
            .find(|c| {
                compat
                    .get(&file_name(&c.path))
                    .is_some_and(|e| e.api().is_some())
                    && probe(&root, &c.path, compat).is_some()
            })
            .map(|c| &c.path);
        let drawn = || {
            let mut best: Option<(&PathBuf, u64)> = None;
            for candidate in all.iter().take(MAX_PARSED) {
                if let Some((_, best_size)) = best
                    && candidate.max_size() < best_size
                {
                    break;
                }
                let Some((_, _, engine_size)) = probe(&root, &candidate.path, compat) else {
                    continue;
                };
                let effective_size = candidate.exe_size.saturating_add(engine_size);
                let better = match best {
                    None => true,
                    Some((best_path, best_size)) => {
                        effective_size > best_size
                            || (effective_size == best_size && candidate.path < *best_path)
                    }
                };
                if better {
                    best = Some((&candidate.path, effective_size));
                }
            }
            best.map(|(p, _)| p)
        };
        match known.or_else(drawn) {
            Some(p) => (p.clone(), true),
            None => {
                // Nothing draws: report the configured program, so a banned
                // launcher is still refused by name.
                if let Some(name) = launcher_banned {
                    return Err(ReshadeError::Banned { exe: name }.into());
                }
                return Err(ReshadeError::NoApi.into());
            }
        }
    };

    let info = pe::read_file(&exe)?;
    let api = api_of(&root, &exe, &info, compat).map(|(a, _)| a);
    let entry = compat.get(&file_name(&exe));
    let mut dir = exe.parent().map(Path::to_path_buf).unwrap_or(root.clone());
    if api != Some(Api::Vulkan)
        && let Some(target) = entry.and_then(|e| e.install_target.as_ref())
    {
        dir = dir.join(target);
    }
    let banned = entry
        .filter(|e| e.banned)
        .map(|_| file_name(&exe))
        .or(launcher_banned);
    Ok(Detection {
        dll: api.and_then(|a| dll_for(&exe, a, compat)),
        machine: info.machine,
        api,
        dir,
        searched,
        banned,
        exe,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pe::tests::build_pe;

    fn imports(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn picks_the_api_like_reshades_installer() {
        assert_eq!(api_from_imports(&imports(&["d3d9.dll"])), Some(Api::D3D9));
        assert_eq!(
            api_from_imports(&imports(&["d3d9.dll", "d3d11.dll"])),
            Some(Api::Dxgi)
        );
        assert_eq!(api_from_imports(&imports(&["dxgi.dll"])), Some(Api::Dxgi));
        assert_eq!(
            api_from_imports(&imports(&["d3d12.dll", "vulkan-1.dll"])),
            Some(Api::Vulkan)
        );
        assert_eq!(
            api_from_imports(&imports(&["opengl32.dll", "d3d9.dll"])),
            Some(Api::D3D9)
        );
        assert_eq!(
            api_from_imports(&imports(&["opengl32.dll"])),
            Some(Api::OpenGl)
        );
        assert_eq!(api_from_imports(&imports(&["d3d8.dll"])), Some(Api::D3D8));
        assert_eq!(api_from_imports(&imports(&["ddraw.dll"])), Some(Api::DDraw));
        assert_eq!(
            api_from_imports(&imports(&["gfsdk_aftermath_lib.x64.dll"])),
            Some(Api::Dxgi)
        );
        assert_eq!(
            api_from_imports(&imports(&["kernel32.dll", "d3dcompiler_47.dll"])),
            None
        );
    }

    fn write(path: &Path, bytes: &[u8]) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }

    #[test]
    fn uses_the_configured_program_when_it_draws() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("Game.exe");
        write(&exe, &build_pe(0x8664, &["d3d11.dll"], &[]));
        let found = detect(dir.path(), Some(&exe), None, &CompatList::default()).unwrap();
        assert_eq!(found.exe, std::fs::canonicalize(&exe).unwrap());
        assert_eq!(found.machine, Machine::X64);
        assert_eq!(found.api, Some(Api::Dxgi));
        assert_eq!(found.dll.as_deref(), Some("dxgi.dll"));
        assert!(!found.searched);
        assert_eq!(found.banned, None);
    }

    #[test]
    fn looks_past_a_launcher() {
        let dir = tempfile::tempdir().unwrap();
        let launcher = dir.path().join("Start.exe");
        write(&launcher, &build_pe(0x014c, &["kernel32.dll"], &[]));
        let mut big = build_pe(0x014c, &["d3d9.dll"], &[]);
        big.resize(big.len() + 4096, 0);
        let game = dir.path().join("bin/win32/GameMain.exe");
        write(&game, &big);
        // A smaller program drawing with OpenGL, and skipped ones.
        write(
            &dir.path().join("tools/Viewer.exe"),
            &build_pe(0x8664, &["opengl32.dll"], &[]),
        );
        let mut huge = build_pe(0x8664, &["d3d11.dll"], &[]);
        huge.resize(huge.len() + 65536, 0);
        write(&dir.path().join("_CommonRedist/dxsetup.exe"), &huge);
        write(&dir.path().join("UnityCrashHandler64.exe"), &huge);

        let found = detect(dir.path(), Some(&launcher), None, &CompatList::default()).unwrap();
        assert_eq!(found.exe, std::fs::canonicalize(&game).unwrap());
        assert_eq!(found.api, Some(Api::D3D9));
        assert_eq!(found.machine, Machine::X86);
        assert!(found.searched);
    }

    #[test]
    fn follows_the_compatibility_list() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("hl2.exe");
        write(&exe, &build_pe(0x014c, &["kernel32.dll"], &[]));
        let list = CompatList::parse("[hl2.exe]\nRenderApi=D3D9\nInstallTarget=bin\n");
        let found = detect(dir.path(), Some(&exe), None, &list).unwrap();
        assert_eq!(found.api, Some(Api::D3D9));
        assert_eq!(
            found.dir,
            std::fs::canonicalize(dir.path()).unwrap().join("bin")
        );

        let forced = CompatList::parse("[hl2.exe]\nRenderApi=D3D12\nForceInstallApi=1\n");
        let found = detect(dir.path(), Some(&exe), None, &forced).unwrap();
        assert_eq!(found.dll.as_deref(), Some("d3d12.dll"));
    }

    #[test]
    fn reports_banned_programs_and_launchers() {
        let dir = tempfile::tempdir().unwrap();
        let launcher = dir.path().join("ApexLauncher.exe");
        write(&launcher, &build_pe(0x8664, &["kernel32.dll"], &[]));
        let game = dir.path().join("r5apex.exe");
        write(&game, &build_pe(0x8664, &["d3d11.dll"], &[]));
        let list = CompatList::parse("[ApexLauncher.exe]\nBanned=1\n[r5apex.exe]\nBanned=1\n");
        let found = detect(dir.path(), Some(&launcher), None, &list).unwrap();
        assert_eq!(found.banned.as_deref(), Some("r5apex.exe"));

        let only_launcher = CompatList::parse("[ApexLauncher.exe]\nBanned=1\n");
        let found = detect(dir.path(), Some(&launcher), None, &only_launcher).unwrap();
        assert_eq!(found.banned.as_deref(), Some("ApexLauncher.exe"));
    }

    #[test]
    fn chosen_programs_must_be_in_the_game_folder() {
        let dir = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        let exe = other.path().join("Other.exe");
        write(&exe, &build_pe(0x8664, &["d3d11.dll"], &[]));
        let err = detect(dir.path(), None, Some(&exe), &CompatList::default()).unwrap_err();
        assert!(matches!(
            ReshadeError::of(&err),
            Some(ReshadeError::OutsideGame)
        ));
    }

    #[test]
    fn says_when_nothing_draws() {
        let dir = tempfile::tempdir().unwrap();
        write(
            &dir.path().join("Tool.exe"),
            &build_pe(0x8664, &["kernel32.dll"], &[]),
        );
        let err = detect(dir.path(), None, None, &CompatList::default()).unwrap_err();
        assert!(matches!(ReshadeError::of(&err), Some(ReshadeError::NoApi)));
    }

    #[test]
    fn finds_unreal_shipping_exe_and_skips_heavy_and_engine_folders() {
        let dir = tempfile::tempdir().unwrap();
        // Root UE bootstrapper imports no graphics API.
        let bootstrap = dir.path().join("MyGame.exe");
        write(&bootstrap, &build_pe(0x8664, &["kernel32.dll"], &[]));

        // Real UE shipping binary under <Game>/Binaries/Win64.
        let shipping = dir
            .path()
            .join("MyGame/Binaries/Win64/MyGame-Win64-Shipping.exe");
        let mut game_pe = build_pe(0x8664, &["d3d12.dll"], &[]);
        game_pe.resize(game_pe.len() + 2048, 0);
        write(&shipping, &game_pe);

        // Larger decoy executables inside Engine/Binaries and heavy asset folders
        // must all be skipped.
        let mut huge = build_pe(0x8664, &["d3d11.dll"], &[]);
        huge.resize(huge.len() + 65536, 0);
        for ignored in [
            "Engine/Binaries/Win64/UnrealTool.exe",
            "MyGame/Content/Paks/Fake.exe",
            "MyGame/Content/Decoy.exe",
            "Paks/Decoy.exe",
            "Pak/Decoy.exe",
            "Shaders/Decoy.exe",
            "Movies/Decoy.exe",
            "Audio/Decoy.exe",
            "Sound/Decoy.exe",
            ".git/Decoy.exe",
            // Deeper than MAX_DEPTH (5 nested directories below root).
            "a/b/c/d/e/TooDeep.exe",
        ] {
            write(&dir.path().join(ignored), &huge);
        }

        let found = detect(dir.path(), Some(&bootstrap), None, &CompatList::default()).unwrap();
        assert_eq!(found.exe, std::fs::canonicalize(&shipping).unwrap());
        assert_eq!(found.machine, Machine::X64);
        assert_eq!(found.api, Some(Api::Dxgi));
        assert!(found.searched);
    }

    #[test]
    fn detects_unity_stub_via_sibling_unity_player_dll() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("REPO.exe");
        write(
            &exe,
            &build_pe(0x8664, &["UnityPlayer.dll", "KERNEL32.dll"], &[]),
        );
        write(
            &dir.path().join("UnityPlayer.dll"),
            &build_pe(
                0x8664,
                &["OPENGL32.dll", "d3d11.dll", "dxgi.dll"],
                &["d3d12.dll"],
            ),
        );
        write(
            &dir.path().join("UnityCrashHandler64.exe"),
            &build_pe(0x8664, &["KERNEL32.dll"], &[]),
        );

        let canonical_exe = std::fs::canonicalize(&exe).unwrap();

        // Folder search (Steam-like game with no configured executable).
        let searched = detect(dir.path(), None, None, &CompatList::default()).unwrap();
        assert_eq!(searched.exe, canonical_exe);
        assert_eq!(searched.machine, Machine::X64);
        assert_eq!(searched.api, Some(Api::Dxgi));
        assert_eq!(searched.dll.as_deref(), Some("dxgi.dll"));
        assert!(searched.searched);

        // Configured executable (sideload/GOG/Epic).
        let configured = detect(dir.path(), Some(&exe), None, &CompatList::default()).unwrap();
        assert_eq!(configured.exe, canonical_exe);
        assert_eq!(configured.api, Some(Api::Dxgi));
        assert!(!configured.searched);

        // Explicitly chosen executable (ReShade/OptiScaler).
        let chosen = detect(dir.path(), None, Some(&exe), &CompatList::default()).unwrap();
        assert_eq!(chosen.exe, canonical_exe);
        assert_eq!(chosen.api, Some(Api::Dxgi));
        assert_eq!(chosen.dll.as_deref(), Some("dxgi.dll"));

        // OptiScaler target detection succeeds on the 64-bit Unity game.
        let opti = crate::optiscaler::detect_target(dir.path(), None, None).unwrap();
        assert_eq!(opti.exe, canonical_exe);
        assert_eq!(opti.api, Some(Api::Dxgi));
    }

    #[test]
    fn unity_stub_ranks_by_unity_player_size_over_larger_helper_exes() {
        let dir = tempfile::tempdir().unwrap();
        let game = dir.path().join("Game.exe");
        write(
            &game,
            &build_pe(0x8664, &["UnityPlayer.dll", "KERNEL32.dll"], &[]),
        );

        let mut unity_player = build_pe(0x8664, &["d3d11.dll", "dxgi.dll"], &[]);
        unity_player.resize(unity_player.len() + 65536, 0);
        write(&dir.path().join("UnityPlayer.dll"), &unity_player);

        // A larger helper tool in a subfolder and a larger unrelated tool in the
        // same folder as UnityPlayer.dll must both lose to the Unity game stub.
        let mut tool = build_pe(0x8664, &["opengl32.dll"], &[]);
        tool.resize(tool.len() + 16384, 0);
        write(&dir.path().join("tools/Viewer.exe"), &tool);
        write(&dir.path().join("AssetTool.exe"), &tool);

        let found = detect(dir.path(), None, None, &CompatList::default()).unwrap();
        assert_eq!(found.exe, std::fs::canonicalize(&game).unwrap());
        assert_eq!(found.api, Some(Api::Dxgi));
    }

    #[test]
    fn unity_detection_handles_non_standard_casing_and_architecture_checks() {
        // Non-standard lowercase UnityPlayer.dll filename on disk.
        let lower_dir = tempfile::tempdir().unwrap();
        let lower_exe = lower_dir.path().join("LowerGame.exe");
        write(&lower_exe, &build_pe(0x8664, &["UnityPlayer.dll"], &[]));
        write(
            &lower_dir.path().join("unityplayer.dll"),
            &build_pe(0x8664, &["d3d11.dll"], &[]),
        );
        let found = detect(lower_dir.path(), None, None, &CompatList::default()).unwrap();
        assert_eq!(found.exe, std::fs::canonicalize(&lower_exe).unwrap());
        assert_eq!(found.api, Some(Api::Dxgi));

        // 32-bit Unity game is detected for ReShade and rejected as 32-bit by OptiScaler.
        let x86_dir = tempfile::tempdir().unwrap();
        let x86_exe = x86_dir.path().join("Unity32.exe");
        write(&x86_exe, &build_pe(0x014c, &["UnityPlayer.dll"], &[]));
        write(
            &x86_dir.path().join("UnityPlayer.dll"),
            &build_pe(0x014c, &["d3d11.dll"], &[]),
        );
        let found32 = detect(x86_dir.path(), None, None, &CompatList::default()).unwrap();
        assert_eq!(found32.machine, Machine::X86);
        assert_eq!(found32.api, Some(Api::Dxgi));
        let err = crate::optiscaler::detect_target(x86_dir.path(), None, None).unwrap_err();
        assert!(matches!(
            ReshadeError::of(&err),
            Some(ReshadeError::UnsupportedArch)
        ));

        // Architecture mismatch between the stub and UnityPlayer.dll is ignored.
        let mismatch_dir = tempfile::tempdir().unwrap();
        write(
            &mismatch_dir.path().join("Mismatch.exe"),
            &build_pe(0x8664, &["UnityPlayer.dll"], &[]),
        );
        write(
            &mismatch_dir.path().join("UnityPlayer.dll"),
            &build_pe(0x014c, &["d3d11.dll"], &[]),
        );
        let err = detect(mismatch_dir.path(), None, None, &CompatList::default()).unwrap_err();
        assert!(matches!(ReshadeError::of(&err), Some(ReshadeError::NoApi)));
    }

    #[cfg(unix)]
    #[test]
    fn unity_detection_refuses_symlink_outside_game_folder() {
        use std::os::unix::fs::symlink;

        let game = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let external_dll = outside.path().join("UnityPlayer.dll");
        write(&external_dll, &build_pe(0x8664, &["d3d11.dll"], &[]));
        write(
            &game.path().join("Game.exe"),
            &build_pe(0x8664, &["UnityPlayer.dll"], &[]),
        );
        symlink(&external_dll, game.path().join("UnityPlayer.dll")).unwrap();

        let err = detect(game.path(), None, None, &CompatList::default()).unwrap_err();
        assert!(matches!(ReshadeError::of(&err), Some(ReshadeError::NoApi)));
    }
}
