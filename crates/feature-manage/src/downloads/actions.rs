//! User actions, background disk/countdown tasks, and dialogs for
//! [`DownloadsView`].

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::{
    ActiveTheme as _, Sizable as _, StyledExt as _, WindowExt as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{Context, ExternalPaths, ParentElement as _, Styled as _, Window, div, rems};
use irori_core::GameId;
use irori_services::{DownloadKind, LaunchParams};
use irori_ui::components::{GameCover, StoreBadge};
use irori_ui::i18n::{t, t_args};
use irori_ui::state::run_infallible;
use irori_ui::{backend, models};

use super::add_dialog::{is_download_link_text, open_add_download_dialog};
use super::history::HISTORY_PAGE;
use super::{DownloadsView, HistoryFilter, Step, when_done};
use crate::PasteDownloadLink;
use crate::common::{backend_op, notify_error};

impl DownloadsView {
    /// Read the queue's disks again, off the UI thread.
    pub(super) fn read_disks(&mut self, cx: &mut Context<Self>) {
        let b = backend(cx);
        let task = irori_ui::run(cx, async move {
            Ok(tokio::task::spawn_blocking(move || b.download_disks()).await?)
        });
        cx.spawn(async move |this, cx| {
            // Unreadable disks leave the line out; the queue still runs.
            if let Ok(disks) = task.await {
                let _ = this.update(cx, |this, cx| {
                    this.disks = disks;
                    cx.notify();
                });
            }
        })
        .detach();
    }

    /// Re-read configured and auto-detected install disks off the UI thread
    /// (for example when returning to the Downloads page after mounting a drive).
    pub fn refresh_install_disks(&mut self, cx: &mut Context<Self>) {
        self.install_disks_loaded = false;
        self.sync_install_disks(cx);
    }

    /// Read configured and auto-detected install disks off the UI thread when
    /// library folders change, a download finishes, or the page is revisited.
    pub(super) fn sync_install_disks(&mut self, cx: &mut Context<Self>) {
        let folders: Vec<String> = models(cx)
            .settings
            .read(cx)
            .app
            .library_folders()
            .into_iter()
            .map(|f| f.path)
            .collect();
        if folders == self.install_folders_key && self.install_disks_loaded {
            return;
        }
        self.install_folders_key = folders;
        self.install_disks_loaded = true;
        let b = backend(cx);
        let task = irori_ui::run(cx, async move {
            Ok(tokio::task::spawn_blocking(move || b.install_disks()).await?)
        });
        cx.spawn(async move |this, cx| {
            if let Ok(disks) = task.await {
                let _ = this.update(cx, |this, cx| {
                    this.install_disks = disks;
                    cx.notify();
                });
            }
        })
        .detach();
    }

    /// Keep redrawing while a countdown runs; stop when it ends.
    pub(super) fn sync_countdown_tick(&mut self, counting: bool, cx: &mut Context<Self>) {
        if !counting {
            self.countdown_tick = None;
        } else if self.countdown_tick.is_none() {
            self.countdown_tick = Some(cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor()
                        .timer(when_done::COUNTDOWN_TICK)
                        .await;
                    if this.update(cx, |_, cx| cx.notify()).is_err() {
                        break;
                    }
                }
            }));
        }
    }

    pub(super) fn toggle_pause(&mut self, cx: &mut Context<Self>) {
        if self.queue.paused {
            backend_op(cx, |b| b.resume_downloads());
        } else {
            backend_op(cx, |b| b.pause_downloads());
        }
    }

    pub(super) fn toggle_pause_all(&mut self, cx: &mut Context<Self>) {
        let web = models(cx).downloads.read(cx).web.clone();
        if self.all_active_paused(&web) {
            backend_op(cx, |b| b.resume_all_downloads());
        } else {
            backend_op(cx, |b| b.pause_all_downloads());
        }
    }

    pub(super) fn paste_link(
        &mut self,
        _: &PasteDownloadLink,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if window.has_active_dialog(cx) {
            return;
        }
        if let Some(text) = cx
            .read_from_clipboard()
            .and_then(|item| item.text())
            .map(|s| s.trim().to_string())
            .filter(|s| is_download_link_text(s))
        {
            open_add_download_dialog(Some(text), window, cx);
        }
    }

    pub(super) fn on_paths_dropped(
        &mut self,
        paths: &ExternalPaths,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if window.has_active_dialog(cx) {
            return;
        }
        let torrent = paths.paths().iter().find(|p| {
            p.extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("torrent"))
        });
        if let Some(path) = torrent {
            open_add_download_dialog(Some(path.display().to_string()), window, cx);
        }
    }

    /// Queue all game updates and start all pending component updates together
    /// (Irori's self-update stays separate because it restarts the app).
    /// Component installs run asynchronously in the background so the header
    /// button does not stay spinning for the full download/extraction time.
    pub(super) fn update_all(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.queuing_updates {
            return;
        }
        self.queuing_updates = true;
        cx.notify();
        let b = backend(cx);
        let components_backend = b.clone();
        run_infallible(cx, async move {
            components_backend.update_all_components().await;
        })
        .detach();
        let task = irori_ui::run(cx, async move {
            b.enqueue_all_updates().await;
            Ok(())
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.queuing_updates = false;
                if let Err(e) = result {
                    notify_error(t("manage.downloads.update_all_failed"), &e, window, cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn move_item(&mut self, id: &GameId, step: Step, cx: &mut Context<Self>) {
        if let Some(target) = self.queue.step_target(id, step) {
            let id = id.clone();
            backend_op(cx, move |b| b.reorder_download(&id, target));
        }
    }

    pub(super) fn drop_item(&mut self, dragged: &GameId, onto: &GameId, cx: &mut Context<Self>) {
        if let Some(target) = self.queue.drop_target(dragged, onto) {
            let id = dragged.clone();
            backend_op(cx, move |b| b.reorder_download(&id, target));
        }
    }

    /// Remove a waiting item, with an undo that puts it back in place.
    pub(super) fn remove_item(&mut self, id: &GameId, window: &mut Window, cx: &mut Context<Self>) {
        let Some(row) = self.queue.waiting.iter().find(|r| &r.item.id == id) else {
            return;
        };
        let item = row.item.clone();
        let restore_at = self.queue.restore_index(row.waiting_ix);
        let remove_id = item.id.clone();
        backend_op(cx, move |b| b.cancel_download(&remove_id, true));
        let title = item.title.clone();
        window.push_notification(
            Notification::info(t_args("manage.downloads.removed", &[("title", &title)]))
                .action(move |_, _, cx| {
                    let note = cx.entity().downgrade();
                    let item = item.clone();
                    Button::new("undo-remove")
                        .label(t("manage.downloads.undo"))
                        .small()
                        .on_click(move |_, window, cx| {
                            let item = item.clone();
                            backend_op(cx, move |b| {
                                match item.kind.clone() {
                                    DownloadKind::Install(req) => b.enqueue_install(
                                        item.id.clone(),
                                        item.title.clone(),
                                        req,
                                        item.download_size,
                                    ),
                                    DownloadKind::Update(req) => {
                                        b.enqueue_update(item.id.clone(), item.title.clone(), req)
                                    }
                                }
                                b.reorder_download(&item.id, restore_at);
                                b.remove_finished_download(&item.id);
                            });
                            let _ = note.update(cx, |n, cx| n.dismiss(window, cx));
                        })
                })
                .autohide(true),
            cx,
        );
    }

    pub(super) fn retry(&mut self, id: &GameId, cx: &mut Context<Self>) {
        let Some(row) = self.queue.history.iter().find(|r| &r.item.id == id) else {
            return;
        };
        let item = row.item.clone();
        backend_op(cx, move |b| {
            b.remove_finished_download(&item.id);
            match item.kind {
                DownloadKind::Install(req) => {
                    b.enqueue_install(item.id, item.title, req, item.download_size)
                }
                DownloadKind::Update(req) => b.enqueue_update(item.id, item.title, req),
            }
        });
    }

    /// Play a game whose download finished. Its status (Launching, then
    /// Playing) shows on the row; a failure is told with what fixes it.
    pub(super) fn play(
        &mut self,
        id: &GameId,
        title: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let b = backend(cx);
        let launch_id = id.clone();
        let task = irori_ui::run(cx, async move {
            b.launch_with(&launch_id, LaunchParams::default()).await
        });
        let (id, failure) = (
            id.clone(),
            t_args("manage.downloads.history.play_failed", &[("title", title)]),
        );
        cx.spawn_in(window, async move |_, cx| {
            if let Err(e) = task.await {
                cx.update(|window, cx| {
                    // Saves that changed in both places, a missing BIOS or
                    // emulator: the fix is offered instead of the error.
                    if !irori_ui::save_conflict::resolve_launch_error(
                        &id,
                        LaunchParams::default(),
                        &e,
                        window,
                        cx,
                    ) {
                        notify_error(failure, &e, window, cx);
                    }
                })
                .ok();
            }
        })
        .detach();
    }

    pub(super) fn set_history_filter(&mut self, filter: HistoryFilter, cx: &mut Context<Self>) {
        if self.history_filter != filter {
            self.history_filter = filter;
            self.history_shown = HISTORY_PAGE;
            cx.notify();
        }
    }

    pub(super) fn confirm_cancel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(current) = self.queue.current.as_ref() else {
            return;
        };
        let id = current.item.id.clone();
        let title = current.item.title.clone();
        let game = current.game.clone();
        let is_install = matches!(current.item.kind, DownloadKind::Install(_));
        let width = window.rem_size() * 34.;
        window.open_dialog(cx, move |dialog, _, cx| {
            let keep_id = id.clone();
            let delete_id = id.clone();
            let (heading, description, keep_label) = if is_install {
                (
                    t_args("manage.downloads.stop_install.title", &[("title", &title)]),
                    t("manage.downloads.stop_install.description"),
                    t("manage.downloads.stop_install.keep_files"),
                )
            } else {
                (
                    t_args("manage.downloads.stop_update.title", &[("title", &title)]),
                    t("manage.downloads.stop_update.description"),
                    t("manage.downloads.stop_update.stop"),
                )
            };
            dialog
                .title(heading)
                .width(width)
                .child(
                    h_flex()
                        .items_center()
                        .gap_4()
                        .py_2()
                        .child(GameCover::new(game.clone()).width(rems(4.)))
                        .child(
                            v_flex()
                                .flex_1()
                                .min_w_0()
                                .gap_2()
                                .child(
                                    h_flex()
                                        .gap_2()
                                        .items_center()
                                        .min_w_0()
                                        .child(
                                            div()
                                                .text_base()
                                                .font_semibold()
                                                .truncate()
                                                .child(title.clone()),
                                        )
                                        .child(StoreBadge::new(id.runner).medium()),
                                )
                                .child(
                                    div()
                                        .text_sm()
                                        .text_color(cx.theme().muted_foreground)
                                        .child(description),
                                ),
                        ),
                )
                .footer(
                    h_flex()
                        .w_full()
                        .items_center()
                        .justify_between()
                        .gap_3()
                        .child(
                            Button::new("keep-downloading")
                                .ghost()
                                .label(t("manage.downloads.keep_downloading"))
                                .on_click(|_, window, cx| window.close_dialog(cx)),
                        )
                        .child(
                            h_flex()
                                .gap_2()
                                .items_center()
                                .child(
                                    Button::new("stop-keep")
                                        .when(is_install, |b| b.outline())
                                        .when(!is_install, |b| b.danger())
                                        .label(keep_label)
                                        .on_click(move |_, window, cx| {
                                            let id = keep_id.clone();
                                            backend_op(cx, move |b| b.cancel_download(&id, true));
                                            window.close_dialog(cx);
                                        }),
                                )
                                .when(is_install, |row| {
                                    row.child(
                                        Button::new("stop-delete")
                                            .danger()
                                            .label(t("manage.downloads.stop_install.delete_files"))
                                            .on_click(move |_, window, cx| {
                                                let id = delete_id.clone();
                                                backend_op(cx, move |b| {
                                                    b.cancel_download(&id, false)
                                                });
                                                window.close_dialog(cx);
                                            }),
                                    )
                                }),
                        ),
                )
        });
    }
}
