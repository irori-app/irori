//! What Big Picture asks the backend to do: game commands, favourites and
//! hidden games, and the quick menu's entries. Failures become notifications.

use std::sync::Arc;

use anyhow::{Context as _, Result, bail};
use gpui_kit::component::WindowExt as _;
use gpui_kit::component::notification::Notification;
use gpui_kit::{App, Context, SharedString, Task, Window};
use irori_core::{GameId, GameInfo, InstallInfoQuery, InstallPlatform, RemoveArgs, UpdateRequest};
use irori_services::LaunchParams;
use irori_ui::i18n::{t, t_args};
use irori_ui::models;

use super::BigPictureView;
use crate::model::{
    GameAction, MenuItem, default_install_request, default_platform, menu_items, next_scale,
};
use crate::text::store_name;

impl BigPictureView {
    /// Run `action` for `game`; Install uses `platform` when given, else
    /// the game's default.
    pub(super) fn run_game_action(
        &mut self,
        action: GameAction,
        game: Arc<GameInfo>,
        platform: Option<InstallPlatform>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let b = irori_ui::backend(cx);
        let id = game.id();
        let title = game.title.clone();
        let error = |key: &str| t_args(key, &[("title", title.as_str())]);
        // What a fix offered for a failed launch starts again.
        let verbose = LaunchParams {
            verbose: true,
            ..Default::default()
        };
        let retry = if action == GameAction::PlayWithLog {
            verbose.clone()
        } else {
            LaunchParams::default()
        };
        let (task, error_title): (Task<Result<()>>, SharedString) = match action {
            GameAction::Play => {
                let id = id.clone();
                (
                    irori_ui::run(cx, async move { b.launch(&id, None, Vec::new()).await }),
                    error("bigpicture.error.launch"),
                )
            }
            GameAction::PlayWithLog => {
                self.show_log = true;
                let id = id.clone();
                let params = verbose.clone();
                (
                    irori_ui::run(cx, async move { b.launch_with(&id, params).await }),
                    error("bigpicture.error.launch"),
                )
            }
            GameAction::Stop => {
                let id = id.clone();
                (
                    irori_ui::run(cx, async move { b.stop(&id).await }),
                    error("bigpicture.error.stop"),
                )
            }
            GameAction::Install => {
                let app = models(cx).settings.read(cx).app.clone();
                let dir = irori_core::paths::expand_home(&app.default_install_path);
                let platform = platform.unwrap_or_else(|| default_platform(&game));
                let id = id.clone();
                let game = game.clone();
                (
                    irori_ui::run(cx, async move {
                        let third_party = |launcher: &str| {
                            t_args(
                                "bigpicture.error.third_party",
                                &[("title", game.title.as_str()), ("launcher", launcher)],
                            )
                        };
                        if let Some(launcher) = &game.third_party_managed_app {
                            bail!("{}", third_party(launcher));
                        }
                        let store = b.store(id.runner).with_context(|| {
                            t_args(
                                "bigpicture.error.store_unavailable",
                                &[("store", store_name(id.runner).as_str())],
                            )
                        })?;
                        let info = store
                            .install_info(&id.app_name, platform, &InstallInfoQuery::default())
                            .await
                            .with_context(|| t("bigpicture.error.install_options"))?;
                        if let Some(launcher) = &info.third_party_managed_app {
                            bail!("{}", third_party(launcher));
                        }
                        let request = default_install_request(&info, &dir, platform);
                        b.enqueue_install(id, game.title.clone(), request, info.download_size);
                        Ok(())
                    }),
                    error("bigpicture.error.install"),
                )
            }
            GameAction::CancelDownload => {
                let id = id.clone();
                (
                    irori_ui::run(cx, async move {
                        b.cancel_download(&id, true);
                        Ok(())
                    }),
                    error("bigpicture.error.cancel_download"),
                )
            }
            GameAction::Update => {
                let id = id.clone();
                let queued_title = title.clone();
                (
                    irori_ui::run(cx, async move {
                        b.enqueue_update(id, queued_title, UpdateRequest::default());
                        Ok(())
                    }),
                    error("bigpicture.error.update"),
                )
            }
            // Asks first; see `Overlay::Uninstall`.
            GameAction::Uninstall => return,
        };
        self.track(id, task, error_title, retry, window, cx);
    }

    pub(super) fn uninstall(&mut self, id: GameId, window: &mut Window, cx: &mut Context<Self>) {
        let title = self
            .find_game(&id)
            .map(|g| g.title.clone())
            .unwrap_or_else(|| id.app_name.to_string());
        let b = irori_ui::backend(cx);
        let task_id = id.clone();
        let task = irori_ui::run(cx, async move {
            b.uninstall(
                &task_id,
                RemoveArgs {
                    delete_files: true,
                    ..Default::default()
                },
            )
            .await
        });
        let error_title = t_args("bigpicture.error.uninstall", &[("title", title.as_str())]);
        self.track(id, task, error_title, LaunchParams::default(), window, cx);
    }

    /// Mark `id` busy until `task` finishes, then report a failure (a
    /// launch's fix starts the game again with `retry`).
    fn track(
        &mut self,
        id: GameId,
        task: Task<Result<()>>,
        error_title: SharedString,
        retry: LaunchParams,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.pending.insert(id.clone());
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.pending.remove(&id);
                match result {
                    Ok(()) => {}
                    // Only Play can hit saves that changed in both places.
                    Err(e)
                        if irori_ui::save_conflict::resolve_launch_error(
                            &id, retry, &e, window, cx,
                        ) => {}
                    Err(e) => window.push_notification(
                        Notification::error(format!("{e:#}")).title(error_title),
                        cx,
                    ),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Saving user data writes a file, so it runs off the UI thread.
    pub(super) fn set_favourite(&mut self, id: GameId, on: bool, cx: &mut Context<Self>) {
        let b = irori_ui::backend(cx);
        irori_ui::state::run_infallible(cx, async move { b.userdata.set_favourite(&id, on) })
            .detach();
    }

    pub(super) fn set_hidden(&mut self, id: GameId, on: bool, cx: &mut Context<Self>) {
        let b = irori_ui::backend(cx);
        irori_ui::state::run_infallible(cx, async move { b.userdata.set_hidden(&id, on) }).detach();
    }

    pub(super) fn menu(&self, cx: &App) -> Vec<MenuItem> {
        let m = models(cx);
        let queue = &m.downloads.read(cx).queue;
        let scale = m.settings.read(cx).app.ui.big_picture_scale;
        menu_items(
            !queue.active.is_empty(),
            queue.paused,
            scale,
            self.look.layout(),
        )
    }

    pub(super) fn activate_menu_item(
        &mut self,
        item: MenuItem,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let b = irori_ui::backend(cx);
        match item {
            MenuItem::PauseDownloads | MenuItem::ResumeDownloads => {
                let pause = item == MenuItem::PauseDownloads;
                let task = irori_ui::run(cx, async move {
                    if pause {
                        b.pause_downloads();
                    } else {
                        b.resume_downloads();
                    }
                    Ok(())
                });
                cx.spawn_in(window, async move |_, cx| {
                    if let Err(e) = task.await {
                        let _ = cx.update(|window, cx| {
                            window.push_notification(
                                Notification::error(format!("{e:#}"))
                                    .title(t("bigpicture.error.downloads")),
                                cx,
                            )
                        });
                    }
                })
                .detach();
            }
            MenuItem::InterfaceSize(current) => {
                let next = next_scale(current);
                let task = irori_ui::run(cx, async move {
                    b.settings.update_app(|app| app.ui.big_picture_scale = next);
                    Ok::<_, anyhow::Error>(())
                });
                task.detach();
            }
            MenuItem::Layout(_) => self.next_layout(cx),
            MenuItem::Themes => self.open_editor(window, cx),
            MenuItem::ToggleFullscreen => window.toggle_fullscreen(),
            MenuItem::SetUpEmulation => {
                self.overlay = None;
                crate::exit_big_picture(cx);
                irori_ui::hooks::open_emulation_setup(window, cx);
            }
            MenuItem::ExitBigPicture => {
                self.overlay = None;
                crate::exit_big_picture(cx);
            }
            MenuItem::QuitIrori => cx.quit(),
        }
        cx.notify();
    }
}
