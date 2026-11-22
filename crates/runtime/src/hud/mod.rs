//! Performance HUD runtime support: Linux hardware detection, MangoHud.conf
//! serialization, X11 case-sensitive keysym mapping, and cross-platform launch
//! environment/wrapper/Flatpak/Steam helpers.

pub mod hardware_linux;
pub mod keysym;
pub mod launch_env;
pub mod mangohud_conf;

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};

/// Atomically write `content` to `path` by writing `<path>.tmp` and renaming it
/// asynchronously. Never touches `~/.config/MangoHud/MangoHud.conf`.
pub async fn write_config_atomic(path: &Path, content: &str) -> Result<()> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("creating HUD config directory {}", parent.display()))?;
    }
    let mut tmp_os = OsString::from(path.as_os_str());
    tmp_os.push(".tmp");
    let tmp_path = PathBuf::from(tmp_os);

    tokio::fs::write(&tmp_path, content.as_bytes())
        .await
        .with_context(|| format!("writing temporary HUD config {}", tmp_path.display()))?;
    tokio::fs::rename(&tmp_path, path).await.with_context(|| {
        format!(
            "renaming temporary HUD config {} to {}",
            tmp_path.display(),
            path.display()
        )
    })?;
    Ok(())
}

/// Update `path` for live MangoHud hot-reload: when `path` already exists,
/// truncates and writes in-place so the file's inode is preserved and
/// MangoHud's `inotify` (`IN_MODIFY`) watcher reloads immediately in running
/// games and `vkcube`. When `path` does not exist yet, creates it atomically
/// via [`write_config_atomic`].
pub async fn write_config_live(path: &Path, content: &str) -> Result<()> {
    if tokio::fs::try_exists(path).await.unwrap_or(false) {
        tokio::fs::write(path, content.as_bytes())
            .await
            .with_context(|| format!("updating live HUD config {}", path.display()))?;
        return Ok(());
    }
    write_config_atomic(path, content).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn write_config_atomic_creates_dirs_and_replaces_cleanly() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("nested/mangohud/global.conf");
        write_config_atomic(&target, "legacy_layout=false\nfps\n")
            .await
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(&target).unwrap(),
            "legacy_layout=false\nfps\n"
        );
        let tmp_file = dir.path().join("nested/mangohud/global.conf.tmp");
        assert!(!tmp_file.exists());

        write_config_atomic(&target, "legacy_layout=false\nfps_only=1\n")
            .await
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(&target).unwrap(),
            "legacy_layout=false\nfps_only=1\n"
        );
        assert!(!tmp_file.exists());
    }

    #[tokio::test]
    async fn write_config_live_creates_and_preserves_inode_on_update() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("mangohud/global.conf");
        write_config_live(&target, "legacy_layout=false\nfps_limit=90\n")
            .await
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(&target).unwrap(),
            "legacy_layout=false\nfps_limit=90\n"
        );
        #[cfg(unix)]
        let inode_before = {
            use std::os::unix::fs::MetadataExt as _;
            std::fs::metadata(&target).unwrap().ino()
        };

        write_config_live(&target, "legacy_layout=false\nfps_limit=30\n")
            .await
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(&target).unwrap(),
            "legacy_layout=false\nfps_limit=30\n"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt as _;
            assert_eq!(std::fs::metadata(&target).unwrap().ino(), inode_before);
        }
    }
}
