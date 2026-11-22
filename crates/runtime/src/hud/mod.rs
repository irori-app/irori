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
}
