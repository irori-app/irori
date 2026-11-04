//! A game's mod folder on disk: `mods.json`, one folder per mod, the files
//! the game wrote while modded, and scratch space for installs.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use irori_core::{AppSettings, GameId, Paths, fs_guard, json_store, paths::sanitize};

use crate::deploy::Layer;
use crate::fsutil::{join_rel, move_file, walk_files};
use crate::model::{ModEntry, ModList, is_valid_id, new_id};

/// The mods of one game, under `<data>/mods/<runner>/<app>/`:
///
/// - `mods.json`: the [`ModList`].
/// - `<id>/files/`: a mod's files, laid out as they go into the game folder.
/// - `<id>/files.json`: the index of those files.
/// - `.written/`: files the game wrote into mod files and folders while
///   modded; applied on top of every mod (the overlay's upper folder).
/// - `.work/`: the overlay's work folder.
/// - `.staging/`: archives being unpacked.
/// - `deployment.json`: present while mods are applied.
#[derive(Debug, Clone)]
pub struct ModLibrary {
    root: PathBuf,
}

impl ModLibrary {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn for_game(id: &GameId) -> Self {
        Self::new(
            Self::base_dir()
                .join(id.runner.id())
                .join(sanitize(&id.app_name)),
        )
    }

    /// `<data>/mods`, holding every game's mod folder.
    pub fn base_dir() -> PathBuf {
        Paths::get().data.join("mods")
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn list_file(&self) -> PathBuf {
        self.root.join("mods.json")
    }

    pub fn manifest_file(&self) -> PathBuf {
        self.root.join("deployment.json")
    }

    /// Files the game wrote into mod files and folders.
    pub fn written_dir(&self) -> PathBuf {
        self.root.join(".written")
    }

    pub fn work_dir(&self) -> PathBuf {
        self.root.join(".work")
    }

    pub fn mod_dir(&self, id: &str) -> Result<PathBuf> {
        if !is_valid_id(id) {
            bail!("“{id}” is not a valid mod id");
        }
        Ok(self.root.join(id))
    }

    pub fn files_dir(&self, id: &str) -> Result<PathBuf> {
        Ok(self.mod_dir(id)?.join("files"))
    }

    /// The saved list, or an empty one.
    pub fn load(&self) -> Result<ModList> {
        let mut list: ModList = json_store::read(&self.list_file())?.unwrap_or_default();
        // A hand-edited file must not name folders outside this one.
        list.mods.retain(|m| is_valid_id(&m.id));
        Ok(list)
    }

    pub fn save(&self, list: &ModList) -> Result<()> {
        json_store::write(&self.list_file(), list)
    }

    /// The files of mod `id`, relative to the game folder.
    pub fn index(&self, id: &str) -> Result<Vec<String>> {
        let path = self.mod_dir(id)?.join("files.json");
        if let Some(index) = json_store::read::<Vec<String>>(&path)? {
            return Ok(index);
        }
        Ok(walk_files(&self.files_dir(id)?)?
            .into_iter()
            .map(|(rel, _)| rel)
            .collect())
    }

    /// A new, empty scratch folder for one install, removed when dropped.
    pub fn staging(&self) -> Result<Staging> {
        let dir = self
            .root
            .join(".staging")
            .join(uuid::Uuid::new_v4().simple().to_string());
        std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
        Ok(Staging { dir })
    }

    /// Add the files under `tree` (laid out as they go into the game folder)
    /// as a new mod at the end of the load order, or in place of `replaces`
    /// (an update keeps its place and whether it is enabled). `tree` is
    /// moved, so it should sit in this library's [`Staging`]. Saves the list
    /// and returns the new mod's id.
    pub fn add(
        &self,
        list: &mut ModList,
        tree: &Path,
        mut entry: ModEntry,
        replaces: Option<&str>,
        app: &AppSettings,
    ) -> Result<String> {
        let files = walk_files(tree)?;
        if files.is_empty() {
            bail!("the mod holds no files");
        }
        let id = new_id(&entry.name);
        let dir = self.mod_dir(&id)?;
        std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
        let target = dir.join("files");
        if std::fs::rename(tree, &target).is_err() {
            for (rel, _) in &files {
                move_file(&join_rel(tree, rel), &join_rel(&target, rel))?;
            }
        }
        let index: Vec<String> = files.iter().map(|(rel, _)| rel.clone()).collect();
        json_store::write(&dir.join("files.json"), &index)?;
        entry.id = id.clone();
        entry.files = files.len();
        entry.size = files.iter().map(|(_, size)| size).sum();
        if entry.installed_at.is_empty() {
            entry.installed_at = irori_core::util::now_rfc3339();
        }
        match replaces.and_then(|old| list.mods.iter().position(|m| m.id == old)) {
            Some(ix) => {
                entry.enabled = list.mods[ix].enabled;
                let old = std::mem::replace(&mut list.mods[ix], entry);
                self.save(list)?;
                self.remove_files(&old.id, app)?;
            }
            None => {
                list.mods.push(entry);
                self.save(list)?;
            }
        }
        Ok(id)
    }

    /// Remove mod `id` and its files. Saves the list.
    pub fn remove(&self, list: &mut ModList, id: &str, app: &AppSettings) -> Result<()> {
        list.mods.retain(|m| m.id != id);
        self.save(list)?;
        self.remove_files(id, app)
    }

    fn remove_files(&self, id: &str, app: &AppSettings) -> Result<()> {
        let dir = self.mod_dir(id)?;
        if !dir.exists() {
            return Ok(());
        }
        fs_guard::ensure_deletable(&dir, app)?;
        std::fs::remove_dir_all(&dir).with_context(|| format!("removing {}", dir.display()))
    }

    /// The enabled mods in load order, then the files the game wrote.
    pub fn layers(&self, list: &ModList) -> Result<Vec<Layer>> {
        let mut layers = Vec::new();
        for m in list.mods.iter().filter(|m| m.enabled) {
            layers.push(Layer {
                id: m.id.clone(),
                dir: self.files_dir(&m.id)?,
            });
        }
        layers.push(Layer {
            id: WRITTEN_LAYER.into(),
            dir: self.written_dir(),
        });
        Ok(layers)
    }

    /// How many files the game wrote into mod files and folders.
    pub fn written_files(&self) -> usize {
        walk_files(&self.written_dir()).map_or(0, |f| f.len())
    }
}

/// Id of the layer holding the files the game wrote.
pub const WRITTEN_LAYER: &str = ".written";

/// Scratch folder of one install; removed when dropped.
#[derive(Debug)]
pub struct Staging {
    dir: PathBuf,
}

impl Staging {
    pub fn path(&self) -> &Path {
        &self.dir
    }
}

impl Drop for Staging {
    fn drop(&mut self) {
        // Only this install's own fresh folder under `.staging`.
        if let Err(e) = std::fs::remove_dir_all(&self.dir)
            && e.kind() != std::io::ErrorKind::NotFound
        {
            tracing::warn!("removing {}: {e}", self.dir.display());
        }
    }
}
