//! Shared UI actions for Irori's own updates, used by Settings > About, the
//! Updates page, shell notices, and the command palette.

use gpui_kit::component::WindowExt as _;
use gpui_kit::component::notification::Notification;
use gpui_kit::{App, Task, Window};
use irori_services::{AppUpdateStatus, SelfInstall};

use crate::i18n::t;

/// Whether Irori can install `status.available` itself.
pub fn can_install(status: &AppUpdateStatus) -> bool {
    status.available.is_some()
        && status.kind.update_command().is_none()
        && !matches!(status.install, SelfInstall::Unavailable)
}

/// Download, check and stage the newer Irori; progress and outcome appear
/// through the app update model.
pub fn install_update(cx: &mut App) {
    let b = crate::backend(cx);
    crate::run(cx, async move { b.install_app_update().await }).detach();
}

/// Start the helper that swaps in the staged update, then quit. Tracks the
/// in-flight restart on `models(cx).app_update.restarting` so every screen and
/// notification shares loading state and ignores concurrent clicks. Resolves
/// to `true` when the restart started and `false` if already restarting or if
/// starting failed (after resetting `restarting` and showing an error notice).
pub fn restart_into_update(window: &mut Window, cx: &mut App) -> Task<bool> {
    let model = crate::models(cx).app_update.clone();
    if model.read(cx).restarting {
        return Task::ready(false);
    }
    model.update(cx, |m, cx| {
        m.restarting = true;
        cx.notify();
    });
    let b = crate::backend(cx);
    let restart = b.clone();
    let job = b.spawn_blocking(move || restart.restart_into_app_update());
    let task = crate::run(cx, async move { job.await? });
    window.spawn(cx, async move |cx| match task.await {
        Ok(()) => {
            let _ = cx.update(|_, cx| cx.quit());
            true
        }
        Err(e) => {
            let _ = cx.update(|window, cx| {
                model.update(cx, |m, cx| {
                    m.restarting = false;
                    cx.notify();
                });
                window.push_notification(
                    Notification::error(format!("{e:#}")).title(t("ui.app_update.restart_failed")),
                    cx,
                );
            });
            false
        }
    })
}

#[cfg(test)]
mod tests {
    use irori_services::{
        AppUpdateStatus, InstallKind, Release, SelfInstall, SelfUpdateChannel, SystemPackageManager,
    };

    use super::can_install;

    fn status(kind: InstallKind, install: SelfInstall) -> AppUpdateStatus {
        AppUpdateStatus {
            current: "1.3.0".into(),
            kind,
            self_update_channel: Some(SelfUpdateChannel::AppImage),
            available: Some(Release {
                version: "1.4.0".into(),
                url: "https://github.com/irori-app/irori/releases/tag/v1.4.0".into(),
            }),
            dismissed: false,
            announce: true,
            updated: false,
            checking: false,
            checked_at: Some(1),
            install,
        }
    }

    #[test]
    fn only_installs_without_a_package_manager_self_update() {
        assert!(can_install(&status(InstallKind::Manual, SelfInstall::Idle)));
        assert!(!can_install(&status(
            InstallKind::Manual,
            SelfInstall::Unavailable
        )));
        assert!(!can_install(&status(InstallKind::Scoop, SelfInstall::Idle)));
        assert!(!can_install(&status(
            InstallKind::Flatpak,
            SelfInstall::Idle
        )));
        assert!(!can_install(&status(
            InstallKind::SystemPackage(Some(SystemPackageManager::Pacman)),
            SelfInstall::Idle
        )));
        let mut none = status(InstallKind::Manual, SelfInstall::Idle);
        none.available = None;
        assert!(!can_install(&none));
    }
}
