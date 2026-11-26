//! Notices about Irori's own versions: a newer release is out, a background
//! download finished or failed, a restart rolled back, or Irori was just
//! updated. Closing an "available" notice by hand dismisses that version
//! (Settings > About still shows it), while clicking "Install" never dismisses
//! the version before installation succeeds.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::{Disableable as _, WindowExt as _};
use gpui_kit::{App, ClipboardItem, Window};
use irori_services::app_update::release_page;
use irori_services::{AppUpdateStatus, Release, SelfInstall};
use irori_ui::app_update::{can_install, install_update, restart_into_update};
use irori_ui::i18n::{t, t_args};
use irori_ui::models::models;

/// Which notices this session already showed.
#[derive(Default)]
pub(crate) struct Announced {
    available: Option<String>,
    /// Guards the currently open `AvailableNote` so dismissing it when an
    /// install begins elsewhere (Settings > About, Updates, palette) never
    /// persists a user dismissal for that version.
    available_installing: Option<Arc<AtomicBool>>,
    ready: Option<String>,
    failed: bool,
    rolled_back: Option<String>,
    updated: bool,
}

#[derive(Debug, PartialEq, Eq)]
enum Announcement {
    Updated {
        version: String,
    },
    Available {
        release: Release,
        current: String,
        command: Option<&'static str>,
        installable: bool,
    },
    Ready {
        version: String,
    },
    Failed {
        version: Option<String>,
        error: String,
    },
    RolledBack {
        version: String,
        current: String,
    },
}

struct AvailableNote;
struct ReadyNote;
struct FailedNote;
struct RolledBackNote;
struct UpdatedNote;

impl Announced {
    /// Decide which lifecycle notices to show for `status`, updating session
    /// deduplication state. Pure so state transitions can be tested directly.
    fn next_events(&mut self, status: &AppUpdateStatus) -> Vec<Announcement> {
        let mut out = Vec::new();
        if status.updated && !self.updated {
            self.updated = true;
            out.push(Announcement::Updated {
                version: status.current.clone(),
            });
        }

        match &status.install {
            SelfInstall::Ready { version } => {
                self.failed = false;
                if self.ready.as_deref() != Some(version.as_str()) {
                    self.ready = Some(version.clone());
                    // Don't also announce "available" for a version already staged.
                    self.available = Some(version.clone());
                    out.push(Announcement::Ready {
                        version: version.clone(),
                    });
                }
            }
            SelfInstall::Failed { error } => {
                self.ready = None;
                if !self.failed {
                    self.failed = true;
                    out.push(Announcement::Failed {
                        version: status.available.as_ref().map(|r| r.version.clone()),
                        error: error.clone(),
                    });
                }
            }
            SelfInstall::RolledBack { version } => {
                self.ready = None;
                self.failed = false;
                if self.rolled_back.as_deref() != Some(version.as_str()) {
                    self.rolled_back = Some(version.clone());
                    // Don't pop a conflicting "available" notice for the version
                    // that just rolled back.
                    self.available = Some(version.clone());
                    out.push(Announcement::RolledBack {
                        version: version.clone(),
                        current: status.current.clone(),
                    });
                }
            }
            SelfInstall::Downloading { .. } => {
                // Leaving Failed/Ready resets them so a retried download
                // notifies again when it finishes or fails.
                self.failed = false;
                self.ready = None;
            }
            SelfInstall::Idle | SelfInstall::Unavailable => {
                self.failed = false;
                self.ready = None;
                if let Some(release) = status.available.as_ref().filter(|_| status.announce)
                    && self.available.as_deref() != Some(release.version.as_str())
                {
                    self.available = Some(release.version.clone());
                    out.push(Announcement::Available {
                        release: release.clone(),
                        current: status.current.clone(),
                        command: status.kind.update_command(),
                        installable: can_install(status),
                    });
                }
            }
        }
        out
    }

    pub(crate) fn show(&mut self, window: &mut Window, cx: &mut App) {
        let status = models(cx).app_update.read(cx).status.clone();
        if matches!(
            status.install,
            SelfInstall::Downloading { .. }
                | SelfInstall::Ready { .. }
                | SelfInstall::Failed { .. }
                | SelfInstall::RolledBack { .. }
        ) {
            if let Some(flag) = self.available_installing.take() {
                flag.store(true, Ordering::Relaxed);
            }
            window.remove_notification::<AvailableNote>(cx);
        }
        if matches!(
            status.install,
            SelfInstall::Downloading { .. }
                | SelfInstall::Ready { .. }
                | SelfInstall::RolledBack { .. }
        ) {
            window.remove_notification::<FailedNote>(cx);
        }
        for event in self.next_events(&status) {
            match event {
                Announcement::Updated { version } => {
                    let url = release_page(&version);
                    let note = Notification::success(t_args(
                        "shell.app_update.updated",
                        &[("version", &version)],
                    ))
                    .id::<UpdatedNote>()
                    .action(move |_, _, cx| {
                        let url = url.clone();
                        Button::new("app-update-notes")
                            .outline()
                            .label(t("shell.app_update.release_notes"))
                            .on_click(cx.listener(move |note, _, window, cx| {
                                cx.open_url(&url);
                                note.dismiss(window, cx);
                            }))
                    })
                    .autohide(true);
                    window.push_notification(note, cx);
                }
                Announcement::Available {
                    release,
                    current,
                    command,
                    installable,
                } => {
                    let title = t_args(
                        "shell.app_update.available",
                        &[("version", &release.version)],
                    );
                    let message = match command {
                        Some(command) => t_args(
                            "shell.app_update.available_command",
                            &[("current", &current), ("command", command)],
                        ),
                        None => t_args(
                            "shell.app_update.available_message",
                            &[("current", &current)],
                        ),
                    };
                    let url = release.url.clone();
                    let version = release.version.clone();
                    // Starting a self-update install must not permanently
                    // dismiss the version before installation succeeds.
                    let installing = Arc::new(AtomicBool::new(false));
                    self.available_installing = Some(installing.clone());
                    let installing_click = installing.clone();
                    let note = Notification::info(message)
                        .title(title)
                        .id::<AvailableNote>()
                        .autohide(false)
                        .on_click(|_, _, cx| irori_ui::navigate(irori_ui::Route::Downloads, cx))
                        .action(move |_, _, cx| {
                            let url = url.clone();
                            let installing_click = installing_click.clone();
                            let button = Button::new("app-update-get").primary();
                            match command {
                                Some(command) => button
                                    .label(t("shell.app_update.copy_command"))
                                    .on_click(cx.listener(move |note, _, window, cx| {
                                        cx.write_to_clipboard(ClipboardItem::new_string(
                                            command.into(),
                                        ));
                                        note.dismiss(window, cx);
                                    })),
                                None if installable => button
                                    .label(t("shell.app_update.install"))
                                    .on_click(cx.listener(move |note, _, window, cx| {
                                        installing_click.store(true, Ordering::Relaxed);
                                        install_update(cx);
                                        irori_ui::navigate(irori_ui::Route::Downloads, cx);
                                        note.dismiss(window, cx);
                                    })),
                                None => button.label(t("shell.app_update.download")).on_click(
                                    cx.listener(move |note, _, window, cx| {
                                        cx.open_url(&url);
                                        note.dismiss(window, cx);
                                    }),
                                ),
                            }
                        })
                        .on_close(move |_, cx| {
                            if installing.load(Ordering::Relaxed) {
                                return;
                            }
                            let b = irori_ui::backend(cx);
                            let version = version.clone();
                            irori_ui::state::run_infallible(cx, async move {
                                b.dismiss_app_update(&version)
                            })
                            .detach();
                        });
                    window.push_notification(note, cx);
                }
                Announcement::Ready { version } => {
                    let title = t_args("shell.app_update.ready_title", &[("version", &version)]);
                    let message = t("shell.app_update.ready_message");
                    let note = Notification::success(message)
                        .title(title)
                        .id::<ReadyNote>()
                        .autohide(false)
                        .on_click(|_, _, cx| irori_ui::navigate(irori_ui::Route::Downloads, cx))
                        .action(|_, _, cx| {
                            let restarting = models(cx).app_update.read(cx).restarting;
                            Button::new("app-update-restart-note")
                                .primary()
                                .label(t("shell.app_update.restart"))
                                .loading(restarting)
                                .disabled(restarting)
                                .on_click(cx.listener(|_, _, window, cx| {
                                    restart_into_update(window, cx).detach();
                                }))
                        });
                    window.push_notification(note, cx);
                }
                Announcement::Failed { version, error } => {
                    let title = match version {
                        Some(version) => t_args(
                            "shell.app_update.failed_title_version",
                            &[("version", &version)],
                        ),
                        None => t("shell.app_update.failed_title"),
                    };
                    let note = Notification::error(error)
                        .title(title)
                        .id::<FailedNote>()
                        .autohide(false)
                        .on_click(|_, _, cx| irori_ui::navigate(irori_ui::Route::Downloads, cx))
                        .action(|_, _, cx| {
                            Button::new("app-update-retry-note")
                                .primary()
                                .label(t("shell.app_update.try_again"))
                                .on_click(cx.listener(|note, _, window, cx| {
                                    install_update(cx);
                                    note.dismiss(window, cx);
                                }))
                        });
                    window.push_notification(note, cx);
                }
                Announcement::RolledBack { version, current } => {
                    let url = release_page(&version);
                    let title = t("shell.app_update.rolled_back_title");
                    let message = t_args(
                        "shell.app_update.rolled_back_message",
                        &[("version", &version), ("current", &current)],
                    );
                    let note = Notification::warning(message)
                        .title(title)
                        .id::<RolledBackNote>()
                        .autohide(false)
                        .on_click(|_, _, cx| irori_ui::navigate(irori_ui::Route::Downloads, cx))
                        .action(move |_, _, cx| {
                            let url = url.clone();
                            Button::new("app-update-rollback-notes")
                                .outline()
                                .label(t("shell.app_update.release_notes"))
                                .on_click(cx.listener(move |note, _, window, cx| {
                                    cx.open_url(&url);
                                    note.dismiss(window, cx);
                                }))
                        });
                    window.push_notification(note, cx);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use irori_services::{
        AppUpdateStatus, InstallKind, Release, SelfInstall, SelfUpdateChannel, SystemPackageManager,
    };

    use super::{Announced, Announcement};

    fn base_status(install: SelfInstall) -> AppUpdateStatus {
        AppUpdateStatus {
            current: "1.2.0".into(),
            kind: InstallKind::Manual,
            self_update_channel: Some(SelfUpdateChannel::AppImage),
            available: Some(Release {
                version: "1.3.0".into(),
                url: "https://github.com/irori-app/irori/releases/tag/v1.3.0".into(),
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
    fn full_download_retry_and_ready_lifecycle() {
        let mut announced = Announced::default();

        // 1. Update becomes available.
        let events = announced.next_events(&base_status(SelfInstall::Idle));
        assert!(matches!(
            events.as_slice(),
            [Announcement::Available {
                installable: true,
                ..
            }]
        ));
        // Repeated idle notifications for the same version do not duplicate.
        assert!(
            announced
                .next_events(&base_status(SelfInstall::Idle))
                .is_empty()
        );

        // 2. Download starts and reports progress -> no extra toast.
        assert!(
            announced
                .next_events(&base_status(SelfInstall::Downloading { percent: Some(25) }))
                .is_empty()
        );

        // 3. Download fails -> error notification fires once.
        let events = announced.next_events(&base_status(SelfInstall::Failed {
            error: "network reset".into(),
        }));
        assert_eq!(
            events,
            vec![Announcement::Failed {
                version: Some("1.3.0".into()),
                error: "network reset".into(),
            }]
        );
        assert!(
            announced
                .next_events(&base_status(SelfInstall::Failed {
                    error: "network reset".into(),
                }))
                .is_empty()
        );

        // 4. User retries -> Downloading clears failed latch -> fails again -> notifies again.
        assert!(
            announced
                .next_events(&base_status(SelfInstall::Downloading { percent: None }))
                .is_empty()
        );
        let events = announced.next_events(&base_status(SelfInstall::Failed {
            error: "checksum mismatch".into(),
        }));
        assert_eq!(
            events,
            vec![Announcement::Failed {
                version: Some("1.3.0".into()),
                error: "checksum mismatch".into(),
            }]
        );

        // 5. User retries again -> succeeds -> Ready notification fires once.
        assert!(
            announced
                .next_events(&base_status(SelfInstall::Downloading { percent: Some(80) }))
                .is_empty()
        );
        let events = announced.next_events(&base_status(SelfInstall::Ready {
            version: "1.3.0".into(),
        }));
        assert_eq!(
            events,
            vec![Announcement::Ready {
                version: "1.3.0".into(),
            }]
        );
        assert!(
            announced
                .next_events(&base_status(SelfInstall::Ready {
                    version: "1.3.0".into(),
                }))
                .is_empty()
        );
    }

    #[test]
    fn startup_with_staged_ready_or_rollback_suppresses_conflicting_available() {
        let mut ready_session = Announced::default();
        let events = ready_session.next_events(&base_status(SelfInstall::Ready {
            version: "1.3.0".into(),
        }));
        assert_eq!(
            events,
            vec![Announcement::Ready {
                version: "1.3.0".into(),
            }]
        );

        let mut rollback_session = Announced::default();
        let events = rollback_session.next_events(&base_status(SelfInstall::RolledBack {
            version: "1.3.0".into(),
        }));
        assert_eq!(
            events,
            vec![Announcement::RolledBack {
                version: "1.3.0".into(),
                current: "1.2.0".into(),
            }]
        );
    }

    #[test]
    fn system_packages_and_flatpak_never_emit_available_popup() {
        let mut announced = Announced::default();
        for kind in [
            InstallKind::Flatpak,
            InstallKind::SystemPackage(Some(SystemPackageManager::Pacman)),
        ] {
            let mut s = base_status(SelfInstall::Unavailable);
            s.kind = kind;
            s.announce = false;
            assert!(announced.next_events(&s).is_empty());
        }
    }

    #[gpui_kit::test]
    fn live_notification_lifecycle_and_restart_guard(cx: &mut gpui_kit::TestAppContext) {
        use gpui_kit::{AppContext as _, WindowOptions};

        let (_backend, _) = crate::shell::title_bar_tests::start(cx);
        let (handle, _shell) = cx.update(|cx| {
            gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
                cx.new(|cx| crate::shell::AppShell::new(window, cx))
            })
            .expect("window opens")
        });

        let set_status = |cx: &mut gpui_kit::TestAppContext, status: AppUpdateStatus| {
            cx.update_window(handle, |_, window, cx| {
                let model = irori_ui::models::models(cx).app_update.clone();
                model.update(cx, |m, cx| {
                    m.status = status;
                    cx.notify();
                });
                window.refresh();
                let _ = window.draw(cx);
            })
            .unwrap();
        };

        // 1. Available notice appears; starting a download removes it without
        // persisting a dismissal for the version.
        set_status(cx, base_status(SelfInstall::Idle));
        set_status(
            cx,
            base_status(SelfInstall::Downloading { percent: Some(50) }),
        );
        cx.run_until_parked();
        let state_file = irori_core::Paths::get().state_dir().join("app-update.json");
        let persisted = std::fs::read_to_string(&state_file).unwrap_or_default();
        assert!(
            !persisted.contains("\"dismissed\":\"1.3.0\"")
                && !persisted.contains("\"dismissed\": \"1.3.0\""),
            "starting an install must not persist a dismissal in app-update.json: {persisted}"
        );

        // 2. Download fails -> Failed notice draws cleanly.
        set_status(
            cx,
            base_status(SelfInstall::Failed {
                error: "network reset".into(),
            }),
        );

        // 3. Retry -> Downloading removes Failed notice -> Ready shows Restart notice.
        set_status(cx, base_status(SelfInstall::Downloading { percent: None }));
        set_status(
            cx,
            base_status(SelfInstall::Ready {
                version: "1.3.0".into(),
            }),
        );

        // 4. Shared restart guard prevents concurrent restart_into_update calls
        // and resets restarting to false when starting the helper fails.
        cx.update_window(handle, |_, window, cx| {
            let first = irori_ui::app_update::restart_into_update(window, cx);
            assert!(irori_ui::models::models(cx).app_update.read(cx).restarting);
            let duplicate = irori_ui::app_update::restart_into_update(window, cx);
            first.detach();
            duplicate.detach();
        })
        .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while cx.read(|cx| irori_ui::models::models(cx).app_update.read(cx).restarting)
            && std::time::Instant::now() < deadline
        {
            cx.run_until_parked();
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(
            !cx.read(|cx| irori_ui::models::models(cx).app_update.read(cx).restarting),
            "restarting must reset to false after failure"
        );
    }
}
