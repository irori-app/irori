//! `Saves & backup -> Game saves` tab: Ludusavi local save backups,
//! progressive-disclosure advanced options, store cloud save defaults &
//! rollback snapshots, and a quick link to Sync cloud copies.

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::Input;
use gpui_kit::component::select::{Select, SelectState};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IndexPath, Sizable as _, StyledExt as _, h_flex,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, AppContext as _, Context, ElementId, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, PathPromptOptions, SharedString, Stateful, Styled as _, Window, div,
};
use irori_core::{CopyMode, SyncMode};
use irori_ui::backend;
use irori_ui::i18n::{t, t_args};

use crate::form::ChoiceItem;
use crate::widgets::{
    PairedRow, RowState, field_row, group_header, group_panel, more_options_disclosure,
    paired_field_row,
};

use super::{BackupPanel, BackupTab, MAX_SNAPSHOTS};

const INTERVALS: [(u32, &str); 5] = [
    (0, "settings.option.ludusavi_interval.0"),
    (15, "settings.option.ludusavi_interval.15"),
    (30, "settings.option.ludusavi_interval.30"),
    (60, "settings.option.ludusavi_interval.60"),
    (120, "settings.option.ludusavi_interval.120"),
];

const FULL_LIMITS: [u32; 6] = [0, 1, 2, 3, 5, 10];
const DIFF_LIMITS: [&str; 6] = ["", "0", "1", "3", "5", "10"];

fn interval_items() -> Vec<ChoiceItem> {
    INTERVALS
        .iter()
        .map(|(mins, key)| ChoiceItem::new(mins.to_string(), t(key)))
        .collect()
}

fn full_limit_items() -> Vec<ChoiceItem> {
    FULL_LIMITS
        .iter()
        .map(|&n| {
            let label = if n == 0 {
                t("settings.option.ludusavi_limit.ludusavi")
            } else {
                SharedString::from(n.to_string())
            };
            ChoiceItem::new(n.to_string(), label)
        })
        .collect()
}

fn diff_limit_items() -> Vec<ChoiceItem> {
    DIFF_LIMITS
        .iter()
        .map(|&v| {
            let label = match v {
                "" => t("settings.option.ludusavi_limit.ludusavi"),
                "0" => t("settings.option.ludusavi_limit.none"),
                other => SharedString::new_static(other),
            };
            let key = if v.is_empty() { "ludusavi" } else { v };
            ChoiceItem::new(key, label)
        })
        .collect()
}

pub(super) fn parse_interval_key(key: &str) -> Option<u32> {
    key.parse::<u32>()
        .ok()
        .filter(|v| INTERVALS.iter().any(|(m, _)| m == v))
}

pub(super) fn parse_full_limit_key(key: &str) -> Option<u32> {
    key.parse::<u32>().ok().filter(|v| FULL_LIMITS.contains(v))
}

pub(super) fn parse_diff_limit_key(key: &str) -> String {
    if key == "ludusavi" {
        String::new()
    } else {
        key.to_string()
    }
}

pub(super) fn build_interval_select(
    current: u32,
    window: &mut Window,
    cx: &mut Context<BackupPanel>,
) -> Entity<SelectState<Vec<ChoiceItem>>> {
    let idx = INTERVALS
        .iter()
        .position(|(m, _)| *m == current)
        .unwrap_or(0);
    let items = interval_items();
    cx.new(|cx| SelectState::new(items, Some(IndexPath::default().row(idx)), window, cx))
}

pub(super) fn build_full_limit_select(
    current: u32,
    window: &mut Window,
    cx: &mut Context<BackupPanel>,
) -> Entity<SelectState<Vec<ChoiceItem>>> {
    let idx = FULL_LIMITS.iter().position(|&n| n == current).unwrap_or(0);
    let items = full_limit_items();
    cx.new(|cx| SelectState::new(items, Some(IndexPath::default().row(idx)), window, cx))
}

pub(super) fn build_diff_limit_select(
    current: &str,
    window: &mut Window,
    cx: &mut Context<BackupPanel>,
) -> Entity<SelectState<Vec<ChoiceItem>>> {
    let idx = DIFF_LIMITS.iter().position(|&v| v == current).unwrap_or(0);
    let items = diff_limit_items();
    cx.new(|cx| SelectState::new(items, Some(IndexPath::default().row(idx)), window, cx))
}

impl BackupPanel {
    /// Keep Game saves controls in sync when settings change elsewhere (e.g.
    /// Game defaults or Ludusavi's Choose program button).
    pub(super) fn sync_saves_controls(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let app = backend(cx).settings.app();
        if self.ludusavi_path.read(cx).value().as_ref() != app.ludusavi_path {
            let v = app.ludusavi_path.clone();
            self.ludusavi_path
                .update(cx, |s, cx| s.set_value(v, window, cx));
        }
        if self.ludusavi_backup_path.read(cx).value().as_ref() != app.ludusavi_backup_path {
            let v = app.ludusavi_backup_path.clone();
            self.ludusavi_backup_path
                .update(cx, |s, cx| s.set_value(v, window, cx));
        }
        let interval_idx = INTERVALS
            .iter()
            .position(|(m, _)| *m == app.game.ludusavi_interval)
            .unwrap_or(0);
        self.ludusavi_interval.update(cx, |s, cx| {
            s.set_selected_index(Some(IndexPath::default().row(interval_idx)), window, cx)
        });
        let full_idx = FULL_LIMITS
            .iter()
            .position(|&n| n == app.ludusavi_full_limit)
            .unwrap_or(0);
        self.ludusavi_full_limit.update(cx, |s, cx| {
            s.set_selected_index(Some(IndexPath::default().row(full_idx)), window, cx)
        });
        let diff_idx = DIFF_LIMITS
            .iter()
            .position(|&v| v == app.ludusavi_differential_limit)
            .unwrap_or(0);
        self.ludusavi_diff_limit.update(cx, |s, cx| {
            s.set_selected_index(Some(IndexPath::default().row(diff_idx)), window, cx)
        });
    }

    pub(super) fn retranslate_saves_controls(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let app = backend(cx).settings.app();
        let interval_items = interval_items();
        let interval_idx = INTERVALS
            .iter()
            .position(|(m, _)| *m == app.game.ludusavi_interval)
            .unwrap_or(0);
        self.ludusavi_interval.update(cx, |s, cx| {
            s.set_items(interval_items, window, cx);
            s.set_selected_index(Some(IndexPath::default().row(interval_idx)), window, cx);
        });
        let full_items = full_limit_items();
        let full_idx = FULL_LIMITS
            .iter()
            .position(|&n| n == app.ludusavi_full_limit)
            .unwrap_or(0);
        self.ludusavi_full_limit.update(cx, |s, cx| {
            s.set_items(full_items, window, cx);
            s.set_selected_index(Some(IndexPath::default().row(full_idx)), window, cx);
        });
        let diff_items = diff_limit_items();
        let diff_idx = DIFF_LIMITS
            .iter()
            .position(|&v| v == app.ludusavi_differential_limit)
            .unwrap_or(0);
        self.ludusavi_diff_limit.update(cx, |s, cx| {
            s.set_items(diff_items, window, cx);
            s.set_selected_index(Some(IndexPath::default().row(diff_idx)), window, cx);
        });
        let p1 = t("settings.field.ludusavi_path.placeholder");
        self.ludusavi_path
            .update(cx, |s, cx| s.set_placeholder(p1, window, cx));
        let p2 = t("settings.field.ludusavi_backup_path.placeholder");
        self.ludusavi_backup_path
            .update(cx, |s, cx| s.set_placeholder(p2, window, cx));
    }

    pub(super) fn commit_ludusavi_paths(&mut self, cx: &mut Context<Self>) {
        let prog = self.ludusavi_path.read(cx).value().trim().to_string();
        let folder = self
            .ludusavi_backup_path
            .read(cx)
            .value()
            .trim()
            .to_string();
        let b = backend(cx);
        let app = b.settings.app();
        if app.ludusavi_path != prog || app.ludusavi_backup_path != folder {
            b.settings.update_app(|s| {
                s.ludusavi_path = prog;
                s.ludusavi_backup_path = folder;
            });
        }
    }

    fn pick_ludusavi_path(&mut self, is_dir: bool, window: &mut Window, cx: &mut Context<Self>) {
        let prompt = if is_dir {
            t("settings.picker.choose_folder")
        } else {
            t("settings.picker.choose_file")
        };
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: !is_dir,
            directories: is_dir,
            multiple: false,
            prompt: Some(prompt),
        });
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(paths))) = rx.await
                && let Some(p) = paths.into_iter().next()
            {
                let _ = this.update_in(cx, |this, window, cx| {
                    let text = p.to_string_lossy().into_owned();
                    if is_dir {
                        this.ludusavi_backup_path
                            .update(cx, |s, cx| s.set_value(text.clone(), window, cx));
                        backend(cx)
                            .settings
                            .update_app(|s| s.ludusavi_backup_path = text);
                    } else {
                        this.ludusavi_path
                            .update(cx, |s, cx| s.set_value(text.clone(), window, cx));
                        backend(cx).settings.update_app(|s| s.ludusavi_path = text);
                    }
                });
            }
        })
        .detach();
    }

    fn highlight_row(
        &self,
        path: &'static str,
        row: gpui_kit::Div,
        cx: &App,
    ) -> Stateful<gpui_kit::Div> {
        let highlighted = self.highlighted_field == Some(path);
        let theme = cx.theme();
        row.id(ElementId::Name(format!("row-{path}").into()))
            .debug_selector(move || format!("row-{path}"))
            .when(highlighted, |d| {
                d.rounded(theme.radius)
                    .bg(theme.primary.opacity(0.14))
                    .border_1()
                    .border_color(theme.primary.opacity(0.45))
            })
    }

    pub(super) fn render_saves_tab(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (border, muted_fg) = (theme.border, theme.muted_foreground);
        let app = backend(cx).settings.app();
        let ludusavi_missing = matches!(self.ludusavi.read(cx).info(), Some(None));

        // ---- Card 1: Local save backups (Ludusavi) ----
        let status_row = self.highlight_row(
            "panel.ludusavi",
            div().w_full().px_4().py_3().child(self.ludusavi.clone()),
            cx,
        );

        let after_help = if ludusavi_missing {
            t("settings.saves.after_session_help_missing")
        } else {
            t("settings.saves.default_after_session_help")
        };
        let after_row = self.highlight_row(
            "iroriLudusaviAutoBackup",
            field_row(
                RowState {
                    label: t("settings.saves.default_after_session"),
                    help: after_help,
                    tooltip: Some((
                        SharedString::new_static("iroriLudusaviAutoBackup"),
                        t("settings.field.ludusavi_auto_backup.tip"),
                    )),
                    ..RowState::default()
                },
                Switch::new("saves-ludusavi-after-session")
                    .checked(app.game.ludusavi_auto_backup)
                    .accessibility_label(t("settings.saves.default_after_session"))
                    .on_change(|on: &bool, _, cx| {
                        let on = *on;
                        backend(cx)
                            .settings
                            .update_app(|s| s.game.ludusavi_auto_backup = on);
                    })
                    .into_any_element(),
                cx,
            ),
            cx,
        );

        let interval_row = self.highlight_row(
            "iroriLudusaviInterval",
            field_row(
                RowState {
                    label: t("settings.field.ludusavi_interval.label"),
                    help: t("settings.field.ludusavi_interval.help"),
                    tooltip: Some((
                        SharedString::new_static("iroriLudusaviInterval"),
                        t("settings.field.ludusavi_interval.tip"),
                    )),
                    ..RowState::default()
                },
                Select::new(&self.ludusavi_interval)
                    .id("saves-ludusavi-interval")
                    .accessibility_label(t("settings.field.ludusavi_interval.label"))
                    .w_56()
                    .into_any_element(),
                cx,
            ),
            cx,
        );

        let ludusavi_card = group_panel(cx)
            .child(status_row)
            .child(div().border_t_1().border_color(border).child(after_row))
            .child(div().border_t_1().border_color(border).child(interval_row));

        // ---- Progressive disclosure: More Ludusavi options (4) ----
        let more_open = self.ludusavi_more_open;
        let more_disclosure = more_options_disclosure(
            "more-ludusavi-options",
            more_open,
            4,
            cx.listener(|this, _, _, cx| {
                this.ludusavi_more_open = !this.ludusavi_more_open;
                cx.notify();
            }),
            || self.render_ludusavi_more(cx).into_any_element(),
        );

        // ---- Card 2: Store cloud saves ----
        let auto_cloud_row = self.highlight_row(
            "autoSyncSaves",
            field_row(
                RowState {
                    label: t("settings.field.auto_sync_saves.label"),
                    help: t("settings.field.auto_sync_saves.help"),
                    tooltip: Some((
                        SharedString::new_static("autoSyncSaves"),
                        t("settings.field.auto_sync_saves.tip"),
                    )),
                    ..RowState::default()
                },
                Switch::new("saves-auto-cloud-sync")
                    .checked(app.game.auto_sync_saves)
                    .accessibility_label(t("settings.field.auto_sync_saves.label"))
                    .on_change(|on: &bool, _, cx| {
                        let on = *on;
                        backend(cx)
                            .settings
                            .update_app(|s| s.game.auto_sync_saves = on);
                    })
                    .into_any_element(),
                cx,
            ),
            cx,
        );

        let snapshots = app.save_snapshots.min(MAX_SNAPSHOTS);
        let snapshots_ctrl = h_flex()
            .gap_1p5()
            .items_center()
            .child(
                Button::new("saves-snapshots-less")
                    .outline()
                    .xsmall()
                    .icon(IconName::Minus)
                    .accessibility_label(t("settings.backup.keep_fewer"))
                    .disabled(snapshots == 0)
                    .on_click(move |_, _, cx| {
                        let next = snapshots.saturating_sub(1);
                        backend(cx).settings.update_app(|s| s.save_snapshots = next);
                    }),
            )
            .child(
                div()
                    .min_w_8()
                    .flex()
                    .justify_center()
                    .text_sm()
                    .font_medium()
                    .child(snapshots.to_string()),
            )
            .child(
                Button::new("saves-snapshots-more")
                    .outline()
                    .xsmall()
                    .icon(IconName::Plus)
                    .accessibility_label(t("settings.backup.keep_more"))
                    .disabled(snapshots >= MAX_SNAPSHOTS)
                    .on_click(move |_, _, cx| {
                        let next = (snapshots + 1).min(MAX_SNAPSHOTS);
                        backend(cx).settings.update_app(|s| s.save_snapshots = next);
                    }),
            )
            .into_any_element();

        let snapshots_row = self.highlight_row(
            "iroriSaveSnapshots",
            field_row(
                RowState {
                    label: t("settings.field.save_snapshots.label"),
                    help: t("settings.field.save_snapshots.help"),
                    ..RowState::default()
                },
                snapshots_ctrl,
                cx,
            ),
            cx,
        );

        let cloud_card = group_panel(cx)
            .child(auto_cloud_row)
            .child(div().border_t_1().border_color(border).child(snapshots_row));

        // ---- Cross-link card to Sync -> Cloud copies ----
        let sync_summary = self.sync.read(cx).summary(cx);
        let bridge_sub = match sync_summary.mode {
            SyncMode::Off => t("settings.saves.sync_bridge_off"),
            mode => {
                let mode_name: SharedString = match mode {
                    SyncMode::Off => t("settings.sync.mode_off"),
                    SyncMode::Folder => t("settings.sync.mode_folder"),
                    SyncMode::Webdav => SharedString::new_static("WebDAV"),
                    SyncMode::Rclone => SharedString::new_static("rclone"),
                };
                let copy_mode = match sync_summary.saves_copy_mode {
                    CopyMode::Off => t("settings.sync.copies.off"),
                    CopyMode::Manual => t("settings.sync.copies.manual"),
                    CopyMode::Automatic => t("settings.sync.copies.automatic"),
                };
                t_args(
                    "settings.saves.sync_bridge_on",
                    &[("mode", &mode_name), ("copies", &copy_mode)],
                )
            }
        };
        let sync_bridge = group_panel(cx).child(
            h_flex()
                .w_full()
                .px_4()
                .py_3()
                .gap_3()
                .items_center()
                .child(Icon::new(IconName::Cloud).text_color(muted_fg))
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .gap_0p5()
                        .child(
                            div()
                                .text_sm()
                                .font_medium()
                                .truncate()
                                .child(t("settings.saves.sync_bridge_title")),
                        )
                        .child(div().text_xs().text_color(muted_fg).child(bridge_sub)),
                )
                .child(
                    Button::new("saves-open-sync")
                        .outline()
                        .small()
                        .icon(IconName::ArrowRight)
                        .label(t("settings.saves.open_sync"))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.set_tab(BackupTab::Sync, cx);
                        })),
                ),
        );

        v_flex()
            .w_full()
            .gap_1()
            .child(group_header(
                t("settings.group.ludusavi_local"),
                None,
                true,
                cx,
            ))
            .child(ludusavi_card)
            .child(more_disclosure)
            .child(group_header(
                t("settings.group.store_cloud_saves"),
                None,
                false,
                cx,
            ))
            .child(cloud_card)
            .child(div().pt_3().child(sync_bridge))
    }

    fn render_ludusavi_more(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let border = theme.border;

        let path_picker = Button::new("pick-iroriLudusaviPath")
            .ghost()
            .xsmall()
            .icon(IconName::FolderOpen)
            .tooltip(t("settings.picker.choose_file"))
            .on_click(
                cx.listener(|this, _, window, cx| this.pick_ludusavi_path(false, window, cx)),
            );
        let path_row = self.highlight_row(
            "iroriLudusaviPath",
            field_row(
                RowState {
                    label: t("settings.field.ludusavi_path.label"),
                    help: t("settings.field.ludusavi_path.help"),
                    tooltip: Some((
                        SharedString::new_static("iroriLudusaviPath"),
                        t("settings.field.ludusavi_path.tip"),
                    )),
                    ..RowState::default()
                },
                Input::new(&self.ludusavi_path)
                    .id("field-iroriLudusaviPath")
                    .aria_label(t("settings.field.ludusavi_path.label"))
                    .w_72()
                    .suffix(path_picker)
                    .into_any_element(),
                cx,
            ),
            cx,
        );

        let folder_picker = Button::new("pick-iroriLudusaviBackupPath")
            .ghost()
            .xsmall()
            .icon(IconName::FolderOpen)
            .tooltip(t("settings.picker.choose_folder"))
            .on_click(cx.listener(|this, _, window, cx| this.pick_ludusavi_path(true, window, cx)));
        let folder_row = self.highlight_row(
            "iroriLudusaviBackupPath",
            field_row(
                RowState {
                    label: t("settings.field.ludusavi_backup_path.label"),
                    help: t("settings.field.ludusavi_backup_path.help"),
                    tooltip: Some((
                        SharedString::new_static("iroriLudusaviBackupPath"),
                        t("settings.field.ludusavi_backup_path.tip"),
                    )),
                    ..RowState::default()
                },
                Input::new(&self.ludusavi_backup_path)
                    .id("field-iroriLudusaviBackupPath")
                    .aria_label(t("settings.field.ludusavi_backup_path.label"))
                    .w_72()
                    .suffix(folder_picker)
                    .into_any_element(),
                cx,
            ),
            cx,
        );

        let pair_highlighted = matches!(
            self.highlighted_field,
            Some("iroriLudusaviFullLimit" | "iroriLudusaviDifferentialLimit")
        );
        let retention_row = paired_field_row(
            PairedRow {
                label: t("settings.pair.ludusavi_retention"),
                help: t("settings.pair.ludusavi_retention_help"),
                tooltip: Some((
                    SharedString::new_static("iroriLudusaviFullLimit"),
                    t("settings.pair.ludusavi_retention.tip"),
                )),
                first_sublabel: t("settings.pair.full_backups"),
                first_ctrl: Select::new(&self.ludusavi_full_limit)
                    .id("field-iroriLudusaviFullLimit")
                    .accessibility_label(t("settings.field.ludusavi_full_limit.label"))
                    .w_40()
                    .into_any_element(),
                separator: None,
                second_sublabel: t("settings.pair.incremental_per_full"),
                second_ctrl: Select::new(&self.ludusavi_diff_limit)
                    .id("field-iroriLudusaviDifferentialLimit")
                    .accessibility_label(t("settings.field.ludusavi_differential_limit.label"))
                    .w_40()
                    .into_any_element(),
                error: None,
            },
            cx,
        )
        .id("pair-iroriLudusaviFullLimit")
        .debug_selector(|| "pair-iroriLudusaviFullLimit".to_string())
        .when(pair_highlighted, |d| {
            d.rounded(theme.radius)
                .bg(theme.primary.opacity(0.14))
                .border_1()
                .border_color(theme.primary.opacity(0.45))
        });

        group_panel(cx)
            .child(path_row)
            .child(div().border_t_1().border_color(border).child(folder_row))
            .child(div().border_t_1().border_color(border).child(retention_row))
    }
}
