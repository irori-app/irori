//! Settings → Backup: automatic backups and the list of them, sync between
//! computers (see [`crate::sync`]), saving what the user set up in Irori to
//! a file, restoring it (on this or another computer), and exporting the
//! game list.

use std::path::{Path, PathBuf};

use gpui_kit::assets::IconName;
use gpui_kit::component::alert::Alert;
use gpui_kit::component::button::{Button, ButtonGroup, ButtonVariants as _};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::notification::Notification;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Selectable as _, Sizable as _, WindowExt as _, h_flex,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, AppContext as _, AsyncWindowContext, Context, Entity, IntoElement, ParentElement as _,
    PathPromptOptions, Render, SharedString, Styled as _, Subscription, WeakEntity, Window, div,
};
use irori_core::{BackupPrefs, BackupSchedule};
use irori_services::backup_schedule::local_time;
use irori_services::{Backend, Backup, BackupEntry, BackupKind, BackupParts, ExportFormat};
use irori_ui::i18n::{t, t_args};
use irori_ui::{backend, models};

use crate::sync::SyncPanel;
use crate::widgets::{group_heading, muted, panel};

const SCHEDULES: [BackupSchedule; 3] = [
    BackupSchedule::Off,
    BackupSchedule::Daily,
    BackupSchedule::Weekly,
];
/// Most scheduled backups kept.
const MAX_KEEP: u32 = 50;

/// The Backup section's state.
pub struct BackupPanel {
    /// What a new backup holds.
    parts: BackupParts,
    /// A backup chosen for restoring, and the parts to restore from it.
    chosen: Option<(PathBuf, Backup)>,
    restore_parts: BackupParts,
    /// The backups in the backup folder, newest first.
    listed: Vec<BackupEntry>,
    /// The backup folder is there (read with the list).
    folder_exists: bool,
    backing_up: bool,
    sync: Entity<SyncPanel>,
    _subscriptions: Vec<Subscription>,
}

impl BackupPanel {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let settings = models(cx).settings.clone();
        let mut this = Self {
            parts: BackupParts::ALL,
            chosen: None,
            restore_parts: BackupParts::ALL,
            listed: Vec::new(),
            folder_exists: false,
            backing_up: false,
            sync: cx.new(|cx| SyncPanel::new(window, cx)),
            _subscriptions: vec![cx.observe(&settings, |this, _, cx| this.list(cx))],
        };
        this.list(cx);
        this
    }

    /// Read the backup folder off the UI thread.
    fn list(&mut self, cx: &mut Context<Self>) {
        let b = backend(cx);
        let task = irori_ui::state::run_infallible(cx, async move {
            let b2 = b.clone();
            b.spawn_blocking(move || {
                let folder = b2.backup_status().folder;
                (b2.scheduled_backups(), folder.is_dir())
            })
            .await
        });
        cx.spawn(async move |this, cx| {
            let Some(Ok((listed, folder_exists))) = task.await else {
                return;
            };
            let _ = this.update(cx, |this, cx| {
                this.listed = listed;
                this.folder_exists = folder_exists;
                cx.notify();
            });
        })
        .detach();
    }

    fn update_prefs(&mut self, cx: &mut Context<Self>, f: impl FnOnce(&mut BackupPrefs)) {
        backend(cx).settings.update_app(|s| f(&mut s.backups));
        cx.notify();
    }

    /// Back up into the backup folder now.
    fn back_up_now(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.backing_up {
            return;
        }
        self.backing_up = true;
        cx.notify();
        let b = backend(cx);
        let task = irori_ui::run(cx, async move { b.back_up_now().await });
        let view = cx.entity().downgrade();
        window
            .spawn(cx, async move |cx| {
                let result = task.await;
                let _ = view.update(cx, |this, cx| {
                    this.backing_up = false;
                    this.list(cx);
                });
                report(
                    cx,
                    result.map(|_| t("settings.backup.backed_up")),
                    "settings.backup.save_failed",
                );
            })
            .detach();
    }

    fn pick_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some(t("settings.backup.choose_folder_prompt")),
        });
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(paths))) = rx.await
                && let Some(p) = paths.into_iter().next()
            {
                let _ = this.update(cx, |this, cx| {
                    let folder = p.to_string_lossy().into_owned();
                    this.update_prefs(cx, |b| b.folder = folder);
                    this.list(cx);
                });
            }
        })
        .detach();
    }

    /// Ask where to save a backup, then write it.
    fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let parts = self.parts;
        let name = format!("irori-backup-{}.json", today());
        let rx = cx.prompt_for_new_path(&start_dir(), Some(&name));
        window
            .spawn(cx, async move |cx| {
                let Ok(Ok(Some(path))) = rx.await else { return };
                let result = run_backend(cx, move |b| b.write_backup(&path, parts)).await;
                report(
                    cx,
                    result.map(|()| t("settings.backup.saved")),
                    "settings.backup.save_failed",
                );
            })
            .detach();
    }

    /// Ask for a backup file and read it, ready to restore.
    fn choose(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(t("settings.backup.choose_prompt")),
        });
        let view = cx.entity().downgrade();
        window
            .spawn(cx, async move |cx| {
                let Ok(Ok(Some(mut paths))) = rx.await else {
                    return;
                };
                let Some(path) = paths.pop() else { return };
                load(view, path, cx).await;
            })
            .detach();
    }

    /// Read a backup from the list, ready to restore.
    fn choose_listed(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let view = cx.entity().downgrade();
        window
            .spawn(cx, async move |cx| load(view, path, cx).await)
            .detach();
    }

    /// Restore the chosen parts of the chosen backup.
    fn restore(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((path, backup)) = self.chosen.clone() else {
            return;
        };
        let parts = self.restore_parts;
        let view = cx.entity().downgrade();
        window
            .spawn(cx, async move |cx| {
                let result = run_backend(cx, move |b| {
                    let summary = b.restore_backup(&backup, parts)?;
                    // Art saved beside an automatic backup comes back too.
                    if let Err(e) = b.restore_backup_art(&path) {
                        tracing::warn!("restoring backed-up art: {e:#}");
                    }
                    Ok(summary)
                })
                .await;
                if result.is_ok() {
                    clear_choice(&view, cx);
                }
                report(
                    cx,
                    result.map(|_| t("settings.backup.restored")),
                    "settings.backup.restore_failed",
                );
            })
            .detach();
    }

    /// Ask where to save the game list, then write it.
    fn export(&mut self, format: ExportFormat, window: &mut Window, cx: &mut Context<Self>) {
        let ext = match format {
            ExportFormat::Csv => "csv",
            ExportFormat::Json => "json",
        };
        let name = format!("irori-games-{}.{ext}", today());
        let rx = cx.prompt_for_new_path(&start_dir(), Some(&name));
        window
            .spawn(cx, async move |cx| {
                let Ok(Ok(Some(path))) = rx.await else { return };
                let result = run_backend(cx, move |b| b.export_games(&path, format)).await;
                report(
                    cx,
                    result
                        .map(|n| t_args("settings.backup.exported", &[("count", &n.to_string())])),
                    "settings.backup.export_failed",
                );
            })
            .detach();
    }
}

impl Render for BackupPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let parts = self.parts;
        let backup = panel(cx)
            .p_4()
            .gap_3()
            .child(muted(t("settings.backup.backup_description"), cx))
            .child(part_checkboxes(
                "backup",
                parts,
                None,
                cx,
                |this, parts, _| this.parts = parts,
            ))
            .child(
                h_flex().child(
                    Button::new("backup-save")
                        .primary()
                        .icon(IconName::Save)
                        .label(t("settings.backup.save"))
                        .disabled(!parts.any())
                        .on_click(cx.listener(|this, _, window, cx| this.save(window, cx))),
                ),
            );

        let restore = match &self.chosen {
            None => panel(cx)
                .p_4()
                .gap_3()
                .child(muted(t("settings.backup.restore_description"), cx))
                .child(
                    h_flex().child(
                        Button::new("backup-choose")
                            .outline()
                            .icon(IconName::FolderOpen)
                            .label(t("settings.backup.choose"))
                            .on_click(cx.listener(|this, _, window, cx| this.choose(window, cx))),
                    ),
                ),
            Some((path, backup)) => {
                let available = backup.parts();
                let created = backup.created.chars().take(10).collect::<String>();
                panel(cx)
                    .p_4()
                    .gap_3()
                    .child(v_flex().gap_0p5().child(file_name(path)).child(muted(
                        t_args("settings.backup.made_on", &[("date", &created)]),
                        cx,
                    )))
                    .child(part_checkboxes(
                        "restore",
                        self.restore_parts,
                        Some(available),
                        cx,
                        |this, parts, _| this.restore_parts = parts,
                    ))
                    .child(muted(t("settings.backup.restore_note"), cx))
                    .child(
                        h_flex()
                            .gap_2()
                            .child(
                                Button::new("backup-restore")
                                    .primary()
                                    .icon(IconName::ArchiveRestore)
                                    .label(t("settings.backup.restore"))
                                    .disabled(!self.restore_parts.any())
                                    .on_click(
                                        cx.listener(|this, _, window, cx| this.restore(window, cx)),
                                    ),
                            )
                            .child(
                                Button::new("backup-cancel")
                                    .ghost()
                                    .label(t("settings.backup.cancel"))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.chosen = None;
                                        cx.notify();
                                    })),
                            ),
                    )
            }
        };

        let export = panel(cx)
            .p_4()
            .gap_3()
            .child(muted(t("settings.backup.export_description"), cx))
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("export-csv")
                            .outline()
                            .icon(IconName::FileSpreadsheet)
                            .label(t("settings.backup.export_csv"))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.export(ExportFormat::Csv, window, cx)
                            })),
                    )
                    .child(
                        Button::new("export-json")
                            .outline()
                            .icon(IconName::FileBraces)
                            .label(t("settings.backup.export_json"))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.export(ExportFormat::Json, window, cx)
                            })),
                    ),
            );

        v_flex()
            .gap_2()
            .child(group_heading(t("settings.backup.auto_title"), cx))
            .child(self.render_schedule(cx))
            .child(group_heading(t("settings.sync.title"), cx))
            .child(self.sync.clone())
            .child(group_heading(t("settings.backup.backup_title"), cx))
            .child(backup)
            .child(group_heading(t("settings.backup.restore_title"), cx))
            .child(restore)
            .child(group_heading(t("settings.backup.export_title"), cx))
            .child(export)
    }
}

/// One part's checkbox: label key, checked, enabled, and how to set it.
type PartEntry = (&'static str, bool, bool, fn(&mut BackupParts, bool));

/// A checkbox per part. With `available`, parts the backup lacks are off and
/// disabled.
fn part_checkboxes(
    prefix: &'static str,
    parts: BackupParts,
    available: Option<BackupParts>,
    cx: &mut Context<BackupPanel>,
    set: fn(&mut BackupPanel, BackupParts, &mut Context<BackupPanel>),
) -> impl IntoElement {
    let has = available.unwrap_or(BackupParts::ALL);
    let entries: [PartEntry; 4] = [
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

/// The parts automatic backups hold.
fn schedule_parts(prefs: &BackupPrefs) -> BackupParts {
    BackupParts {
        settings: prefs.settings,
        game_settings: prefs.game_settings,
        library: prefs.library,
        playtime: prefs.playtime,
    }
}

/// Read the backup at `path` and offer it for restoring.
async fn load(view: WeakEntity<BackupPanel>, path: PathBuf, cx: &mut AsyncWindowContext) {
    let read_path = path.clone();
    let result = run_backend(cx, move |_| Backend::read_backup(&read_path)).await;
    match result {
        Ok(backup) => {
            let _ = view.update(cx, |this, cx| {
                this.restore_parts = backup.parts();
                this.chosen = Some((path, backup));
                cx.notify();
            });
        }
        Err(e) => report::<SharedString>(cx, Err(e), "settings.backup.read_failed"),
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

impl BackupPanel {
    /// Automatic backups: schedule, folder, how many, what goes in, the
    /// last result and the backups there.
    fn render_schedule(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let b = backend(cx);
        let prefs = b.settings.app().backups.clone();
        let status = b.backup_status();
        let busy = self.backing_up || status.running;

        let schedule = ButtonGroup::new("backup-schedule")
            .outline()
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

        let last = match (&status.last_error, &status.last_ok) {
            (Some(error), _) => Alert::error("backup-error", error.clone())
                .title(t("settings.backup.last_failed"))
                .into_any_element(),
            (None, Some(ok)) => h_flex()
                .gap_2()
                .child(
                    gpui_kit::component::Icon::new(IconName::CircleCheck)
                        .text_color(cx.theme().success),
                )
                .child(t_args(
                    "settings.backup.last_ok",
                    &[("time", &local_time(ok))],
                ))
                .into_any_element(),
            (None, None) => muted(t("settings.backup.never"), cx).into_any_element(),
        };

        let custom_folder = !prefs.folder.trim().is_empty();
        let folder = v_flex()
            .gap_1()
            .child(irori_ui::style::caption(t("settings.backup.folder"), cx))
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .child(status.folder.to_string_lossy().into_owned()),
                    )
                    .child(
                        Button::new("backup-folder")
                            .outline()
                            .small()
                            .icon(IconName::FolderOpen)
                            .label(t("settings.backup.choose_folder"))
                            .on_click(
                                cx.listener(|this, _, window, cx| this.pick_folder(window, cx)),
                            ),
                    )
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
                    }),
            );

        let keep = prefs.keep.clamp(1, MAX_KEEP);
        let keep_row = h_flex()
            .gap_2()
            .child(div().flex_1().min_w_0().child(t("settings.backup.keep")))
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

        let parts = schedule_parts(&prefs);
        let contents = v_flex()
            .gap_2()
            .child(irori_ui::style::caption(t("settings.backup.contents"), cx))
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
            )
            .child(
                Checkbox::new("schedule-before-updates")
                    .label(t("settings.backup.before_updates"))
                    .checked(prefs.before_updates)
                    .small()
                    .on_change(cx.listener(|this, v: &bool, _, cx| {
                        let v = *v;
                        this.update_prefs(cx, |p| p.before_updates = v)
                    })),
            );

        let actions = h_flex()
            .gap_2()
            .child(
                Button::new("backup-now")
                    .primary()
                    .icon(IconName::Save)
                    .label(if busy {
                        t("settings.backup.running")
                    } else {
                        t("settings.backup.now")
                    })
                    .loading(busy)
                    .disabled(busy || !parts.any())
                    .on_click(cx.listener(|this, _, window, cx| this.back_up_now(window, cx))),
            )
            .child(
                Button::new("backup-open-folder")
                    .ghost()
                    .icon(IconName::FolderOpen)
                    .label(t("settings.backup.open_folder"))
                    .disabled(!self.folder_exists)
                    .on_click({
                        let folder = status.folder.clone();
                        move |_, _, cx: &mut App| cx.open_with_system(&folder)
                    }),
            );

        let list = v_flex()
            .gap_1()
            .child(irori_ui::style::caption(
                t("settings.backup.list_title"),
                cx,
            ))
            .when(self.listed.is_empty(), |l| {
                l.child(muted(t("settings.backup.list_empty"), cx))
            })
            .children(self.listed.iter().enumerate().map(|(ix, entry)| {
                let path = entry.path.clone();
                let mut kind = kind_label(&entry.kind).to_string();
                if entry.art {
                    kind = t_args("settings.backup.with_art", &[("kind", &kind)]).to_string();
                }
                h_flex()
                    .gap_2()
                    .py_1()
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .child(div().truncate().child(local_time(&entry.created)))
                            .child(muted(kind, cx).truncate()),
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
                    )
            }));

        panel(cx)
            .p_4()
            .gap_4()
            .child(muted(t("settings.backup.auto_description"), cx))
            .child(h_flex().child(schedule))
            .child(last)
            .child(folder)
            .child(keep_row)
            .child(contents)
            .child(actions)
            .child(list)
    }
}

/// Run blocking backend work off the UI thread.
async fn run_backend<T: Send + 'static>(
    cx: &mut AsyncWindowContext,
    f: impl FnOnce(Backend) -> anyhow::Result<T> + Send + 'static,
) -> anyhow::Result<T> {
    let task = cx.update(|_, cx| {
        let b = backend(cx);
        irori_ui::run(cx, async move { f(b) })
    })?;
    task.await
}

/// Tell the user how an operation went.
fn report<S: Into<SharedString>>(
    cx: &mut AsyncWindowContext,
    result: anyhow::Result<S>,
    failed_key: &'static str,
) {
    let _ = cx.update(|window, cx| match result {
        Ok(message) => window.push_notification(Notification::success(message.into()), cx),
        Err(e) => window.push_notification(
            Notification::error(format!("{e:#}")).title(t(failed_key)),
            cx,
        ),
    });
}

fn clear_choice(view: &WeakEntity<BackupPanel>, cx: &mut AsyncWindowContext) {
    let _ = view.update(cx, |this, cx| {
        this.chosen = None;
        cx.notify();
    });
}

/// Where save dialogs open: Documents when there is one, else home.
fn start_dir() -> PathBuf {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default();
    let documents = home.join("Documents");
    if documents.is_dir() { documents } else { home }
}

/// Today as `YYYY-MM-DD`, for file names.
fn today() -> String {
    irori_core::util::now_rfc3339().chars().take(10).collect()
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}
