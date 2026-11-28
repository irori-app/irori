//! Settings > About: whether a newer Irori is out, who updates this install,
//! "Check now", install channel, last-checked time, release notes, and for
//! installs no package manager updates, installing the update at the next
//! restart (`irori_services::self_update`).

use std::time::Duration;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::tag::Tag;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, Sizable as _, StyledExt as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    ClipboardItem, Context, IntoElement, ParentElement as _, Render, SharedString, Styled as _,
    Subscription, Task, Window, div, rems,
};
use irori_core::util::unix_now;
use irori_services::app_update::release_page;
use irori_services::{AppUpdateStatus, InstallKind, SelfInstall, SelfUpdateChannel};
use irori_ui::components::BrandMark;
use irori_ui::i18n::{t, t_args};
use irori_ui::models::models;

pub use irori_ui::app_update::{can_install, install_update, restart_into_update};

use crate::widgets::panel;

pub struct AppUpdatePanel {
    /// Why the last "Check now" failed.
    error: Option<String>,
    copied: bool,
    copied_version: bool,
    copy_reset: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl AppUpdatePanel {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let model = models(cx).app_update.clone();
        Self {
            error: None,
            copied: false,
            copied_version: false,
            copy_reset: None,
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

    fn copy_text(
        &mut self,
        text: String,
        version: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        (self.copied, self.copied_version) = (!version, version);
        cx.notify();
        self.copy_reset = Some(cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(Duration::from_secs(2)).await;
            let _ = this.update(cx, |this, cx| {
                (this.copied, this.copied_version, this.copy_reset) = (false, false, None);
                cx.notify();
            });
        }));
    }

    fn install(&mut self, cx: &mut Context<Self>) {
        self.error = None;
        install_update(cx);
        cx.notify();
    }

    fn restart(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        restart_into_update(window, cx).detach();
    }

    #[cfg(test)]
    pub(crate) fn trigger_restart_for_test(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.restart(window, cx);
    }
}

/// Short label showing how this copy of Irori was installed.
fn channel_label(status: &AppUpdateStatus) -> SharedString {
    match &status.kind {
        InstallKind::Flatpak => "Flatpak".into(),
        InstallKind::SystemPackage(Some(pm)) => pm.name().into(),
        InstallKind::SystemPackage(None) => t("settings.about.update.channel.system"),
        InstallKind::Nix => "Nix".into(),
        InstallKind::Source => t("settings.about.update.channel.source"),
        InstallKind::Packaged(_) => t("settings.about.update.channel.packaged"),
        InstallKind::Homebrew => "Homebrew".into(),
        InstallKind::Scoop => "Scoop".into(),
        InstallKind::Winget => "winget".into(),
        InstallKind::Manual => match status.self_update_channel {
            Some(SelfUpdateChannel::AppImage) => "AppImage".into(),
            Some(SelfUpdateChannel::Tarball) => t("settings.about.update.channel.tarball"),
            Some(SelfUpdateChannel::MacApp) => "macOS".into(),
            Some(SelfUpdateChannel::WindowsZip) => t("settings.about.update.channel.windows_zip"),
            Some(SelfUpdateChannel::WindowsPortable) => {
                t("settings.about.update.channel.windows_portable")
            }
            Some(SelfUpdateChannel::WindowsSetup) => {
                t("settings.about.update.channel.windows_setup")
            }
            None => t("settings.about.update.channel.manual"),
        },
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StatusTone {
    Normal,
    Warning,
    Danger,
}

/// The sentence and semantic tone for this install's update state, computed
/// together so color and text can never disagree.
fn status_presentation(
    status: &AppUpdateStatus,
    error: Option<&str>,
) -> (SharedString, StatusTone) {
    match &status.kind {
        InstallKind::Nix => return (t("settings.about.update.system"), StatusTone::Normal),
        InstallKind::Packaged(text) => return (text.clone().into(), StatusTone::Normal),
        InstallKind::Source => return (t("settings.about.update.source"), StatusTone::Normal),
        _ => {}
    }
    match &status.install {
        SelfInstall::Downloading { percent } => {
            let text = match percent {
                Some(p) => t_args(
                    "settings.about.update.downloading_percent",
                    &[("percent", &p.to_string())],
                ),
                None => t("settings.about.update.downloading"),
            };
            return (text, StatusTone::Normal);
        }
        SelfInstall::Ready { version } => {
            return (
                t_args("settings.about.update.ready", &[("version", version)]),
                StatusTone::Normal,
            );
        }
        _ => {}
    }
    if status.checking {
        return (t("settings.about.update.checking"), StatusTone::Normal);
    }
    match &status.install {
        SelfInstall::Failed { error } => {
            return (
                t_args("settings.about.update.install_failed", &[("error", error)]),
                StatusTone::Danger,
            );
        }
        SelfInstall::RolledBack { version } => {
            return (
                t_args(
                    "settings.about.update.rolled_back",
                    &[("version", version), ("current", &status.current)],
                ),
                StatusTone::Warning,
            );
        }
        SelfInstall::Idle
        | SelfInstall::Unavailable
        | SelfInstall::Downloading { .. }
        | SelfInstall::Ready { .. } => {}
    }
    if let Some(error) = error {
        return (
            t_args("settings.about.update.failed", &[("error", error)]),
            StatusTone::Danger,
        );
    }
    let text = match (&status.available, status.kind.update_command()) {
        (Some(r), Some(command)) => t_args(
            "settings.about.update.available_command",
            &[("version", &r.version), ("command", command)],
        ),
        (Some(r), None) => t_args(
            "settings.about.update.available",
            &[("version", &r.version)],
        ),
        (None, _) if status.checked_at.is_some() => t("settings.about.update.up_to_date"),
        (None, _) => match &status.kind {
            InstallKind::Flatpak => t("settings.about.update.flatpak"),
            InstallKind::SystemPackage(_) => t("settings.about.update.system"),
            _ => t("settings.about.update.not_checked"),
        },
    };
    (text, StatusTone::Normal)
}

/// URL of the release notes most relevant to `status`, or `None` for source builds.
fn notes_url(status: &AppUpdateStatus) -> Option<String> {
    if status.kind == InstallKind::Source {
        return None;
    }
    let version = match &status.install {
        SelfInstall::Ready { version } => Some(version.as_str()),
        _ => status
            .available
            .as_ref()
            .map(|r| r.version.as_str())
            .or(match &status.install {
                SelfInstall::RolledBack { version } => Some(version.as_str()),
                _ => None,
            }),
    };
    Some(match version {
        Some(v) => status
            .available
            .as_ref()
            .filter(|r| r.version == v)
            .map(|r| r.url.clone())
            .unwrap_or_else(|| release_page(v)),
        None => release_page(&status.current),
    })
}

impl AppUpdatePanel {
    fn command_pill(
        &self,
        command: &'static str,
        big: bool,
        cx: &mut Context<Self>,
    ) -> gpui_kit::Div {
        let th = cx.theme();
        let (icon, key) = if self.copied {
            (IconName::Check, "settings.common.copied")
        } else {
            (IconName::Copy, "settings.about.update.copy_command")
        };
        let on_copy =
            cx.listener(move |this, _, w, cx| this.copy_text(command.into(), false, w, cx));
        let btn = Button::new("app-update-copy").xsmall().icon(icon);
        let btn = if big {
            btn.primary().label(t(key))
        } else {
            btn.ghost().compact().tooltip(t(key))
        }
        .on_click(on_copy);
        let fg = if big {
            th.foreground
        } else {
            th.muted_foreground
        };
        let code = div()
            .min_w_0()
            .truncate()
            .font_family(th.mono_font_family.clone())
            .text_xs()
            .text_color(fg);
        let code = if big { code.flex_1() } else { code }.child(command);
        let bar = h_flex()
            .items_center()
            .rounded(th.radius)
            .bg(th.background)
            .border_1()
            .border_color(th.border);
        if big {
            bar.w_full()
                .justify_between()
                .gap_3()
                .pl_3()
                .pr_1p5()
                .py_1p5()
        } else {
            bar.max_w_full().gap_2().pl_2p5().pr_1().py_0p5()
        }
        .child(code)
        .child(btn)
    }
}

impl Render for AppUpdatePanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let model = models(cx).app_update.read(cx);
        let status = model.status.clone();
        let restarting = model.restarting;
        let (text, tone) = status_presentation(&status, self.error.as_deref());
        let downloading_percent = match status.install {
            SelfInstall::Downloading { percent } => Some(percent),
            _ => None,
        };
        let command = status.kind.update_command();
        let show_command_strip = command.is_some()
            && (status.available.is_some()
                || matches!(
                    status.kind,
                    InstallKind::Flatpak | InstallKind::SystemPackage(_)
                ));
        let notes_url = notes_url(&status);

        let mut actions = h_flex().flex_wrap().items_center().gap_2().flex_shrink_0();
        if let Some(release) = &status.available
            && status.kind.can_check_manually()
            && command.is_none()
        {
            let url = release.url.clone();
            let installable = can_install(&status);
            actions = match &status.install {
                SelfInstall::Ready { .. } if installable => actions.child(
                    Button::new("app-update-restart")
                        .primary()
                        .small()
                        .icon(IconName::RotateCw)
                        .label(t("settings.about.update.restart"))
                        .loading(restarting)
                        .disabled(restarting)
                        .on_click(cx.listener(|this, _, window, cx| this.restart(window, cx))),
                ),
                install if installable => {
                    let downloading = matches!(install, SelfInstall::Downloading { .. });
                    actions.child(
                        Button::new("app-update-install")
                            .primary()
                            .small()
                            .icon(IconName::Download)
                            .label(t("settings.about.update.install"))
                            .loading(downloading)
                            .disabled(downloading)
                            .on_click(cx.listener(|this, _, _, cx| this.install(cx))),
                    )
                }
                _ => actions,
            };
            let download = Button::new("app-update-download")
                .small()
                .icon(IconName::ExternalLink)
                .label(t("settings.about.update.download"))
                .on_click(move |_, _, cx| cx.open_url(&url));
            actions = actions.child(if installable {
                download.outline()
            } else {
                download.primary()
            });
        } else if matches!(status.install, SelfInstall::Ready { .. }) {
            actions = actions.child(
                Button::new("app-update-restart")
                    .primary()
                    .small()
                    .icon(IconName::RotateCw)
                    .label(t("settings.about.update.restart"))
                    .loading(restarting)
                    .disabled(restarting)
                    .on_click(cx.listener(|this, _, window, cx| this.restart(window, cx))),
            );
        }
        if let Some(cmd) = command.filter(|_| show_command_strip && status.available.is_none()) {
            actions = actions.child(self.command_pill(cmd, false, cx));
        }
        let prominent_bar = command
            .filter(|_| show_command_strip && status.available.is_some())
            .map(|cmd| self.command_pill(cmd, true, cx));

        let mut header_actions = h_flex().flex_wrap().items_center().gap_2().flex_shrink_0();
        if let Some(url) = notes_url {
            header_actions = header_actions.child(
                Button::new("app-update-notes")
                    .ghost()
                    .small()
                    .icon(IconName::ExternalLink)
                    .label(t("settings.about.update.release_notes"))
                    .on_click(move |_, _, cx| cx.open_url(&url)),
            );
        }
        if status.kind.can_check_manually() {
            header_actions = header_actions.child(
                Button::new("app-update-check")
                    .outline()
                    .small()
                    .icon(IconName::RefreshCw)
                    .label(t("settings.about.update.check_now"))
                    .loading(status.checking)
                    .disabled(status.checking || downloading_percent.is_some())
                    .on_click(cx.listener(|this, _, window, cx| this.check(window, cx))),
            );
        }

        let channel = channel_label(&status);
        let ver_text = format!("Irori {} ({channel})", env!("CARGO_PKG_VERSION"));
        let copy_v =
            cx.listener(move |this, _, w, cx| this.copy_text(ver_text.clone(), true, w, cx));
        let (v_icon, v_tip) = if self.copied_version {
            (IconName::Check, "settings.common.copied")
        } else {
            (IconName::Copy, "settings.about.copy_version")
        };
        let version_btn = Button::new("about-copy-version")
            .outline()
            .xsmall()
            .icon(v_icon)
            .label(format!("v{}", env!("CARGO_PKG_VERSION")))
            .tooltip(t(v_tip))
            .on_click(copy_v);

        let theme = cx.theme();
        let text_color = match tone {
            StatusTone::Danger => theme.danger,
            StatusTone::Warning => theme.warning,
            StatusTone::Normal => theme.foreground,
        };
        let muted = theme.muted_foreground;
        let last_checked = status
            .kind
            .can_check_manually()
            .then_some(())
            .and_then(|()| {
                status.checked_at.map(|at| {
                    let when = irori_ui::dates::relative_time(at, unix_now());
                    t_args("settings.about.update.last_checked", &[("when", &when)])
                })
            });

        let is_ready = matches!(status.install, SelfInstall::Ready { .. });
        let is_avail = status.available.is_some() && !is_ready && tone == StatusTone::Normal;
        let up_to_date =
            status.available.is_none() && status.checked_at.is_some() && tone == StatusTone::Normal;
        let dot_color = match tone {
            StatusTone::Danger => theme.danger,
            StatusTone::Warning => theme.warning,
            StatusTone::Normal if is_ready || up_to_date => theme.success,
            StatusTone::Normal if is_avail || downloading_percent.is_some() => theme.primary,
            StatusTone::Normal => muted,
        };
        let (strip_bg, strip_bdr) = if is_ready {
            (theme.success.opacity(0.10), theme.success.opacity(0.32))
        } else if is_avail || downloading_percent.is_some() {
            (theme.primary.opacity(0.10), theme.primary.opacity(0.32))
        } else {
            (theme.muted, theme.border)
        };
        let ch = h_flex()
            .items_center()
            .gap_1()
            .child(Icon::new(IconName::Package).xsmall())
            .child(channel);
        let title_row = h_flex()
            .flex_wrap()
            .items_center()
            .gap_2()
            .child(div().text_xl().font_bold().child("Irori"))
            .child(version_btn)
            .child(Tag::secondary().xsmall().outline().child(ch));
        let identity = v_flex().flex_1().min_w_0().gap_1().child(title_row).child(
            div()
                .text_xs()
                .text_color(muted)
                .child(t("settings.about.tagline")),
        );
        let hero_top = h_flex()
            .flex_wrap()
            .items_center()
            .justify_between()
            .gap_4()
            .px_4()
            .py_3p5()
            .child(
                h_flex()
                    .flex_1()
                    .min_w_0()
                    .items_center()
                    .gap_3p5()
                    .child(BrandMark::new(rems(3.75)))
                    .child(identity),
            )
            .child(header_actions);
        let status_row = h_flex()
            .flex_1()
            .min_w_0()
            .flex_wrap()
            .items_center()
            .gap_2()
            .child(div().flex_none().size_2().rounded_full().bg(dot_color))
            .child(
                div()
                    .text_sm()
                    .font_medium()
                    .text_color(text_color)
                    .child(text),
            )
            .when_some(last_checked, |row, checked| {
                row.child(
                    div()
                        .text_xs()
                        .text_color(muted)
                        .child(format!("· {checked}")),
                )
            });

        panel(cx).overflow_hidden().child(hero_top).child(
            v_flex()
                .gap_2p5()
                .px_4()
                .py_2p5()
                .border_t_1()
                .border_color(strip_bdr)
                .bg(strip_bg)
                .child(
                    h_flex()
                        .flex_wrap()
                        .items_center()
                        .justify_between()
                        .gap_3()
                        .child(status_row)
                        .child(actions),
                )
                .when_some(downloading_percent, |col, percent| {
                    col.child(irori_ui::motion::gliding_progress(
                        "app-update-progress",
                        percent.map(f32::from),
                    ))
                })
                .children(prominent_bar),
        )
    }
}

#[cfg(test)]
mod tests {
    use irori_services::{
        AppUpdateStatus, InstallKind, Release, SelfInstall, SelfUpdateChannel, SystemPackageManager,
    };

    use super::{StatusTone, can_install, channel_label, notes_url, status_presentation};

    fn status_text(status: &AppUpdateStatus, error: Option<&str>) -> gpui_kit::SharedString {
        status_presentation(status, error).0
    }

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

    #[test]
    fn rollback_warning_is_shown_even_when_available_is_some() {
        crate::i18n::register();
        let rolled_back = status(
            InstallKind::Manual,
            SelfInstall::RolledBack {
                version: "1.4.0".into(),
            },
        );
        assert!(rolled_back.available.is_some());
        let text = status_text(&rolled_back, None);
        assert!(
            text.contains("1.4.0") && text.contains("1.3.0"),
            "expected rollback text mentioning both versions, got: {text}"
        );
        // Even if a prior manual check failed, rollback stays warning-toned,
        // and active download / ready / checking states stay normal-toned.
        let (_, rollback_tone) = status_presentation(&rolled_back, Some("network error"));
        assert_eq!(rollback_tone, StatusTone::Warning);
        let downloading = status(
            InstallKind::Manual,
            SelfInstall::Downloading { percent: Some(30) },
        );
        let (_, dl_tone) = status_presentation(&downloading, Some("network error"));
        assert_eq!(dl_tone, StatusTone::Normal);
        let ready = status(
            InstallKind::Manual,
            SelfInstall::Ready {
                version: "1.4.0".into(),
            },
        );
        let (_, ready_tone) = status_presentation(&ready, Some("network error"));
        assert_eq!(ready_tone, StatusTone::Normal);
        let mut checking = status(InstallKind::Manual, SelfInstall::Idle);
        checking.checking = true;
        let (_, checking_tone) = status_presentation(&checking, Some("network error"));
        assert_eq!(checking_tone, StatusTone::Normal);
        let idle = status(InstallKind::Manual, SelfInstall::Idle);
        let (_, err_tone) = status_presentation(&idle, Some("network error"));
        assert_eq!(err_tone, StatusTone::Danger);
    }

    #[test]
    fn flatpak_and_linux_system_packages_support_manual_check_and_commands() {
        crate::i18n::register();
        let mut pacman = status(
            InstallKind::SystemPackage(Some(SystemPackageManager::Pacman)),
            SelfInstall::Unavailable,
        );
        pacman.announce = false;
        assert!(status_text(&pacman, None).contains("sudo pacman -Syu irori"));
        assert_eq!(channel_label(&pacman), "pacman");

        pacman.available = None;
        pacman.checked_at = Some(100);
        assert_eq!(status_text(&pacman, None), "Irori is up to date.");

        pacman.checked_at = None;
        assert!(status_text(&pacman, None).contains("package manager"));

        let mut flatpak = status(InstallKind::Flatpak, SelfInstall::Unavailable);
        flatpak.announce = false;
        assert!(status_text(&flatpak, None).contains("flatpak update app.irori.Irori"));
        assert_eq!(channel_label(&flatpak), "Flatpak");

        flatpak.available = None;
        flatpak.checked_at = None;
        assert!(status_text(&flatpak, None).contains("Flatpak"));
    }

    #[test]
    fn channel_labels_cover_manual_and_package_managers() {
        crate::i18n::register();
        let mut s = status(InstallKind::Manual, SelfInstall::Idle);
        s.self_update_channel = Some(SelfUpdateChannel::AppImage);
        assert_eq!(channel_label(&s), "AppImage");
        s.self_update_channel = Some(SelfUpdateChannel::Tarball);
        assert_eq!(channel_label(&s), "Tarball");
        s.kind = InstallKind::SystemPackage(Some(SystemPackageManager::Dnf));
        assert_eq!(channel_label(&s), "dnf");
        s.kind = InstallKind::SystemPackage(Some(SystemPackageManager::Apt));
        assert_eq!(channel_label(&s), "apt");
        s.kind = InstallKind::Homebrew;
        assert_eq!(channel_label(&s), "Homebrew");
    }

    #[test]
    fn notes_url_prefers_staged_or_rolled_back_version_when_available_is_missing() {
        let mut ready = status(
            InstallKind::Manual,
            SelfInstall::Ready {
                version: "1.5.0".into(),
            },
        );
        ready.available = None;
        assert_eq!(
            notes_url(&ready).as_deref(),
            Some("https://github.com/irori-app/irori/releases/tag/v1.5.0")
        );

        let mut rolled_back = status(
            InstallKind::Manual,
            SelfInstall::RolledBack {
                version: "1.5.0".into(),
            },
        );
        rolled_back.available = None;
        assert_eq!(
            notes_url(&rolled_back).as_deref(),
            Some("https://github.com/irori-app/irori/releases/tag/v1.5.0")
        );

        let mut current = status(InstallKind::Manual, SelfInstall::Idle);
        current.available = None;
        assert_eq!(
            notes_url(&current).as_deref(),
            Some("https://github.com/irori-app/irori/releases/tag/v1.3.0")
        );

        let source = status(InstallKind::Source, SelfInstall::Unavailable);
        assert_eq!(notes_url(&source), None);
    }
}
