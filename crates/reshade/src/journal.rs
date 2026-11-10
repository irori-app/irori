//! Installing a graphics mod into a game folder with a record of every
//! write. Every mod ([`GraphicsMod`]) keeps its own manifest.
//!
//! The manifest lists each file Irori writes (path relative to the install
//! folder, SHA-256, size) and each folder it creates, and is saved before
//! the first file is touched, so whatever happens next can be undone:
//!
//! - Files are written through [`json_store::write_bytes`] (a temporary
//!   sibling renamed into place), so each is either absent or complete.
//! - A file that already exists is never replaced: the mod's own file (the
//!   ReShade DLL) refuses the install ([`ReshadeError::DllTaken`]);
//!   `ReShade.ini`, `d3dcompiler_47.dll` and shaders the folder already has
//!   are left as they are and stay out of the manifest.
//! - [`set_enabled`] turns a mod off by renaming its one switch file
//!   ([`Manifest::dll`]) with [`DISABLED_SUFFIX`], and on by renaming it
//!   back.
//! - [`remove`] deletes only recorded files whose hash still matches, then
//!   the recorded folders that are empty. ReShade rewrites `ReShade.ini` and
//!   writes presets as the user plays, so those are kept.
//! - A failed or cancelled [`apply`] runs [`remove`] on what it wrote.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use irori_core::archive_guard::{ensure_no_symlinks, relative_entry_path};
use irori_core::json_store;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use crate::layer::GraphicsMod;
use crate::pe::Machine;
use crate::{Api, ReshadeError};

/// Suffix of the switch file while its mod is turned off.
pub const DISABLED_SUFFIX: &str = ".irori-disabled";

/// A file Irori wrote.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileRecord {
    /// Relative to [`Manifest::dir`].
    pub path: PathBuf,
    pub sha256: String,
    pub size: u64,
}

/// One game's install of one graphics mod.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Manifest {
    /// Which mod this is. ReShade's manifests leave it out, as they did
    /// before Irori knew other mods.
    #[serde(default, skip_serializing_if = "GraphicsMod::is_reshade")]
    pub kind: GraphicsMod,
    /// The mod's version (ReShade's `6.8.0`).
    pub version: String,
    /// The program the mod was installed for.
    pub exe: PathBuf,
    pub machine: Machine,
    pub api: Api,
    /// The switch file, in [`Self::dir`]: renamed to turn the mod off. For
    /// ReShade, the DLL it is installed as (`dxgi.dll`).
    pub dll: String,
    /// The install folder.
    pub dir: PathBuf,
    pub enabled: bool,
    /// False until every file is written; a crash leaves it false and the
    /// install can be removed.
    pub complete: bool,
    /// DLLs Wine must load from the game folder (`WINEDLLOVERRIDES`
    /// `<name>=n,b`), without `.dll`.
    #[serde(default)]
    pub overrides: Vec<String>,
    pub files: Vec<FileRecord>,
    /// Folders created, relative to [`Self::dir`], parents first.
    #[serde(default)]
    pub created_dirs: Vec<PathBuf>,
}

impl Manifest {
    /// Path of the switch file, under its current name.
    pub fn dll_path(&self) -> PathBuf {
        if self.enabled {
            self.dir.join(&self.dll)
        } else {
            self.dir.join(format!("{}{DISABLED_SUFFIX}", self.dll))
        }
    }

    /// The manifest's paths, refusing any that would leave the install
    /// folder (the file is Irori's own, but a hand edit must not widen what
    /// a removal reaches).
    fn checked(&self) -> Result<()> {
        if !self.dir.is_absolute() {
            bail!(
                "The {} install record names no game folder",
                self.kind.name()
            );
        }
        for p in self.files.iter().map(|f| &f.path).chain(&self.created_dirs) {
            if relative_entry_path(p).as_deref() != Some(p.as_path()) {
                bail!(
                    "The {} install record names {} outside the game folder",
                    self.kind.name(),
                    p.display()
                );
            }
        }
        if relative_entry_path(Path::new(&self.dll)).as_deref() != Some(Path::new(&self.dll)) {
            bail!(
                "The {} install record names an invalid file",
                self.kind.name()
            );
        }
        Ok(())
    }
}

/// What to do when a planned file already exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Existing {
    /// Refuse the whole install.
    Refuse,
    /// Leave the file and don't record it.
    Keep,
}

/// A file to write.
#[derive(Debug, Clone)]
pub struct Planned {
    pub path: PathBuf,
    pub bytes: Vec<u8>,
    pub existing: Existing,
}

impl Planned {
    pub fn new(path: impl Into<PathBuf>, bytes: Vec<u8>, existing: Existing) -> Self {
        Self {
            path: path.into(),
            bytes,
            existing,
        }
    }
}

/// What [`remove`] did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RemoveReport {
    pub removed: usize,
    /// Files Irori wrote that changed since, left in place.
    pub kept: Vec<PathBuf>,
}

fn sha256_of(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn sha256_file(path: &Path) -> Result<String> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    std::io::copy(&mut file, &mut hasher)?;
    Ok(hex::encode(hasher.finalize()))
}

fn exists(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok()
}

/// Read a game's manifest; `None` when the mod isn't installed.
pub fn load(journal: &Path) -> Result<Option<Manifest>> {
    let manifest: Option<Manifest> = json_store::read(journal)?;
    if let Some(m) = &manifest {
        m.checked()?;
    }
    Ok(manifest)
}

fn save(journal: &Path, manifest: &Manifest) -> Result<()> {
    json_store::write(journal, manifest)
        .with_context(|| format!("Could not save the {} install record", manifest.kind.name()))
}

/// Write `files` into `base.dir`, recording them in `base` and saving it at
/// `journal`. `cancelled` is checked between files. On any failure the
/// files written so far are removed again and the journal is deleted.
/// Blocking.
pub fn apply(
    journal: &Path,
    mut base: Manifest,
    files: Vec<Planned>,
    cancelled: &dyn Fn() -> bool,
) -> Result<Manifest> {
    let dir = base.dir.clone();
    let mut planned = Vec::new();
    for file in files {
        let rel = relative_entry_path(&file.path)
            .with_context(|| format!("unsafe path {}", file.path.display()))?;
        ensure_no_symlinks(&dir, &rel)?;
        let target = dir.join(&rel);
        let disabled = target.with_file_name(format!(
            "{}{DISABLED_SUFFIX}",
            rel.file_name().unwrap_or_default().to_string_lossy()
        ));
        if exists(&target) || (file.existing == Existing::Refuse && exists(&disabled)) {
            match file.existing {
                Existing::Refuse => bail!(ReshadeError::DllTaken {
                    name: rel.to_string_lossy().into_owned()
                }),
                Existing::Keep => continue,
            }
        }
        planned.push((rel, file.bytes));
    }

    let mut created: Vec<PathBuf> = Vec::new();
    // The install folder is the game's (or one the compatibility list names
    // inside it): never created, so never removed.
    if !dir.is_dir() {
        bail!("{} does not exist", dir.display());
    }
    for (rel, _) in &planned {
        let mut acc = PathBuf::new();
        if let Some(parent) = rel.parent() {
            for part in parent.components() {
                acc.push(part);
                if !exists(&dir.join(&acc)) && !created.contains(&acc) {
                    created.push(acc.clone());
                }
            }
        }
    }
    base.files = planned
        .iter()
        .map(|(rel, bytes)| FileRecord {
            path: rel.clone(),
            sha256: sha256_of(bytes),
            size: bytes.len() as u64,
        })
        .collect();
    base.created_dirs = created;
    base.complete = false;
    save(journal, &base)?;

    let written = (|| -> Result<()> {
        for rel in &base.created_dirs {
            std::fs::create_dir(dir.join(rel))
                .with_context(|| format!("Could not create {}", dir.join(rel).display()))?;
        }
        for (rel, bytes) in &planned {
            if cancelled() {
                bail!(irori_runtime::Cancelled);
            }
            json_store::write_bytes(&dir.join(rel), bytes)
                .with_context(|| format!("Could not write {}", dir.join(rel).display()))?;
        }
        Ok(())
    })();
    if let Err(e) = written {
        if let Err(undo) = remove_files(&base) {
            tracing::warn!(
                "undoing the {} install in {}: {undo:#}",
                base.kind.name(),
                dir.display()
            );
        }
        let _ = std::fs::remove_file(journal);
        return Err(e);
    }
    base.complete = true;
    save(journal, &base)?;
    Ok(base)
}

/// Rename the switch file to turn the mod on or off. Blocking.
pub fn set_enabled(journal: &Path, manifest: &mut Manifest, enabled: bool) -> Result<()> {
    manifest.checked()?;
    if manifest.enabled == enabled {
        return Ok(());
    }
    let from = manifest.dll_path();
    let mut next = manifest.clone();
    next.enabled = enabled;
    let to = next.dll_path();
    if exists(&to) {
        bail!(ReshadeError::DllTaken {
            name: to
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned()
        });
    }
    if !exists(&from) {
        bail!(
            "{}'s {} is no longer in the game folder",
            manifest.kind.name(),
            manifest.dll
        );
    }
    std::fs::rename(&from, &to).with_context(|| format!("Could not rename {}", from.display()))?;
    if let Err(e) = save(journal, &next) {
        let _ = std::fs::rename(&to, &from);
        return Err(e);
    }
    *manifest = next;
    Ok(())
}

fn remove_files(manifest: &Manifest) -> Result<RemoveReport> {
    manifest.checked()?;
    let mut report = RemoveReport::default();
    for file in manifest.files.iter().rev() {
        let mut path = manifest.dir.join(&file.path);
        if file.path == Path::new(&manifest.dll) && !manifest.enabled {
            path = manifest.dll_path();
        }
        let Ok(meta) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        let unchanged = meta.is_file()
            && meta.len() == file.size
            && sha256_file(&path).is_ok_and(|h| h == file.sha256);
        if unchanged {
            std::fs::remove_file(&path)
                .with_context(|| format!("Could not delete {}", path.display()))?;
            report.removed += 1;
        } else {
            report.kept.push(file.path.clone());
        }
    }
    for rel in manifest.created_dirs.iter().rev() {
        // Only empty folders go; anything the game or the user put there stays.
        let _ = std::fs::remove_dir(manifest.dir.join(rel));
    }
    report.kept.reverse();
    Ok(report)
}

/// Remove what Irori wrote and still matches the manifest, then the
/// journal. `guard` vets the install folder first
/// (`irori_core::fs_guard::ensure_deletable` in the app). Blocking.
pub fn remove(
    journal: &Path,
    manifest: &Manifest,
    guard: &dyn Fn(&Path) -> Result<()>,
) -> Result<RemoveReport> {
    guard(&manifest.dir)?;
    let report = remove_files(manifest)?;
    match std::fs::remove_file(journal) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => {
            return Err(e).with_context(|| {
                format!(
                    "Could not delete the {} install record",
                    manifest.kind.name()
                )
            });
        }
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base(dir: &Path) -> Manifest {
        Manifest {
            kind: GraphicsMod::Reshade,
            version: "6.8.0".into(),
            exe: dir.join("Game.exe"),
            machine: Machine::X64,
            api: Api::Dxgi,
            dll: "dxgi.dll".into(),
            dir: dir.to_path_buf(),
            enabled: true,
            complete: false,
            overrides: vec!["dxgi".into(), "d3dcompiler_47".into()],
            files: Vec::new(),
            created_dirs: Vec::new(),
        }
    }

    fn plan() -> Vec<Planned> {
        vec![
            Planned::new("dxgi.dll", b"reshade".to_vec(), Existing::Refuse),
            Planned::new("d3dcompiler_47.dll", b"compiler".to_vec(), Existing::Keep),
            Planned::new("ReShade.ini", b"[GENERAL]\r\n".to_vec(), Existing::Keep),
            Planned::new(
                "reshade-shaders/Shaders/Deband.fx",
                b"shader".to_vec(),
                Existing::Keep,
            ),
            Planned::new(
                "reshade-shaders/Textures/x.png",
                b"png".to_vec(),
                Existing::Keep,
            ),
        ]
    }

    fn allow(_: &Path) -> Result<()> {
        Ok(())
    }

    #[test]
    fn installs_disables_and_removes_keeping_changed_files() {
        let game = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let journal = state.path().join("legendary/Game.json");
        // The game ships its own compiler: left alone and unrecorded.
        std::fs::write(game.path().join("d3dcompiler_47.dll"), b"game's").unwrap();

        let mut m = apply(&journal, base(game.path()), plan(), &|| false).unwrap();
        assert!(m.complete);
        assert_eq!(m.files.len(), 4);
        assert_eq!(
            m.created_dirs,
            [
                PathBuf::from("reshade-shaders"),
                PathBuf::from("reshade-shaders/Shaders"),
                PathBuf::from("reshade-shaders/Textures"),
            ]
        );
        assert_eq!(load(&journal).unwrap().as_ref(), Some(&m));
        assert_eq!(
            std::fs::read(game.path().join("dxgi.dll")).unwrap(),
            b"reshade"
        );

        set_enabled(&journal, &mut m, false).unwrap();
        assert!(!game.path().join("dxgi.dll").exists());
        assert!(game.path().join("dxgi.dll.irori-disabled").exists());
        assert!(!load(&journal).unwrap().unwrap().enabled);

        // ReShade saved its settings and the user a preset and a shader.
        std::fs::write(game.path().join("ReShade.ini"), b"[GENERAL]\r\nchanged\r\n").unwrap();
        std::fs::write(game.path().join("ReShadePreset.ini"), b"preset").unwrap();
        std::fs::write(game.path().join("reshade-shaders/Shaders/Mine.fx"), b"mine").unwrap();

        let report = remove(&journal, &m, &allow).unwrap();
        assert_eq!(report.removed, 3);
        assert_eq!(report.kept, [PathBuf::from("ReShade.ini")]);
        assert!(!game.path().join("dxgi.dll.irori-disabled").exists());
        assert!(
            !game
                .path()
                .join("reshade-shaders/Shaders/Deband.fx")
                .exists()
        );
        assert!(!game.path().join("reshade-shaders/Textures").exists());
        assert!(game.path().join("reshade-shaders/Shaders/Mine.fx").exists());
        assert_eq!(
            std::fs::read(game.path().join("d3dcompiler_47.dll")).unwrap(),
            b"game's"
        );
        assert!(game.path().join("ReShade.ini").exists());
        assert!(game.path().join("ReShadePreset.ini").exists());
        assert!(load(&journal).unwrap().is_none());
    }

    #[test]
    fn refuses_a_dll_that_is_not_ours() {
        let game = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let journal = state.path().join("g.json");
        std::fs::write(game.path().join("dxgi.dll"), b"dxvk").unwrap();
        let err = apply(&journal, base(game.path()), plan(), &|| false).unwrap_err();
        assert!(matches!(
            ReshadeError::of(&err),
            Some(ReshadeError::DllTaken { .. })
        ));
        assert_eq!(
            std::fs::read(game.path().join("dxgi.dll")).unwrap(),
            b"dxvk"
        );
        assert!(!game.path().join("ReShade.ini").exists());
        assert!(!journal.exists());
    }

    #[test]
    fn a_cancelled_install_undoes_its_writes() {
        let game = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let journal = state.path().join("g.json");
        let calls = std::cell::Cell::new(0);
        let err = apply(&journal, base(game.path()), plan(), &|| {
            calls.set(calls.get() + 1);
            calls.get() > 3
        })
        .unwrap_err();
        assert!(irori_runtime::is_cancelled(&err));
        let left: Vec<_> = std::fs::read_dir(game.path()).unwrap().flatten().collect();
        assert!(left.is_empty(), "{left:?}");
        assert!(!journal.exists());
    }

    #[test]
    fn a_disabled_dll_cannot_be_shadowed() {
        let game = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let journal = state.path().join("g.json");
        let mut m = apply(&journal, base(game.path()), plan(), &|| false).unwrap();
        set_enabled(&journal, &mut m, false).unwrap();
        // Something else installed a dxgi.dll meanwhile.
        std::fs::write(game.path().join("dxgi.dll"), b"other").unwrap();
        assert!(set_enabled(&journal, &mut m, true).is_err());
        assert!(!m.enabled);
        let report = remove(&journal, &m, &allow).unwrap();
        assert_eq!(
            std::fs::read(game.path().join("dxgi.dll")).unwrap(),
            b"other"
        );
        assert!(!game.path().join("dxgi.dll.irori-disabled").exists());
        assert!(report.kept.is_empty());
    }

    #[test]
    fn records_never_reach_outside_the_folder() {
        let game = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let journal = state.path().join("g.json");
        let mut m = base(game.path());
        m.files.push(FileRecord {
            path: PathBuf::from("../outside.txt"),
            sha256: String::new(),
            size: 0,
        });
        json_store::write(&journal, &m).unwrap();
        assert!(load(&journal).is_err());
        assert!(remove(&journal, &m, &allow).is_err());
    }

    #[test]
    fn the_guard_is_asked_first() {
        let game = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let journal = state.path().join("g.json");
        let m = apply(&journal, base(game.path()), plan(), &|| false).unwrap();
        let refuse = |_: &Path| -> Result<()> { bail!("refused") };
        assert!(remove(&journal, &m, &refuse).is_err());
        assert!(game.path().join("dxgi.dll").exists());
        assert!(journal.exists());
    }

    /// A manifest Irori wrote before it knew other graphics mods.
    const OLD_MANIFEST: &str = r#"{
  "version": "6.7.3",
  "exe": "/games/Game/Game.exe",
  "machine": "x64",
  "api": "dxgi",
  "dll": "dxgi.dll",
  "dir": "/games/Game",
  "enabled": true,
  "complete": true,
  "overrides": ["dxgi", "d3dcompiler_47"],
  "files": [{"path": "dxgi.dll", "sha256": "ab", "size": 2}],
  "createdDirs": ["reshade-shaders"]
}"#;

    #[test]
    fn manifests_written_before_other_mods_read_as_reshade() {
        let m: Manifest = serde_json::from_str(OLD_MANIFEST).unwrap();
        assert_eq!(m.kind, GraphicsMod::Reshade);
        assert_eq!(m.version, "6.7.3");
        assert_eq!(m.dll, "dxgi.dll");
        assert_eq!(m.files.len(), 1);
        // Written back, a ReShade manifest keeps the shape it had.
        let back = serde_json::to_value(&m).unwrap();
        assert_eq!(
            back,
            serde_json::from_str::<serde_json::Value>(OLD_MANIFEST).unwrap()
        );
    }

    #[test]
    fn unknown_fields_of_a_newer_irori_are_ignored() {
        let mut v: serde_json::Value = serde_json::from_str(OLD_MANIFEST).unwrap();
        v["somethingNew"] = serde_json::json!({"a": 1});
        let m: Manifest = serde_json::from_value(v).unwrap();
        assert_eq!(m.kind, GraphicsMod::Reshade);
    }
}
