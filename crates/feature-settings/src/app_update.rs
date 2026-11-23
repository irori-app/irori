//! Settings → About: whether a newer Irori is out, who updates this install,
//! "Check now", and for installs no package manager updates, installing the
//! update at the next restart (`irori_services::self_update`).

use std::time::Duration;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme as _, Disableable as _, h_flex};
use gpui_kit::{
    ClipboardItem, Context, IntoElement, ParentElement as _, Render, SharedString, Styled as _,
    Subscription, Window, div,
};
use irori_services::{AppUpdateStatus, InstallKind, SelfInstall};
use irori_ui::i18n::{t, t_args};
use irori_ui::models::models;

use crate::widgets::panel;

pub struct AppUpdatePanel {
    /// Why the last "Check now" failed.
    error: Option<String>,
    copied: bool,
    /// Starting the restart that puts the update in.
    restarting: bool,
    _subscriptions: Vec<Subscription>,
}

impl AppUpdatePanel {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let model = models(cx).app_update.clone();
        Self {
            error: None,
            copied: false,
            restarting: false,
            _subscriptions: vec![cx.observe(&model, |_, _, cx| cx.notify())],
        }
    }

    fn check(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.error = None;
        let b = irori_ui::backend(cx);
        let task = irori_ui::run(cx, async move { b.check_app_update().await });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                this.error = result.err().map(|e| format!("{e:#}"));
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn copy(&mut self, command: &str, window: &mut Window, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(command.to_string()));
        self.copied = true;
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(Duration::from_secs(2)).await;
            let _ = this.update(cx, |this, cx| {
                this.copied = false;
                cx.notify();
            });
        })
        .detach();
    }

    fn restart(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.restarting = true;
        cx.notify();
        restart_into_update(window, cx);
    }
}

/// Download, check and stage the newer Irori; the progress and the outcome
/// show through the app update model.
pub fn install_update(cx: &mut gpui_kit::App) {
    let b = irori_ui::backend(cx);
    irori_ui::run(cx, async move { b.install_app_update().await }).detach();
}

/// Start what puts the downloaded update in, then quit; say why when it
/// can't.
pub fn restart_into_update(window: &mut Window, cx: &mut gpui_kit::App) {
    use gpui_kit::component::WindowExt as _;
    use gpui_kit::component::notification::Notification;
    let b = irori_ui::backend(cx);
    let restart = b.clone();
    let job = b.spawn_blocking(move || restart.restart_into_app_update());
    let task = irori_ui::run(cx, async move { job.await? });
    window
        .spawn(cx, async move |cx| match task.await {
            Ok(()) => {
                let _ = cx.update(|_, cx| cx.quit());
            }
            Err(e) => {
                let _ = cx.update(|window, cx| {
                    window.push_notification(
                        Notification::error(format!("{e:#}"))
                            .title(t("settings.about.update.restart_failed")),
                        cx,
                    );
                });
            }
        })
        .detach();
}

/// The sentence about this install's updates.
fn status_text(status: &AppUpdateStatus, error: Option<&str>) -> SharedString {
    match &status.kind {
        InstallKind::Flatpak => return t("settings.about.update.flatpak"),
        InstallKind::SystemPackage | InstallKind::Nix => {
            return t("settings.about.update.system");
        }
        InstallKind::Packaged(text) => return text.clone().into(),
        InstallKind::Source => return t("settings.about.update.source"),
        _ => {}
    }
    match &status.install {
        SelfInstall::Downloading { percent } => {
            return match percent {
                Some(p) => t_args(
                    "settings.about.update.downloading_percent",
                    &[("percent", &p.to_string())],
                ),
                None => t("settings.about.update.downloading"),
            };
        }
        SelfInstall::Ready { version } => {
            return t_args("settings.about.update.ready", &[("version", version)]);
        }
        SelfInstall::Failed { error } => {
            return t_args("settings.about.update.install_failed", &[("error", error)]);
        }
        SelfInstall::RolledBack { version } if status.available.is_none() => {
            return t_args(
                "settings.about.update.rolled_back",
                &[("version", version), ("current", &status.current)],
            );
        }
        _ => {}
    }
    if status.checking {
        return t("settings.about.update.checking");
    }
    if let Some(error) = error {
        return t_args("settings.about.update.failed", &[("error", error)]);
    }
    match (&status.available, status.kind.update_command()) {
        (Some(r), Some(command)) => t_args(
            "settings.about.update.available_command",
            &[("version", &r.version), ("command", command)],
        ),
        (Some(r), None) => t_args(
            "settings.about.update.available",
            &[("version", &r.version)],
        ),
        (None, _) if status.checked_at.is_some() => t("settings.about.update.up_to_date"),
        (None, _) => t("settings.about.update.not_checked"),
    }
}

/// Whether Irori can install `status.available` itself.
pub fn can_install(status: &AppUpdateStatus) -> bool {
    status.available.is_some()
        && status.kind.update_command().is_none()
        && !matches!(status.install, SelfInstall::Unavailable)
}

impl Render for AppUpdatePanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let status = models(cx).app_update.read(cx).status.clone();
        let text = status_text(&status, self.error.as_deref());
        let failed = self.error.is_some() || matches!(status.install, SelfInstall::Failed { .. });
        let mut actions = h_flex().gap_2().flex_shrink_0();
        if let Some(release) = &status.available
            && status.kind.checks()
        {
            actions = match status.kind.update_command() {
                Some(command) => actions.child(
                    Button::new("app-update-copy")
                        .primary()
                        .icon(if self.copied {
                            IconName::Check
                        } else {
                            IconName::Copy
                        })
                        .label(if self.copied {
                            t("settings.common.copied")
                        } else {
                            t("settings.about.update.copy_command")
                        })
                        .on_click(
                            cx.listener(move |this, _, window, cx| this.copy(command, window, cx)),
                        ),
                ),
                None => {
                    let url = release.url.clone();
                    let installable = can_install(&status);
                    let actions = match &status.install {
                        SelfInstall::Ready { .. } if installable => actions.child(
                            Button::new("app-update-restart")
                                .primary()
                                .icon(IconName::RotateCw)
                                .label(t("settings.about.update.restart"))
                                .loading(self.restarting)
                                .disabled(self.restarting)
                                .on_click(
                                    cx.listener(|this, _, window, cx| this.restart(window, cx)),
                                ),
                        ),
                        install if installable => {
                            let downloading = matches!(install, SelfInstall::Downloading { .. });
                            actions.child(
                                Button::new("app-update-install")
                                    .primary()
                                    .icon(IconName::Download)
                                    .label(t("settings.about.update.install"))
                                    .loading(downloading)
                                    .disabled(downloading)
                                    .on_click(|_, _, cx| install_update(cx)),
                            )
                        }
                        _ => actions,
                    };
                    let download = Button::new("app-update-download")
                        .icon(IconName::ExternalLink)
                        .label(t("settings.about.update.download"))
                        .on_click(move |_, _, cx| cx.open_url(&url));
                    actions.child(if installable {
                        download.outline()
                    } else {
                        download.primary()
                    })
                }
            };
        }
        if status.kind.checks() {
            actions = actions.child(
                Button::new("app-update-check")
                    .outline()
                    .icon(IconName::RefreshCw)
                    .label(t("settings.about.update.check_now"))
                    .loading(status.checking)
                    .disabled(status.checking)
                    .on_click(cx.listener(|this, _, window, cx| this.check(window, cx))),
            );
        }
        panel(cx).child(
            h_flex()
                .flex_wrap()
                .gap_3()
                .px_4()
                .py_3()
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_sm()
                        .text_color(if failed {
                            cx.theme().danger
                        } else {
                            cx.theme().foreground
                        })
                        .child(text),
                )
                .child(actions),
        )
    }
}

#[cfg(test)]
mod tests {
    use irori_services::{AppUpdateStatus, InstallKind, Release, SelfInstall};

    use super::{can_install, status_text};

    fn status(kind: InstallKind, install: SelfInstall) -> AppUpdateStatus {
        AppUpdateStatus {
            current: "1.3.0".into(),
            kind,
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
    fn only_installs_without_a_package_manager_install_updates() {
        assert!(can_install(&status(InstallKind::Manual, SelfInstall::Idle)));
        assert!(!can_install(&status(
            InstallKind::Manual,
            SelfInstall::Unavailable
        )));
        assert!(!can_install(&status(InstallKind::Scoop, SelfInstall::Idle)));
        let mut none = status(InstallKind::Manual, SelfInstall::Idle);
        none.available = None;
        assert!(!can_install(&none));
    }

    #[test]
    fn the_sentence_follows_the_install() {
        crate::i18n::register();
        let ready = status(
            InstallKind::Manual,
            SelfInstall::Ready {
                version: "1.4.0".into(),
            },
        );
        assert!(status_text(&ready, None).contains("1.4.0"));
        let failed = status(
            InstallKind::Manual,
            SelfInstall::Failed {
                error: "checksum".into(),
            },
        );
        assert!(status_text(&failed, None).contains("checksum"));
    }
}
