//! `Saves & backup -> Library backup` tab: automatic backups card (with top
//! status + primary CTA and collapsible "Included in backups" checklist),
//! backups list with inline Restore review and on-demand Save-to-file composer,
//! and compact Export game list row.

use gpui_kit::assets::IconName;
use gpui_kit::component::alert::Alert;
use gpui_kit::component::button::{Button, ButtonGroup, ButtonVariants as _};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::switch::Switch;
use gpui_kit::component::tag::Tag;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, Selectable as _, Sizable as _, StyledExt as _,
    h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{App, Context, IntoElement, ParentElement as _, SharedString, Styled as _, div};
use irori_core::{BackupPrefs, BackupSchedule};
use irori_services::backup_schedule::local_time;
use irori_services::{BackupKind, BackupParts, ExportFormat};
use irori_ui::backend;
use irori_ui::i18n::{t, t_args};

use crate::widgets::{RowState, field_row, group_header, group_panel};

use super::{BackupPanel, MAX_KEEP, file_name};

const SCHEDULES: [BackupSchedule; 3] = [
    BackupSchedule::Off,
    BackupSchedule::Daily,
    BackupSchedule::Weekly,
];

/// Backups listed before the "Show all" toggle.
const PREVIEW_BACKUPS: usize = 5;

/// One part's checkbox: label key, checked, enabled, and how to set it.
type PartEntry = (&'static str, bool, bool, fn(&mut BackupParts, bool));

/// The parts automatic backups hold.
fn schedule_parts(prefs: &BackupPrefs) -> BackupParts {
    BackupParts {
        settings: prefs.settings,
        game_settings: prefs.game_settings,
        library: prefs.library,
        playtime: prefs.playtime,
        installs: prefs.installs,
    }
}

/// Short comma-separated summary of what automatic backups include.
fn included_summary(prefs: &BackupPrefs) -> SharedString {
    let mut items: Vec<SharedString> = Vec::with_capacity(6);
    if prefs.settings {
        items.push(t("settings.backup.summary_settings"));
    }
    if prefs.game_settings {
        items.push(t("settings.backup.summary_game_settings"));
    }
    if prefs.library {
        items.push(t("settings.backup.summary_library"));
    }
    if prefs.playtime {
        items.push(t("settings.backup.summary_playtime"));
    }
    if prefs.installs {
        items.push(t("settings.backup.summary_installs"));
    }
    if prefs.art {
        items.push(t("settings.backup.summary_art"));
    }
    if items.is_empty() {
        t("settings.backup.summary_none")
    } else {
        items
            .iter()
            .map(SharedString::as_ref)
            .collect::<Vec<_>>()
            .join(", ")
            .into()
    }
}

/// What kind of backup a listed one is.
fn kind_label(kind: &BackupKind) -> SharedString {
    match kind {
        BackupKind::Scheduled => t("settings.backup.kind_scheduled"),
        BackupKind::BeforeUpdate(version) => t_args(
            "settings.backup.kind_before_update",
            &[("version", version)],
        ),
        BackupKind::Other => t("settings.backup.kind_other"),
    }
}

/// A checkbox per part. With `available`, parts the backup lacks are off and
/// disabled.
pub(super) fn part_checkboxes(
    prefix: &'static str,
    parts: BackupParts,
    available: Option<BackupParts>,
    cx: &mut Context<BackupPanel>,
    set: fn(&mut BackupPanel, BackupParts, &mut Context<BackupPanel>),
) -> impl IntoElement {
    let has = available.unwrap_or(BackupParts::ALL);
    let entries: [PartEntry; 5] = [
        (
            "settings.backup.part_settings",
            parts.settings,
            has.settings,
            |p, v| p.settings = v,
        ),
        (
            "settings.backup.part_game_settings",
            parts.game_settings,
            has.game_settings,
            |p, v| p.game_settings = v,
        ),
        (
            "settings.backup.part_library",
            parts.library,
            has.library,
            |p, v| p.library = v,
        ),
        (
            "settings.backup.part_playtime",
            parts.playtime,
            has.playtime,
            |p, v| p.playtime = v,
        ),
        (
            "settings.backup.part_installs",
            parts.installs,
            has.installs,
            |p, v| p.installs = v,
        ),
    ];
    v_flex()
        .gap_2()
        .children(entries.into_iter().enumerate().map(
            move |(ix, (label, checked, enabled, apply))| {
                Checkbox::new(SharedString::from(format!("{prefix}-part-{ix}")))
                    .label(t(label))
                    .checked(checked && enabled)
                    .disabled(!enabled)
                    .small()
                    .on_change(cx.listener(move |this, v: &bool, _, cx| {
                        let mut next = match prefix {
                            "restore" => this.restore_parts,
                            "schedule" => schedule_parts(&backend(cx).settings.app().backups),
                            _ => this.parts,
                        };
                        apply(&mut next, *v);
                        set(this, next, cx);
                        cx.notify();
                    }))
            },
        ))
}

impl BackupPanel {
    pub(super) fn render_library_tab(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .w_full()
            .gap_1()
            .child(group_header(
                t("settings.backup.auto_title"),
                None,
                true,
                cx,
            ))
            .child(self.render_automatic_card(cx))
            .child(self.render_backups_restore_section(cx))
            .child(group_header(
                t("settings.backup.export_title"),
                None,
                false,
                cx,
            ))
            .child(self.render_export_card(cx))
            .child(self.render_danger_zone(cx))
    }

    fn render_automatic_card(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (border, muted_fg, success, mono) = (
            theme.border,
            theme.muted_foreground,
            theme.success,
            theme.mono_font_family.clone(),
        );
        let b = backend(cx);
        let prefs = b.settings.app().backups.clone();
        let status = b.backup_status();
        let busy = self.backing_up || status.running;
        let parts = schedule_parts(&prefs);

        // Row 1: Status + primary 1-click "Back up now" CTA
        let status_left = match (&status.last_error, &status.last_ok) {
            (Some(error), _) => v_flex()
                .flex_1()
                .min_w_0()
                .gap_1()
                .child(
                    Alert::error("backup-error", error.clone())
                        .title(t("settings.backup.last_failed")),
                )
                .into_any_element(),
            (None, Some(ok)) => h_flex()
                .flex_1()
                .min_w_0()
                .gap_2p5()
                .items_start()
                .child(
                    Icon::new(IconName::CircleCheck)
                        .text_color(success)
                        .mt_0p5(),
                )
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .gap_0p5()
                        .child(div().text_sm().font_semibold().truncate().child(t_args(
                            "settings.backup.last_ok",
                            &[("time", &local_time(ok))],
                        )))
                        .child(
                            div()
                                .text_xs()
                                .text_color(muted_fg)
                                .child(t("settings.backup.auto_description")),
                        ),
                )
                .into_any_element(),
            (None, None) => v_flex()
                .flex_1()
                .min_w_0()
                .gap_0p5()
                .child(
                    div()
                        .text_sm()
                        .font_semibold()
                        .child(t("settings.backup.never")),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(muted_fg)
                        .child(t("settings.backup.auto_description")),
                )
                .into_any_element(),
        };

        let status_row = h_flex()
            .w_full()
            .px_4()
            .py_3()
            .gap_3()
            .flex_wrap()
            .items_center()
            .justify_between()
            .child(status_left)
            .child(
                h_flex()
                    .gap_2()
                    .flex_shrink_0()
                    .child(
                        Button::new("backup-now")
                            .primary()
                            .small()
                            .icon(IconName::Save)
                            .label(if busy {
                                t("settings.backup.running")
                            } else {
                                t("settings.backup.now")
                            })
                            .loading(busy)
                            .disabled(busy || !parts.any())
                            .on_click(
                                cx.listener(|this, _, window, cx| this.back_up_now(window, cx)),
                            ),
                    )
                    .child(
                        Button::new("backup-open-folder")
                            .ghost()
                            .small()
                            .icon(IconName::FolderOpen)
                            .label(t("settings.backup.open_folder"))
                            .disabled(!self.folder_exists)
                            .on_click({
                                let folder = status.folder.clone();
                                move |_, _, cx: &mut App| cx.open_with_system(&folder)
                            }),
                    ),
            );

        // Row 2: Schedule
        let schedule_ctrl = ButtonGroup::new("backup-schedule")
            .outline()
            .small()
            .children(SCHEDULES.iter().enumerate().map(|(ix, s)| {
                let label = match s {
                    BackupSchedule::Off => t("settings.backup.schedule_off"),
                    BackupSchedule::Daily => t("settings.backup.schedule_daily"),
                    BackupSchedule::Weekly => t("settings.backup.schedule_weekly"),
                };
                Button::new(SharedString::from(format!("backup-schedule-{ix}")))
                    .label(label)
                    .selected(prefs.schedule == *s)
            }))
            .on_click(cx.listener(|this, clicked: &Vec<usize>, _, cx| {
                if let Some(&ix) = clicked.first() {
                    this.update_prefs(cx, |p| p.schedule = SCHEDULES[ix]);
                }
            }));
        let schedule_row = field_row(
            RowState {
                label: t("settings.backup.schedule_label"),
                help: t("settings.backup.schedule_help"),
                ..RowState::default()
            },
            schedule_ctrl.into_any_element(),
            cx,
        );

        // Row 3: Backups to keep
        let keep = prefs.keep.clamp(1, MAX_KEEP);
        let keep_ctrl = h_flex()
            .gap_1p5()
            .items_center()
            .child(
                Button::new("backup-keep-less")
                    .outline()
                    .xsmall()
                    .icon(IconName::Minus)
                    .accessibility_label(t("settings.backup.keep_fewer"))
                    .disabled(keep <= 1)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.update_prefs(cx, |p| p.keep = keep - 1)
                    })),
            )
            .child(
                div()
                    .min_w_8()
                    .flex()
                    .justify_center()
                    .text_sm()
                    .font_medium()
                    .child(keep.to_string()),
            )
            .child(
                Button::new("backup-keep-more")
                    .outline()
                    .xsmall()
                    .icon(IconName::Plus)
                    .accessibility_label(t("settings.backup.keep_more"))
                    .disabled(keep >= MAX_KEEP)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.update_prefs(cx, |p| p.keep = keep + 1)
                    })),
            );
        let keep_row = field_row(
            RowState {
                label: t("settings.backup.keep"),
                help: t("settings.backup.keep_help"),
                ..RowState::default()
            },
            keep_ctrl.into_any_element(),
            cx,
        );

        // Row 4: Back up before updates
        let before_updates_row = field_row(
            RowState {
                label: t("settings.backup.before_updates_label"),
                help: t("settings.backup.before_updates"),
                ..RowState::default()
            },
            Switch::new("schedule-before-updates")
                .checked(prefs.before_updates)
                .accessibility_label(t("settings.backup.before_updates_label"))
                .on_change(cx.listener(|this, v: &bool, _, cx| {
                    let v = *v;
                    this.update_prefs(cx, |p| p.before_updates = v)
                }))
                .into_any_element(),
            cx,
        );

        // Row 5: Backup folder
        let custom_folder = !prefs.folder.trim().is_empty();
        let folder_row = h_flex()
            .w_full()
            .px_4()
            .py_2p5()
            .gap_3()
            .items_center()
            .justify_between()
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_0p5()
                    .child(
                        div()
                            .text_sm()
                            .font_medium()
                            .child(t("settings.backup.folder")),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(muted_fg)
                            .font_family(mono)
                            .truncate()
                            .child(status.folder.to_string_lossy().into_owned()),
                    ),
            )
            .child(
                h_flex()
                    .gap_2()
                    .flex_shrink_0()
                    .when(custom_folder, |row| {
                        row.child(
                            Button::new("backup-folder-default")
                                .ghost()
                                .small()
                                .label(t("settings.backup.use_default_folder"))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.update_prefs(cx, |p| p.folder.clear());
                                    this.list(cx);
                                })),
                        )
                    })
                    .child(
                        Button::new("backup-folder")
                            .outline()
                            .small()
                            .icon(IconName::FolderOpen)
                            .label(t("settings.backup.choose_folder"))
                            .on_click(
                                cx.listener(|this, _, window, cx| this.pick_folder(window, cx)),
                            ),
                    ),
            );

        // Row 6: Collapsible "Included in backups" summary + checklist
        let included_open = self.included_open;
        let included_row = v_flex()
            .w_full()
            .px_4()
            .py_2p5()
            .gap_3()
            .child(
                h_flex()
                    .w_full()
                    .gap_3()
                    .items_center()
                    .justify_between()
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap_0p5()
                            .child(
                                div()
                                    .text_sm()
                                    .font_medium()
                                    .child(t("settings.backup.contents")),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(muted_fg)
                                    .truncate()
                                    .child(included_summary(&prefs)),
                            ),
                    )
                    .child(
                        Button::new("backup-customize-parts")
                            .outline()
                            .small()
                            .icon(if included_open {
                                IconName::ChevronUp
                            } else {
                                IconName::ChevronDown
                            })
                            .label(if included_open {
                                t("settings.backup.done")
                            } else {
                                t("settings.backup.customize")
                            })
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.included_open = !this.included_open;
                                cx.notify();
                            })),
                    ),
            )
            .when(included_open, |col| {
                col.child(
                    v_flex()
                        .pt_2p5()
                        .gap_2()
                        .border_t_1()
                        .border_color(border)
                        .child(part_checkboxes(
                            "schedule",
                            parts,
                            None,
                            cx,
                            |this, parts, cx| {
                                this.update_prefs(cx, |p| {
                                    p.settings = parts.settings;
                                    p.game_settings = parts.game_settings;
                                    p.library = parts.library;
                                    p.playtime = parts.playtime;
                                    p.installs = parts.installs;
                                })
                            },
                        ))
                        .child(
                            Checkbox::new("schedule-art")
                                .label(t("settings.backup.part_art"))
                                .checked(prefs.art)
                                .small()
                                .on_change(cx.listener(|this, v: &bool, _, cx| {
                                    let v = *v;
                                    this.update_prefs(cx, |p| p.art = v)
                                })),
                        ),
                )
            });

        group_panel(cx)
            .child(status_row)
            .child(div().border_t_1().border_color(border).child(schedule_row))
            .child(div().border_t_1().border_color(border).child(keep_row))
            .child(
                div()
                    .border_t_1()
                    .border_color(border)
                    .child(before_updates_row),
            )
            .child(div().border_t_1().border_color(border).child(folder_row))
            .child(div().border_t_1().border_color(border).child(included_row))
    }

    fn render_backups_restore_section(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (border, muted_fg, mono) = (
            theme.border,
            theme.muted_foreground,
            theme.mono_font_family.clone(),
        );

        let section_bar = h_flex()
            .w_full()
            .pt_4()
            .pb_1()
            .gap_2()
            .flex_wrap()
            .items_center()
            .justify_between()
            .child(irori_ui::style::caption(
                t("settings.backup.backups_restore_title"),
                cx,
            ))
            .child(
                h_flex()
                    .gap_2()
                    .flex_wrap()
                    .child(
                        Button::new("backup-save-file-toggle")
                            .outline()
                            .xsmall()
                            .icon(IconName::Download)
                            .label(t("settings.backup.save_to_file"))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.manual_save_open = !this.manual_save_open;
                                if this.manual_save_open {
                                    this.chosen = None;
                                }
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("backup-choose")
                            .outline()
                            .xsmall()
                            .icon(IconName::FolderOpen)
                            .label(t("settings.backup.restore_from_file"))
                            .on_click(cx.listener(|this, _, window, cx| this.choose(window, cx))),
                    ),
            );

        let manual_save = self.render_manual_save_overlay(cx);
        let restore_preview = self.render_restore_overlay(cx);

        // Backup list card
        let total = self.listed.len();
        let shown = if self.show_all_backups {
            total
        } else {
            total.min(PREVIEW_BACKUPS)
        };
        let mut list_card = group_panel(cx);
        if self.listed.is_empty() {
            list_card = list_card.child(
                div()
                    .px_4()
                    .py_3()
                    .text_sm()
                    .text_color(muted_fg)
                    .child(t("settings.backup.list_empty")),
            );
        } else {
            for (ix, entry) in self.listed.iter().take(shown).enumerate() {
                let path = entry.path.clone();
                let kind = kind_label(&entry.kind);
                let fname = file_name(&entry.path);
                let row = h_flex()
                    .w_full()
                    .px_4()
                    .py_2p5()
                    .gap_3()
                    .items_center()
                    .justify_between()
                    .when(ix > 0, |r| r.border_t_1().border_color(border))
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
                                            .font_medium()
                                            .child(local_time(&entry.created)),
                                    )
                                    .child(Tag::secondary().xsmall().outline().child(kind))
                                    .when(entry.art, |r| {
                                        r.child(
                                            Tag::secondary()
                                                .xsmall()
                                                .outline()
                                                .child(t("settings.backup.badge_with_art")),
                                        )
                                    }),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(muted_fg)
                                    .font_family(mono.clone())
                                    .truncate()
                                    .child(fname),
                            ),
                    )
                    .child(
                        Button::new(SharedString::from(format!("backup-restore-{ix}")))
                            .outline()
                            .small()
                            .icon(IconName::ArchiveRestore)
                            .label(t("settings.backup.restore_this"))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.choose_listed(path.clone(), window, cx)
                            })),
                    );
                list_card = list_card.child(row);
            }
            if total > PREVIEW_BACKUPS {
                let show_all = self.show_all_backups;
                let label = if show_all {
                    t("settings.backup.show_fewer")
                } else {
                    t_args("settings.backup.show_all", &[("count", &total.to_string())])
                };
                list_card = list_card.child(
                    h_flex()
                        .w_full()
                        .px_4()
                        .py_2()
                        .border_t_1()
                        .border_color(border)
                        .child(
                            Button::new("backup-list-toggle")
                                .ghost()
                                .xsmall()
                                .icon(if show_all {
                                    IconName::ChevronUp
                                } else {
                                    IconName::ChevronDown
                                })
                                .label(label)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.show_all_backups = !this.show_all_backups;
                                    cx.notify();
                                })),
                        ),
                );
            }
        }

        v_flex()
            .w_full()
            .gap_2()
            .child(section_bar)
            .children(manual_save)
            .children(restore_preview)
            .child(list_card)
    }

    fn render_export_card(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let muted_fg = cx.theme().muted_foreground;
        group_panel(cx).child(
            h_flex()
                .w_full()
                .px_4()
                .py_3()
                .gap_3()
                .flex_wrap()
                .items_center()
                .justify_between()
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .gap_0p5()
                        .child(
                            div()
                                .text_sm()
                                .font_medium()
                                .child(t("settings.backup.export_catalog_label")),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(muted_fg)
                                .child(t("settings.backup.export_description")),
                        ),
                )
                .child(
                    h_flex()
                        .gap_2()
                        .flex_shrink_0()
                        .child(
                            Button::new("export-csv")
                                .outline()
                                .small()
                                .icon(IconName::FileSpreadsheet)
                                .label(t("settings.backup.export_csv"))
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.export(ExportFormat::Csv, window, cx)
                                })),
                        )
                        .child(
                            Button::new("export-json")
                                .outline()
                                .small()
                                .icon(IconName::FileBraces)
                                .label(t("settings.backup.export_json"))
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.export(ExportFormat::Json, window, cx)
                                })),
                        ),
                ),
        )
    }
}
