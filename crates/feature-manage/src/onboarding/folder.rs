//! Step 3 of first-run onboarding: choose the default install folder with
//! auto-detected writable drives and free space read off the UI thread.

use gpui_kit::assets::IconName;
use gpui_kit::component::button::Button;
use gpui_kit::component::tag::Tag;
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, Context, InteractiveElement as _, IntoElement, ParentElement as _, SharedString,
    StatefulInteractiveElement as _, Styled as _, div, relative,
};
use irori_core::settings::same_folder;
use irori_core::util::format_bytes;
use irori_services::InstallDriveProbe;
use irori_ui::i18n::{t, t_args};
use irori_ui::style::KeyboardFocusable as _;

use super::OnboardingView;
use crate::common::backend_op;

/// One clickable drive option in the Install Folder step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DriveChoice {
    pub label: SharedString,
    pub path: String,
    pub free: Option<u64>,
    pub total: Option<u64>,
    pub is_default: bool,
}

/// Build the selectable drive list (default folder first, then extra writable
/// drives) from a single off-thread [`InstallDriveProbe`].
pub(crate) fn build_drive_choices(
    default_path: &str,
    probe: InstallDriveProbe,
) -> Vec<DriveChoice> {
    let default_label = probe
        .configured_labels
        .into_iter()
        .next()
        .flatten()
        .unwrap_or_else(|| default_path.to_string());
    let (free, total) = probe
        .configured_space
        .into_iter()
        .next()
        .flatten()
        .map_or((None, None), |(f, t)| (Some(f), Some(t)));
    let mut choices = vec![DriveChoice {
        label: default_label.into(),
        path: default_path.to_string(),
        free,
        total,
        is_default: true,
    }];
    for drive in probe.extras {
        if choices
            .iter()
            .any(|c| same_folder(&c.path, &drive.suggested_base))
        {
            continue;
        }
        choices.push(DriveChoice {
            label: drive.disk_label.into(),
            path: drive.suggested_base,
            free: Some(drive.free),
            total: Some(drive.total),
            is_default: false,
        });
    }
    choices
}

/// Probe the default install path and auto-detected drives off the UI thread.
pub(crate) fn probe_drives(default_path: String) -> Vec<DriveChoice> {
    let expanded = irori_core::paths::expand_home(default_path.trim());
    let probe = irori_services::probe_install_drives(&[expanded]);
    build_drive_choices(&default_path, probe)
}

fn used_fraction(free: Option<u64>, total: Option<u64>) -> Option<f32> {
    let total = total.filter(|&t| t > 0)?;
    let free = free?;
    let used = total.saturating_sub(free) as f64 / total as f64;
    Some(used.clamp(0.0, 1.0) as f32)
}

impl OnboardingView {
    fn select_drive(&mut self, path: String, cx: &mut Context<Self>) {
        self.install_path = path.clone();
        cx.notify();
        backend_op(cx, move |b| {
            b.settings.update_app(|s| s.default_install_path = path)
        });
    }

    fn render_drive_card(
        &self,
        ix: usize,
        drive: &DriveChoice,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme();
        let selected = same_folder(&self.install_path, &drive.path);
        let path = drive.path.clone();
        let free_text = drive.free.map(|free| {
            t_args(
                "manage.onboarding.folder.free_space",
                &[("free", &format_bytes(free))],
            )
        });
        let used = used_fraction(drive.free, drive.total);
        v_flex()
            .id(("onboarding-drive", ix))
            .debug_selector(move || format!("onboarding-drive-{ix}"))
            .keyboard_focusable(cx)
            .cursor_pointer()
            .gap_2()
            .p_3()
            .rounded(theme.radius)
            .border_1()
            .border_color(if selected {
                theme.primary
            } else {
                theme.border
            })
            .bg(if selected {
                theme.secondary
            } else {
                theme.background
            })
            .on_click(cx.listener(move |this, _, _, cx| this.select_drive(path.clone(), cx)))
            .child(
                h_flex()
                    .gap_2p5()
                    .items_center()
                    .child(
                        Icon::new(IconName::HardDrive)
                            .small()
                            .text_color(if selected {
                                theme.primary
                            } else {
                                theme.muted_foreground
                            }),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .child(
                                h_flex()
                                    .gap_2()
                                    .items_center()
                                    .child(
                                        div()
                                            .text_sm()
                                            .font_medium()
                                            .truncate()
                                            .child(drive.label.clone()),
                                    )
                                    .when(drive.is_default, |d| {
                                        d.child(
                                            Tag::secondary()
                                                .xsmall()
                                                .child(t("manage.onboarding.folder.default_tag")),
                                        )
                                    }),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(theme.muted_foreground)
                                    .truncate()
                                    .child(drive.path.clone()),
                            ),
                    )
                    .when_some(free_text, |d, text| {
                        d.child(
                            div()
                                .flex_shrink_0()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .child(text),
                        )
                    })
                    .when(selected, |d| {
                        d.child(Icon::new(IconName::Check).small().text_color(theme.primary))
                    }),
            )
            .when_some(used, |d, fraction| {
                d.child(
                    div()
                        .w_full()
                        .h_1()
                        .rounded_full()
                        .overflow_hidden()
                        .bg(theme.muted)
                        .when(fraction > 0.0, |bar| {
                            bar.child(div().h_full().w(relative(fraction)).bg(if selected {
                                theme.primary
                            } else {
                                theme.muted_foreground.opacity(0.45)
                            }))
                        }),
                )
            })
            .into_any_element()
    }

    pub(super) fn render_folder(&self, cx: &mut Context<Self>) -> AnyElement {
        let drives: Vec<AnyElement> = self
            .drives
            .iter()
            .enumerate()
            .map(|(ix, d)| self.render_drive_card(ix, d, cx))
            .collect();
        let theme = cx.theme();
        v_flex()
            .gap_3()
            .child(
                div()
                    .text_xl()
                    .font_semibold()
                    .child(t("manage.onboarding.folder.title")),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(theme.muted_foreground)
                    .child(t("manage.onboarding.folder.description")),
            )
            .child(
                h_flex()
                    .gap_3()
                    .p_3()
                    .items_center()
                    .rounded(theme.radius)
                    .border_1()
                    .border_color(theme.border)
                    .bg(theme.background)
                    .child(Icon::new(IconName::FolderOpen).text_color(theme.muted_foreground))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_sm()
                            .truncate()
                            .child(self.install_path.clone()),
                    )
                    .child(
                        Button::new("choose-folder")
                            .small()
                            .label(t("manage.onboarding.folder.choose"))
                            .on_click(
                                cx.listener(|this, _, window, cx| this.choose_folder(window, cx)),
                            ),
                    ),
            )
            .when(!drives.is_empty(), |this| {
                this.child(
                    v_flex()
                        .gap_2()
                        .pt_1()
                        .child(
                            div()
                                .text_xs()
                                .font_semibold()
                                .text_color(theme.muted_foreground)
                                .child(t("manage.onboarding.folder.drives_title")),
                        )
                        .children(drives),
                )
            })
            .child(
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(t("manage.onboarding.folder.scope_note")),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use irori_services::DetectedDrive;

    #[test]
    fn builds_default_and_extra_drive_choices_without_duplicates() {
        let probe = InstallDriveProbe {
            configured_labels: vec![Some("/ · NVMe".into())],
            configured_space: vec![Some((100, 500))],
            extras: vec![
                DetectedDrive {
                    label: "/mnt/Games".into(),
                    disk_label: "/mnt/Games".into(),
                    suggested_base: "/mnt/Games/Games/Irori".into(),
                    total: 1000,
                    free: 600,
                    mount: PathBuf::from("/mnt/Games"),
                },
                DetectedDrive {
                    label: "Duplicate".into(),
                    disk_label: "Duplicate".into(),
                    suggested_base: "/home/me/Games/Irori".into(),
                    total: 500,
                    free: 100,
                    mount: PathBuf::from("/"),
                },
            ],
        };
        let choices = build_drive_choices("/home/me/Games/Irori", probe);
        assert_eq!(choices.len(), 2);
        assert!(choices[0].is_default);
        assert_eq!(choices[0].free, Some(100));
        assert_eq!(choices[1].path, "/mnt/Games/Games/Irori");
        assert!(!choices[1].is_default);
    }
}
