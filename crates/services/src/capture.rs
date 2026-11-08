//! Screenshots Irori takes for the player: Guide + RB on a controller and
//! the in-game overlay's Screenshot.
//!
//! On Linux the picture comes from the desktop portal's Screenshot
//! interface without its dialog (GNOME, KDE Plasma and the wlroots portal
//! answer it; Plasma asks once whether Irori may), else from `grim` on
//! wlroots desktops without a portal. It is saved in the desktop's
//! screenshot folder as `<game> <date> <time>.png`, where the screenshot
//! index finds it for the session it was taken in. Inside gamescope
//! neither works: Steam's own screenshot is the one to use there.

use std::path::{Path, PathBuf};

use anyhow::Result;
use irori_core::{GameId, Notice, NoticeLevel};

use crate::Backend;

/// `file://` URI as the portal returns it, as a path: percent escapes
/// decoded, `None` for anything that isn't a local file.
pub fn file_uri_path(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    // `file://localhost/…` names this computer too.
    let rest = rest.strip_prefix("localhost").unwrap_or(rest);
    if !rest.starts_with('/') {
        return None;
    }
    let bytes = rest.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && let Some(hex) = rest.get(i + 1..i + 3)
            && let Ok(byte) = u8::from_str_radix(hex, 16)
        {
            out.push(byte);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt as _;
        Some(PathBuf::from(std::ffi::OsString::from_vec(out)))
    }
    #[cfg(not(unix))]
    {
        Some(PathBuf::from(String::from_utf8(out).ok()?))
    }
}

/// The name Irori gives a screenshot of `title` taken at `at`.
pub fn screenshot_name(title: &str, at: chrono::NaiveDateTime) -> String {
    crate::replay::clip_name(title, at, "png")
}

impl Backend {
    /// Take a screenshot of what is on screen for the running game `id`
    /// (or the screen, without one) and announce where it went.
    pub fn take_screenshot(&self, id: Option<GameId>) {
        let b = self.clone();
        self.spawn(async move {
            let title = id
                .as_ref()
                .and_then(|id| b.game(id))
                .map(|g| g.title.clone())
                .unwrap_or_else(|| "Irori".to_string());
            let at = chrono::Local::now().naive_local();
            let dir = crate::screenshots::save_folder();
            let target = crate::replay::free_path(&dir, &screenshot_name(&title, at));
            match platform::capture(&target).await {
                Ok(path) => {
                    tracing::info!("screenshot saved to {}", path.display());
                    let mut notice =
                        Notice::new(NoticeLevel::Success, format!("Screenshot of {title} saved"))
                            .message(path.display().to_string())
                            .keys("shell.notice.screenshot_saved", None, &[("title", &title)]);
                    if let Some(id) = id {
                        notice = notice.game(id);
                    }
                    b.bus.notice(notice);
                }
                Err(e) => {
                    tracing::warn!("taking a screenshot: {e:#}");
                    b.bus.notice(
                        Notice::new(NoticeLevel::Error, "The screenshot wasn't taken")
                            .message(format!("{e:#}"))
                            .keys("shell.notice.screenshot_failed", None, &[]),
                    );
                }
            }
        });
    }
}

/// Move what the portal wrote to `target`; it stays where it is when it
/// can't be moved (another disk and no copy either). Only Linux takes
/// screenshots, so elsewhere only the tests call it.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn settle(from: &Path, target: &Path) -> PathBuf {
    if from == target {
        return target.to_path_buf();
    }
    if let Some(dir) = target.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if std::fs::rename(from, target).is_ok() {
        return target.to_path_buf();
    }
    match std::fs::copy(from, target) {
        Ok(_) => {
            let _ = std::fs::remove_file(from);
            target.to_path_buf()
        }
        Err(e) => {
            tracing::debug!("keeping the screenshot at {}: {e}", from.display());
            from.to_path_buf()
        }
    }
}

#[cfg(target_os = "linux")]
mod platform {
    use super::*;
    use anyhow::{Context as _, bail};
    use irori_core::process::{self, CommandSpec, RunOptions};

    async fn portal() -> Result<PathBuf> {
        use ashpd::desktop::screenshot::Screenshot;
        let response = Screenshot::request()
            .interactive(false)
            .modal(false)
            .send()
            .await?
            .response()?;
        file_uri_path(response.uri().as_str()).context("The desktop saved the screenshot elsewhere")
    }

    pub(super) async fn capture(target: &Path) -> Result<PathBuf> {
        let portal_error = match portal().await {
            Ok(path) => {
                let target = target.to_path_buf();
                return tokio::task::spawn_blocking(move || settle(&path, &target))
                    .await
                    .context("Could not save the screenshot");
            }
            Err(e) => e,
        };
        let Some(grim) = irori_core::host::find_program("grim").await else {
            bail!("The desktop took no screenshot: {portal_error:#}");
        };
        if let Some(dir) = target.parent() {
            tokio::fs::create_dir_all(dir).await?;
        }
        let spec = irori_core::host::command(CommandSpec::new(grim).arg(target));
        process::run(&spec, RunOptions::default()).await?.check()?;
        Ok(target.to_path_buf())
    }
}

#[cfg(not(target_os = "linux"))]
mod platform {
    use super::*;

    pub(super) async fn capture(_target: &Path) -> Result<PathBuf> {
        anyhow::bail!("Screenshots aren't available on this system yet")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_portals_file_uris() {
        assert_eq!(
            file_uri_path("file:///home/me/Pictures/Screenshot%20from%202026.png"),
            Some(PathBuf::from("/home/me/Pictures/Screenshot from 2026.png"))
        );
        assert_eq!(
            file_uri_path("file://localhost/tmp/a.png"),
            Some(PathBuf::from("/tmp/a.png"))
        );
        assert_eq!(
            file_uri_path("file:///tmp/%E2%9C%93%zz.png"),
            Some(PathBuf::from("/tmp/✓%zz.png"))
        );
        assert_eq!(file_uri_path("https://example.com/a.png"), None);
        assert_eq!(file_uri_path("file://server/share/a.png"), None);
    }

    #[test]
    fn names_screenshots_after_the_game() {
        let at = chrono::NaiveDate::from_ymd_opt(2026, 3, 4)
            .unwrap()
            .and_hms_opt(5, 6, 7)
            .unwrap();
        assert_eq!(
            screenshot_name("Hades: II", at),
            format!(
                "{} 2026-03-04 05-06-07.png",
                irori_core::paths::sanitize("Hades: II")
            )
        );
    }

    #[test]
    fn moves_the_picture_to_its_name() {
        let dir = tempfile::tempdir().unwrap();
        let from = dir.path().join("Screenshot.png");
        std::fs::write(&from, b"png").unwrap();
        let target = dir.path().join("shots").join("Game.png");
        assert_eq!(settle(&from, &target), target);
        assert!(!from.exists());
        assert_eq!(std::fs::read(&target).unwrap(), b"png");
    }
}
