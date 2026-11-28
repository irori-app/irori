//! Inline overlays inside `Library backup`: on-demand "Save copy to file"
//! composer and inline "Restore review" panel rendered right above the backups
//! list.

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::tag::Tag;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Sizable as _, StyledExt as _, h_flex, v_flex,
};
use gpui_kit::{
    AnyElement, Context, InteractiveElement as _, IntoElement, ParentElement as _, Styled as _, div,
};
use irori_ui::i18n::{t, t_args};

use crate::widgets::muted;

use super::library_tab::part_checkboxes;
use super::{BackupPanel, file_name};

impl BackupPanel {
    /// On-demand standalone file save composer (replaces the duplicated card).
    pub(super) fn render_manual_save_overlay(
        &mut self,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if !self.manual_save_open {
            return None;
        }
        let theme = cx.theme();
        let (primary, radius_lg) = (theme.primary, theme.radius_lg);
        let parts = self.parts;
        Some(
            v_flex()
                .id("backup-manual-save-panel")
                .debug_selector(|| "backup-manual-save-panel".to_string())
                .w_full()
                .p_4()
                .gap_3()
                .rounded(radius_lg)
                .bg(primary.opacity(0.08))
                .border_1()
                .border_color(primary.opacity(0.45))
                .child(
                    h_flex()
                        .w_full()
                        .items_start()
                        .justify_between()
                        .gap_2()
                        .child(
                            v_flex()
                                .flex_1()
                                .min_w_0()
                                .gap_0p5()
                                .child(
                                    div()
                                        .text_sm()
                                        .font_semibold()
                                        .child(t("settings.backup.backup_title")),
                                )
                                .child(muted(t("settings.backup.backup_description"), cx)),
                        )
                        .child(
                            Button::new("backup-manual-close")
                                .ghost()
                                .xsmall()
                                .icon(IconName::X)
                                .accessibility_label(t("settings.backup.cancel"))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.manual_save_open = false;
                                    cx.notify();
                                })),
                        ),
                )
                .child(part_checkboxes(
                    "backup",
                    parts,
                    None,
                    cx,
                    |this, parts, _| this.parts = parts,
                ))
                .child(
                    h_flex()
                        .gap_2()
                        .child(
                            Button::new("backup-save")
                                .primary()
                                .small()
                                .icon(IconName::Save)
                                .label(t("settings.backup.save"))
                                .disabled(!parts.any())
                                .on_click(cx.listener(|this, _, window, cx| this.save(window, cx))),
                        )
                        .child(
                            Button::new("backup-save-cancel")
                                .ghost()
                                .small()
                                .label(t("settings.backup.cancel"))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.manual_save_open = false;
                                    cx.notify();
                                })),
                        ),
                )
                .into_any_element(),
        )
    }

    /// Inline Restore review panel right above the backup list.
    pub(super) fn render_restore_overlay(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (path, backup) = self.chosen.clone()?;
        let theme = cx.theme();
        let (primary, radius_lg) = (theme.primary, theme.radius_lg);
        let available = backup.parts();
        let created = irori_ui::dates::localized_day(&backup.created);
        Some(
            v_flex()
                .id("backup-restore-preview")
                .debug_selector(|| "backup-restore-preview".to_string())
                .w_full()
                .p_4()
                .gap_3()
                .rounded(radius_lg)
                .bg(primary.opacity(0.08))
                .border_1()
                .border_color(primary.opacity(0.45))
                .child(
                    h_flex()
                        .w_full()
                        .items_start()
                        .justify_between()
                        .gap_2()
                        .child(
                            v_flex()
                                .flex_1()
                                .min_w_0()
                                .gap_0p5()
                                .child(
                                    h_flex()
                                        .gap_2()
                                        .flex_wrap()
                                        .items_center()
                                        .child(
                                            div()
                                                .text_sm()
                                                .font_semibold()
                                                .truncate()
                                                .child(file_name(&path)),
                                        )
                                        .child(
                                            Tag::info()
                                                .xsmall()
                                                .child(t("settings.backup.ready_to_restore")),
                                        ),
                                )
                                .child(muted(
                                    t_args("settings.backup.made_on", &[("date", &created)]),
                                    cx,
                                )),
                        )
                        .child(
                            Button::new("backup-restore-close")
                                .ghost()
                                .xsmall()
                                .icon(IconName::X)
                                .accessibility_label(t("settings.backup.cancel"))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.chosen = None;
                                    cx.notify();
                                })),
                        ),
                )
                .child(part_checkboxes(
                    "restore",
                    self.restore_parts,
                    Some(available),
                    cx,
                    |this, parts, cx| {
                        this.restore_parts = parts;
                        this.plan_paths(cx);
                    },
                ))
                .children(self.paths.render(cx))
                .child(muted(t("settings.backup.restore_note"), cx))
                .child(
                    h_flex()
                        .gap_2()
                        .child(
                            Button::new("backup-restore")
                                .primary()
                                .small()
                                .icon(IconName::ArchiveRestore)
                                .label(t("settings.backup.restore"))
                                .loading(self.restoring)
                                .disabled(!self.restore_parts.any() || self.restoring)
                                .on_click(
                                    cx.listener(|this, _, window, cx| this.restore(window, cx)),
                                ),
                        )
                        .child(
                            Button::new("backup-cancel")
                                .ghost()
                                .small()
                                .label(t("settings.backup.cancel"))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.chosen = None;
                                    cx.notify();
                                })),
                        ),
                )
                .into_any_element(),
        )
    }
}
