//! The unified downloads page ([`Route::Downloads`](irori_ui::Route::Downloads)):
//! active download hero, queued installs and updates, game and component
//! updates, Irori self-update banner, offline files and finished history.

mod indicator;
mod queue;
mod space;
mod speed;
mod updates;
mod when_done;

pub use indicator::{DownloadsBadge, DownloadsPanel, DownloadsPill};
pub use when_done::{
    DownloadCommand, download_command_keywords, download_commands, keep_on, run_download_command,
};

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonGroup, ButtonVariants as _};
use gpui_kit::component::chart::AreaChart;
use gpui_kit::component::notification::Notification;
use gpui_kit::component::progress::Progress;
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, Selectable as _, Sizable as _, StyledExt as _,
    WindowExt as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, ElementId, Entity, InteractiveElement as _,
    IntoElement, ParentElement as _, Render, SharedString, StatefulInteractiveElement as _,
    Styled as _, Subscription, Task, Window, div, px, rems,
};
use irori_core::util::{format_bytes, unix_now};
use irori_core::{GameId, GameStatus, StatusKind};
use irori_services::{
    AppUpdateStatus, DownloadKind, DownloadState, FileState, LaunchParams, SelfInstall,
};
use irori_ui::art::{LruImageCache, ambient, art_store};
use irori_ui::components::{
    ArtBackdrop, BackdropInk, BrandMark, GameCover, ProgressSummary, SectionTitle, StoreBadge,
};
use irori_ui::i18n::{t, t_args};
use irori_ui::state::run_infallible;
use irori_ui::{Route, backend, models, navigate};

use crate::common::{backend_op, card, notify_error};
use crate::format;
pub(crate) use queue::{Estimates, HistoryFilter, QueuePresentation, Step};
use speed::{SPEED_SAMPLES, SpeedHistory};

/// History rows rendered per "Show more" step.
const HISTORY_PAGE: usize = 25;

/// Bordered card container without inner padding so list rows can stretch
/// flush with full-width hairlines between them.
pub(super) fn list_card(cx: &App) -> gpui_kit::Div {
    let theme = cx.theme();
    v_flex()
        .rounded(theme.radius_lg)
        .border_1()
        .border_color(theme.border)
        .bg(theme.group_box)
        .overflow_hidden()
}

/// The unified downloads and updates view: active transfer hero card,
/// drag-and-drop queue, game updates, component updates, Irori self-update
/// banner, offline files and filterable history.
///
/// Shows items of every store, including ZOOM, manually added apps and
/// redistributables. Observes downloads, library, components, app updates,
/// settings, art and status models.
pub struct DownloadsView {
    queue: QueuePresentation,
    speeds: SpeedHistory,
    /// Unqueued game updates plus non-skipped, not-yet-running component updates.
    pending_updates: usize,
    /// Component updates currently downloading/extracting.
    running_components: usize,
    queuing_updates: bool,
    checking: bool,
    irori_copied: bool,
    irori_copy_reset: Option<Task<()>>,
    games_shown: usize,
    history_shown: usize,
    /// Which finished downloads the history lists.
    history_filter: HistoryFilter,
    /// When the queue's downloads start and finish.
    estimates: Estimates,
    /// The queue the disks were last read for.
    space_key: space::SpaceKey,
    /// Finished download count last seen, so idle disk pills refresh after
    /// an install completes.
    finished_count: usize,
    /// What the queue still downloads to each disk, and what is free there.
    disks: Vec<irori_services::QueueDisk>,
    /// Configured library folders last read for the idle disk pills.
    install_folders_key: Vec<String>,
    /// Whether `install_disks` has been read for `install_folders_key`.
    install_disks_loaded: bool,
    /// Free space on configured install disks for the idle empty state.
    install_disks: Vec<irori_services::QueueDisk>,
    images: Entity<LruImageCache>,
    /// Redraws the countdown to sleeping or powering off every second while
    /// it runs, and only then.
    countdown_tick: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl DownloadsView {
    pub fn new(_window: &mut Window, cx: &mut Context<Self>) -> Self {
        crate::i18n::register();
        let m = models(cx).clone();
        let subscriptions = vec![
            cx.observe(&m.downloads, |this, _, cx| {
                this.rebuild(cx);
                cx.notify();
            }),
            cx.observe(&m.library, |this, _, cx| {
                this.rebuild(cx);
                cx.notify();
            }),
            cx.observe(&m.components, |this, _, cx| {
                this.rebuild(cx);
                cx.notify();
            }),
            cx.observe(&m.app_update, |_, _, cx| cx.notify()),
            cx.observe(&m.settings, |this, _, cx| {
                this.sync_install_disks(cx);
            }),
            cx.observe(&art_store(cx), |_, _, cx| cx.notify()),
            // Progress while downloading, and the history's Play following a
            // launch; statuses change only while something happens.
            cx.observe(&m.statuses, |this, _, cx| {
                this.sample_speed(cx);
                cx.notify();
            }),
        ];
        let mut view = Self {
            queue: QueuePresentation::default(),
            speeds: SpeedHistory::new(SPEED_SAMPLES),
            pending_updates: 0,
            running_components: 0,
            queuing_updates: false,
            checking: false,
            irori_copied: false,
            irori_copy_reset: None,
            games_shown: updates::GAMES_PAGE,
            history_shown: HISTORY_PAGE,
            history_filter: HistoryFilter::All,
            estimates: Estimates::default(),
            space_key: space::SpaceKey::new(),
            finished_count: 0,
            disks: Vec::new(),
            install_folders_key: Vec::new(),
            install_disks_loaded: false,
            install_disks: Vec::new(),
            images: LruImageCache::new(48, cx),
            countdown_tick: None,
            _subscriptions: subscriptions,
        };
        view.rebuild(cx);
        view.sync_install_disks(cx);
        view
    }

    fn rebuild(&mut self, cx: &mut Context<Self>) {
        let m = models(cx).clone();
        let b = backend(cx);
        let library = m.library.read(cx);
        let snapshot = library.snapshot.clone();
        let game_updates = library.updates.clone();
        let components = m.components.read(cx).report.clone();
        let pending_components = components
            .pending()
            .filter(|u| !components.running.contains(&u.id))
            .count();
        self.running_components = components.running.len();
        let downloads = &m.downloads.read(cx).queue;
        self.queue = QueuePresentation::build(downloads, |id| {
            snapshot.get(id).cloned().or_else(|| b.game(id))
        });
        let pending_games = game_updates
            .iter()
            .filter(|id| !downloads.active.iter().any(|i| &i.id == *id))
            .count();
        self.pending_updates = pending_games + pending_components;
        let space_key = space::key_of(downloads);
        let finished = downloads.finished.len();
        if space_key != self.space_key {
            self.space_key = space_key;
            self.read_disks(cx);
        }
        if finished > self.finished_count {
            self.install_disks_loaded = false;
            self.sync_install_disks(cx);
        }
        self.finished_count = finished;
        self.sample_speed(cx);
    }

    /// Read the queue's disks again, off the UI thread.
    fn read_disks(&mut self, cx: &mut Context<Self>) {
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
    fn sync_install_disks(&mut self, cx: &mut Context<Self>) {
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

    fn sample_speed(&mut self, cx: &mut Context<Self>) -> bool {
        let id = self
            .queue
            .current
            .as_ref()
            .filter(|c| !c.paused)
            .map(|c| c.item.id.clone());
        let progress = id
            .as_ref()
            .and_then(|id| models(cx).statuses.read(cx).get(id))
            .and_then(|s| s.progress);
        let changed = self.speeds.record(id.as_ref(), progress);
        // From the averaged rate, not the latest reading, so the times
        // shown hold still while the speed wobbles.
        self.estimates = if id.is_some() && !self.queue.paused {
            let rate = self.speeds.average_download();
            let sizes: Vec<u64> = self
                .queue
                .waiting
                .iter()
                .map(|r| r.item.download_size)
                .collect();
            queue::estimate(queue::current_left(progress, rate), &sizes, rate)
        } else {
            Estimates::default()
        };
        changed
    }

    fn toggle_pause(&mut self, cx: &mut Context<Self>) {
        if self.queue.paused {
            backend_op(cx, |b| b.resume_downloads());
        } else {
            backend_op(cx, |b| b.pause_downloads());
        }
    }

    /// Queue all game updates and start all pending component updates together
    /// (Irori's self-update stays separate because it restarts the app).
    /// Component installs run asynchronously in the background so the header
    /// button does not stay spinning for the full download/extraction time.
    fn update_all(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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

    fn move_item(&mut self, id: &GameId, step: Step, cx: &mut Context<Self>) {
        if let Some(target) = self.queue.step_target(id, step) {
            let id = id.clone();
            backend_op(cx, move |b| b.reorder_download(&id, target));
        }
    }

    fn drop_item(&mut self, dragged: &GameId, onto: &GameId, cx: &mut Context<Self>) {
        if let Some(target) = self.queue.drop_target(dragged, onto) {
            let id = dragged.clone();
            backend_op(cx, move |b| b.reorder_download(&id, target));
        }
    }

    /// Remove a waiting item, with an undo that puts it back in place.
    fn remove_item(&mut self, id: &GameId, window: &mut Window, cx: &mut Context<Self>) {
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

    fn retry(&mut self, id: &GameId, cx: &mut Context<Self>) {
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
    fn play(&mut self, id: &GameId, title: &str, window: &mut Window, cx: &mut Context<Self>) {
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

    fn set_history_filter(&mut self, filter: HistoryFilter, cx: &mut Context<Self>) {
        if self.history_filter != filter {
            self.history_filter = filter;
            self.history_shown = HISTORY_PAGE;
            cx.notify();
        }
    }

    fn confirm_cancel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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

    /// Status line under the title: independent facts joined by " · ".
    fn summary(&self, checked_at: Option<u64>, app_update: &AppUpdateStatus) -> String {
        let mut parts: Vec<SharedString> = Vec::new();
        if let Some(c) = &self.queue.current {
            parts.push(if c.paused {
                t("manage.downloads.summary.paused")
            } else {
                t("manage.downloads.summary.downloading")
            });
        } else if self.running_components > 0 {
            parts.push(t("manage.updates.updating"));
        } else if matches!(app_update.install, SelfInstall::Downloading { .. }) {
            parts.push(t("manage.downloads.summary.downloading"));
        }
        let waiting = self.queue.waiting.len();
        if waiting > 0 {
            let count = waiting.to_string();
            parts.push(if waiting == 1 {
                t_args("manage.downloads.summary.waiting_one", &[("count", &count)])
            } else {
                t_args(
                    "manage.downloads.summary.waiting_other",
                    &[("count", &count)],
                )
            });
        }
        if let Some(secs) = self.estimates.done {
            parts.push(t_args(
                "manage.downloads.summary.finish",
                &[("when", &format::finish_from_now(secs))],
            ));
        }
        let total_updates = self.pending_updates + usize::from(app_update.has_update());
        if total_updates > 0 {
            let count = total_updates.to_string();
            parts.push(if total_updates == 1 {
                t_args("manage.downloads.summary.updates_one", &[("count", &count)])
            } else {
                t_args(
                    "manage.downloads.summary.updates_other",
                    &[("count", &count)],
                )
            });
        }
        if parts.is_empty() {
            parts.push(t("manage.updates.up_to_date"));
        }
        if let Some(at) = checked_at {
            let when = format::relative_time(at, unix_now());
            parts.push(t_args("manage.updates.checked", &[("when", &when)]));
        }
        parts.join(" · ")
    }

    /// Keep redrawing while a countdown runs; stop when it ends.
    fn sync_countdown_tick(&mut self, counting: bool, cx: &mut Context<Self>) {
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

    fn render_header(
        &self,
        checked_at: Option<u64>,
        checking: bool,
        app_update: &AppUpdateStatus,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let queue = models(cx).downloads.read(cx).queue.clone();
        let active = self.queue.has_active();
        let power_btn = when_done::power_menu_button(&queue, cx);

        let mut primary_actions: Vec<AnyElement> = Vec::new();
        if self.pending_updates > 0 {
            primary_actions.push(
                Button::new("update-all")
                    .when(!self.queue.paused, |b| b.primary())
                    .label(t_args(
                        "manage.downloads.update_all",
                        &[("count", &self.pending_updates.to_string())],
                    ))
                    .loading(self.queuing_updates)
                    .on_click(cx.listener(|this, _, window, cx| this.update_all(window, cx)))
                    .into_any_element(),
            );
        }
        if active {
            let paused = self.queue.paused;
            primary_actions.push(
                Button::new("toggle-pause")
                    .when(paused, |b| b.primary())
                    .when(!paused, |b| b.outline())
                    .icon(if paused {
                        IconName::Play
                    } else {
                        IconName::Pause
                    })
                    .label(if paused {
                        t("manage.downloads.resume")
                    } else {
                        t("manage.downloads.pause")
                    })
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_pause(cx)))
                    .into_any_element(),
            );
        }
        if let Some(power) = power_btn {
            primary_actions.push(power);
        }

        let mut utility_actions: Vec<AnyElement> = Vec::new();
        let check_btn = Button::new("updates-check")
            .ghost()
            .icon(IconName::RefreshCw)
            .loading(checking)
            .on_click(cx.listener(|this, _, window, cx| this.check_now(window, cx)));
        let check_btn = if active {
            check_btn
                .tooltip(t("manage.updates.check_now"))
                .accessibility_label(t("manage.updates.check_now"))
        } else {
            check_btn.label(t("manage.updates.check_now"))
        };
        utility_actions.push(check_btn.into_any_element());
        utility_actions.push(
            Button::new("downloads-settings")
                .debug_selector(|| "downloads-settings".into())
                .ghost()
                .icon(IconName::Settings)
                .tooltip(t("manage.downloads.settings"))
                .accessibility_label(t("manage.downloads.settings"))
                .on_click(|_, _, cx| navigate(Route::Settings("downloads".into()), cx))
                .into_any_element(),
        );

        let has_primary = !primary_actions.is_empty();
        let border = cx.theme().border;
        h_flex()
            .debug_selector(|| "downloads-header".into())
            .w_full()
            .flex_wrap()
            .items_center()
            .justify_between()
            .gap_x_4()
            .gap_y_3()
            .child(
                v_flex()
                    .debug_selector(|| "downloads-title".into())
                    .flex_1()
                    .min_w(rems(12.))
                    .gap_1()
                    .child(
                        div()
                            .text_3xl()
                            .font_extrabold()
                            .child(t("manage.downloads.title")),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child(self.summary(checked_at, app_update)),
                    ),
            )
            .child(
                h_flex()
                    .debug_selector(|| "downloads-header-actions".into())
                    .flex_wrap()
                    .items_center()
                    .gap_2()
                    .children(primary_actions)
                    .when(has_primary, |row| {
                        row.child(div().w(px(1.)).h_5().mx_0p5().bg(border))
                    })
                    .children(utility_actions),
            )
    }

    fn render_current(&self, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let Some(current) = self.queue.current.as_ref() else {
            if self.queue.waiting.is_empty() {
                return None;
            }
            return Some(
                card(cx)
                    .child(
                        h_flex()
                            .gap_3()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child(Icon::new(IconName::CirclePause).small())
                            .child(t("manage.downloads.current.paused_queue")),
                    )
                    .into_any_element(),
            );
        };
        let id = current.item.id.clone();
        let status = models(cx).statuses.read(cx).get(&id).cloned();
        let progress = status.as_ref().and_then(|s| s.progress);
        let backdrop_url = (!irori_ui::theme::reduce_transparency(cx))
            .then(|| current.game.hero_art().map(str::to_owned))
            .flatten();
        // Calling `ambient` warms the blur cache off-thread when missing, and
        // we only paint `ArtBackdrop` once ready so the card keeps `group_box`
        // instead of flashing `theme.background` while loading.
        let has_backdrop = backdrop_url
            .as_deref()
            .and_then(|url| ambient(url, cx))
            .is_some();
        let ink = BackdropInk::of(
            has_backdrop.then_some(backdrop_url.as_deref()).flatten(),
            cx,
        );
        let theme = cx.theme();
        let radius_lg = theme.radius_lg;
        let border = theme.border;
        let group_box = theme.group_box;
        let install_folder = match &current.item.kind {
            DownloadKind::Install(req) if !req.path.as_os_str().is_empty() => {
                Some(req.path.display().to_string())
            }
            _ => None,
        };
        let meta = [
            Some(queue::kind_label(&current.item.kind).to_string()),
            format::size(current.item.download_size),
            install_folder,
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" · ");

        let paused_progress = progress.or(current.item.progress);
        let progress_block: AnyElement = if current.paused {
            let left_status = h_flex()
                .flex_1()
                .min_w_0()
                .items_center()
                .gap_2()
                .text_sm()
                .font_medium()
                .text_color(ink.muted)
                .child(Icon::new(IconName::CirclePause).small())
                .child(div().truncate().child(t("manage.downloads.current.paused")));
            if let Some(p) = paused_progress
                && let Some(pct) = p.percent
            {
                let paused_pct_color = if has_backdrop {
                    ink.text.opacity(0.85)
                } else {
                    theme.foreground.opacity(0.8)
                };
                let mut right_parts: Vec<AnyElement> = vec![
                    div()
                        .font_bold()
                        .text_color(paused_pct_color)
                        .child(format!("{pct:.0}%"))
                        .into_any_element(),
                ];
                match (p.bytes_done, p.bytes_total) {
                    (Some(d), Some(total)) if total > 0 => {
                        right_parts.push(div().text_color(ink.muted).child("·").into_any_element());
                        right_parts.push(
                            h_flex()
                                .items_baseline()
                                .gap_1()
                                .child(
                                    div()
                                        .font_semibold()
                                        .text_color(ink.muted)
                                        .child(format_bytes(d)),
                                )
                                .child(
                                    div()
                                        .font_medium()
                                        .text_color(ink.muted.opacity(0.75))
                                        .child(format!("/ {}", format_bytes(total))),
                                )
                                .into_any_element(),
                        );
                        if total > d {
                            right_parts
                                .push(div().text_color(ink.muted).child("·").into_any_element());
                            right_parts.push(
                                div()
                                    .font_medium()
                                    .text_color(ink.muted.opacity(0.75))
                                    .child(t_args(
                                        "manage.downloads.pill.left",
                                        &[("time", &format_bytes(total - d))],
                                    ))
                                    .into_any_element(),
                            );
                        }
                    }
                    (Some(d), _) if d > 0 => {
                        right_parts.push(div().text_color(ink.muted).child("·").into_any_element());
                        right_parts.push(
                            div()
                                .font_semibold()
                                .text_color(ink.muted)
                                .child(format_bytes(d))
                                .into_any_element(),
                        );
                    }
                    _ => {}
                }
                v_flex()
                    .gap_2()
                    .w_full()
                    .child(
                        h_flex()
                            .w_full()
                            .items_baseline()
                            .justify_between()
                            .gap_4()
                            .child(left_status)
                            .child(
                                h_flex()
                                    .flex_shrink_0()
                                    .items_baseline()
                                    .gap_1p5()
                                    .text_sm()
                                    .children(right_parts),
                            ),
                    )
                    .child(
                        div()
                            .w_full()
                            .opacity(0.45)
                            .child(Progress::new("current-progress-paused").value(pct)),
                    )
                    .into_any_element()
            } else {
                left_status.into_any_element()
            }
        } else {
            let status = status.clone().unwrap_or_else(|| {
                let kind = match current.item.kind {
                    DownloadKind::Install(_) => StatusKind::Installing,
                    DownloadKind::Update(_) => StatusKind::Updating,
                };
                GameStatus::new(id.clone(), kind)
            });
            ProgressSummary::new("current-progress", status)
                .prominent()
                .when(has_backdrop, |p| p.ink(ink))
                .into_any_element()
        };

        let game_id = id.clone();
        let cover_id = id.clone();
        let title_id = id.clone();
        let paused = current.paused;
        Some(
            div()
                .id("current-download")
                .relative()
                .overflow_hidden()
                .rounded(radius_lg)
                .border_1()
                .border_color(border)
                .bg(group_box)
                .when_some(backdrop_url.filter(|_| has_backdrop), |card, url| {
                    card.child(
                        ArtBackdrop::new(Some(url))
                            .strength(0.55)
                            .dim(0.25)
                            .image_cache(&self.images),
                    )
                })
                .child(
                    v_flex().relative().p_5().gap_4().child(
                        h_flex()
                            .items_start()
                            .gap_6()
                            .child(
                                div()
                                    .id("hero-cover-open")
                                    .flex_shrink_0()
                                    .cursor_pointer()
                                    .on_click(move |_, _, cx| {
                                        navigate(Route::Game(cover_id.clone()), cx)
                                    })
                                    .child(GameCover::new(current.game.clone()).width(rems(8.5))),
                            )
                            .child(
                                v_flex()
                                    .flex_1()
                                    .min_w_0()
                                    .gap_3()
                                    .child(
                                        h_flex()
                                            .items_start()
                                            .justify_between()
                                            .gap_3()
                                            .child(
                                                v_flex()
                                                    .flex_1()
                                                    .min_w_0()
                                                    .gap_1()
                                                    .child(
                                                        h_flex()
                                                            .gap_2()
                                                            .items_center()
                                                            .min_w_0()
                                                            .child(
                                                                div()
                                                                    .id("hero-title-open")
                                                                    .cursor_pointer()
                                                                    .text_2xl()
                                                                    .font_semibold()
                                                                    .truncate()
                                                                    .when(has_backdrop, |d| {
                                                                        d.text_color(ink.text)
                                                                    })
                                                                    .on_click(move |_, _, cx| {
                                                                        navigate(
                                                                            Route::Game(
                                                                                title_id.clone(),
                                                                            ),
                                                                            cx,
                                                                        )
                                                                    })
                                                                    .child(
                                                                        current.item.title.clone(),
                                                                    ),
                                                            )
                                                            .child(
                                                                StoreBadge::new(id.runner).large(),
                                                            ),
                                                    )
                                                    .child(
                                                        div()
                                                            .text_sm()
                                                            .truncate()
                                                            .text_color(ink.muted)
                                                            .child(meta),
                                                    ),
                                            )
                                            .child(
                                                h_flex()
                                                    .flex_shrink_0()
                                                    .gap_1p5()
                                                    .when(!paused, |row| {
                                                        row.child(
                                                            Button::new("hero-toggle-pause")
                                                                .outline()
                                                                .small()
                                                                .icon(IconName::Pause)
                                                                .tooltip(t(
                                                                    "manage.downloads.pause",
                                                                ))
                                                                .on_click(cx.listener(
                                                                    |this, _, _, cx| {
                                                                        this.toggle_pause(cx)
                                                                    },
                                                                )),
                                                        )
                                                        .child(
                                                            Button::new("cancel-current")
                                                                .outline()
                                                                .small()
                                                                .icon(IconName::X)
                                                                .tooltip(t(
                                                                    "manage.downloads.cancel",
                                                                ))
                                                                .accessibility_label(t(
                                                                    "manage.downloads.cancel",
                                                                ))
                                                                .on_click(cx.listener(
                                                                    |this, _, window, cx| {
                                                                        this.confirm_cancel(
                                                                            window, cx,
                                                                        )
                                                                    },
                                                                )),
                                                        )
                                                    })
                                                    .child(
                                                        Button::new("open-current")
                                                            .ghost()
                                                            .small()
                                                            .icon(IconName::ExternalLink)
                                                            .tooltip(t(
                                                                "manage.downloads.game_page",
                                                            ))
                                                            .accessibility_label(t(
                                                                "manage.downloads.game_page",
                                                            ))
                                                            .on_click(move |_, _, cx| {
                                                                navigate(
                                                                    Route::Game(game_id.clone()),
                                                                    cx,
                                                                )
                                                            }),
                                                    ),
                                            ),
                                    )
                                    .child(progress_block)
                                    .when(!current.paused, |this| {
                                        this.child(self.render_rates(
                                            progress,
                                            ink,
                                            has_backdrop,
                                            window,
                                            cx,
                                        ))
                                    })
                                    .when(current.paused, |this| {
                                        this.child(
                                            h_flex()
                                                .pt_1()
                                                .items_center()
                                                .gap_2()
                                                .child(
                                                    Button::new("hero-toggle-pause")
                                                        .primary()
                                                        .icon(IconName::Play)
                                                        .label(t("manage.downloads.resume"))
                                                        .on_click(cx.listener(|this, _, _, cx| {
                                                            this.toggle_pause(cx)
                                                        })),
                                                )
                                                .child(
                                                    Button::new("cancel-current")
                                                        .outline()
                                                        .icon(IconName::X)
                                                        .label(t("manage.downloads.cancel"))
                                                        .on_click(cx.listener(
                                                            |this, _, window, cx| {
                                                                this.confirm_cancel(window, cx)
                                                            },
                                                        )),
                                                ),
                                        )
                                    }),
                            ),
                    ),
                )
                .into_any_element(),
        )
    }

    /// Rate sparkline plus the current download/disk rates, time left and
    /// what limits the speed.
    fn render_rates(
        &self,
        progress: Option<irori_core::InstallProgress>,
        ink: BackdropInk,
        has_backdrop: bool,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = cx.theme();
        let label_color = if has_backdrop {
            ink.muted
        } else {
            theme.muted_foreground
        };
        let value_color = if has_backdrop {
            ink.text
        } else {
            theme.foreground
        };
        let progress = progress.unwrap_or_default();
        let stat = |label: SharedString, value: String, dot: Option<gpui_kit::Hsla>| {
            v_flex()
                .gap_0p5()
                .child(
                    h_flex()
                        .gap_1()
                        .items_center()
                        .when_some(dot, |row, color| {
                            row.child(div().size(rems(0.45)).rounded_full().bg(color))
                        })
                        .child(div().text_xs().text_color(label_color).child(label)),
                )
                .child(
                    div()
                        .text_base()
                        .font_semibold()
                        .text_color(value_color)
                        .child(value),
                )
        };
        let chart: AnyElement = if self.speeds.len() >= 2 {
            const MIB: f64 = 1024.0 * 1024.0;
            let data: Vec<(usize, f64, f64)> = self
                .speeds
                .samples()
                .enumerate()
                .map(|(ix, s)| (ix, s.download as f64 / MIB, s.disk as f64 / MIB))
                .collect();
            AreaChart::new(data)
                .id("download-rates")
                .x(|d: &(usize, f64, f64)| d.0.to_string())
                .y(|d: &(usize, f64, f64)| d.1)
                .stroke(theme.chart_1)
                .fill(theme.chart_1.opacity(0.25))
                .name(t("manage.downloads.chart.download"))
                .y(|d: &(usize, f64, f64)| d.2)
                .stroke(theme.chart_2)
                .fill(theme.chart_2.opacity(0.0))
                .name(t("manage.downloads.chart.disk"))
                .x_axis(false)
                .y_axis(false)
                .grid(false)
                .point_count(self.speeds.capacity())
                .appear(false)
                .into_any_element()
        } else {
            div()
                .size_full()
                .flex()
                .items_center()
                .text_xs()
                .text_color(label_color)
                .child(t("manage.downloads.measuring"))
                .into_any_element()
        };
        h_flex()
            .gap_6()
            .items_end()
            .child(div().flex_1().min_w_0().h(rems(5.)).child(chart))
            .child(stat(
                t("manage.downloads.stat.download"),
                format::rate(progress.download_speed),
                Some(theme.chart_1),
            ))
            .child(stat(
                t("manage.downloads.stat.disk"),
                format::rate(progress.disk_speed),
                Some(theme.chart_2),
            ))
            .child(stat(
                t("manage.downloads.stat.time_left"),
                format::eta(progress.eta_secs),
                None,
            ))
            .child(stat(
                t("manage.downloads.stat.finishes"),
                self.estimates
                    .current
                    .map(|secs| format::finish_from_now(secs).to_string())
                    .unwrap_or_else(|| "—".to_string()),
                None,
            ))
            .child(stat(
                t("manage.downloads.stat.limit"),
                format::bottleneck(progress.bottleneck).to_string(),
                None,
            ))
    }

    fn render_queue(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let now = unix_now();
        let rows = self.queue.waiting.iter().enumerate().map(|(ix, row)| {
            let theme = cx.theme();
            let border = theme.border;
            let id = row.item.id.clone();
            let key = id.key();
            let starts = self
                .estimates
                .starts
                .get(row.waiting_ix)
                .copied()
                .flatten()
                .map(|secs| {
                    t_args(
                        "manage.downloads.queue.starts",
                        &[("when", &format::finish_from_now(secs))],
                    )
                    .to_string()
                });
            let paused_state = (row.item.state == DownloadState::Paused).then(|| {
                match row.item.progress.and_then(|p| p.percent) {
                    Some(pct) => format!("{} · {pct:.0}%", t("manage.downloads.summary.paused")),
                    None => t("manage.downloads.summary.paused").to_string(),
                }
            });
            let meta = [
                Some(queue::kind_label(&row.item.kind).to_string()),
                paused_state,
                format::size(row.item.download_size),
                starts,
                Some(
                    t_args(
                        "manage.downloads.queue.added",
                        &[("time", &format::relative_time(row.item.added_at, now))],
                    )
                    .to_string(),
                ),
            ]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" · ");
            let dragged = DraggedDownload {
                id: id.clone(),
                title: row.item.title.clone().into(),
            };
            let (prioritize_id, up_id, down_id, remove_id, drop_id, page_id) = (
                id.clone(),
                id.clone(),
                id.clone(),
                id.clone(),
                id.clone(),
                id.clone(),
            );
            h_flex()
                .id(ElementId::Name(format!("queue-{key}").into()))
                .debug_selector(|| format!("queue-{key}"))
                .gap_3()
                .px_4()
                .py_3()
                .when(ix > 0, |d| d.border_t_1().border_color(border))
                .hover(|s| s.bg(theme.muted.opacity(0.35)))
                .on_drag(dragged, |d, _, _, cx| cx.new(|_| d.clone()))
                .drag_over::<DraggedDownload>(|style, _, _, cx| style.bg(cx.theme().drop_target))
                .on_drop(cx.listener(move |this, d: &DraggedDownload, _, cx| {
                    this.drop_item(&d.id, &drop_id, cx)
                }))
                .child(
                    Icon::new(IconName::GripVertical)
                        .small()
                        .text_color(theme.muted_foreground),
                )
                .child(
                    h_flex()
                        .id(ElementId::Name(format!("queue-open-{key}").into()))
                        .flex_1()
                        .min_w_0()
                        .items_center()
                        .gap_3()
                        .cursor_pointer()
                        .on_click(move |_, _, cx| navigate(Route::Game(page_id.clone()), cx))
                        .child(GameCover::new(row.game.clone()).width(rems(3.5)))
                        .child(
                            v_flex()
                                .flex_1()
                                .min_w_0()
                                .gap_1()
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
                                                .child(row.item.title.clone()),
                                        )
                                        .child(StoreBadge::new(id.runner).medium()),
                                )
                                .child(
                                    div()
                                        .text_sm()
                                        .text_color(theme.muted_foreground)
                                        .truncate()
                                        .child(meta),
                                ),
                        ),
                )
                .child(
                    h_flex()
                        .gap_1()
                        .flex_shrink_0()
                        .child(
                            Button::new("download-now")
                                .ghost()
                                .small()
                                .icon(IconName::Play)
                                .tooltip(t("manage.downloads.queue.download_now"))
                                .accessibility_label(t("manage.downloads.queue.download_now"))
                                .on_click(move |_, _, cx| {
                                    let id = prioritize_id.clone();
                                    backend_op(cx, move |b| b.prioritize_download(&id));
                                }),
                        )
                        .child(
                            Button::new("move-up")
                                .ghost()
                                .small()
                                .icon(IconName::ArrowUp)
                                .tooltip(t("manage.downloads.queue.move_up"))
                                .disabled(row.is_first)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.move_item(&up_id, Step::Up, cx)
                                })),
                        )
                        .child(
                            Button::new("move-down")
                                .ghost()
                                .small()
                                .icon(IconName::ArrowDown)
                                .tooltip(t("manage.downloads.queue.move_down"))
                                .disabled(row.is_last)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.move_item(&down_id, Step::Down, cx)
                                })),
                        )
                        .child(
                            Button::new("remove")
                                .ghost()
                                .small()
                                .icon(IconName::X)
                                .tooltip(t("manage.downloads.queue.remove"))
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.remove_item(&remove_id, window, cx)
                                })),
                        ),
                )
        });
        v_flex()
            .gap_2()
            .child(SectionTitle::new(t_args(
                "manage.downloads.queue.title",
                &[("count", &self.queue.waiting.len().to_string())],
            )))
            .child(list_card(cx).children(rows))
    }

    /// Offline installers and goodies being or recently downloaded.
    fn render_files(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let files = models(cx).downloads.read(cx).files.clone();
        if files.is_empty() {
            return None;
        }
        let library = models(cx).library.read(cx).snapshot.clone();
        let any_finished = files.iter().any(|f| !f.is_active());
        let rows = files.iter().rev().enumerate().map(|(ix, f)| {
            let theme = cx.theme();
            let border = theme.border;
            let key = f.key.clone();
            let game = library.get(&f.game).cloned();
            let name = f.name.clone().unwrap_or_else(|| f.file.name.clone());
            let (state, color) = match f.state {
                FileState::Queued => (t("manage.downloads.files.queued"), theme.muted_foreground),
                FileState::Running => (t("manage.downloads.files.running"), theme.muted_foreground),
                FileState::Done => (t("manage.downloads.history.done"), theme.success),
                FileState::Failed => (t("manage.downloads.history.failed"), theme.danger),
                FileState::Canceled => (
                    t("manage.downloads.history.canceled"),
                    theme.muted_foreground,
                ),
            };
            let progress = (f.state == FileState::Running && f.file.size > 0)
                .then(|| (f.downloaded as f32 / f.file.size as f32 * 100.0).clamp(0.0, 100.0));
            let meta = [
                game.as_ref().map(|g| g.title.clone()),
                format::size(f.file.size),
                Some(match progress {
                    Some(p) => format!("{state} · {p:.0}%"),
                    None => state.to_string(),
                }),
                f.error.clone(),
            ]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" · ");
            let path = f.path().filter(|_| f.state == FileState::Done);
            let retry = (matches!(f.state, FileState::Failed | FileState::Canceled))
                .then(|| (f.game.clone(), f.file.clone(), f.folder.clone()));
            let cancel_key = key.clone();
            h_flex()
                .id(ElementId::Name(format!("file-{key}").into()))
                .gap_3()
                .px_4()
                .py_3()
                .when(ix > 0, |d| d.border_t_1().border_color(border))
                .child(match game {
                    Some(g) => GameCover::new(g).width(rems(3.5)).into_any_element(),
                    None => div().w(rems(3.5)).into_any_element(),
                })
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .gap_1()
                        .child(div().text_base().font_semibold().truncate().child(name))
                        .child(div().text_sm().text_color(color).truncate().child(meta))
                        .children(progress.map(|p| {
                            irori_ui::motion::gliding_progress(
                                ElementId::Name(format!("file-progress-{key}").into()),
                                Some(p),
                            )
                        })),
                )
                .when(f.is_active(), |d| {
                    d.child(
                        Button::new(ElementId::Name(format!("file-cancel-{key}").into()))
                            .ghost()
                            .small()
                            .label(t("manage.downloads.files.cancel"))
                            .on_click(move |_, _, cx| backend(cx).cancel_extra(&cancel_key)),
                    )
                })
                .children(path.map(|path| {
                    Button::new(ElementId::Name(format!("file-show-{key}").into()))
                        .ghost()
                        .small()
                        .label(t("manage.downloads.files.show"))
                        .on_click(move |_, _, cx| cx.reveal_path(&path))
                }))
                .children(retry.map(|(game, file, folder)| {
                    Button::new(ElementId::Name(format!("file-retry-{key}").into()))
                        .ghost()
                        .small()
                        .label(t("manage.downloads.history.retry"))
                        .on_click(move |_, _, cx| {
                            backend(cx).download_extra(game.clone(), file.clone(), folder.clone())
                        })
                }))
        });
        Some(
            v_flex()
                .gap_2()
                .child(SectionTitle::new(t("manage.downloads.files.title")).when(
                    any_finished,
                    |s| {
                        s.trailing(
                            Button::new("clear-files")
                                .ghost()
                                .small()
                                .label(t("manage.downloads.files.clear"))
                                .on_click(|_, _, cx| backend(cx).clear_finished_extras()),
                        )
                    },
                ))
                .child(list_card(cx).children(rows))
                .into_any_element(),
        )
    }

    fn render_history(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let now = unix_now();
        let filter = self.history_filter;
        let total = self.queue.history_in(filter).count();
        let shown: Vec<_> = {
            let statuses = models(cx).statuses.read(cx);
            self.queue
                .history_in(filter)
                .take(self.history_shown)
                .map(|row| (row, statuses.get(&row.item.id).map(|s| s.kind)))
                .collect()
        };
        let rows = shown.into_iter().enumerate().map(|(ix, (row, status))| {
            // Launching or already running: Play waits, or is not offered.
            let launching = status == Some(StatusKind::Launching);
            let playable = row.is_playable()
                && !status.is_some_and(|k| k.is_busy() && k != StatusKind::Launching);
            let theme = cx.theme();
            let border = theme.border;
            let id = row.item.id.clone();
            let key = format!("{}-{}", id.key(), row.item.finished_at.unwrap_or(0));
            let (icon, color, pill_bg, state) = match row.item.state {
                DownloadState::Done => (
                    IconName::CircleCheck,
                    theme.success,
                    theme.success.opacity(0.14),
                    t("manage.downloads.history.done"),
                ),
                DownloadState::Failed => (
                    IconName::CircleX,
                    theme.danger,
                    theme.danger.opacity(0.14),
                    t("manage.downloads.history.failed"),
                ),
                _ => (
                    IconName::Ban,
                    theme.muted_foreground,
                    theme.muted,
                    t("manage.downloads.history.canceled"),
                ),
            };
            let when = row
                .item
                .finished_at
                .map(|at| format::relative_time(at, now).to_string())
                .unwrap_or_default();
            let meta = [
                Some(queue::kind_label(&row.item.kind).to_string()),
                format::size(row.item.download_size),
                (!when.is_empty()).then_some(when),
            ]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" · ");
            let (retry_id, remove_id, page_id, open_id, play_id) =
                (id.clone(), id.clone(), id.clone(), id.clone(), id.clone());
            let play_title = row.item.title.clone();
            let retryable = row.is_retryable();
            let is_done = row.item.state == DownloadState::Done;
            let open_key = format!("history-open-{key}");
            let open_sel = open_key.clone();
            h_flex()
                .id(ElementId::Name(format!("history-{key}").into()))
                .gap_3()
                .px_4()
                .py_3()
                .when(ix > 0, |d| d.border_t_1().border_color(border))
                .hover(|s| s.bg(theme.muted.opacity(0.35)))
                .child(
                    h_flex()
                        .id(ElementId::Name(open_key.into()))
                        .debug_selector(move || open_sel.clone())
                        .flex_1()
                        .min_w_0()
                        .items_center()
                        .gap_3()
                        .cursor_pointer()
                        .on_click(move |_, _, cx| navigate(Route::Game(open_id.clone()), cx))
                        .child(GameCover::new(row.game.clone()).width(rems(3.5)))
                        .child(
                            v_flex()
                                .flex_1()
                                .min_w_0()
                                .gap_1()
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
                                                .child(row.item.title.clone()),
                                        )
                                        .child(StoreBadge::new(id.runner).medium()),
                                )
                                .child(
                                    h_flex()
                                        .gap_2()
                                        .items_center()
                                        .min_w_0()
                                        .child(
                                            h_flex()
                                                .gap_1()
                                                .items_center()
                                                .flex_shrink_0()
                                                .px_2()
                                                .py_0p5()
                                                .rounded_full()
                                                .bg(pill_bg)
                                                .text_xs()
                                                .font_medium()
                                                .text_color(color)
                                                .child(Icon::new(icon).xsmall().text_color(color))
                                                .child(state),
                                        )
                                        .child(
                                            div()
                                                .text_sm()
                                                .text_color(theme.muted_foreground)
                                                .truncate()
                                                .child(meta),
                                        ),
                                )
                                .when_some(row.item.error.clone(), |this, err| {
                                    this.child(
                                        div()
                                            .text_xs()
                                            .text_color(theme.danger)
                                            .line_clamp(2)
                                            .child(err),
                                    )
                                }),
                        ),
                )
                .child(
                    h_flex()
                        .gap_1p5()
                        .flex_shrink_0()
                        .when(retryable, |this| {
                            this.child(
                                Button::new("retry")
                                    .small()
                                    .outline()
                                    .icon(IconName::RotateCcw)
                                    .label(t("manage.downloads.history.retry"))
                                    .on_click(
                                        cx.listener(move |this, _, _, cx| {
                                            this.retry(&retry_id, cx)
                                        }),
                                    ),
                            )
                        })
                        .when(playable, |this| {
                            this.child(
                                Button::new("play")
                                    .small()
                                    .primary()
                                    .icon(IconName::Play)
                                    .label(t("manage.downloads.history.play"))
                                    .loading(launching)
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        if !launching {
                                            this.play(&play_id, &play_title, window, cx)
                                        }
                                    })),
                            )
                        })
                        .when(is_done, |this| {
                            this.child(
                                Button::new("open")
                                    .small()
                                    .ghost()
                                    .icon(IconName::ExternalLink)
                                    .tooltip(t("manage.downloads.game_page"))
                                    .accessibility_label(t("manage.downloads.game_page"))
                                    .on_click(move |_, _, cx| {
                                        navigate(Route::Game(page_id.clone()), cx)
                                    }),
                            )
                        })
                        .child(
                            Button::new("forget")
                                .ghost()
                                .small()
                                .icon(IconName::X)
                                .tooltip(t("manage.downloads.history.remove"))
                                .on_click(move |_, _, cx| {
                                    let id = remove_id.clone();
                                    backend_op(cx, move |b| b.remove_finished_download(&id));
                                }),
                        ),
                )
        });
        let filters = ButtonGroup::new("history-filter")
            .small()
            .outline()
            .children(HistoryFilter::ALL.iter().enumerate().map(|(ix, f)| {
                Button::new(ElementId::Name(format!("history-filter-{ix}").into()))
                    .label(f.label())
                    .selected(*f == filter)
            }))
            .on_click(cx.listener(|this, clicked: &Vec<usize>, _, cx| {
                if let Some(f) = clicked.first().and_then(|ix| HistoryFilter::ALL.get(*ix)) {
                    this.set_history_filter(*f, cx);
                }
            }));
        v_flex()
            .gap_2()
            .child(
                SectionTitle::new(t("manage.downloads.history.title")).trailing(
                    h_flex().gap_2().child(filters).child(
                        Button::new("clear-history")
                            .ghost()
                            .small()
                            .label(t("manage.downloads.history.clear"))
                            .on_click(|_, _, cx| backend_op(cx, |b| b.clear_finished_downloads())),
                    ),
                ),
            )
            .child(if total == 0 {
                list_card(cx).child(
                    div()
                        .px_4()
                        .py_4()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child(t("manage.downloads.history.filter.none")),
                )
            } else {
                list_card(cx).children(rows)
            })
            .when(total > self.history_shown, |this| {
                this.child(
                    div().child(
                        Button::new("history-more")
                            .ghost()
                            .small()
                            .label(t_args(
                                "manage.downloads.history.show_more",
                                &[("count", &(total - self.history_shown).to_string())],
                            ))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.history_shown += HISTORY_PAGE;
                                cx.notify();
                            })),
                    ),
                )
            })
    }

    /// Compact horizontal idle card shown above History/Files when nothing is
    /// downloading and no updates are pending, keeping the warm hearth mark and
    /// configured drive space pills visible post-download.
    fn render_idle_banner(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        card(cx)
            .id("downloads-idle-banner")
            .debug_selector(|| "downloads-idle-banner".into())
            .child(
                h_flex()
                    .flex_wrap()
                    .items_center()
                    .justify_between()
                    .gap_4()
                    .child(
                        h_flex()
                            .items_center()
                            .gap_4()
                            .min_w_0()
                            .flex_1()
                            .child(
                                div()
                                    .p_2p5()
                                    .rounded_full()
                                    .bg(theme.primary.opacity(0.10))
                                    .border_1()
                                    .border_color(theme.primary.opacity(0.18))
                                    .flex_shrink_0()
                                    .child(BrandMark::new(rems(2.5))),
                            )
                            .child(
                                v_flex()
                                    .min_w_0()
                                    .gap_0p5()
                                    .child(
                                        div()
                                            .text_base()
                                            .font_semibold()
                                            .child(t("manage.downloads.empty.title")),
                                    )
                                    .child(
                                        div()
                                            .text_sm()
                                            .text_color(theme.muted_foreground)
                                            .child(t("manage.downloads.empty.description")),
                                    ),
                            ),
                    )
                    .children(space::render_compact_idle_pills(&self.install_disks, cx)),
            )
    }

    /// Cozy idle empty state with the static Irori bonfire mark (~72 px, 0%
    /// idle CPU), warm subtle halo, OS-neutral copy and configured drive pills.
    fn render_empty(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        v_flex()
            .id("downloads-empty")
            .debug_selector(|| "downloads-empty".into())
            .items_center()
            .justify_center()
            .gap_3()
            .p_8()
            .child(
                div()
                    .p_5()
                    .rounded_full()
                    .bg(theme.primary.opacity(0.10))
                    .border_1()
                    .border_color(theme.primary.opacity(0.18))
                    .child(BrandMark::new(rems(4.5))),
            )
            .child(
                div()
                    .text_lg()
                    .font_semibold()
                    .child(t("manage.downloads.empty.title")),
            )
            .child(
                div()
                    .max_w_96()
                    .text_sm()
                    .text_center()
                    .text_color(theme.muted_foreground)
                    .child(t("manage.downloads.empty.description")),
            )
            .children(space::render_idle_pills(&self.install_disks, cx))
    }
}

impl Render for DownloadsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let m = models(cx).clone();
        let countdown = m.downloads.read(cx).queue.countdown;
        self.sync_countdown_tick(countdown.is_some(), cx);

        let report = m.components.read(cx).report.clone();
        let app_status = m.app_update.read(cx).status.clone();
        let game_update_ids = m.library.read(cx).updates.clone();
        let games = updates::sorted_games(game_update_ids.iter().cloned(), cx);
        let failed_stores = backend(cx).update_check_failures();
        let checking = self.checking || report.checking || app_status.checking;
        let checked_at = match (
            report.checked_at,
            app_status
                .checked_at
                .filter(|_| app_status.kind.can_check_manually()),
        ) {
            (Some(a), Some(b)) => Some(a.max(b)),
            (a, b) => a.or(b),
        };

        let alerts = self.render_update_alerts(&report, &failed_stores);
        let irori = self.render_irori(cx);
        let current = self.render_current(window, cx);
        let disk_strip = if self.queue.has_active() {
            space::render(&self.disks, cx)
        } else {
            None
        };
        let game_updates = self.render_game_updates(games, cx);
        let component_updates = self.render_component_updates(&report, cx);
        let files = self.render_files(cx);

        let is_completely_idle = self.queue.is_empty()
            && files.is_none()
            && game_updates.is_none()
            && component_updates.is_none()
            && irori.is_none();

        let show_idle_banner = !is_completely_idle
            && !self.queue.has_active()
            && game_updates.is_none()
            && component_updates.is_none()
            && irori.is_none();

        let body: AnyElement = if is_completely_idle {
            v_flex()
                .id("downloads-body")
                .flex_1()
                .min_h_0()
                .px_6()
                .pb_6()
                .when(!alerts.is_empty(), |d| {
                    d.child(v_flex().gap_3().pb_4().children(alerts))
                })
                .child(
                    div()
                        .flex_1()
                        .min_h_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(self.render_empty(cx)),
                )
                .into_any_element()
        } else {
            let sections = v_flex()
                .gap_6()
                .when(!alerts.is_empty(), |d| {
                    d.child(v_flex().gap_3().children(alerts))
                })
                .when(show_idle_banner, |d| d.child(self.render_idle_banner(cx)))
                .children(irori)
                .when(current.is_some() || disk_strip.is_some(), |this| {
                    this.child(v_flex().gap_2().children(current).children(disk_strip))
                })
                .when(!self.queue.waiting.is_empty(), |this| {
                    this.child(self.render_queue(cx))
                })
                .children(game_updates)
                .children(component_updates)
                .children(files)
                .when(!self.queue.history.is_empty(), |this| {
                    this.child(self.render_history(cx))
                });
            div()
                .id("downloads-body")
                .flex_1()
                .min_h_0()
                .overflow_y_scrollbar()
                .child(
                    div()
                        .px_6()
                        .pb_6()
                        .image_cache(self.images.clone())
                        .child(sections),
                )
                .into_any_element()
        };

        v_flex()
            .size_full()
            .min_h_0()
            .child(
                v_flex()
                    .px_6()
                    .pt_6()
                    .pb_4()
                    .gap_4()
                    .child(self.render_header(checked_at, checking, &app_status, cx))
                    .children(countdown.map(|c| when_done::countdown_banner(&c, cx))),
            )
            .child(body)
    }
}

/// Drag payload and preview of a queued download.
#[derive(Clone)]
struct DraggedDownload {
    id: GameId,
    title: SharedString,
}

impl Render for DraggedDownload {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        div()
            .px_3()
            .py_1()
            .rounded(theme.radius)
            .bg(theme.popover)
            .text_color(theme.popover_foreground)
            .border_1()
            .border_color(theme.border)
            .text_sm()
            .child(self.title.clone())
    }
}

#[cfg(test)]
impl DownloadsView {
    pub(crate) fn queue_for_tests(&self) -> &QueuePresentation {
        &self.queue
    }

    pub(crate) fn summary_for_tests(&self, cx: &App) -> String {
        let app_status = models(cx).app_update.read(cx).status.clone();
        self.summary(None, &app_status)
    }

    pub(crate) fn set_install_disks_for_tests(&mut self, disks: Vec<irori_services::QueueDisk>) {
        self.install_disks = disks;
    }
}
