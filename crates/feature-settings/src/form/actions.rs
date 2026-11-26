use anyhow::Context as _;
use gpui_kit::component::WindowExt as _;
use gpui_kit::component::button::ButtonVariant;
use gpui_kit::component::notification::Notification;
use gpui_kit::{Context, Task, Window};
use irori_core::Paths;
use irori_core::settings::AppSettings;
use irori_ui::i18n::{t, t_args};
use irori_ui::{backend, models};

use crate::registry::ActionKind;

use super::SettingsForm;

impl SettingsForm {
    // ---- actions ----

    pub(super) fn run_action(
        &mut self,
        action: ActionKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match action {
            ActionKind::OpenLogsFolder => {
                let dir = Paths::get().logs_dir();
                if let Err(e) = std::fs::create_dir_all(&dir) {
                    window.push_notification(
                        Notification::error(format!("{e}")).title(t("settings.error.logs_folder")),
                        cx,
                    );
                    return;
                }
                cx.open_with_system(&dir);
            }
            ActionKind::ClearCaches => {
                let weak = cx.entity().downgrade();
                window.open_alert_dialog(cx, move |alert, _, _| {
                    let weak = weak.clone();
                    alert
                        .title(t("settings.clear_caches.title"))
                        .description(t("settings.clear_caches.description"))
                        .show_cancel(true)
                        .cancel_text(t("settings.common.cancel"))
                        .ok_text(t("settings.clear_caches.confirm"))
                        .on_ok(move |_, window, cx| {
                            weak.update(cx, |this, cx| this.clear_caches(window, cx))
                                .ok();
                            true
                        })
                });
            }
            ActionKind::ResetSettings => {
                let weak = cx.entity().downgrade();
                window.open_alert_dialog(cx, move |alert, _, _| {
                    let weak = weak.clone();
                    alert
                        .title(t("settings.reset_settings.title"))
                        .description(t("settings.reset_settings.description"))
                        .show_cancel(true)
                        .cancel_text(t("settings.common.cancel"))
                        .ok_text(t("settings.reset_settings.confirm"))
                        .ok_variant(ButtonVariant::Danger)
                        .on_ok(move |_, window, cx| {
                            weak.update(cx, |this, cx| this.reset_settings(window, cx))
                                .ok();
                            true
                        })
                });
            }
            ActionKind::ImportHeroic => self.import_heroic(window, cx),
            ActionKind::ImportLutris => self.import_lutris(window, cx),
            ActionKind::ImportBottles => self.import_launcher(
                ActionKind::ImportBottles,
                irori_ui::hooks::import_from_bottles(cx),
                window,
                cx,
            ),
            ActionKind::ImportFaugus => self.import_launcher(
                ActionKind::ImportFaugus,
                irori_ui::hooks::import_from_faugus(cx),
                window,
                cx,
            ),
            ActionKind::ImportPlaynite => self.import_launcher(
                ActionKind::ImportPlaynite,
                irori_ui::hooks::import_from_playnite(cx),
                window,
                cx,
            ),
            ActionKind::OpenWindowsFiles => self.open_windows_files(window, cx),
        }
    }

    fn open_windows_files(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let action = ActionKind::OpenWindowsFiles;
        self.busy.insert(action);
        let b = backend(cx);
        let task = irori_ui::run(cx, async move { b.open_windows_files_by_default().await });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            this.update_in(cx, |this, window, cx| {
                this.busy.remove(&action);
                let note = match result {
                    Ok(()) => Notification::success(t("settings.windows_files.done")),
                    Err(e) => Notification::error(format!("{e:#}"))
                        .title(t("settings.windows_files.failed")),
                };
                window.push_notification(note, cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn clear_caches(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.busy.insert(ActionKind::ClearCaches);
        let task = irori_ui::run(cx, async move {
            let paths = Paths::get();
            for dir in [paths.images_dir(), paths.cache.join("stores")] {
                match tokio::fs::remove_dir_all(&dir).await {
                    Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                        return Err(e).with_context(|| format!("removing {}", dir.display()));
                    }
                    _ => {}
                }
            }
            Ok(())
        });
        self.finish_action(
            ActionKind::ClearCaches,
            task,
            "settings.clear_caches.done",
            window,
            cx,
        );
    }

    fn reset_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.busy.insert(ActionKind::ResetSettings);
        let b = backend(cx);
        let task = irori_ui::run(cx, async move {
            // Both calls write files synchronously.
            tokio::task::spawn_blocking(move || {
                b.settings.replace_app(AppSettings::default());
                b.settings.reset_all_games();
            })
            .await
            .context("resetting settings")
        });
        self.finish_action(
            ActionKind::ResetSettings,
            task,
            "settings.reset_settings.done",
            window,
            cx,
        );
    }

    fn import_lutris(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.busy.insert(ActionKind::ImportLutris);
        let task = irori_ui::hooks::import_from_lutris(cx);
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            this.update_in(cx, |this, window, cx| {
                this.busy.remove(&ActionKind::ImportLutris);
                match result {
                    Ok(summary) => {
                        models::refresh_library(false, cx);
                        irori_ui::lutris_summary::open_lutris_summary(summary, window, cx);
                    }
                    Err(e) => window.push_notification(
                        Notification::error(format!("{e:#}"))
                            .title(t("settings.error.import_lutris")),
                        cx,
                    ),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// Run a Bottles, Faugus Launcher or Playnite import and show what it did.
    fn import_launcher(
        &mut self,
        action: ActionKind,
        task: gpui_kit::Task<anyhow::Result<irori_ui::hooks::LutrisSummary>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.busy.insert(action);
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            this.update_in(cx, |this, window, cx| {
                this.busy.remove(&action);
                match result {
                    Ok(summary) => {
                        models::refresh_library(false, cx);
                        irori_ui::lutris_summary::open_lutris_summary(summary, window, cx);
                    }
                    Err(e) => window.push_notification(
                        Notification::error(format!("{e:#}"))
                            .title(t("settings.error.import_launcher")),
                        cx,
                    ),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn import_heroic(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.busy.insert(ActionKind::ImportHeroic);
        self.import_summary = None;
        let task = irori_ui::hooks::import_from_heroic(cx);
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            this.update_in(cx, |this, window, cx| {
                this.busy.remove(&ActionKind::ImportHeroic);
                match result {
                    Ok(s) => {
                        let stores = if s.stores.is_empty() {
                            t("settings.import.no_accounts").to_string()
                        } else {
                            s.stores.join(", ")
                        };
                        this.import_summary = Some(t_args(
                            "settings.import.summary",
                            &[
                                ("stores", &stores),
                                ("games", &s.games_with_settings.to_string()),
                                ("favourites", &s.favourites.to_string()),
                                ("categories", &s.categories.to_string()),
                            ],
                        ));
                        models::refresh_library(false, cx);
                    }
                    Err(e) => window.push_notification(
                        Notification::error(format!("{e:#}"))
                            .title(t("settings.error.import_heroic")),
                        cx,
                    ),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn finish_action(
        &mut self,
        action: ActionKind,
        task: Task<anyhow::Result<()>>,
        success_key: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            this.update_in(cx, |this, window, cx| {
                this.busy.remove(&action);
                match result {
                    Ok(()) => {
                        window.push_notification(Notification::success(t(success_key)), cx);
                        if action == ActionKind::ResetSettings {
                            irori_ui::theme::apply(
                                &backend(cx).settings.app().ui,
                                Some(window),
                                cx,
                            );
                            this.reload(window, cx);
                        }
                        if action == ActionKind::ClearCaches {
                            models::refresh_library(true, cx);
                        }
                    }
                    Err(e) => window.push_notification(
                        Notification::error(format!("{e:#}")).title(t("settings.error.generic")),
                        cx,
                    ),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }
}
