//! macOS: `screencapture` of the main display, with the Screen Recording
//! permission.

use super::*;
use anyhow::bail;
use irori_core::process::{self, CommandSpec, RunOptions};

// SAFETY: CoreGraphics' screen capture access checks, available since macOS
// 10.15 (Irori needs 11); they take no arguments and only read or ask for
// the permission.
#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGPreflightScreenCaptureAccess() -> bool;
    fn CGRequestScreenCaptureAccess() -> bool;
}

/// Whether Irori may record the screen. The first time it may not, macOS
/// asks the user (and opens nothing on later calls).
fn screen_access() -> bool {
    // SAFETY: see the declarations above.
    unsafe { CGPreflightScreenCaptureAccess() || CGRequestScreenCaptureAccess() }
}

pub(super) async fn capture(target: &Path) -> Result<PathBuf> {
    let allowed = tokio::task::spawn_blocking(screen_access)
        .await
        .unwrap_or(false);
    if !allowed {
        bail!(
            "Irori needs the Screen Recording permission: allow it in System Settings › \
             Privacy & Security › Screen & System Audio Recording, then try again"
        );
    }
    if let Some(dir) = target.parent() {
        tokio::fs::create_dir_all(dir).await?;
    }
    // -x: no sound, -m: the main display, where fullscreen games play.
    let spec = CommandSpec::new("/usr/sbin/screencapture")
        .args(["-x", "-m", "-t", "png"])
        .arg(target);
    process::run(&spec, RunOptions::default()).await?.check()?;
    if !tokio::fs::try_exists(target).await.unwrap_or(false) {
        bail!("screencapture saved no picture");
    }
    Ok(target.to_path_buf())
}
