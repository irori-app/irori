//! Safe extraction and removal for locally stored eXo `.zip` game archives.
//!
//! Operates strictly on local archives already present in the user's eXo
//! collection folder. Every archive entry is validated with
//! [`irori_core::archive_guard`] and every destructive cleanup goes through
//! [`irori_core::fs_guard::ensure_deletable`].

use std::fs::File;
use std::io::{Read as _, Write as _};
use std::path::{Component, Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use irori_core::{AppSettings, InstallProgress, OpContext, Outcome, archive_guard, fs_guard};

use super::ExoEntryMeta;

/// Maximum ZIP entries accepted for a single game archive.
const MAX_ZIP_ENTRIES: usize = 100_000;

/// Read the uncompressed byte size of a local eXo `.zip` archive from its
/// central directory (no extraction performed).
pub fn archive_uncompressed_size(zip_path: &Path) -> Result<u64> {
    let file = File::open(zip_path)
        .with_context(|| format!("opening eXo archive {}", zip_path.display()))?;
    let mut archive = zip::ZipArchive::new(file)
        .with_context(|| format!("{} is not a valid zip archive", zip_path.display()))?;
    if archive.len() > MAX_ZIP_ENTRIES {
        bail!(
            "{} holds more than {} entries",
            zip_path.display(),
            MAX_ZIP_ENTRIES
        );
    }
    let mut total = 0u64;
    for i in 0..archive.len() {
        let entry = archive
            .by_index_raw(i)
            .with_context(|| format!("reading entry {i} of {}", zip_path.display()))?;
        total = total.saturating_add(entry.size());
    }
    Ok(total)
}

/// Extract `exo.archive_path` into `exo.game_dir` via an atomic sibling staging
/// folder, validating every path with [`archive_guard`] and honouring
/// [`OpContext::cancel`].
pub fn extract_local_archive(exo: &ExoEntryMeta, ctx: &OpContext) -> Result<Outcome> {
    let zip_path = exo
        .archive_path
        .as_ref()
        .context("This eXo game has no local archive to extract")?;
    if !zip_path.is_file() {
        bail!("Local eXo archive {} was not found", zip_path.display());
    }
    let parent = exo
        .game_dir
        .parent()
        .context("eXo game directory has no parent folder")?;
    std::fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;

    (ctx.step)("Extracting local eXo archive");
    let file = File::open(zip_path)
        .with_context(|| format!("opening eXo archive {}", zip_path.display()))?;
    let mut archive = zip::ZipArchive::new(file)
        .with_context(|| format!("{} is not a valid zip archive", zip_path.display()))?;
    if archive.len() > MAX_ZIP_ENTRIES {
        bail!(
            "{} holds more than {} entries",
            zip_path.display(),
            MAX_ZIP_ENTRIES
        );
    }

    // Detect whether every file inside the zip is wrapped under `<shortcode>/`
    // (standard eXoDOS / eXoWin3x archive layout) so we do not double-nest it.
    let strip_prefix = detect_shortcode_prefix(&mut archive, &exo.shortcode)?;
    let mut total_bytes = 0u64;
    for i in 0..archive.len() {
        if let Ok(raw) = archive.by_index_raw(i) {
            total_bytes = total_bytes.saturating_add(raw.size());
        }
    }

    let safe_code = irori_core::paths::sanitize(&exo.shortcode);
    let staging = parent.join(format!(".irori-exo-extract-{safe_code}"));
    if staging.exists() {
        std::fs::remove_dir_all(&staging)
            .with_context(|| format!("cleaning previous staging {}", staging.display()))?;
    }
    std::fs::create_dir_all(&staging)
        .with_context(|| format!("creating staging {}", staging.display()))?;

    let result = extract_into_staging(
        &mut archive,
        &staging,
        strip_prefix.as_deref(),
        total_bytes,
        ctx,
    );
    match result {
        Ok(Outcome::Done) => {
            if exo.game_dir.exists() {
                if let Err(e) = std::fs::remove_dir_all(&staging) {
                    tracing::warn!("cleaning staging {}: {e}", staging.display());
                }
                return Ok(Outcome::Done);
            }
            if let Err(e) = std::fs::rename(&staging, &exo.game_dir) {
                let _ = std::fs::remove_dir_all(&staging);
                return Err(e).with_context(|| {
                    format!("moving extracted game to {}", exo.game_dir.display())
                });
            }
            Ok(Outcome::Done)
        }
        Ok(other) => {
            if let Err(e) = std::fs::remove_dir_all(&staging) {
                tracing::warn!("cleaning cancelled staging {}: {e}", staging.display());
            }
            Ok(other)
        }
        Err(e) => {
            if let Err(rm_err) = std::fs::remove_dir_all(&staging) {
                tracing::warn!("cleaning failed staging {}: {rm_err}", staging.display());
            }
            Err(e)
        }
    }
}

fn detect_shortcode_prefix(
    archive: &mut zip::ZipArchive<File>,
    shortcode: &str,
) -> Result<Option<String>> {
    let mut saw_entry = false;
    let mut matched_prefix: Option<String> = None;
    for i in 0..archive.len() {
        let entry = archive.by_index_raw(i)?;
        let entry_name = entry.name()?.into_owned();
        let normalized = entry_name.replace('\\', "/");
        let Some(rel) = archive_guard::relative_entry_path(Path::new(&normalized)) else {
            bail!("unsafe path in eXo archive: {entry_name}");
        };
        saw_entry = true;
        let first = rel
            .components()
            .next()
            .and_then(|c| match c {
                Component::Normal(n) => Some(n.to_string_lossy().into_owned()),
                _ => None,
            })
            .unwrap_or_default();
        if !first.eq_ignore_ascii_case(shortcode) {
            return Ok(None);
        }
        if matched_prefix.is_none() {
            matched_prefix = Some(first);
        }
    }
    Ok(saw_entry.then_some(matched_prefix).flatten())
}

fn extract_into_staging(
    archive: &mut zip::ZipArchive<File>,
    staging: &Path,
    strip_prefix: Option<&str>,
    total_bytes: u64,
    ctx: &OpContext,
) -> Result<Outcome> {
    let mut done_bytes = 0u64;
    let mut buf = [0u8; 64 * 1024];

    for i in 0..archive.len() {
        if ctx.cancel.is_cancelled() {
            return Ok(Outcome::Abort);
        }
        let mut entry = archive.by_index(i)?;
        let entry_name = entry.name()?.into_owned();
        if entry.is_symlink() {
            bail!("unsafe symbolic link in eXo archive: {entry_name}");
        }
        let normalized = entry_name.replace('\\', "/");
        let Some(mut rel) = archive_guard::relative_entry_path(Path::new(&normalized)) else {
            bail!("unsafe path in eXo archive: {entry_name}");
        };
        if let Some(prefix) = strip_prefix {
            rel = match strip_leading_ci(&rel, prefix) {
                Some(stripped) => stripped,
                None => continue,
            };
        }
        if rel.as_os_str().is_empty() {
            continue;
        }
        archive_guard::ensure_no_symlinks(staging, &rel)?;
        let dest = staging.join(&rel);

        if entry.is_dir() {
            std::fs::create_dir_all(&dest)
                .with_context(|| format!("creating {}", dest.display()))?;
            continue;
        }
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        let mut out =
            File::create(&dest).with_context(|| format!("creating {}", dest.display()))?;
        loop {
            if ctx.cancel.is_cancelled() {
                return Ok(Outcome::Abort);
            }
            let n = entry
                .read(&mut buf)
                .with_context(|| format!("reading {entry_name} from eXo archive"))?;
            if n == 0 {
                break;
            }
            out.write_all(&buf[..n])
                .with_context(|| format!("writing {}", dest.display()))?;
            done_bytes = done_bytes.saturating_add(n as u64);
        }
        let percent = if total_bytes > 0 {
            Some(((done_bytes as f64 / total_bytes as f64) * 100.0).min(100.0) as f32)
        } else {
            Some(100.0)
        };
        (ctx.progress)(InstallProgress {
            percent,
            bytes_done: Some(done_bytes),
            bytes_total: Some(total_bytes),
            ..Default::default()
        });
    }
    Ok(Outcome::Done)
}

fn strip_leading_ci(rel: &Path, prefix: &str) -> Option<PathBuf> {
    let mut comps = rel.components();
    let first = comps.next()?;
    let Component::Normal(name) = first else {
        return None;
    };
    if !name.to_string_lossy().eq_ignore_ascii_case(prefix) {
        return Some(rel.to_path_buf());
    }
    let rest: PathBuf = comps.collect();
    (!rest.as_os_str().is_empty()).then_some(rest)
}

/// Delete ONLY the extracted game directory `exo.game_dir`, preserving the local
/// `.zip` archive and `!dos`/`!win3x`/`!ScummVM` config folder.
pub fn remove_extracted_game(exo: &ExoEntryMeta, app: &AppSettings) -> Result<()> {
    if exo.archive_path.is_none() {
        bail!("Refusing to delete an eXo game that has no local archive backup");
    }
    if !exo.game_dir.exists() {
        return Ok(());
    }
    let root_canon = std::fs::canonicalize(&exo.exo_root)
        .with_context(|| format!("resolving {}", exo.exo_root.display()))?;
    let game_canon = std::fs::canonicalize(&exo.game_dir)
        .with_context(|| format!("resolving {}", exo.game_dir.display()))?;
    if !game_canon.starts_with(&root_canon) || game_canon == root_canon {
        bail!(
            "Refusing to delete {} outside eXo collection root",
            exo.game_dir.display()
        );
    }
    fs_guard::ensure_deletable(&game_canon, app)?;
    std::fs::remove_dir_all(&game_canon)
        .with_context(|| format!("removing {}", game_canon.display()))?;
    Ok(())
}
