//! The unified downloads page ([`Route::Downloads`](irori_ui::Route::Downloads)):
//! active download hero, queued installs and updates, game and component
//! updates, Irori self-update banner, offline files and finished history.

mod actions;
mod history;
mod indicator;
mod queue;
mod sections;
mod space;
mod speed;
mod updates;
mod when_done;

pub use indicator::{DownloadsBadge, DownloadsPanel, DownloadsPill};
pub use when_done::{
    DownloadCommand, download_command_keywords, download_commands, keep_on, run_download_command,
};

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::{ActiveTheme as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, Context, Entity, InteractiveElement as _, IntoElement, ParentElement as _,
    Render, SharedString, Styled as _, Subscription, Task, Window, div, px, rems,
};
use irori_core::util::unix_now;
use irori_services::{AppUpdateStatus, SelfInstall};
use irori_ui::art::{LruImageCache, art_store};
use irori_ui::components::BrandMark;
use irori_ui::i18n::{t, t_args};
use irori_ui::{Route, backend, models, navigate};

use crate::common::card;
use crate::format;
pub(crate) use queue::{Estimates, HistoryFilter, QueuePresentation, Step};
use speed::{SPEED_SAMPLES, SpeedHistory};

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
            history_shown: history::HISTORY_PAGE,
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

#[cfg(test)]
impl DownloadsView {
    pub(crate) fn queue_for_tests(&self) -> &QueuePresentation {
        &self.queue
    }

    pub(crate) fn summary_for_tests(&self, cx: &App) -> String {
        let app_status = models(cx).app_update.read(cx).status.clone();
        self.summary(None, &app_status)
    }
}
