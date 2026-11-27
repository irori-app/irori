//! Setup installer execution (native Windows with UAC fallback, or Wine/Proton
//! on Linux/macOS) and `fs_guard`-verified file cleanup / Trash helpers.

use std::path::{Path, PathBuf};
#[cfg(not(windows))]
use std::sync::Arc;

use anyhow::{Context as _, Result, bail};
use irori_core::AppSettings;
use irori_core::process::CancellationToken;
use irori_runtime::setup::{self, Setup};

use super::model::{PostAction, WebDownload};
use super::post_process::{clean_game_title, register_candidate};
use crate::Backend;

/// Pick a non-colliding directory path inside `parent` for `clean_name`.
pub(crate) fn unique_child_dir(parent: &Path, clean_name: &str) -> PathBuf {
    let base = irori_core::fs_part::sanitize_download_name(clean_name);
    let first = parent.join(&base);
    if !first.exists() {
        return first;
    }
    for n in 2..1000 {
        let candidate = parent.join(format!("{base} ({n})"));
        if !candidate.exists() {
            return candidate;
        }
    }
    parent.join(format!("{base}-{}", uuid::Uuid::new_v4().simple()))
}

pub(crate) fn canonical_key(path: &Path) -> String {
    let resolved = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let text = resolved.to_string_lossy().replace('\\', "/");
    if cfg!(windows) {
        text.trim_start_matches("//?/").to_ascii_lowercase()
    } else {
        text
    }
}

pub(crate) async fn move_dir_across_fs(src: &Path, dst: &Path) -> Result<()> {
    if let Some(parent) = dst.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    match tokio::fs::rename(src, dst).await {
        Ok(()) => Ok(()),
        Err(_) => {
            let (s, d) = (src.to_path_buf(), dst.to_path_buf());
            tokio::task::spawn_blocking(move || -> Result<()> {
                copy_dir_recursive(&s, &d)?;
                std::fs::remove_dir_all(&s)?;
                Ok(())
            })
            .await
            .context("directory move task failed")?
        }
    }
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if ty.is_dir() {
            copy_dir_recursive(&from, &to)?;
        } else if ty.is_file() {
            std::fs::copy(&from, &to)?;
        }
    }
    Ok(())
}

pub(crate) fn maybe_trash_archive(item: &WebDownload, archive: &Path, app: &AppSettings) {
    if !item.delete_archive_after_extract || !archive.is_file() {
        return;
    }
    if let Err(err) = irori_core::fs_guard::ensure_deletable(archive, app) {
        tracing::warn!(
            "skipping archive cleanup for {}: {err:#}",
            archive.display()
        );
        return;
    }
    if let Err(err) = crate::trash::trash(archive) {
        tracing::warn!("moving archive {} to Trash: {err:#}", archive.display());
    }
}

/// Execute a detected setup installer (`Inno`, `NSIS`, or `MSI`) into
/// `<default_install_path>/<CleanTitle>` and register the installed game.
pub(crate) async fn run_setup_installer(
    backend: &Backend,
    item: &WebDownload,
    installer: &Path,
    cancel: &CancellationToken,
) -> Result<(PathBuf, PostAction)> {
    let name = installer
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let installer_buf = installer.to_path_buf();
    let name_clone = name.clone();
    let kind = tokio::task::spawn_blocking(move || setup::detect(&installer_buf, &name_clone))
        .await?
        .context("This file is no longer a supported Inno Setup, NSIS, or MSI installer")?;

    let app = backend.settings.app();
    let install_root = irori_core::paths::expand_home(&app.default_install_path);
    tokio::fs::create_dir_all(&install_root)
        .await
        .with_context(|| format!("creating {}", install_root.display()))?;
    let clean_title = clean_game_title(&item.title);
    let install_dir = unique_child_dir(&install_root, &clean_title);
    tokio::fs::create_dir_all(&install_dir)
        .await
        .with_context(|| format!("creating {}", install_dir.display()))?;

    let log_dir = irori_core::Paths::get().logs_dir().join("web-installs");
    tokio::fs::create_dir_all(&log_dir).await?;
    let log_file = log_dir.join(format!("{}.log", item.id.as_str()));

    let result = run_setup_process(
        backend,
        item,
        kind,
        installer,
        &install_dir,
        &log_file,
        cancel,
    )
    .await?;
    if result.aborted || cancel.is_cancelled() {
        bail!("Installation cancelled");
    }
    if !result.success() {
        bail!(
            "Installer exited with code {}: {}",
            result.code.map_or_else(|| "none".into(), |c| c.to_string()),
            result.error_summary()
        );
    }

    let bridge = backend.sideload_bridge.read().clone();
    let post_action = if let Some(b) = &bridge
        && let Some(mut candidate) = b.scan_extracted(&install_dir).await
    {
        candidate.title = clean_title.clone();
        register_candidate(backend, b, candidate, &clean_title).await
    } else {
        PostAction::ShowInFolder
    };
    Ok((install_dir, post_action))
}

async fn run_setup_process(
    backend: &Backend,
    item: &WebDownload,
    kind: Setup,
    installer: &Path,
    install_dir: &Path,
    log_file: &Path,
    cancel: &CancellationToken,
) -> Result<irori_core::process::ExecResult> {
    #[cfg(windows)]
    {
        let _ = (backend, item);
        let mut spec = setup::command_spec(kind, installer, install_dir, log_file, true);
        if let Some(dir) = installer.parent().filter(|p| !p.as_os_str().is_empty()) {
            spec = spec.cwd(dir);
        }
        crate::games::run_windows_command_spec(&spec, cancel.clone()).await
    }
    #[cfg(not(windows))]
    {
        let app = backend.settings.app();
        let game_id = irori_core::GameId::new(
            irori_core::Runner::Sideload,
            format!("web-setup-{}", item.id.as_str()),
        );
        let title = clean_game_title(&item.title);
        let game = irori_core::GameInfo {
            runner: Some(irori_core::Runner::Sideload),
            app_name: game_id.app_name.clone(),
            title: title.clone(),
            folder_name: Some(irori_core::fs_part::sanitize_download_name(&title)),
            ..Default::default()
        };
        let settings = backend.settings.game(&game_id);
        let target = irori_runtime::Target {
            id: &game_id,
            game: &game,
            platform: irori_core::InstallPlatform::Windows,
        };
        let argv = setup::command(kind, installer, install_dir, log_file, false);
        let ctx = irori_core::OpContext {
            cancel: cancel.clone(),
            log_file: (!app.disable_logs).then(|| log_file.to_path_buf()),
            settings: Some(Arc::new(settings.clone())),
            ..irori_core::OpContext::detached()
        };
        irori_runtime::run_in_prefix(&app, &settings, target, argv, true, ctx).await
    }
}

/// Safely move a download's files to Trash (or delete partial artifacts) after
/// verifying every single path with [`irori_core::fs_guard::ensure_deletable`].
pub(crate) fn delete_download_files(item: &WebDownload, app: &AppSettings) -> Result<()> {
    let mut targets: Vec<(PathBuf, bool)> = Vec::new();
    if let Some(out) = &item.output_path {
        targets.push((out.clone(), false));
        let part = irori_depot::path::part_path(out);
        let seg = irori_depot::path::seg_path(out);
        let aria2_ctrl = append_suffix(out, ".aria2");
        for partial in [part, seg, aria2_ctrl] {
            if partial.exists() {
                targets.push((partial, true));
            }
        }
        if item.kind.is_bittorrent()
            && let Ok(rel) = out.strip_prefix(&item.destination)
            && let Some(std::path::Component::Normal(top)) = rel.components().next()
        {
            let root_ctrl = append_suffix(&item.destination.join(top), ".aria2");
            if root_ctrl.exists() && !targets.iter().any(|(p, _)| p == &root_ctrl) {
                targets.push((root_ctrl, true));
            }
        }
    }
    if item.kind.is_bittorrent() && !item.title.trim().is_empty() {
        let mut comps = Path::new(&item.title).components();
        if matches!(comps.next(), Some(std::path::Component::Normal(_))) && comps.next().is_none() {
            let title_ctrl = append_suffix(&item.destination.join(&item.title), ".aria2");
            if title_ctrl.exists() && !targets.iter().any(|(p, _)| p == &title_ctrl) {
                targets.push((title_ctrl, true));
            }
        }
    }
    if let Some(installed) = &item.installed_dir {
        targets.push((installed.clone(), false));
    }

    let protected_roots = [
        item.destination.clone(),
        irori_core::paths::expand_home(&app.default_install_path),
    ];

    for (path, is_partial) in targets {
        if !path.exists() {
            continue;
        }
        if protected_roots
            .iter()
            .any(|root| !root.as_os_str().is_empty() && canonical_key(root) == canonical_key(&path))
        {
            bail!(
                "Refusing to delete {} because it is a download or library root folder",
                path.display()
            );
        }
        irori_core::fs_guard::ensure_deletable(&path, app)?;
        let permanent = is_partial || !crate::trash::trash(&path)?;
        if permanent {
            if path.is_dir() {
                std::fs::remove_dir_all(&path)?;
            } else {
                std::fs::remove_file(&path)?;
            }
        }
    }
    Ok(())
}

fn append_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut os = path.as_os_str().to_os_string();
    os.push(suffix);
    PathBuf::from(os)
}
