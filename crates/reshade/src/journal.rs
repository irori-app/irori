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

use std::io::{BufRead as _, BufReader};
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use irori_core::archive_guard::{ensure_no_symlinks, relative_entry_path};
use irori_core::json_store;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use crate::layer::GraphicsMod;
use crate::optiscaler::OptiscalerConfig;
use crate::pe::Machine;
use crate::{Api, ReshadeError};

/// Buffer size for streaming file SHA-256 calculations (64 KiB).
const HASH_BUF_SIZE: usize = 64 * 1024;

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
    /// ReShade only: the build with full add-on support, which RenoDX
    /// needs. Left out for the standard build.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub addon: bool,
    /// Where an add-on came from: the RenoDX mod, or nothing for a file the
    /// user chose.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<ModSource>,
    /// OptiScaler only: the configuration used when installing OptiScaler.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub optiscaler: Option<OptiscalerConfig>,
}

/// The official mod an add-on install came from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModSource {
    /// The mod's id in its catalog (`1000xresist`).
    pub id: String,
    /// Its title, shown without asking the catalog again.
    pub title: String,
}

impl Manifest {
    /// The record of the switch file.
    pub fn switch_record(&self) -> Option<&FileRecord> {
        self.files.iter().find(|f| f.path == Path::new(&self.dll))
    }

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
    pub(crate) fn checked(&self) -> Result<()> {
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

/// Content source for a journaled file write.
#[derive(Debug, Clone)]
pub enum FileSource {
    /// Small in-memory payload (such as a patched INI file).
    Bytes(Vec<u8>),
    /// Staged file on disk, copied via `std::fs::copy` without loading into RAM.
    File(PathBuf),
}

/// A file to write through [`apply_staged`].
#[derive(Debug, Clone)]
pub struct StagedFile {
    pub path: PathBuf,
    pub source: FileSource,
    pub existing: Existing,
}

impl StagedFile {
    pub fn from_bytes(path: impl Into<PathBuf>, bytes: Vec<u8>, existing: Existing) -> Self {
        Self {
            path: path.into(),
            source: FileSource::Bytes(bytes),
            existing,
        }
    }

    pub fn from_file(
        path: impl Into<PathBuf>,
        src: impl Into<PathBuf>,
        existing: Existing,
    ) -> Self {
        Self {
            path: path.into(),
            source: FileSource::File(src.into()),
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

/// Compute lowercase hex SHA-256 and byte length of `path` using a 64 KiB
/// buffered reader without loading the whole file into memory.
pub fn sha256_file_with_size(path: &Path) -> Result<(String, u64)> {
    let file =
        std::fs::File::open(path).with_context(|| format!("Could not open {}", path.display()))?;
    let mut reader = BufReader::with_capacity(HASH_BUF_SIZE, file);
    let mut hasher = Sha256::new();
    let mut total = 0u64;
    loop {
        let chunk = reader
            .fill_buf()
            .with_context(|| format!("Could not read {}", path.display()))?;
        if chunk.is_empty() {
            break;
        }
        hasher.update(chunk);
        let n = chunk.len();
        total += n as u64;
        reader.consume(n);
    }
    Ok((hex::encode(hasher.finalize()), total))
}

fn sha256_file(path: &Path) -> Result<String> {
    sha256_file_with_size(path).map(|(hash, _)| hash)
}

/// Copy `src` to `dest` via a temporary sibling in `dest`'s folder, then
/// rename into place so `dest` is never partially written.
fn copy_file_atomic(src: &Path, dest: &Path) -> Result<()> {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let name = dest
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "file".to_string());
    let tmp = dest.with_file_name(format!(".{name}.irori-tmp-{}-{n}", std::process::id()));
    let res = (|| -> Result<()> {
        std::fs::copy(src, &tmp)
            .with_context(|| format!("Could not copy {} to {}", src.display(), dest.display()))?;
        std::fs::rename(&tmp, dest)
            .with_context(|| format!("Could not save {}", dest.display()))?;
        Ok(())
    })();
    if res.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    res
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

pub(crate) fn save_manifest(journal: &Path, manifest: &Manifest) -> Result<()> {
    save(journal, manifest)
}

pub(crate) fn copy_file_atomic_pub(src: &Path, dest: &Path) -> Result<()> {
    copy_file_atomic(src, dest)
}

pub(crate) fn sha256_bytes(bytes: &[u8]) -> String {
    sha256_of(bytes)
}

/// Update an existing installation in the same directory without rewriting
/// files whose SHA-256 and size already match on disk. Files the user modified
/// are preserved; obsolete tracked files that are still unmodified are removed.
/// Blocking.
pub fn update_staged(
    journal: &Path,
    previous: &Manifest,
    mut base: Manifest,
    files: Vec<StagedFile>,
    cancelled: &dyn Fn() -> bool,
) -> Result<Manifest> {
    previous.checked()?;
    let dir = base.dir.clone();
    if previous.dir != dir {
        bail!("update_staged requires the same install directory");
    }
    if !dir.is_dir() {
        bail!("{} does not exist", dir.display());
    }

    let mut prev_map: std::collections::HashMap<PathBuf, &FileRecord> =
        std::collections::HashMap::with_capacity(previous.files.len());
    for rec in &previous.files {
        prev_map.insert(rec.path.clone(), rec);
    }

    let mut to_write: Vec<(PathBuf, FileSource, Option<PathBuf>)> = Vec::new();
    let mut records: Vec<FileRecord> = Vec::new();
    let mut new_paths: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();

    for file in files {
        let rel = relative_entry_path(&file.path)
            .with_context(|| format!("unsafe path {}", file.path.display()))?;
        ensure_no_symlinks(&dir, &rel)?;
        let target = dir.join(&rel);
        let disabled = target.with_file_name(format!(
            "{}{DISABLED_SUFFIX}",
            rel.file_name().unwrap_or_default().to_string_lossy()
        ));
        let (new_sha, new_size) = match &file.source {
            FileSource::Bytes(bytes) => (sha256_of(bytes), bytes.len() as u64),
            FileSource::File(src) => sha256_file_with_size(src)?,
        };

        let prev_rec = prev_map.get(&rel).copied();
        let prev_is_disabled_switch =
            !previous.enabled && rel == Path::new(&previous.dll) && exists(&disabled);

        if exists(&target) {
            if let Some(prev) = prev_rec {
                let unmodified_prev = std::fs::symlink_metadata(&target)
                    .ok()
                    .is_some_and(|m| m.is_file() && m.len() == prev.size)
                    && sha256_file(&target).is_ok_and(|h| h == prev.sha256);
                if !unmodified_prev {
                    // User modified this file since Irori wrote it: leave untouched
                    // and keep the original baseline record so future updates and
                    // removal continue to treat it as user-modified.
                    records.push(prev.clone());
                    new_paths.insert(rel);
                    continue;
                }
                // On-disk contents match `prev`: skip rewriting when the new
                // file has the same size and SHA-256 (zero disk writes, zero
                // second hash pass).
                records.push(FileRecord {
                    path: rel.clone(),
                    sha256: new_sha.clone(),
                    size: new_size,
                });
                new_paths.insert(rel.clone());
                if prev.size != new_size || prev.sha256 != new_sha {
                    to_write.push((rel, file.source, None));
                }
            } else {
                match file.existing {
                    Existing::Refuse => bail!(ReshadeError::DllTaken {
                        name: rel.to_string_lossy().into_owned()
                    }),
                    Existing::Keep => continue,
                }
            }
        } else if prev_is_disabled_switch {
            let prev = prev_rec.expect("disabled switch exists in previous.files");
            let unmodified_prev = std::fs::symlink_metadata(&disabled)
                .ok()
                .is_some_and(|m| m.is_file() && m.len() == prev.size)
                && sha256_file(&disabled).is_ok_and(|h| h == prev.sha256);
            if unmodified_prev {
                records.push(FileRecord {
                    path: rel.clone(),
                    sha256: new_sha,
                    size: new_size,
                });
                new_paths.insert(rel.clone());
                to_write.push((rel, file.source, Some(disabled)));
            } else if file.existing == Existing::Refuse {
                bail!(ReshadeError::DllTaken {
                    name: rel.to_string_lossy().into_owned()
                });
            }
        } else if file.existing == Existing::Refuse && exists(&disabled) {
            bail!(ReshadeError::DllTaken {
                name: rel.to_string_lossy().into_owned()
            });
        } else {
            records.push(FileRecord {
                path: rel.clone(),
                sha256: new_sha,
                size: new_size,
            });
            new_paths.insert(rel.clone());
            to_write.push((rel, file.source, None));
        }
    }

    let mut created: Vec<PathBuf> = previous.created_dirs.clone();
    for (rel, _, _) in &to_write {
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

    for rel in &created {
        let full = dir.join(rel);
        if !exists(&full) {
            std::fs::create_dir(&full)
                .with_context(|| format!("Could not create {}", full.display()))?;
        }
    }
    for (rel, source, cleanup_disabled) in &to_write {
        if cancelled() {
            bail!(irori_runtime::Cancelled);
        }
        let target = dir.join(rel);
        match source {
            FileSource::Bytes(bytes) => json_store::write_bytes(&target, bytes)
                .with_context(|| format!("Could not write {}", target.display()))?,
            FileSource::File(src) => copy_file_atomic(src, &target)?,
        }
        if let Some(old_disabled) = cleanup_disabled {
            let _ = std::fs::remove_file(old_disabled);
        }
    }

    // Remove obsolete files from previous manifest that remain unmodified.
    for old in previous.files.iter().rev() {
        if new_paths.contains(&old.path) {
            continue;
        }
        let mut path = dir.join(&old.path);
        if old.path == Path::new(&previous.dll) && !previous.enabled {
            path = previous.dll_path();
        }
        let Ok(meta) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        let unchanged = meta.is_file()
            && meta.len() == old.size
            && sha256_file(&path).is_ok_and(|h| h == old.sha256);
        if unchanged {
            let _ = std::fs::remove_file(&path);
        }
    }

    let mut kept_dirs = Vec::new();
    for rel in created.iter().rev() {
        let still_used = records.iter().any(|r| r.path.starts_with(rel));
        if still_used {
            kept_dirs.push(rel.clone());
        } else {
            let _ = std::fs::remove_dir(dir.join(rel));
        }
    }
    kept_dirs.reverse();

    base.files = records;
    base.created_dirs = kept_dirs;
    base.complete = true;
    save(journal, &base)?;
    Ok(base)
}

/// Write `files` into `base.dir`, recording them in `base` and saving it at
/// `journal`. `cancelled` is checked between files. On any failure the
/// files written so far are removed again and the journal is deleted.
/// Blocking.
pub fn apply(
    journal: &Path,
    base: Manifest,
    files: Vec<Planned>,
    cancelled: &dyn Fn() -> bool,
) -> Result<Manifest> {
    let staged = files
        .into_iter()
        .map(|f| StagedFile::from_bytes(f.path, f.bytes, f.existing))
        .collect();
    apply_staged(journal, base, staged, cancelled)
}

/// Write `files` (in-memory bytes or staged files on disk) into `base.dir`,
/// recording their SHA-256 and size in `base` and saving it at `journal`.
/// Large files are hashed with a 64 KiB reader and copied with
/// `std::fs::copy` + atomic rename without allocating them in memory.
/// Blocking.
pub fn apply_staged(
    journal: &Path,
    mut base: Manifest,
    files: Vec<StagedFile>,
    cancelled: &dyn Fn() -> bool,
) -> Result<Manifest> {
    let dir = base.dir.clone();
    // The install folder is the game's (or one the compatibility list names
    // inside it): never created, so never removed.
    if !dir.is_dir() {
        bail!("{} does not exist", dir.display());
    }
    let mut planned = Vec::new();
    let mut records = Vec::new();
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
        let (sha256, size) = match &file.source {
            FileSource::Bytes(bytes) => (sha256_of(bytes), bytes.len() as u64),
            FileSource::File(src) => sha256_file_with_size(src)?,
        };
        records.push(FileRecord {
            path: rel.clone(),
            sha256,
            size,
        });
        planned.push((rel, file.source));
    }

    let mut created: Vec<PathBuf> = Vec::new();
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
    base.files = records;
    base.created_dirs = created;
    base.complete = false;
    save(journal, &base)?;

    let written = (|| -> Result<()> {
        for rel in &base.created_dirs {
            std::fs::create_dir(dir.join(rel))
                .with_context(|| format!("Could not create {}", dir.join(rel).display()))?;
        }
        for (rel, source) in &planned {
            if cancelled() {
                bail!(irori_runtime::Cancelled);
            }
            let target = dir.join(rel);
            match source {
                FileSource::Bytes(bytes) => json_store::write_bytes(&target, bytes)
                    .with_context(|| format!("Could not write {}", target.display()))?,
                FileSource::File(src) => copy_file_atomic(src, &target)?,
            }
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
            addon: false,
            source: None,
            optiscaler: None,
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
    fn an_addon_install_round_trips() {
        let game = tempfile::tempdir().unwrap();
        let mut m = base(game.path());
        m.kind = GraphicsMod::Renodx;
        m.dll = "renodx-game.addon64".into();
        m.overrides.clear();
        m.source = Some(ModSource {
            id: "game".into(),
            title: "Game".into(),
        });
        let json = serde_json::to_value(&m).unwrap();
        assert_eq!(json["kind"], "renodx");
        assert_eq!(json["source"]["id"], "game");
        assert!(json.get("addon").is_none());
        assert_eq!(serde_json::from_value::<Manifest>(json).unwrap(), m);

        let mut reshade = base(game.path());
        reshade.addon = true;
        let json = serde_json::to_value(&reshade).unwrap();
        assert_eq!(json["addon"], true);
        assert!(json.get("kind").is_none());
        assert_eq!(serde_json::from_value::<Manifest>(json).unwrap(), reshade);
    }

    #[test]
    fn unknown_fields_of_a_newer_irori_are_ignored() {
        let mut v: serde_json::Value = serde_json::from_str(OLD_MANIFEST).unwrap();
        v["somethingNew"] = serde_json::json!({"a": 1});
        let m: Manifest = serde_json::from_value(v).unwrap();
        assert_eq!(m.kind, GraphicsMod::Reshade);
    }

    #[test]
    fn staged_files_copy_from_disk_and_round_trip_optiscaler_manifest() {
        use crate::optiscaler::{GpuDriverMode, OptiscalerChannel, OptiscalerConfig, ProxyDll};

        let game = tempfile::tempdir().unwrap();
        let stage = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let journal = state.path().join("optiscaler/legendary/Game.json");

        let dll_src = stage.path().join("OptiScaler.dll");
        std::fs::write(&dll_src, b"optiscaler-binary-payload").unwrap();

        let mut m = base(game.path());
        m.kind = GraphicsMod::Optiscaler;
        m.version = "v0.7.7".into();
        m.dll = "winmm.dll".into();
        m.overrides = vec!["winmm".into(), "nvapi64".into()];
        m.optiscaler = Some(OptiscalerConfig {
            channel: OptiscalerChannel::Nightly,
            gpu_driver: GpuDriverMode::MesaAuto,
            proxy_dll: ProxyDll::Winmm,
            enable_optipatcher: true,
        });

        let files = vec![
            StagedFile::from_file("winmm.dll", &dll_src, Existing::Refuse),
            StagedFile::from_bytes("OptiScaler.ini", b"[Plugins]\n".to_vec(), Existing::Keep),
        ];
        let installed = apply_staged(&journal, m, files, &|| false).unwrap();
        assert!(installed.complete);
        assert_eq!(
            std::fs::read(game.path().join("winmm.dll")).unwrap(),
            b"optiscaler-binary-payload"
        );
        let reloaded = load(&journal).unwrap().unwrap();
        assert_eq!(reloaded, installed);
        assert_eq!(
            reloaded.optiscaler.as_ref().unwrap().proxy_dll,
            ProxyDll::Winmm
        );
    }
}
