//! Post-download extraction, installer detection, and sideload game
//! registration with Steam cover art lookup.

use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context as _, Result, bail};
use irori_core::process::CancellationToken;
use irori_core::{Event, GameId, Runner};
use irori_engines::archive::{self as engine_archive, Kind as EngineKind};
use irori_runtime::setup::{self, Setup};

use super::model::{PostAction, SideloadBridge, SideloadCandidate, WebDownload, WebDownloadState};
use super::setup_cleanup::{
    canonical_key, maybe_trash_archive, move_dir_across_fs, unique_child_dir,
};
pub(crate) use super::setup_cleanup::{delete_download_files, run_setup_installer};
use crate::Backend;

/// Archive format recognized by post-processing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ArchiveFormat {
    Zip,
    SevenZip,
    Tar,
    Dmg,
    Rar,
}

/// Result of post-processing a completed web download.
#[derive(Debug, Clone)]
pub(crate) struct PostProcessOutcome {
    pub output_path: PathBuf,
    pub installed_dir: Option<PathBuf>,
    pub post_action: PostAction,
    pub status_note: Option<String>,
}

/// Classify a file path as an extractable archive format, if supported.
pub(crate) fn classify_archive(path: &Path) -> Option<ArchiveFormat> {
    if !path.is_file() {
        return None;
    }
    let name = path.file_name()?.to_string_lossy();
    let lower = name.to_ascii_lowercase();
    if lower.ends_with(".rar")
        || irori_mods::archive::kind(path) == Some(irori_mods::archive::Kind::Rar)
    {
        return Some(ArchiveFormat::Rar);
    }
    match engine_archive::classify(&name) {
        EngineKind::Zip => Some(ArchiveFormat::Zip),
        EngineKind::SevenZip => Some(ArchiveFormat::SevenZip),
        EngineKind::Tar => Some(ArchiveFormat::Tar),
        EngineKind::Dmg => Some(ArchiveFormat::Dmg),
        EngineKind::Plain => None,
    }
}

/// Strip archive extensions, version tags (`v1.38`), and platform/packaging tags
/// (`win64`, `portable`, `setup`, …) to obtain a clean game title suitable for
/// install folder naming and Steam cover art lookup.
pub fn clean_game_title(raw: &str) -> String {
    let mut name = raw.trim().to_string();
    let lower = name.to_ascii_lowercase();
    for ext in [
        ".tar.gz", ".tar.xz", ".tar.zst", ".tar.bz2", ".tgz", ".txz", ".tbz2", ".zip", ".7z",
        ".rar", ".dmg", ".exe", ".msi", ".torrent",
    ] {
        if lower.ends_with(ext) && name.len() > ext.len() {
            name.truncate(name.len() - ext.len());
            break;
        }
    }

    let stripped = strip_tag_groups(&name);
    let mut kept: Vec<String> = Vec::new();
    for token in split_preserving_versions(&stripped) {
        let trimmed = token.trim_matches(|c: char| matches!(c, '-' | '(' | ')' | '[' | ']'));
        if trimmed.is_empty() || is_release_tag(trimmed) {
            continue;
        }
        kept.push(trimmed.to_string());
    }

    let result = kept.join(" ").trim_matches(['-', ' ']).trim().to_string();
    if result.is_empty() {
        irori_core::fs_part::sanitize_download_name(raw)
    } else {
        result
    }
}

fn split_preserving_versions(input: &str) -> Vec<String> {
    let mut out = Vec::new();
    for raw in input.replace('_', " ").split_whitespace() {
        let trimmed = raw.trim_matches(|c: char| matches!(c, '-' | '(' | ')' | '[' | ']'));
        if trimmed.is_empty() {
            continue;
        }
        if !trimmed.contains('.') || is_release_tag(trimmed) {
            out.push(trimmed.to_string());
            continue;
        }
        let parts: Vec<&str> = trimmed.split('.').filter(|p| !p.is_empty()).collect();
        let mut i = 0;
        while i < parts.len() {
            let p = parts[i];
            let starts_v = p
                .strip_prefix(['v', 'V'])
                .is_some_and(|r| !r.is_empty() && r.chars().all(|c| c.is_ascii_digit()));
            let all_digits = !p.is_empty() && p.chars().all(|c| c.is_ascii_digit());
            if starts_v || all_digits {
                let mut merged = p.to_string();
                while i + 1 < parts.len()
                    && !parts[i + 1].is_empty()
                    && parts[i + 1].chars().all(|c| c.is_ascii_digit())
                {
                    i += 1;
                    merged.push('.');
                    merged.push_str(parts[i]);
                }
                out.push(merged);
            } else {
                out.push(p.to_string());
            }
            i += 1;
        }
    }
    out
}

fn strip_tag_groups(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let chars: Vec<char> = input.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let open = chars[i];
        let close = match open {
            '[' => Some(']'),
            '(' => Some(')'),
            _ => None,
        };
        if let Some(closing) = close
            && let Some(rel_end) = chars[i + 1..].iter().position(|&c| c == closing)
        {
            let inner: String = chars[i + 1..i + 1 + rel_end].iter().collect();
            let all_tags = inner
                .split(|c: char| c.is_whitespace() || matches!(c, '-' | '_' | '/' | ','))
                .filter(|t| !t.is_empty())
                .all(is_release_tag);
            if all_tags {
                out.push(' ');
                i += rel_end + 2;
                continue;
            }
        }
        out.push(open);
        i += 1;
    }
    out
}

fn is_release_tag(token: &str) -> bool {
    let lower = token.to_ascii_lowercase();
    if matches!(
        lower.as_str(),
        "win"
            | "win32"
            | "win64"
            | "windows"
            | "linux"
            | "linux32"
            | "linux64"
            | "mac"
            | "macos"
            | "osx"
            | "x64"
            | "x86"
            | "x86_64"
            | "amd64"
            | "arm64"
            | "aarch64"
            | "64bit"
            | "32bit"
            | "portable"
            | "setup"
            | "installer"
            | "install"
            | "gog"
            | "drm"
            | "free"
            | "drm-free"
            | "repack"
            | "final"
            | "hotfix"
            | "patch"
            | "build"
            | "release"
            | "edition"
    ) {
        return true;
    }
    if let Some(rest) = lower.strip_prefix('v')
        && rest.chars().next().is_some_and(|c| c.is_ascii_digit())
        && rest
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
    {
        return true;
    }
    if lower.contains('.')
        && lower.chars().next().is_some_and(|c| c.is_ascii_digit())
        && lower
            .chars()
            .all(|c| c.is_ascii_digit() || matches!(c, '.' | '-'))
    {
        return true;
    }
    false
}

/// Pre-check available disk space before unpacking `archive` into `dest_parent`.
pub(crate) fn check_extract_space(
    archive: &Path,
    format: ArchiveFormat,
    dest_parent: &Path,
) -> Result<()> {
    let needed = estimate_unpacked_bytes(archive, format);
    if needed == 0 {
        return Ok(());
    }
    let Some(existing) = dest_parent.ancestors().find(|p| p.is_dir()) else {
        return Ok(());
    };
    if let Ok(free) = fs4::available_space(existing)
        && free < needed
    {
        bail!(
            "Not enough free disk space in {} to extract ({} needed, {} available)",
            existing.display(),
            irori_core::util::format_bytes(needed),
            irori_core::util::format_bytes(free),
        );
    }
    Ok(())
}

fn estimate_unpacked_bytes(archive: &Path, format: ArchiveFormat) -> u64 {
    if format == ArchiveFormat::Zip
        && let Ok(file) = File::open(archive)
        && let Ok(mut zip) = zip::ZipArchive::new(BufReader::new(file))
    {
        let mut total: u64 = 0;
        for i in 0..zip.len() {
            if let Ok(entry) = zip.by_index_raw(i) {
                total = total.saturating_add(entry.size());
            }
        }
        if total > 0 {
            return total;
        }
    }
    std::fs::metadata(archive)
        .map_or(0, |m| m.len())
        .saturating_mul(2)
}

/// Whether `err` indicates an encrypted / password-protected archive.
pub(crate) fn is_password_error(err: &anyhow::Error) -> bool {
    if irori_mods::archive::is_password_required(err) {
        return true;
    }
    let msg = format!("{err:#}").to_ascii_lowercase();
    msg.contains("password") || msg.contains("encrypted")
}

/// Locate a Windows setup program (`Inno`, `NSIS`, or `MSI`) directly at `path`
/// or up to two folder levels inside `path`.
pub(crate) fn find_setup(path: &Path) -> Option<(PathBuf, Setup)> {
    if path.is_file() {
        let name = path.file_name()?.to_string_lossy();
        return setup::detect(path, &name).map(|kind| (path.to_path_buf(), kind));
    }
    if !path.is_dir() {
        return None;
    }
    fn walk(dir: &Path, depth: usize) -> Option<(PathBuf, Setup)> {
        let entries = std::fs::read_dir(dir).ok()?;
        let mut subdirs = Vec::new();
        for entry in entries.flatten() {
            let Ok(ft) = entry.file_type() else { continue };
            if ft.is_symlink() {
                continue;
            }
            let p = entry.path();
            if ft.is_file() {
                let name = entry.file_name().to_string_lossy().into_owned();
                let lower = name.to_ascii_lowercase();
                if (lower.ends_with(".exe") || lower.ends_with(".msi"))
                    && let Some(kind) = setup::detect(&p, &name)
                {
                    return Some((p, kind));
                }
            } else if ft.is_dir() && depth > 0 {
                subdirs.push(p);
            }
        }
        for sub in subdirs {
            if let Some(found) = walk(&sub, depth - 1) {
                return Some(found);
            }
        }
        None
    }
    walk(path, 2)
}

fn setup_kind_label(setup: Setup) -> String {
    match setup {
        Setup::Inno => "Inno Setup".into(),
        Setup::Nsis => "NSIS".into(),
        Setup::Msi => "MSI".into(),
    }
}

/// Run the full post-download pipeline (serial extraction, setup detection,
/// sideload scanning + Steam cover art, optional archive Trash).
pub(crate) async fn run_post_process(
    backend: &Backend,
    item: &WebDownload,
    output_path: PathBuf,
    cancel: &CancellationToken,
) -> Result<PostProcessOutcome> {
    let app = backend.settings.app();
    let install_root = irori_core::paths::expand_home(&app.default_install_path);
    let clean_title = clean_game_title(&item.title);
    let bridge = backend.sideload_bridge.read().clone();

    if item.auto_extract
        && let Some(format) = classify_archive(&output_path)
    {
        if format == ArchiveFormat::Dmg && !cfg!(target_os = "macos") {
            return Ok(PostProcessOutcome {
                output_path,
                installed_dir: None,
                post_action: PostAction::ShowInFolder,
                status_note: None,
            });
        }
        if format == ArchiveFormat::Rar && !irori_mods::archive::can_unpack_rar_async().await {
            return Ok(PostProcessOutcome {
                output_path,
                installed_dir: None,
                post_action: PostAction::ShowInFolder,
                status_note: Some("Requires manual extraction".into()),
            });
        }

        // Dedicated serial extraction queue (1 extraction at a time, separate from network slots).
        let _permit = tokio::select! {
            biased;
            () = cancel.cancelled() => bail!("cancelled"),
            permit = backend.web_extract_slots.clone().acquire_owned() => {
                permit.context("extraction queue closed")?
            }
        };

        backend.set_web_item_state(&item.id, WebDownloadState::Extracting);

        let stage_parent = if install_root.as_os_str().is_empty() {
            item.destination.clone()
        } else {
            install_root.clone()
        };
        tokio::fs::create_dir_all(&stage_parent)
            .await
            .with_context(|| format!("creating {}", stage_parent.display()))?;
        check_extract_space(&output_path, format, &stage_parent)?;

        let staging = stage_parent.join(format!(".irori-extract-{}", item.id.as_str()));
        let _ = tokio::fs::remove_dir_all(&staging).await;

        let extracted = extract_to_dir(&output_path, format, &staging, cancel).await;
        if let Err(err) = extracted {
            let _ = tokio::fs::remove_dir_all(&staging).await;
            if is_password_error(&err) {
                return Ok(PostProcessOutcome {
                    output_path,
                    installed_dir: None,
                    post_action: PostAction::ShowInFolder,
                    status_note: Some("Requires manual extraction".into()),
                });
            }
            return Err(err);
        }

        let stage_scan = staging.clone();
        let found_setup = tokio::task::spawn_blocking(move || find_setup(&stage_scan))
            .await
            .ok()
            .flatten();

        if let Some((setup_in_stage, setup_kind)) = found_setup {
            let final_dir = unique_child_dir(&item.destination, &clean_title);
            move_dir_across_fs(&staging, &final_dir).await?;
            let rel = setup_in_stage
                .strip_prefix(&staging)
                .unwrap_or(&setup_in_stage);
            let final_installer = final_dir.join(rel);
            maybe_trash_archive(item, &output_path, &app);
            return Ok(PostProcessOutcome {
                output_path: final_dir,
                installed_dir: None,
                post_action: PostAction::InstallSetup {
                    installer: final_installer,
                    kind: setup_kind_label(setup_kind),
                },
                status_note: None,
            });
        }

        let candidate = match &bridge {
            Some(b) => b.scan_extracted(&staging).await,
            None => None,
        };
        if let Some(mut candidate) = candidate {
            let final_dir = unique_child_dir(&stage_parent, &clean_title);
            move_dir_across_fs(&staging, &final_dir).await?;
            reroot_candidate(&mut candidate, &staging, &final_dir);
            candidate.title = clean_title.clone();
            maybe_trash_archive(item, &output_path, &app);

            let post_action = if item.auto_add_to_library
                && let Some(b) = &bridge
            {
                register_candidate(backend, b, candidate, &clean_title).await
            } else {
                PostAction::ShowInFolder
            };
            return Ok(PostProcessOutcome {
                output_path,
                installed_dir: Some(final_dir),
                post_action,
                status_note: None,
            });
        }

        let final_dir = unique_child_dir(&item.destination, &clean_title);
        move_dir_across_fs(&staging, &final_dir).await?;
        maybe_trash_archive(item, &output_path, &app);
        return Ok(PostProcessOutcome {
            output_path: final_dir,
            installed_dir: None,
            post_action: PostAction::ShowInFolder,
            status_note: None,
        });
    }

    let probe = output_path.clone();
    if let Some((installer, kind)) = tokio::task::spawn_blocking(move || find_setup(&probe))
        .await
        .ok()
        .flatten()
    {
        return Ok(PostProcessOutcome {
            output_path,
            installed_dir: None,
            post_action: PostAction::InstallSetup {
                installer,
                kind: setup_kind_label(kind),
            },
            status_note: None,
        });
    }

    if item.auto_add_to_library
        && let Some(b) = &bridge
        && let Some(mut candidate) = b.scan_extracted(&output_path).await
    {
        candidate.title = clean_title.clone();
        let post_action = register_candidate(backend, b, candidate, &clean_title).await;
        return Ok(PostProcessOutcome {
            output_path,
            installed_dir: None,
            post_action,
            status_note: None,
        });
    }

    Ok(PostProcessOutcome {
        output_path,
        installed_dir: None,
        post_action: PostAction::ShowInFolder,
        status_note: None,
    })
}

async fn extract_to_dir(
    archive: &Path,
    format: ArchiveFormat,
    dest: &Path,
    cancel: &CancellationToken,
) -> Result<()> {
    match format {
        ArchiveFormat::Rar => irori_mods::archive::extract_rar_checked(archive, dest, cancel).await,
        ArchiveFormat::Dmg => {
            let scratch =
                dest.with_file_name(format!(".dmg-scratch-{}", uuid::Uuid::new_v4().simple()));
            let res = engine_archive::unpack_dmg(archive, dest, &scratch).await;
            let _ = tokio::fs::remove_dir_all(&scratch).await;
            res
        }
        ArchiveFormat::Zip | ArchiveFormat::SevenZip | ArchiveFormat::Tar => {
            let archive_buf = archive.to_path_buf();
            let dest_buf = dest.to_path_buf();
            let name = archive
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let cancel = cancel.clone();
            tokio::task::spawn_blocking(move || {
                engine_archive::unpack(&archive_buf, &name, &dest_buf, &cancel)
            })
            .await
            .context("extraction task panicked")?
        }
    }
}

fn reroot_candidate(candidate: &mut SideloadCandidate, old_root: &Path, new_root: &Path) {
    if let Ok(rel) = candidate.executable.strip_prefix(old_root) {
        candidate.executable = new_root.join(rel);
    }
    if let Some(folder) = &candidate.install_folder {
        if let Ok(rel) = folder.strip_prefix(old_root) {
            candidate.install_folder = Some(new_root.join(rel));
        }
    } else {
        candidate.install_folder = Some(new_root.to_path_buf());
    }
}

pub(crate) async fn register_candidate(
    backend: &Backend,
    bridge: &Arc<dyn SideloadBridge>,
    mut candidate: SideloadCandidate,
    clean_title: &str,
) -> PostAction {
    if let Some(existing) = find_existing_sideload(backend, &candidate) {
        return PostAction::Play(existing);
    }
    if candidate.cover_art.is_none()
        && backend.is_online()
        && !backend.isolated
        && let Some((cover, hero)) = irori_extras::wiki::steam_art_for_title(clean_title).await
    {
        candidate.cover_art = Some(cover);
        candidate.background_art = Some(hero);
    }
    match bridge.register_game(candidate).await {
        Ok(id) => {
            backend.bus.emit(Event::LibraryChanged(Runner::Sideload));
            PostAction::Play(id)
        }
        Err(err) => {
            tracing::warn!("registering extracted game '{clean_title}': {err:#}");
            PostAction::ShowInFolder
        }
    }
}

fn find_existing_sideload(backend: &Backend, candidate: &SideloadCandidate) -> Option<GameId> {
    let want_exe = canonical_key(&candidate.executable);
    let want_dir = candidate.install_folder.as_deref().map(canonical_key);
    backend
        .library()
        .games
        .iter()
        .filter(|g| g.runner == Some(Runner::Sideload))
        .find(|g| {
            let Some(inst) = &g.install else { return false };
            if !inst.executable.is_empty() && canonical_key(Path::new(&inst.executable)) == want_exe
            {
                return true;
            }
            if let Some(dir) = &want_dir
                && !inst.install_path.is_empty()
                && canonical_key(Path::new(&inst.install_path)) == *dir
            {
                return true;
            }
            false
        })
        .map(|g| g.id())
}
