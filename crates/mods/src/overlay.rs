//! Linux: the enabled mods as an overlay over the game folder while it runs.
//!
//! `fuse-overlayfs` mounts the mods (highest priority first) and the game
//! folder itself as read-only lower layers, and the library's `.written`
//! folder as the upper layer, right over the game folder. The game sees the
//! merged folder at its usual path; nothing in the game folder is written.
//!
//! Kernel overlayfs needs no tool, but an unprivileged user may mount it
//! only inside a user and mount namespace of its own, and only processes in
//! that namespace see it. The game would have to start inside it, together
//! with Steam's runtime container, Wine's server and every store client,
//! while being root of a remapped user: Proton and pressure-vessel break.
//! So the kernel route is not used; without `fuse-overlayfs`, mods are
//! linked in instead (see [`deploy`](crate::deploy)).
//!
//! When the overlay comes down, [`settle`] moves what the game wrote outside
//! the mods' own files and folders (saves, settings, logs) into the game
//! folder, where it would have gone without mods; writes to mod files stay
//! in the upper folder and apply next time.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context as _, Result, bail};
use irori_core::process::{self, CommandSpec, RunOptions};

use crate::deploy::Layer;
use crate::fsutil::{join_rel, move_file, walk_files};

/// The `fuse-overlayfs` program, when installed (Linux only).
pub fn tool() -> Option<PathBuf> {
    if cfg!(target_os = "linux") {
        which::which("fuse-overlayfs").ok()
    } else {
        None
    }
}

/// Whether `fuse-overlayfs` can take these folders: its option string
/// separates folders with `:` and options with `,`.
pub fn paths_supported(paths: &[&Path]) -> bool {
    paths.iter().all(|p| {
        p.to_str()
            .is_some_and(|s| !s.contains([':', ',', '\\', '\n']))
    })
}

/// The `-o` value for these layers (lowest priority first).
pub(crate) fn mount_options(
    game_dir: &Path,
    layers: &[Layer],
    upper: &Path,
    work: &Path,
) -> String {
    let mut lower: Vec<String> = layers
        .iter()
        .rev()
        .map(|l| l.dir.display().to_string())
        .collect();
    lower.push(game_dir.display().to_string());
    format!(
        "lowerdir={},upperdir={},workdir={}",
        lower.join(":"),
        upper.display(),
        work.display()
    )
}

/// Mount `layers` (lowest priority first, excluding the upper folder) over
/// `game_dir`.
pub async fn mount(
    tool: &Path,
    game_dir: &Path,
    layers: &[Layer],
    upper: &Path,
    work: &Path,
) -> Result<()> {
    if is_mounted(game_dir) {
        bail!("mods are already mounted over {}", game_dir.display());
    }
    tokio::fs::create_dir_all(upper).await?;
    tokio::fs::create_dir_all(work).await?;
    let mut all: Vec<&Path> = layers.iter().map(|l| l.dir.as_path()).collect();
    all.extend([game_dir, upper, work]);
    if !paths_supported(&all) {
        bail!("a folder name holds a character fuse-overlayfs can't take");
    }
    for layer in layers {
        tokio::fs::create_dir_all(&layer.dir).await?;
    }
    let spec = CommandSpec::new(tool)
        .arg("-o")
        .arg(mount_options(game_dir, layers, upper, work))
        .arg(game_dir.as_os_str());
    let result = process::run(&spec, RunOptions::default()).await?;
    if !result.success() || !is_mounted(game_dir) {
        bail!("fuse-overlayfs failed: {}", result.error_summary());
    }
    Ok(())
}

/// Unmount the overlay over `game_dir`. Waits a little for files the game
/// left open, then detaches it lazily: the game has exited by then.
pub async fn unmount(game_dir: &Path) -> Result<()> {
    if !is_mounted(game_dir) {
        return Ok(());
    }
    let tools: Vec<PathBuf> = ["fusermount3", "fusermount"]
        .iter()
        .filter_map(|t| which::which(t).ok())
        .collect();
    let Some(tool) = tools.first() else {
        bail!("fusermount was not found to unmount {}", game_dir.display());
    };
    for attempt in 0..6 {
        let lazy = attempt == 5;
        let spec = CommandSpec::new(tool)
            .arg(if lazy { "-uz" } else { "-u" })
            .arg(game_dir.as_os_str());
        let _ = process::run(&spec, RunOptions::default()).await;
        if !is_mounted(game_dir) {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    bail!("{} is still in use", game_dir.display())
}

/// Whether an overlay is mounted at `dir`.
pub fn is_mounted(dir: &Path) -> bool {
    let Ok(info) = std::fs::read_to_string("/proc/self/mountinfo") else {
        return false;
    };
    let dir = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
    mounted_in(&info, &dir)
}

/// Whether `mountinfo` lists a `fuse-overlayfs` mount at `dir`.
pub(crate) fn mounted_in(mountinfo: &str, dir: &Path) -> bool {
    mountinfo.lines().any(|line| {
        let mut halves = line.splitn(2, " - ");
        let (Some(left), Some(right)) = (halves.next(), halves.next()) else {
            return false;
        };
        let point = left.split(' ').nth(4).map(unescape_mount_path);
        let fstype = right.split(' ').next().unwrap_or("");
        fstype.contains("fuse-overlayfs") && point.is_some_and(|p| Path::new(&p) == dir)
    })
}

/// `mountinfo` writes spaces, tabs, newlines and backslashes as `\ooo`.
fn unescape_mount_path(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\'
            && i + 3 < bytes.len() + 1
            && let Some(code) = raw
                .get(i + 1..i + 4)
                .and_then(|o| u8::from_str_radix(o, 8).ok())
        {
            out.push(code);
            i += 4;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// After unmounting: move files the game wrote outside the mods' files and
/// folders from `upper` into `game_dir`, replacing the game's own copy as
/// the game meant to. `mod_files` are every enabled mod's paths (any case).
/// Returns how many files moved.
pub fn settle(game_dir: &Path, upper: &Path, mod_files: &HashSet<String>) -> Result<usize> {
    let mod_dirs: HashSet<String> = mod_files
        .iter()
        .flat_map(|f| {
            let parts: Vec<&str> = f.split('/').collect();
            (1..parts.len())
                .map(|d| parts[..d].join("/").to_lowercase())
                .collect::<Vec<_>>()
        })
        .collect();
    let mut moved = 0;
    for (rel, _) in walk_files(upper)? {
        let key = rel.to_lowercase();
        if mod_files.contains(&key) {
            continue;
        }
        // Inside a folder only a mod has: the game wrote into the mod.
        let parts: Vec<&str> = rel.split('/').collect();
        let in_mod_dir = (1..parts.len()).any(|depth| {
            let dir = parts[..depth].join("/");
            mod_dirs.contains(&dir.to_lowercase()) && !join_rel(game_dir, &dir).is_dir()
        });
        if in_mod_dir {
            continue;
        }
        let target = join_rel(game_dir, &rel);
        move_file(&join_rel(upper, &rel), &target)
            .with_context(|| format!("keeping {rel} in the game folder"))?;
        moved += 1;
    }
    drop_whiteouts(upper, mod_files);
    Ok(moved)
}

/// Remove the overlay's deletion markers for game files (character
/// devices, or `.wh.<name>` files when the user may not create devices): a
/// game deleting its own file must not hide it for good.
fn drop_whiteouts(upper: &Path, mod_files: &HashSet<String>) {
    fn walk(root: &Path, dir: &Path, mod_files: &HashSet<String>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            let hidden = if kind.is_dir() {
                walk(root, &path, mod_files);
                continue;
            } else if kind.is_file() {
                match name.strip_prefix(WHITEOUT_PREFIX) {
                    Some(target) if !target.starts_with(WHITEOUT_PREFIX) => {
                        path.with_file_name(target)
                    }
                    _ => continue,
                }
            } else if kind.is_symlink() {
                continue;
            } else {
                path.clone()
            };
            let rel = crate::fsutil::rel_string(hidden.strip_prefix(root).unwrap_or(&hidden));
            if !mod_files.contains(&rel.to_lowercase()) {
                let _ = std::fs::remove_file(&path);
            }
        }
    }
    walk(upper, upper, mod_files);
}

/// Name prefix of `fuse-overlayfs`'s deletion markers.
pub(crate) const WHITEOUT_PREFIX: &str = ".wh.";
