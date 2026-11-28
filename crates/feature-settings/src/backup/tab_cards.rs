//! Three live status cards acting directly as the tab switcher for
//! `Settings -> Saves & backup` (single navigation level, zero duplicate tab
//! bar below).

use gpui_kit::component::{ActiveTheme as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    Context, ElementId, Hsla, InteractiveElement as _, IntoElement, ParentElement as _,
    SharedString, StatefulInteractiveElement as _, Styled as _, div, px,
};
use irori_core::{BackupSchedule, CopyMode, SyncMode};
use irori_ui::backend;
use irori_ui::i18n::{t, t_args};
use irori_ui::style::KeyboardFocusable as _;

use super::{BackupPanel, BackupTab};

impl BackupPanel {
    /// Three live status cards acting directly as the section's tab switcher.
    pub(super) fn render_tab_cards(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (success, warning, danger, muted_fg) = (
            theme.success,
            theme.warning,
            theme.danger,
            theme.muted_foreground,
        );
        let b = backend(cx);
        let app = b.settings.app();

        // Card 1: Game saves summary
        let (saves_dot, saves_primary) = match self.ludusavi.read(cx).info() {
            None => (muted_fg, t("settings.backup.card.saves_checking")),
            Some(None) => (warning, t("settings.backup.card.saves_missing")),
            Some(Some(_)) => (
                success,
                if app.game.ludusavi_auto_backup {
                    t("settings.backup.card.saves_ready_auto")
                } else {
                    t("settings.backup.card.saves_ready_manual")
                },
            ),
        };
        let saves_sub = match app.save_snapshots {
            0 => t("settings.backup.card.snapshots_none"),
            1 => t("settings.backup.card.snapshots_one"),
            n => t_args(
                "settings.backup.card.snapshots_other",
                &[("count", &n.to_string())],
            ),
        };

        // Card 2: Library backup summary
        let backup_status = b.backup_status();
        let sched_label = match app.backups.schedule {
            BackupSchedule::Off => t("settings.backup.card.schedule_manual"),
            BackupSchedule::Daily => t("settings.backup.schedule_daily"),
            BackupSchedule::Weekly => t("settings.backup.schedule_weekly"),
        };
        let (lib_dot, lib_primary) = if backup_status.last_error.is_some() {
            (danger, t("settings.backup.last_failed"))
        } else if let Some(ok) = &backup_status.last_ok {
            let day = irori_ui::dates::localized_day(ok);
            let dot = if app.backups.schedule == BackupSchedule::Off {
                muted_fg
            } else {
                success
            };
            (
                dot,
                t_args(
                    "settings.backup.card.schedule_last",
                    &[("schedule", &sched_label), ("date", &day)],
                ),
            )
        } else {
            let dot = if app.backups.schedule == BackupSchedule::Off {
                muted_fg
            } else {
                success
            };
            (dot, sched_label)
        };
        let lib_sub = match self.listed.len() {
            0 => t("settings.backup.card.backups_none"),
            1 => t("settings.backup.card.backups_one"),
            n => t_args(
                "settings.backup.card.backups_other",
                &[("count", &n.to_string())],
            ),
        };

        // Card 3: Sync summary
        let sync_summary = self.sync.read(cx).summary(cx);
        let (sync_dot, sync_primary, sync_sub) = match sync_summary.mode {
            SyncMode::Off => (
                muted_fg,
                t("settings.sync.mode_off"),
                t("settings.backup.card.sync_single"),
            ),
            mode => {
                let mode_name: SharedString = match mode {
                    SyncMode::Off => t("settings.sync.mode_off"),
                    SyncMode::Folder => t("settings.sync.mode_folder"),
                    SyncMode::Webdav => SharedString::new_static("WebDAV"),
                    SyncMode::Rclone => SharedString::new_static("rclone"),
                };
                let dot = if sync_summary.has_error {
                    danger
                } else if sync_summary.conflicts > 0 {
                    warning
                } else {
                    success
                };
                let primary = if sync_summary.has_error {
                    t("settings.sync.failed")
                } else if sync_summary.devices == 0 {
                    mode_name
                } else if sync_summary.devices == 1 {
                    t_args(
                        "settings.backup.card.sync_devices_one",
                        &[("mode", &mode_name)],
                    )
                } else {
                    t_args(
                        "settings.backup.card.sync_devices_other",
                        &[
                            ("mode", &mode_name),
                            ("count", &sync_summary.devices.to_string()),
                        ],
                    )
                };
                let sub = if sync_summary.conflicts == 1 {
                    t("settings.backup.card.sync_conflicts_one")
                } else if sync_summary.conflicts > 1 {
                    t_args(
                        "settings.backup.card.sync_conflicts_other",
                        &[("count", &sync_summary.conflicts.to_string())],
                    )
                } else {
                    match sync_summary.saves_copy_mode {
                        CopyMode::Off => t("settings.backup.card.save_copies_off"),
                        CopyMode::Manual => t("settings.backup.card.save_copies_manual"),
                        CopyMode::Automatic => t("settings.backup.card.save_copies_auto"),
                    }
                };
                (dot, primary, sub)
            }
        };

        let [saves_spec, library_spec, sync_spec] = crate::layout::BACKUP_TABS else {
            unreachable!("BACKUP_TABS defines saves, library, and sync");
        };

        h_flex()
            .w_full()
            .gap_2p5()
            .items_stretch()
            .child(self.render_tab_card(
                BackupTab::Saves,
                "backup-tab-saves",
                t(saves_spec.label_key),
                saves_dot,
                saves_primary,
                saves_sub,
                cx,
            ))
            .child(self.render_tab_card(
                BackupTab::Library,
                "backup-tab-library",
                t(library_spec.label_key),
                lib_dot,
                lib_primary,
                lib_sub,
                cx,
            ))
            .child(self.render_tab_card(
                BackupTab::Sync,
                "backup-tab-sync",
                t(sync_spec.label_key),
                sync_dot,
                sync_primary,
                sync_sub,
                cx,
            ))
    }

    #[allow(clippy::too_many_arguments)]
    fn render_tab_card(
        &self,
        tab: BackupTab,
        id: &'static str,
        title: SharedString,
        dot: Hsla,
        primary: SharedString,
        subtitle: SharedString,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let active = self.tab == tab;
        let theme = cx.theme();
        let (bg, hover_bg, border, accent, muted_fg) = if active {
            (
                theme.primary.opacity(0.10),
                theme.primary.opacity(0.14),
                theme.primary.opacity(0.65),
                theme.primary,
                theme.muted_foreground,
            )
        } else {
            (
                theme.secondary,
                theme.secondary_hover,
                theme.border,
                theme.primary,
                theme.muted_foreground,
            )
        };
        v_flex()
            .id(ElementId::Name(id.into()))
            .debug_selector(move || id.to_string())
            .flex_1()
            .min_w_0()
            .cursor_pointer()
            .rounded(theme.radius_lg)
            .bg(bg)
            .border_1()
            .border_color(border)
            .hover(move |s| s.bg(hover_bg))
            .keyboard_focusable(cx)
            .on_click(cx.listener(move |this, _, _, cx| this.set_tab(tab, cx)))
            .child(
                v_flex()
                    .w_full()
                    .px_3()
                    .pt_2p5()
                    .pb_2()
                    .gap_0p5()
                    .child(
                        h_flex()
                            .w_full()
                            .items_center()
                            .justify_between()
                            .gap_2()
                            .child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .text_sm()
                                    .font_semibold()
                                    .child(title),
                            )
                            .child(div().flex_none().size_2().rounded_full().bg(dot)),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_xs()
                            .font_medium()
                            .child(primary),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_xs()
                            .text_color(muted_fg)
                            .child(subtitle),
                    ),
            )
            .child(
                div()
                    .h(px(2.))
                    .mx_3()
                    .mb_0p5()
                    .rounded_full()
                    .when(active, |d| d.bg(accent)),
            )
    }
}
