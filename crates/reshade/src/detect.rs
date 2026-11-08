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
//! imports name an API. The real game is nearly always the largest program
//! that draws anything; the user can still choose another.

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

/// Folders that never hold the game's program.
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
];

/// Programs that are not the game.
static SKIP_EXE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r"(?i)^(unins\d*|setup.*|install.*|.*redist.*|dxsetup|dxwebsetup|.*crash.*|.*report.*|cef.*|.*helper.*|.*launcher.*|.*updater.*|.*_be|.*_eac|easyanticheat.*|start_protected_game|.*prereq.*|.*benchmark.*|.*config.*|.*settings.*|.*editor.*|.*server.*)\.exe$",
    )
    .expect("valid regex")
});

/// How deep the folder search goes, and how much it looks at.
const MAX_DEPTH: usize = 6;
const MAX_ENTRIES: usize = 50_000;
const MAX_PARSED: usize = 40;

fn is_exe(path: &Path) -> bool {
    path.extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("exe"))
}

/// Programs under `root` that might be the game, largest first.
fn candidates(root: &Path) -> Vec<(PathBuf, u64)> {
    let mut found = Vec::new();
    let mut stack = vec![(root.to_path_buf(), 0usize)];
    let mut seen = 0usize;
    while let Some((dir, depth)) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
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
                if depth < MAX_DEPTH && !SKIP_DIRS.contains(&name.as_str()) {
                    stack.push((path, depth + 1));
                }
            } else if kind.is_file() && is_exe(&path) && !SKIP_EXE.is_match(&name) {
                let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
                found.push((path, size));
            }
        }
    }
    found.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    found
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// The API of `exe`: the list's, when it names one (forced or not), else
/// the imports'.
fn api_of(exe: &Path, imports: &[String], compat: &CompatList) -> Option<Api> {
    compat
        .get(&file_name(exe))
        .and_then(|e| e.api())
        .or_else(|| api_from_imports(imports))
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
fn probe(exe: &Path, compat: &CompatList) -> Option<(Machine, Api)> {
    let info = pe::read_file(exe).ok()?;
    if !matches!(info.machine, Machine::X86 | Machine::X64) {
        return None;
    }
    Some((info.machine, api_of(exe, &info.imports, compat)?))
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
    let inside = |p: &Path| {
        std::fs::canonicalize(p)
            .ok()
            .filter(|p| p.starts_with(&root))
    };
    let launcher_banned = configured
        .map(file_name)
        .filter(|name| compat.is_banned(name));

    let (exe, searched) = if let Some(chosen) = chosen {
        (inside(chosen).ok_or(ReshadeError::OutsideGame)?, false)
    } else if let Some(exe) = configured
        .and_then(inside)
        .filter(|e| e.is_file() && probe(e, compat).is_some())
    {
        (exe, false)
    } else {
        let all = candidates(&root);
        let known = all.iter().find(|(p, _)| {
            compat.get(&file_name(p)).is_some_and(|e| e.api().is_some())
                && probe(p, compat).is_some()
        });
        let drawn = || {
            all.iter()
                .take(MAX_PARSED)
                .find(|(p, _)| probe(p, compat).is_some())
        };
        match known.or_else(drawn) {
            Some((p, _)) => (p.clone(), true),
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
    let api = api_of(&exe, &info.imports, compat);
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
}
