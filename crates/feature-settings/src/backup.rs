//! Settings → Saves & backup: unified data protection hub with three tabs
//! selected by live status cards (`Game saves`, `Library backup`, and `Sync`).

mod danger_zone;
mod library_tab;
mod overlays;
mod saves_tab;
mod tab_cards;

use std::path::{Path, PathBuf};
use std::time::Duration;

use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::select::{SelectEvent, SelectState};
use gpui_kit::component::{WindowExt as _, v_flex};
use gpui_kit::{
    AppContext as _, AsyncWindowContext, Context, Entity, IntoElement, ParentElement as _,
    PathPromptOptions, Render, SharedString, Styled as _, Subscription, Task, WeakEntity, Window,
};
use irori_core::BackupPrefs;
use irori_services::{Backend, Backup, BackupEntry, BackupParts, ExportFormat};
use irori_ui::i18n::{t, t_args};
use irori_ui::save_backups::LudusaviPanel;
use irori_ui::{backend, models};

use crate::form::ChoiceItem;
use crate::i18n::LanguageStamp;
use crate::restore_paths::RestorePaths;
use crate::sync::{SyncPanel, SyncPanelEvent};

/// Tabs of Settings → Saves & backup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BackupTab {
    Saves,
    Library,
    Sync,
}

/// Most scheduled backups kept.
pub(super) const MAX_KEEP: u32 = 50;
/// Most pre-sync safety snapshots kept per game.
pub(super) const MAX_SNAPSHOTS: u32 = 50;

/// Unified Saves & backup settings panel.
pub struct BackupPanel {
    pub(super) tab: BackupTab,
    /// Status-only Ludusavi detector shared between Game saves, header cards, and Sync.
    pub(super) ludusavi: Entity<LudusaviPanel>,
    /// Game saves tab controls.
    pub(super) ludusavi_interval: Entity<SelectState<Vec<ChoiceItem>>>,
    pub(super) ludusavi_path: Entity<InputState>,
    pub(super) ludusavi_backup_path: Entity<InputState>,
    pub(super) ludusavi_full_limit: Entity<SelectState<Vec<ChoiceItem>>>,
    pub(super) ludusavi_diff_limit: Entity<SelectState<Vec<ChoiceItem>>>,
    pub(super) ludusavi_more_open: bool,
    /// Highlighted field path after a deep link from global search.
    pub(super) highlighted_field: Option<&'static str>,
    highlight_timer: Option<Task<()>>,
    /// What a standalone manual backup file holds.
    pub(super) parts: BackupParts,
    /// Whether the inline "Save copy to file" composer is open.
    pub(super) manual_save_open: bool,
    /// Whether the "Included in backups" checklist is expanded.
    pub(super) included_open: bool,
    /// Whether all backups beyond the default preview limit are shown.
    pub(super) show_all_backups: bool,
    /// A backup chosen for restoring, and the parts to restore from it.
    pub(super) chosen: Option<(PathBuf, Backup)>,
    pub(super) restore_parts: BackupParts,
    /// The chosen backup's paths, as found on this computer.
    pub(crate) paths: RestorePaths,
    /// A restore is running.
    pub(super) restoring: bool,
    /// The backups in the backup folder, newest first.
    pub(super) listed: Vec<BackupEntry>,
    /// The backup folder is there (read with the list).
    pub(super) folder_exists: bool,
    pub(super) backing_up: bool,
    /// A full settings reset is running.
    pub(super) resetting: bool,
    pub(super) sync: Entity<SyncPanel>,
    language: LanguageStamp,
    _subscriptions: Vec<Subscription>,
}

impl BackupPanel {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let app = backend(cx).settings.app();
        let ludusavi = cx.new(|cx| LudusaviPanel::new(window, cx).status_only(true));
        let sync = cx.new(|cx| SyncPanel::new(ludusavi.clone(), window, cx));

        let ludusavi_interval =
            saves_tab::build_interval_select(app.game.ludusavi_interval, window, cx);
        let ludusavi_full_limit =
            saves_tab::build_full_limit_select(app.ludusavi_full_limit, window, cx);
        let ludusavi_diff_limit =
            saves_tab::build_diff_limit_select(&app.ludusavi_differential_limit, window, cx);

        let ludusavi_path = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(t("settings.field.ludusavi_path.placeholder"))
                .default_value(app.ludusavi_path.clone())
        });
        let ludusavi_backup_path = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(t("settings.field.ludusavi_backup_path.placeholder"))
                .default_value(app.ludusavi_backup_path.clone())
        });

        let settings = models(cx).settings.clone();
        let subscriptions = vec![
            cx.observe_in(&settings, window, |this, _, window, cx| {
                this.sync_saves_controls(window, cx);
                this.list(cx);
            }),
            cx.observe(&ludusavi, |_, _, cx| cx.notify()),
            cx.observe(&sync, |_, _, cx| cx.notify()),
            cx.subscribe(&sync, |this, _, event: &SyncPanelEvent, cx| match event {
                SyncPanelEvent::OpenSaves => this.set_tab(BackupTab::Saves, cx),
            }),
            cx.subscribe_in(
                &ludusavi_interval,
                window,
                |_, _, event: &SelectEvent<Vec<ChoiceItem>>, _, cx| {
                    if let SelectEvent::Confirm(Some(key)) = event
                        && let Some(mins) = saves_tab::parse_interval_key(key)
                    {
                        backend(cx)
                            .settings
                            .update_app(|s| s.game.ludusavi_interval = mins);
                    }
                },
            ),
            cx.subscribe_in(
                &ludusavi_full_limit,
                window,
                |_, _, event: &SelectEvent<Vec<ChoiceItem>>, _, cx| {
                    if let SelectEvent::Confirm(Some(key)) = event
                        && let Some(limit) = saves_tab::parse_full_limit_key(key)
                    {
                        backend(cx)
                            .settings
                            .update_app(|s| s.ludusavi_full_limit = limit);
                    }
                },
            ),
            cx.subscribe_in(
                &ludusavi_diff_limit,
                window,
                |_, _, event: &SelectEvent<Vec<ChoiceItem>>, _, cx| {
                    if let SelectEvent::Confirm(Some(key)) = event {
                        let val = saves_tab::parse_diff_limit_key(key);
                        backend(cx)
                            .settings
                            .update_app(|s| s.ludusavi_differential_limit = val);
                    }
                },
            ),
            cx.subscribe_in(
                &ludusavi_path,
                window,
                |this, _, event: &InputEvent, _, cx| {
                    if matches!(event, InputEvent::Blur | InputEvent::PressEnter { .. }) {
                        this.commit_ludusavi_paths(cx);
                    }
                },
            ),
            cx.subscribe_in(
                &ludusavi_backup_path,
                window,
                |this, _, event: &InputEvent, _, cx| {
                    if matches!(event, InputEvent::Blur | InputEvent::PressEnter { .. }) {
                        this.commit_ludusavi_paths(cx);
                    }
                },
            ),
        ];

        let mut this = Self {
            tab: BackupTab::Saves,
            ludusavi,
            ludusavi_interval,
            ludusavi_path,
            ludusavi_backup_path,
            ludusavi_full_limit,
            ludusavi_diff_limit,
            ludusavi_more_open: false,
            highlighted_field: None,
            highlight_timer: None,
            parts: BackupParts::ALL,
            manual_save_open: false,
            included_open: false,
            show_all_backups: false,
            chosen: None,
            restore_parts: BackupParts::ALL,
            paths: RestorePaths::default(),
            restoring: false,
            listed: Vec::new(),
            folder_exists: false,
            backing_up: false,
            resetting: false,
            sync,
            language: LanguageStamp::new(),
            _subscriptions: subscriptions,
        };
        this.list(cx);
        this
    }

    pub(crate) fn set_tab(&mut self, tab: BackupTab, cx: &mut Context<Self>) {
        if self.tab != tab {
            self.tab = tab;
            cx.notify();
        }
    }

    /// Deep-link from global search into a `FieldSection::Saves` setting:
    /// opens the owning tab (`Library` for `action.reset-settings`, `Saves`
    /// otherwise), expands `More options` if advanced, and highlights the
    /// target row briefly.
    pub(crate) fn reveal_field(
        &mut self,
        path: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if path == "action.reset-settings" {
            self.tab = BackupTab::Library;
        } else {
            self.tab = BackupTab::Saves;
            if matches!(
                path,
                "iroriLudusaviPath"
                    | "iroriLudusaviBackupPath"
                    | "iroriLudusaviFullLimit"
                    | "iroriLudusaviDifferentialLimit"
            ) {
                self.ludusavi_more_open = true;
            }
        }
        self.highlighted_field = Some(path);
        self.highlight_timer = Some(cx.spawn_in(window, async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(2500))
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.highlighted_field == Some(path) {
                    this.highlighted_field = None;
                    cx.notify();
                }
            });
        }));
        cx.notify();
        true
    }

    #[cfg(test)]
    pub(crate) fn set_tab_for_test(&mut self, tab: BackupTab, cx: &mut Context<Self>) {
        self.set_tab(tab, cx);
    }

    #[cfg(test)]
    pub(crate) fn tab_for_test(&self) -> BackupTab {
        self.tab
    }

    #[cfg(test)]
    pub(crate) fn highlighted_field_for_test(&self) -> Option<&'static str> {
        self.highlighted_field
    }

    #[cfg(test)]
    pub(crate) fn ludusavi_more_open_for_test(&self) -> bool {
        self.ludusavi_more_open
    }

    #[cfg(test)]
    pub(crate) fn set_manual_save_for_test(&mut self, open: bool, cx: &mut Context<Self>) {
        self.manual_save_open = open;
        cx.notify();
    }

    #[cfg(test)]
    pub(crate) fn set_included_open_for_test(&mut self, open: bool, cx: &mut Context<Self>) {
        self.included_open = open;
        cx.notify();
    }

    #[cfg(test)]
    pub(crate) fn sync_panel_for_test(&self) -> Entity<SyncPanel> {
        self.sync.clone()
    }

    /// Read the backup folder off the UI thread.
    pub(super) fn list(&mut self, cx: &mut Context<Self>) {
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

    pub(super) fn update_prefs(
        &mut self,
        cx: &mut Context<Self>,
        f: impl FnOnce(&mut BackupPrefs),
    ) {
        backend(cx).settings.update_app(|s| f(&mut s.backups));
        cx.notify();
    }

    /// Back up into the backup folder now.
    pub(super) fn back_up_now(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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

    pub(super) fn pick_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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

    /// Ask where to save a standalone backup file, then write it.
    pub(super) fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let parts = self.parts;
        let name = format!("irori-backup-{}.json", today());
        let rx = cx.prompt_for_new_path(&start_dir(), Some(&name));
        let view = cx.entity().downgrade();
        window
            .spawn(cx, async move |cx| {
                let Ok(Ok(Some(path))) = rx.await else { return };
                let result = run_backend(cx, move |b| b.write_backup(&path, parts)).await;
                if result.is_ok() {
                    let _ = view.update(cx, |this, cx| {
                        this.manual_save_open = false;
                        cx.notify();
                    });
                }
                report(
                    cx,
                    result.map(|()| t("settings.backup.saved")),
                    "settings.backup.save_failed",
                );
            })
            .detach();
    }

    /// Ask for a backup file and read it, ready to restore inline.
    pub(super) fn choose(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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

    /// Read a backup from the list, ready to restore inline right above the list.
    pub(super) fn choose_listed(
        &mut self,
        path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.manual_save_open = false;
        let view = cx.entity().downgrade();
        window
            .spawn(cx, async move |cx| load(view, path, cx).await)
            .detach();
    }

    /// Check the chosen backup's paths on this computer, with the folders
    /// the user found.
    pub(crate) fn plan_paths(&mut self, cx: &mut Context<Self>) {
        let Some((_, backup)) = self.chosen.clone() else {
            return;
        };
        let parts = self.restore_parts;
        let remaps = self.paths.remaps.clone();
        self.paths.automatic = backup.automatic_remaps();
        self.paths.checks = None;
        cx.notify();
        let b = backend(cx);
        let task = irori_ui::state::run_infallible(cx, async move {
            b.plan_restore(&backup, parts, &remaps).await
        });
        cx.spawn(async move |this, cx| {
            let Some(checks) = task.await else { return };
            let _ = this.update(cx, |this, cx| {
                this.paths.checks = Some(checks);
                cx.notify();
            });
        })
        .detach();
    }

    /// Restore the chosen parts of the chosen backup.
    pub(super) fn restore(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((path, backup)) = self.chosen.clone() else {
            return;
        };
        if self.restoring {
            return;
        }
        self.restoring = true;
        cx.notify();
        let parts = self.restore_parts;
        let remaps = self.paths.remaps.clone();
        let b = backend(cx);
        let task = irori_ui::run(cx, async move {
            let summary = b.restore_backup_with(&backup, parts, &remaps).await?;
            // Art saved beside an automatic backup comes back too.
            let art = b.clone();
            let _ = b
                .spawn_blocking(move || {
                    if let Err(e) = art.restore_backup_art(&path) {
                        tracing::warn!("restoring backed-up art: {e:#}");
                    }
                })
                .await;
            Ok(summary)
        });
        let view = cx.entity().downgrade();
        window
            .spawn(cx, async move |cx| {
                let result = task.await;
                let _ = view.update(cx, |this, cx| {
                    this.restoring = false;
                    cx.notify();
                });
                match result {
                    Ok(summary) => {
                        clear_choice(&view, cx);
                        let details = crate::restore_paths::restored_details(&summary);
                        let _ = cx.update(|window, cx| {
                            let note = match details {
                                Some(details) => Notification::success(details)
                                    .title(t("settings.backup.restored")),
                                None => Notification::success(t("settings.backup.restored")),
                            };
                            window.push_notification(note, cx);
                        });
                    }
                    Err(e) => report::<SharedString>(cx, Err(e), "settings.backup.restore_failed"),
                }
            })
            .detach();
    }

    /// Ask where to save the game list, then write it.
    pub(super) fn export(
        &mut self,
        format: ExportFormat,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
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
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.language.changed() {
            cx.defer_in(window, |this, window, cx| {
                this.retranslate_saves_controls(window, cx);
            });
        }
        let body = match self.tab {
            BackupTab::Saves => self.render_saves_tab(cx).into_any_element(),
            BackupTab::Library => self.render_library_tab(cx).into_any_element(),
            BackupTab::Sync => self.sync.clone().into_any_element(),
        };

        v_flex()
            .w_full()
            .gap_3()
            .child(self.render_tab_cards(cx))
            .child(body)
    }
}

/// Read the backup at `path` and offer it for restoring inline.
async fn load(view: WeakEntity<BackupPanel>, path: PathBuf, cx: &mut AsyncWindowContext) {
    let read_path = path.clone();
    let result = run_backend(cx, move |_| Backend::read_backup(&read_path)).await;
    match result {
        Ok(backup) => {
            let _ = view.update(cx, |this, cx| {
                this.manual_save_open = false;
                this.restore_parts = backup.parts();
                this.chosen = Some((path, backup));
                this.paths = RestorePaths::default();
                this.plan_paths(cx);
            });
        }
        Err(e) => report::<SharedString>(cx, Err(e), "settings.backup.read_failed"),
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

pub(super) fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}
