//! Applying mods to a game folder and taking them back out.
//!
//! [`plan`] decides which layer provides each file: later layers win, and
//! paths compare without letter case because the games are Windows games.
//! [`deploy_links`] then links each file into the game folder. Before it
//! touches anything it writes the full manifest, and every step can be
//! undone from the manifest alone, so [`undeploy`] restores the folder after
//! a crash at any point:
//!
//! - A game file a mod replaces is renamed to `<name>.irori-original` beside
//!   itself (same folder, so the rename is atomic and nothing is copied),
//!   and renamed back afterwards. Game files are never deleted.
//! - Folders the deployment created are removed again once empty.
//! - What the game wrote into mod files or created folders is kept in the
//!   library's `.written` folder and applied on top next time, the way an
//!   overlay keeps its upper folder.
//!
//! [`overlay`](crate::overlay) mounts are recorded in the same manifest.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use irori_core::{AppSettings, archive_guard, fs_guard, json_store};
use serde::{Deserialize, Serialize};

use crate::fsutil::{align_case, join_rel, modified_ns, move_file, same_file, walk_files};

/// Suffix of a game file set aside while a mod replaces it.
pub const BACKUP_SUFFIX: &str = ".irori-original";

/// One folder of files applied over the game folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layer {
    pub id: String,
    pub dir: PathBuf,
}

/// What a deployment did, written before it does it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Deployment {
    pub game_dir: PathBuf,
    pub applied: Applied,
    #[serde(default)]
    pub files: Vec<DeployedFile>,
    /// Folders the deployment created, parents first.
    #[serde(default)]
    pub dirs: Vec<String>,
    #[serde(default)]
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Applied {
    Links,
    /// An overlay mounted over `game_dir`, whose upper folder is `upper`.
    #[serde(rename_all = "camelCase")]
    Overlay {
        upper: PathBuf,
    },
}

/// One file put into the game folder.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeployedFile {
    /// Path in the game folder, `/`-separated.
    pub rel: String,
    /// The layer's file.
    pub source: PathBuf,
    /// How it was put there; `None` until done.
    #[serde(default)]
    pub how: Option<LinkKind>,
    /// A game file was in the way and set aside.
    #[serde(default)]
    pub backup: bool,
    /// Size and change time of a copy, to tell whether the game changed it.
    #[serde(default)]
    pub size: u64,
    #[serde(default)]
    pub modified_ns: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LinkKind {
    Hard,
    Symbolic,
    Copy,
}

/// Which layer wins each file: path in the game folder (as the game folder
/// spells it) → (layer index, path inside the layer).
pub fn plan(game_dir: &Path, layers: &[Layer]) -> Result<BTreeMap<String, (usize, String)>> {
    let mut by_key: BTreeMap<String, (String, usize, String)> = BTreeMap::new();
    for (ix, layer) in layers.iter().enumerate() {
        for (rel, _) in walk_files(&layer.dir)? {
            if archive_guard::relative_entry_path(Path::new(&rel)).is_none() {
                continue;
            }
            let key = rel.to_lowercase();
            let target = by_key
                .get(&key)
                .map(|(t, _, _)| t.clone())
                .unwrap_or_else(|| align_case(game_dir, &rel));
            by_key.insert(key, (target, ix, rel));
        }
    }
    Ok(by_key
        .into_values()
        .map(|(target, ix, rel)| (target, (ix, rel)))
        .collect())
}

/// Link the layers' files into `game_dir`, recording each step in
/// `manifest` first. On failure the manifest stays, and [`undeploy`] undoes
/// whatever was done.
pub fn deploy_links(game_dir: &Path, layers: &[Layer], manifest: &Path) -> Result<Deployment> {
    deploy_links_using(
        game_dir,
        layers,
        manifest,
        &[LinkKind::Hard, LinkKind::Symbolic, LinkKind::Copy],
    )
}

/// [`deploy_links`] trying only `kinds`, in order.
pub(crate) fn deploy_links_using(
    game_dir: &Path,
    layers: &[Layer],
    manifest: &Path,
    kinds: &[LinkKind],
) -> Result<Deployment> {
    if manifest.exists() {
        bail!("mods are already applied to this game");
    }
    if !game_dir.is_dir() {
        bail!("the game folder {} was not found", game_dir.display());
    }
    let mut deployment = Deployment {
        game_dir: game_dir.to_path_buf(),
        applied: Applied::Links,
        files: Vec::new(),
        dirs: Vec::new(),
        created_at: irori_core::util::now_rfc3339(),
    };
    let mut new_dirs: BTreeSet<String> = BTreeSet::new();
    for (rel, (ix, layer_rel)) in plan(game_dir, layers)? {
        let target = join_rel(game_dir, &rel);
        let backup = match std::fs::symlink_metadata(&target) {
            Ok(m) if m.is_dir() => {
                tracing::warn!("{rel} is a folder in the game; the mod's file is skipped");
                continue;
            }
            Ok(_) => true,
            Err(_) => false,
        };
        if backup && backup_path(&target).symlink_metadata().is_ok() {
            bail!(
                "{} is in the way; move it out of the game folder and try again",
                backup_path(&target).display()
            );
        }
        let parts: Vec<&str> = rel.split('/').collect();
        for depth in 1..parts.len() {
            let parent = parts[..depth].join("/");
            if join_rel(game_dir, &parent).symlink_metadata().is_err() {
                new_dirs.insert(parent);
            }
        }
        deployment.files.push(DeployedFile {
            rel,
            source: join_rel(&layers[ix].dir, &layer_rel),
            how: None,
            backup,
            size: 0,
            modified_ns: None,
        });
    }
    // Sorted paths put every parent before its children.
    deployment.dirs = new_dirs.into_iter().collect();
    json_store::write(manifest, &deployment)?;

    let result = link_all(game_dir, &mut deployment, kinds);
    json_store::write(manifest, &deployment)?;
    result.map(|()| deployment)
}

fn link_all(game_dir: &Path, deployment: &mut Deployment, kinds: &[LinkKind]) -> Result<()> {
    for dir in &deployment.dirs {
        let path = join_rel(game_dir, dir);
        std::fs::create_dir_all(&path).with_context(|| format!("creating {}", path.display()))?;
    }
    for file in &mut deployment.files {
        let target = join_rel(game_dir, &file.rel);
        if file.backup {
            std::fs::rename(&target, backup_path(&target))
                .with_context(|| format!("setting {} aside", target.display()))?;
        }
        let how = link_one(&file.source, &target, kinds)?;
        if how == LinkKind::Copy {
            let meta = std::fs::metadata(&target)?;
            file.size = meta.len();
            file.modified_ns = modified_ns(&meta);
        }
        file.how = Some(how);
    }
    Ok(())
}

/// A hard link when the mod and the game share a file system (the game
/// sees a plain file), else a symbolic link, else a copy.
fn link_one(source: &Path, target: &Path, kinds: &[LinkKind]) -> Result<LinkKind> {
    for kind in kinds {
        let done = match kind {
            LinkKind::Hard => std::fs::hard_link(source, target).is_ok(),
            #[cfg(unix)]
            LinkKind::Symbolic => std::os::unix::fs::symlink(source, target).is_ok(),
            // Windows allows symbolic links only to administrators and in
            // developer mode; a copy is the safe fallback.
            #[cfg(not(unix))]
            LinkKind::Symbolic => false,
            LinkKind::Copy => {
                std::fs::copy(source, target).with_context(|| {
                    format!("copying {} to {}", source.display(), target.display())
                })?;
                true
            }
        };
        if done {
            return Ok(*kind);
        }
    }
    bail!(
        "{} could not be linked into the game folder",
        target.display()
    )
}

pub(crate) fn backup_path(target: &Path) -> PathBuf {
    let mut name = target.file_name().unwrap_or_default().to_os_string();
    name.push(BACKUP_SUFFIX);
    target.with_file_name(name)
}

/// What [`undeploy`] did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Undeployed {
    /// Game files put back.
    pub restored: usize,
    /// Files the game wrote, kept in the written folder.
    pub kept: usize,
}

/// Take a links deployment back out of its game folder and remove its
/// manifest. Safe to run again after a failure; files the game wrote into
/// mod files or created folders move to `written`.
pub fn undeploy(manifest: &Path, written: &Path, app: &AppSettings) -> Result<Undeployed> {
    let Some(deployment) = json_store::read::<Deployment>(manifest)? else {
        return Ok(Undeployed::default());
    };
    if deployment.applied != Applied::Links {
        bail!("an overlay is unmounted, not undeployed");
    }
    let game_dir = &deployment.game_dir;
    let mut report = Undeployed::default();
    if game_dir.is_dir() {
        fs_guard::ensure_deletable(game_dir, app)?;
        report = remove_links(&deployment, written)?;
    }
    std::fs::remove_file(manifest).with_context(|| format!("removing {}", manifest.display()))?;
    Ok(report)
}

fn remove_links(deployment: &Deployment, written: &Path) -> Result<Undeployed> {
    let game_dir = &deployment.game_dir;
    let mut report = Undeployed::default();
    let ours: HashSet<String> = deployment
        .files
        .iter()
        .map(|f| f.rel.to_lowercase())
        .collect();
    for file in &deployment.files {
        if archive_guard::relative_entry_path(Path::new(&file.rel)).is_none() {
            continue;
        }
        let target = join_rel(game_dir, &file.rel);
        let backup = backup_path(&target);
        match state_of(file, &target) {
            State::Missing => {}
            State::Ours => remove(&target)?,
            State::Changed => {
                move_file(&target, &join_rel(written, &file.rel))?;
                report.kept += 1;
            }
            State::Original => continue,
        }
        if file.backup && backup.symlink_metadata().is_ok() {
            std::fs::rename(&backup, &target)
                .with_context(|| format!("restoring {}", target.display()))?;
            report.restored += 1;
        }
    }
    // Folders the deployment created held nothing of the game's: whatever
    // is left in them now, the game wrote while modded.
    for dir in deployment.dirs.iter() {
        let path = join_rel(game_dir, dir);
        for (rel, _) in walk_files(&path)? {
            let game_rel = format!("{dir}/{rel}");
            if ours.contains(&game_rel.to_lowercase()) {
                continue;
            }
            move_file(&join_rel(&path, &rel), &join_rel(written, &game_rel))?;
            report.kept += 1;
        }
    }
    for dir in deployment.dirs.iter().rev() {
        remove_empty_tree(&join_rel(game_dir, dir));
    }
    Ok(report)
}

enum State {
    Missing,
    /// The link or copy Irori put there.
    Ours,
    /// Irori's file, since replaced or changed by the game.
    Changed,
    /// The game file, not yet set aside when the deployment stopped.
    Original,
}

fn state_of(file: &DeployedFile, target: &Path) -> State {
    let Ok(meta) = std::fs::symlink_metadata(target) else {
        return State::Missing;
    };
    if meta.is_dir() {
        // The game made a folder of it; leave the game's work alone.
        return State::Original;
    }
    let source = std::fs::metadata(&file.source).ok();
    let is_link_to_source =
        meta.file_type().is_symlink() && std::fs::read_link(target).is_ok_and(|t| t == file.source);
    let is_hard_link =
        !meta.file_type().is_symlink() && source.as_ref().is_some_and(|s| same_file(&meta, s));
    match file.how {
        Some(LinkKind::Symbolic) if is_link_to_source => State::Ours,
        Some(LinkKind::Hard) if is_hard_link => State::Ours,
        Some(LinkKind::Copy)
            if meta.is_file()
                && meta.len() == file.size
                && modified_ns(&meta) == file.modified_ns =>
        {
            State::Ours
        }
        Some(_) if meta.file_type().is_symlink() && !is_link_to_source => State::Ours,
        Some(_) if meta.is_file() => State::Changed,
        Some(_) => State::Ours,
        // Stopped before this file was done: a set-aside game file means
        // whatever is here now is Irori's (perhaps half copied); otherwise a
        // game file that still sits here was never touched.
        None if file.backup && backup_path(target).symlink_metadata().is_err() => State::Original,
        None => State::Ours,
    }
}

fn remove(path: &Path) -> Result<()> {
    std::fs::remove_file(path).with_context(|| format!("removing {}", path.display()))
}

/// Remove `dir` and the folders in it, as long as they hold no files.
fn remove_empty_tree(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        if entry.file_type().is_ok_and(|t| t.is_dir()) {
            remove_empty_tree(&entry.path());
        }
    }
    let _ = std::fs::remove_dir(dir);
}

/// Whether a deployment is recorded at `manifest`.
pub fn is_deployed(manifest: &Path) -> bool {
    manifest.exists()
}

/// The recorded deployment, if any.
pub fn read(manifest: &Path) -> Result<Option<Deployment>> {
    json_store::read(manifest)
}
