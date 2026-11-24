//! NVIDIA DLSS translated to Apple's MetalFX on macOS, for the Wine builds
//! that ship the translation: Game Porting Toolkit 3's D3DMetal (also in
//! recent CrossOver), which brings `nvngx-on-metalfx.dll`, and DXMT, which
//! brings its own `nvngx.dll` beside `winemetal.dll`. A build without those
//! files can't do it, so nothing is switched on there.
//!
//! Turning it on for a game copies the build's NGX and NVAPI DLLs into the
//! prefix's `system32` (the build itself is never changed), points
//! `HKLM\Software\NVIDIA Corporation\Global\NGXCore\FullPath` at that folder
//! (where games look for NGX), prefers those copies over Wine's own and sets
//! the build's switch (`D3DM_ENABLE_METALFX=1` or `DXMT_ENABLE_NVEXT=1`).
//! What it wrote is recorded in the prefix first (`.irori-metalfx.json`), so
//! turning it off, or a build that can't, puts back exactly that: files
//! still as written are removed and a file they replaced comes back.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use irori_core::{AppSettings, GameSettings, OpContext, WineInstallation, json_store};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use crate::env::EnvMap;
use crate::prefix::{self, WineCommand, WineContext};

const NGX_KEY: &str = r"HKLM\Software\NVIDIA Corporation\Global\NGXCore";
const NGX_PATH: &str = r"C:\Windows\System32";
const RECORD: &str = ".irori-metalfx.json";
/// Suffix of a prefix file a copy replaced, kept beside it.
const KEPT: &str = ".irori-metalfx-original";

/// Which translation a Wine build ships, with the DLLs it needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Translation {
    /// D3DMetal 3 (Game Porting Toolkit 3, CrossOver).
    D3dMetal { ngx: PathBuf, nvapi: PathBuf },
    /// DXMT's NVIDIA extension.
    Dxmt { ngx: PathBuf, nvapi: PathBuf },
}

impl Translation {
    /// The variable that switches the translation on.
    pub fn switch(&self) -> &'static str {
        match self {
            Translation::D3dMetal { .. } => "D3DM_ENABLE_METALFX",
            Translation::Dxmt { .. } => "DXMT_ENABLE_NVEXT",
        }
    }

    /// The prefix's `system32` names and the build's files they are copied
    /// from. NGX's loader asks for `_nvngx.dll`, games for `nvngx.dll`.
    fn copies(&self) -> [(&'static str, &Path); 3] {
        let (ngx, nvapi) = match self {
            Translation::D3dMetal { ngx, nvapi } | Translation::Dxmt { ngx, nvapi } => (ngx, nvapi),
        };
        [
            ("nvngx.dll", ngx),
            ("_nvngx.dll", ngx),
            ("nvapi64.dll", nvapi),
        ]
    }
}

/// Folders of a Wine build where its 64-bit Windows DLLs and add-ons live,
/// from the build's `bin/wine`.
pub fn build_dirs(wine: &WineInstallation) -> Vec<PathBuf> {
    let Some(root) = Path::new(&wine.bin).parent().and_then(Path::parent) else {
        return Vec::new();
    };
    [
        "lib/wine/x86_64-windows",
        "lib64/wine/x86_64-windows",
        "lib/external",
        "lib/dxmt/x86_64-windows",
    ]
    .iter()
    .map(|d| root.join(d))
    .collect()
}

/// The translation the files in `dirs` provide, D3DMetal's first.
pub fn find_translation(dirs: &[PathBuf]) -> Option<Translation> {
    let file = |name: &str| dirs.iter().map(|d| d.join(name)).find(|p| p.is_file());
    if let (Some(ngx), Some(nvapi)) = (file("nvngx-on-metalfx.dll"), file("nvapi64.dll")) {
        return Some(Translation::D3dMetal { ngx, nvapi });
    }
    let dxmt = dirs.iter().find(|d| d.join("winemetal.dll").is_file())?;
    let (ngx, nvapi) = (dxmt.join("nvngx.dll"), dxmt.join("nvapi64.dll"));
    (ngx.is_file() && nvapi.is_file()).then_some(Translation::Dxmt { ngx, nvapi })
}

/// The translation `wine` ships, if any (blocking: looks at files).
pub fn translation(wine: &WineInstallation) -> Option<Translation> {
    find_translation(&build_dirs(wine))
}

/// What turning it on wrote into a prefix.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Record {
    /// `system32` files written, with the SHA-256 of what was written.
    pub files: Vec<Written>,
    /// Whether the NGX registry value was set by Irori.
    pub registry: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Written {
    pub name: String,
    pub sha256: String,
    /// A file that was there before was kept as `<name>.irori-metalfx-original`.
    pub kept: bool,
}

fn sha256(path: &Path) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    Some(hex::encode(Sha256::digest(&bytes)))
}

fn system32(prefix: &Path) -> PathBuf {
    prefix.join("drive_c/windows/system32")
}

fn record_path(prefix: &Path) -> PathBuf {
    prefix.join(RECORD)
}

/// Copy the translation's DLLs into the prefix, recording each before it is
/// written. Blocking. Returns the record as it now is.
pub fn install_files(prefix: &Path, translation: &Translation) -> Result<Record> {
    let dir = system32(prefix);
    anyhow::ensure!(dir.is_dir(), "The prefix has no system32 folder yet");
    let mut record: Record = json_store::read(&record_path(prefix))?.unwrap_or_default();
    for (name, source) in translation.copies() {
        let wanted =
            sha256(source).with_context(|| format!("Could not read {}", source.display()))?;
        let target = dir.join(name);
        if sha256(&target).as_deref() == Some(wanted.as_str()) {
            continue;
        }
        let ours = record.files.iter().position(|w| w.name == name);
        let kept = match ours {
            Some(i) => record.files[i].kept,
            // Not ours: keep what is there, so turning it off brings it back.
            None if target.exists() => {
                std::fs::rename(&target, dir.join(format!("{name}{KEPT}")))
                    .with_context(|| format!("Could not set aside {}", target.display()))?;
                true
            }
            None => false,
        };
        let entry = Written {
            name: name.to_string(),
            sha256: wanted,
            kept,
        };
        match ours {
            Some(i) => record.files[i] = entry,
            None => record.files.push(entry),
        }
        json_store::write(&record_path(prefix), &record)?;
        std::fs::copy(source, &target)
            .with_context(|| format!("Could not copy {} into the prefix", source.display()))?;
    }
    Ok(record)
}

/// Take out what [`install_files`] wrote: files still as written go (a file
/// they replaced comes back), changed ones stay. Blocking. Returns the
/// record as it was, for the registry value, or `None` when there was none.
pub fn remove_files(prefix: &Path) -> Result<Option<Record>> {
    let Some(record) = json_store::read::<Record>(&record_path(prefix))? else {
        return Ok(None);
    };
    let dir = system32(prefix);
    for written in &record.files {
        if written.name.contains(['/', '\\']) || written.name.starts_with('.') {
            continue;
        }
        let target = dir.join(&written.name);
        if sha256(&target).as_deref() == Some(written.sha256.as_str()) {
            std::fs::remove_file(&target)
                .with_context(|| format!("Could not remove {}", target.display()))?;
        }
        let kept = dir.join(format!("{}{KEPT}", written.name));
        if written.kept && kept.is_file() && !target.exists() {
            std::fs::rename(&kept, &target)
                .with_context(|| format!("Could not put back {}", target.display()))?;
        }
    }
    Ok(Some(record))
}

/// Whether `system.reg`'s text has NGX's `FullPath`.
pub fn ngx_path_in(system_reg: &str) -> bool {
    let mut in_section = false;
    for line in system_reg.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_section = line.split(']').next().is_some_and(|h| {
                h.eq_ignore_ascii_case(r"[Software\\NVIDIA Corporation\\Global\\NGXCore")
            });
            continue;
        }
        if in_section && line.starts_with("\"FullPath\"=") {
            return true;
        }
    }
    false
}

async fn set_registry(
    app: &AppSettings,
    settings: &GameSettings,
    wc: &WineContext,
    on: bool,
    ctx: &OpContext,
) -> Result<()> {
    let argv: Vec<&str> = if on {
        vec![
            "reg", "add", NGX_KEY, "/v", "FullPath", "/t", "REG_SZ", "/d", NGX_PATH, "/f",
        ]
    } else {
        vec!["reg", "delete", NGX_KEY, "/v", "FullPath", "/f"]
    };
    let mut cmd = WineCommand::new(argv);
    cmd.wait = true;
    cmd.skip_prefix_check = true;
    prefix::run_wine(app, settings, wc, cmd, ctx)
        .await?
        .check()?;
    Ok(())
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> Result<T> + Send + 'static) -> Result<T> {
    tokio::task::spawn_blocking(f)
        .await
        .context("The DLSS setup stopped")?
}

/// What [`apply`] left the launch to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Applied {
    /// The build's switch, when DLSS goes through MetalFX this session.
    pub switch: Option<&'static str>,
    /// Why it doesn't although the game asks for it.
    pub unavailable: Option<&'static str>,
}

impl Applied {
    /// Add the switch and the DLL preferences to a launch's variables, after
    /// the user's own `WINEDLLOVERRIDES` (only these DLLs' entries change).
    pub fn add_to(&self, vars: &mut EnvMap) {
        let Some(switch) = self.switch else {
            return;
        };
        vars.insert(switch.into(), "1".into());
        let key = vars
            .keys()
            .find(|k| k.eq_ignore_ascii_case("WINEDLLOVERRIDES"))
            .cloned()
            .unwrap_or_else(|| "WINEDLLOVERRIDES".to_string());
        let value = crate::env::add_dll_overrides(
            vars.get(&key).map(String::as_str),
            &[("nvngx", "n"), ("_nvngx", "n"), ("nvapi64", "n")],
        );
        vars.insert(key, value);
    }
}

/// Why DLSS doesn't go through MetalFX with a build that lacks it.
const UNAVAILABLE: &str = "This Wine build can't translate DLSS to MetalFX. Use Game Porting Toolkit 3, a recent CrossOver or a DXMT build for it.";

/// What [`apply`] would leave the launch to do with `found`, the build's
/// translation, when the game asks for it (`enabled`).
fn planned_with(found: Option<&Translation>, enabled: bool) -> Applied {
    match found.filter(|_| enabled) {
        Some(found) => Applied {
            switch: Some(found.switch()),
            unavailable: None,
        },
        None => Applied {
            switch: None,
            unavailable: enabled.then_some(UNAVAILABLE),
        },
    }
}

/// What [`apply`] would leave the launch to do for `wine`, without touching
/// the prefix (a launch's preview). Blocking: looks in the build's folders.
pub fn planned(wine: &WineInstallation, enabled: bool) -> Applied {
    let found = enabled.then(|| translation(wine)).flatten();
    planned_with(found.as_ref(), enabled)
}

/// Bring the prefix in line with the game's DLSS-through-MetalFX setting.
/// A build without the translation takes it back out, and says why when the
/// game asks for it. macOS only.
pub async fn apply(
    app: &AppSettings,
    settings: &GameSettings,
    wc: &WineContext,
    enabled: bool,
    ctx: &OpContext,
) -> Result<Applied> {
    if !cfg!(target_os = "macos") {
        return Ok(Applied::default());
    }
    let prefix = wc.prefix.clone();
    let wine = wc.wine.clone();
    let found = if enabled {
        tokio::task::spawn_blocking(move || translation(&wine))
            .await
            .ok()
            .flatten()
    } else {
        None
    };
    let registry_set = tokio::fs::read_to_string(prefix.join("system.reg"))
        .await
        .is_ok_and(|text| ngx_path_in(&text));
    let Some(found) = found else {
        let removed = blocking({
            let prefix = prefix.clone();
            move || remove_files(&prefix)
        })
        .await?;
        if let Some(record) = removed {
            if record.registry && registry_set {
                set_registry(app, settings, wc, false, ctx).await?;
            }
            let _ = tokio::fs::remove_file(record_path(&prefix)).await;
        }
        return Ok(planned_with(None, enabled));
    };
    let mut record = blocking({
        let (prefix, found) = (prefix.clone(), found.clone());
        move || install_files(&prefix, &found)
    })
    .await?;
    if !registry_set {
        record.registry = true;
        json_store::write(&record_path(&prefix), &record)?;
        set_registry(app, settings, wc, true, ctx).await?;
    }
    Ok(planned_with(Some(&found), true))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn touch(path: &Path, bytes: &[u8]) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }

    #[test]
    fn finds_what_the_build_ships() {
        let dir = tempfile::tempdir().unwrap();
        let wine = WineInstallation {
            bin: dir.path().join("bin/wine64").to_string_lossy().into_owned(),
            ..Default::default()
        };
        let dirs = build_dirs(&wine);
        assert_eq!(find_translation(&dirs), None);

        // DXMT without its NVIDIA extension DLLs can't.
        let dxmt = dir.path().join("lib/wine/x86_64-windows");
        touch(&dxmt.join("winemetal.dll"), b"dxmt");
        assert_eq!(find_translation(&dirs), None);
        touch(&dxmt.join("nvngx.dll"), b"ngx");
        touch(&dxmt.join("nvapi64.dll"), b"nvapi");
        assert!(matches!(
            find_translation(&dirs),
            Some(Translation::Dxmt { .. })
        ));

        // D3DMetal's file wins.
        touch(
            &dir.path().join("lib/external/nvngx-on-metalfx.dll"),
            b"d3dm",
        );
        let found = find_translation(&dirs).unwrap();
        assert_eq!(found.switch(), "D3DM_ENABLE_METALFX");
        assert!(matches!(found, Translation::D3dMetal { .. }));
    }

    #[test]
    fn installs_and_takes_back_exactly_what_it_wrote() {
        let dir = tempfile::tempdir().unwrap();
        let build = dir.path().join("build");
        touch(&build.join("nvngx-on-metalfx.dll"), b"metalfx ngx");
        touch(&build.join("nvapi64.dll"), b"nvapi 3");
        let prefix = dir.path().join("pfx");
        let sys = system32(&prefix);
        // Wine's own placeholder is there before.
        touch(&sys.join("nvapi64.dll"), b"wine placeholder");
        let found = find_translation(std::slice::from_ref(&build)).unwrap();

        let record = install_files(&prefix, &found).unwrap();
        assert_eq!(
            std::fs::read(sys.join("nvngx.dll")).unwrap(),
            b"metalfx ngx"
        );
        assert_eq!(
            std::fs::read(sys.join("_nvngx.dll")).unwrap(),
            b"metalfx ngx"
        );
        assert_eq!(std::fs::read(sys.join("nvapi64.dll")).unwrap(), b"nvapi 3");
        assert_eq!(record.files.len(), 3);
        assert!(
            record
                .files
                .iter()
                .find(|w| w.name == "nvapi64.dll")
                .unwrap()
                .kept
        );
        // Again: nothing to do, the placeholder stays set aside once.
        assert_eq!(install_files(&prefix, &found).unwrap(), record);

        // The user replaced one file since: it stays.
        touch(&sys.join("_nvngx.dll"), b"the user's own");
        let removed = remove_files(&prefix).unwrap().unwrap();
        assert_eq!(removed.files.len(), 3);
        assert!(!sys.join("nvngx.dll").exists());
        assert_eq!(
            std::fs::read(sys.join("_nvngx.dll")).unwrap(),
            b"the user's own"
        );
        assert_eq!(
            std::fs::read(sys.join("nvapi64.dll")).unwrap(),
            b"wine placeholder"
        );
        assert!(!sys.join(format!("nvapi64.dll{KEPT}")).exists());
    }

    #[test]
    fn switch_and_dll_preferences_join_the_users_overrides() {
        let mut vars = EnvMap::new();
        vars.insert("WINEDLLOVERRIDES".into(), "dxgi=n,b;nvapi64=b".into());
        Applied::default().add_to(&mut vars);
        assert_eq!(vars.len(), 1);
        Applied {
            switch: Some("DXMT_ENABLE_NVEXT"),
            unavailable: None,
        }
        .add_to(&mut vars);
        assert_eq!(vars["DXMT_ENABLE_NVEXT"], "1");
        let overrides = &vars["WINEDLLOVERRIDES"];
        assert!(overrides.contains("dxgi=n,b"), "{overrides}");
        assert!(overrides.contains("nvapi64=n"), "{overrides}");
        assert!(!overrides.contains("nvapi64=b"), "{overrides}");
        assert!(overrides.contains("_nvngx=n"), "{overrides}");
    }

    #[test]
    fn nothing_to_remove_without_a_record() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(remove_files(dir.path()).unwrap(), None);
    }

    #[test]
    fn reads_the_ngx_path_from_system_reg() {
        let reg = "WINE REGISTRY Version 2\n\n[Software\\\\NVIDIA Corporation\\\\Global\\\\NGXCore] 1700000000\n#time=1da\n\"FullPath\"=\"C:\\\\Windows\\\\System32\"\n";
        assert!(ngx_path_in(reg));
        assert!(!ngx_path_in(
            "[Software\\\\NVIDIA Corporation\\\\Global] 1\n\"FullPath\"=\"x\"\n"
        ));
        assert!(!ngx_path_in(""));
    }
}
