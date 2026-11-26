//! The downloads indicator outside the downloads page: the title bar's pill
//! (progress ring, percent, speed, how many more wait, and a panel with
//! pause and resume) and the sidebar's badge (the percent while a download
//! runs, else how many wait, else pending game and component updates).
//!
//! Each is its own small view that re-renders only when what it shows
//! changes: progress arrives a few times a second while downloading, and the
//! rest of the window doesn't redraw for it.

use std::rc::Rc;
use std::time::Instant;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::popover::Popover;
use gpui_kit::component::progress::ProgressCircle;
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render, SharedString,
    Styled as _, Subscription, Task, Window, div,
};
use irori_core::InstallProgress;
use irori_services::{DownloadQueueSnapshot, DownloadState, PowerCountdown};
use irori_ui::i18n::{t, t_args};
use irori_ui::{Route, backend, models, navigate};

use super::when_done;
use crate::format;

/// What the indicators show, derived from the queue and the current
/// download's progress. Rates are rounded so a reading that changes only in
/// digits nobody sees doesn't redraw anything.
#[derive(Debug, Clone, PartialEq, Default)]
pub(crate) struct Indicator {
    /// Title of the download being worked on (running or paused).
    pub title: Option<String>,
    /// Whole percent done of the running download.
    pub percent: Option<u8>,
    /// Download rate in bytes per second, rounded to 100 KiB/s.
    pub speed: Option<u64>,
    /// Seconds left of the running download, by the store's estimate,
    /// rounded to the minute past the first one.
    pub eta: Option<u64>,
    /// The queue (or its current download) is paused.
    pub paused: bool,
    /// Downloads waiting after the current one.
    pub waiting: usize,
    /// Unqueued game updates plus non-skipped component updates, shown on the
    /// sidebar badge when the download queue is idle.
    pub pending_updates: usize,
    /// The countdown to sleeping or powering off, once the queue is done.
    pub countdown: Option<PowerCountdown>,
}

/// The rounding step of [`Indicator::speed`].
const SPEED_STEP: u64 = 100 * 1024;

impl Indicator {
    pub fn new(
        queue: &DownloadQueueSnapshot,
        progress: Option<InstallProgress>,
        pending_updates: usize,
    ) -> Self {
        let current = queue.current();
        let paused = queue.paused || current.is_some_and(|c| c.state == DownloadState::Paused);
        let running = current.filter(|_| !paused);
        let progress = running.and(progress).unwrap_or_default();
        Self {
            title: current.map(|c| c.title.clone()),
            percent: running
                .and(progress.percent)
                .map(|p| p.clamp(0.0, 100.0).floor() as u8),
            speed: progress
                .download_speed
                .filter(|s| *s > 0)
                .map(|s| (s / SPEED_STEP).max(1) * SPEED_STEP),
            eta: progress
                .eta_secs
                .filter(|s| *s > 0)
                .map(|s| if s > 60 { s.div_ceil(60) * 60 } else { s }),
            paused,
            waiting: queue.active.len() - usize::from(current.is_some()),
            pending_updates,
            countdown: queue.countdown,
        }
    }

    /// Anything to show: a download running, paused or waiting.
    pub fn is_active(&self) -> bool {
        self.title.is_some() || self.waiting > 0
    }

    /// The sidebar badge: the percent of the running download, else how
    /// many downloads are in the queue, else pending updates when idle.
    pub fn badge(&self) -> Option<String> {
        if let Some(p) = self.percent {
            return Some(format!("{p}%"));
        }
        let queued = self.waiting + usize::from(self.title.is_some());
        if queued > 0 {
            return Some(queued.to_string());
        }
        (self.pending_updates > 0).then(|| self.pending_updates.to_string())
    }

    /// Read the indicator from the models.
    fn now(cx: &App) -> Self {
        let m = models(cx);
        let queue = &m.downloads.read(cx).queue;
        let progress = queue
            .current()
            .and_then(|c| m.statuses.read(cx).get(&c.id))
            .and_then(|s| s.progress);
        let pending_games = m
            .library
            .read(cx)
            .updates
            .iter()
            .filter(|id| !queue.active.iter().any(|i| &i.id == *id))
            .count();
        let pending_components = m.components.read(cx).report.pending().count();
        let pending_app = usize::from(m.app_update.read(cx).status.has_update());
        Self::new(
            queue,
            progress,
            pending_games + pending_components + pending_app,
        )
    }
}

/// Follow the queue, statuses and update models, redrawing the view only when
/// its indicator changed.
fn follow<V: 'static>(cx: &mut Context<V>, get: fn(&mut V) -> &mut Indicator) -> Vec<Subscription> {
    let m = models(cx).clone();
    let refresh = move |this: &mut V, cx: &mut Context<V>| {
        let now = Indicator::now(cx);
        let slot = get(this);
        if *slot != now {
            *slot = now;
            cx.notify();
        }
    };
    vec![
        cx.observe(&m.downloads, move |this, _, cx| refresh(this, cx)),
        cx.observe(&m.statuses, move |this, _, cx| refresh(this, cx)),
        cx.observe(&m.library, move |this, _, cx| refresh(this, cx)),
        cx.observe(&m.components, move |this, _, cx| refresh(this, cx)),
        cx.observe(&m.app_update, move |this, _, cx| refresh(this, cx)),
    ]
}

/// The title bar's downloads pill. A click opens a panel with the current
/// download, pause or resume, and the way to the downloads page.
pub struct DownloadsPill {
    state: Indicator,
    open: bool,
    /// Redraws the countdown every second while it runs.
    tick: Option<Task<()>>,
    panel: Entity<DownloadsPanel>,
    _subscriptions: Vec<Subscription>,
}

impl DownloadsPill {
    pub fn new(cx: &mut Context<Self>) -> Self {
        crate::i18n::register();
        let pill = cx.entity().downgrade();
        let close: Rc<dyn Fn(&mut App)> = Rc::new(move |cx| {
            let _ = pill.update(cx, |this, cx| {
                this.open = false;
                cx.notify();
            });
        });
        let panel = cx.new(|cx| DownloadsPanel::new(close, cx));
        Self {
            state: Indicator::now(cx),
            open: false,
            tick: None,
            panel,
            _subscriptions: follow(cx, |this: &mut Self| &mut this.state),
        }
    }
}

/// Keep `tick` redrawing the view every second while `counting`.
fn sync_tick<V: 'static>(tick: &mut Option<Task<()>>, counting: bool, cx: &mut Context<V>) {
    if !counting {
        *tick = None;
    } else if tick.is_none() {
        *tick = Some(cx.spawn(async move |this, cx| {
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

impl Render for DownloadsPill {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        sync_tick(&mut self.tick, self.state.countdown.is_some(), cx);
        let s = &self.state;
        let muted = cx.theme().muted_foreground;
        let tooltip: SharedString = match (&s.title, s.percent) {
            (Some(title), Some(p)) => t_args(
                "manage.downloads.pill.progress",
                &[("title", title), ("percent", &p.to_string())],
            ),
            _ if s.paused => t("manage.downloads.pill.paused"),
            _ => t("manage.downloads.pill.title"),
        };
        let more = (s.waiting > 0).then(|| {
            div()
                .flex_shrink_0()
                .text_xs()
                .text_color(muted)
                .child(format!("+{}", s.waiting))
        });
        let trigger = Button::new("title-downloads")
            .ghost()
            .small()
            .min_w_0()
            .flex_shrink(1.)
            .tooltip(tooltip)
            .map(|b| match (s.percent, s.paused, s.title.is_some()) {
                _ if s.countdown.is_some() => b.child(
                    h_flex()
                        .min_w_0()
                        .gap_1p5()
                        .text_color(cx.theme().warning)
                        .child(Icon::new(IconName::Power).xsmall())
                        .children(s.countdown.map(|c| {
                            div()
                                .min_w_0()
                                .truncate()
                                .text_xs()
                                .child(when_done::countdown_text(&c, Instant::now()))
                        })),
                ),
                (Some(p), _, _) => b.child(
                    h_flex()
                        .min_w_0()
                        .gap_1p5()
                        .child(
                            ProgressCircle::new("title-download-progress")
                                .value(f32::from(p))
                                .xsmall(),
                        )
                        .child(div().flex_shrink_0().text_xs().child(format!("{p}%")))
                        .children(s.speed.map(|speed| {
                            div()
                                .min_w_0()
                                .truncate()
                                .text_xs()
                                .text_color(muted)
                                .child(format::rate(Some(speed)))
                        }))
                        .children(more),
                ),
                (None, true, true) => b.child(
                    h_flex()
                        .min_w_0()
                        .gap_1p5()
                        .child(Icon::new(IconName::CirclePause).xsmall())
                        .child(
                            div()
                                .min_w_0()
                                .truncate()
                                .text_xs()
                                .child(t("manage.downloads.summary.paused")),
                        )
                        .children(more),
                ),
                _ if s.is_active() => b.child(
                    h_flex()
                        .gap_1p5()
                        .child(Icon::new(IconName::Download).xsmall())
                        .children(more),
                ),
                _ => b.icon(IconName::Download),
            });
        let pill = cx.entity().downgrade();
        let panel = self.panel.clone();
        Popover::new("title-downloads-popover")
            .anchor(gpui_kit::Anchor::TopRight)
            .open(self.open)
            .on_open_change(move |open, _, cx| {
                let open = *open;
                let _ = pill.update(cx, |this, cx| {
                    this.open = open;
                    cx.notify();
                });
            })
            .trigger(trigger)
            .content(move |_, _, _| panel.clone())
    }
}

/// The pill's panel: the current download with its progress and rate, how
/// many wait, pause or resume, and the downloads page.
pub struct DownloadsPanel {
    state: Indicator,
    tick: Option<Task<()>>,
    close: Rc<dyn Fn(&mut App)>,
    _subscriptions: Vec<Subscription>,
}

impl DownloadsPanel {
    fn new(close: Rc<dyn Fn(&mut App)>, cx: &mut Context<Self>) -> Self {
        Self {
            state: Indicator::now(cx),
            tick: None,
            close,
            _subscriptions: follow(cx, |this: &mut Self| &mut this.state),
        }
    }
}

impl Render for DownloadsPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        sync_tick(&mut self.tick, self.state.countdown.is_some(), cx);
        let s = self.state.clone();
        let muted = cx.theme().muted_foreground;
        let facts = [
            s.percent.map(|p| format!("{p}%")),
            s.speed.map(|speed| format::rate(Some(speed))),
            s.eta.map(|secs| {
                t_args(
                    "manage.downloads.pill.left",
                    &[("time", &format::eta(Some(secs)))],
                )
                .to_string()
            }),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" · ");
        let waiting = (s.waiting > 0).then(|| {
            let count = s.waiting.to_string();
            if s.waiting == 1 {
                t_args("manage.downloads.summary.waiting_one", &[("count", &count)])
            } else {
                t_args(
                    "manage.downloads.summary.waiting_other",
                    &[("count", &count)],
                )
            }
        });
        let (close_open, paused, active) = (self.close.clone(), s.paused, s.is_active());
        let countdown = s.countdown.map(|c| when_done::countdown_banner(&c, cx));
        v_flex()
            .w(gpui_kit::rems(18.))
            .gap_3()
            .children(countdown)
            .child(match &s.title {
                Some(title) => v_flex()
                    .gap_1()
                    .child(div().font_medium().truncate().child(title.clone()))
                    .child(if paused {
                        div()
                            .text_xs()
                            .text_color(muted)
                            .child(t("manage.downloads.summary.paused"))
                            .into_any_element()
                    } else {
                        irori_ui::motion::gliding_progress(
                            "title-downloads-progress",
                            s.percent.map(f32::from),
                        )
                    })
                    .when(!facts.is_empty() && !paused, |d| {
                        d.child(div().text_xs().text_color(muted).child(facts))
                    })
                    .into_any_element(),
                None => div()
                    .text_sm()
                    .text_color(muted)
                    .child(t("manage.downloads.summary.idle"))
                    .into_any_element(),
            })
            .children(waiting.map(|w| div().text_xs().text_color(muted).child(w)))
            .child(
                h_flex()
                    .gap_2()
                    .when(active, |row| {
                        row.child(
                            Button::new("title-downloads-pause")
                                .small()
                                .when(paused, |b| b.primary())
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
                                .on_click(move |_, _, cx| {
                                    let b = backend(cx);
                                    if paused {
                                        b.resume_downloads();
                                    } else {
                                        b.pause_downloads();
                                    }
                                }),
                        )
                    })
                    .child(
                        Button::new("title-downloads-open")
                            .small()
                            .ghost()
                            .label(t("manage.downloads.pill.open"))
                            .on_click(move |_, _, cx| {
                                close_open(cx);
                                navigate(Route::Downloads, cx);
                            }),
                    ),
            )
    }
}

/// The sidebar's Downloads badge: the running download's percent, else how
/// many downloads are queued, else pending game and component updates when idle.
pub struct DownloadsBadge {
    state: Indicator,
    _subscriptions: Vec<Subscription>,
}

impl DownloadsBadge {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            state: Indicator::now(cx),
            _subscriptions: follow(cx, |this: &mut Self| &mut this.state),
        }
    }
}

impl Render for DownloadsBadge {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        div().children(self.state.badge().map(|text| {
            div()
                .text_xs()
                .font_bold()
                .px_1p5()
                .rounded_full()
                .bg(theme.primary)
                .text_color(theme.primary_foreground)
                .child(text)
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::downloads::queue::tests::item;
    use irori_core::Runner;

    fn progress(percent: f32, speed: u64, eta: u64) -> Option<InstallProgress> {
        Some(InstallProgress {
            percent: Some(percent),
            download_speed: Some(speed),
            eta_secs: Some(eta),
            ..Default::default()
        })
    }

    #[test]
    fn running_download_with_others_waiting() {
        let queue = DownloadQueueSnapshot {
            active: vec![
                item(Runner::Gog, "a", DownloadState::Running),
                item(Runner::Gog, "b", DownloadState::Queued),
                item(Runner::Gog, "c", DownloadState::Queued),
            ],
            ..Default::default()
        };
        let s = Indicator::new(&queue, progress(42.7, 3 * SPEED_STEP + 5, 125), 4);
        assert_eq!(s.title.as_deref(), Some("A"));
        assert_eq!(s.percent, Some(42));
        assert_eq!(s.speed, Some(3 * SPEED_STEP), "rounded down to the step");
        assert_eq!(s.eta, Some(180), "rounded up to the minute");
        assert_eq!(s.waiting, 2);
        assert_eq!(s.badge().as_deref(), Some("42%"));
        // A rate that moves inside its step doesn't change what shows.
        assert_eq!(
            s,
            Indicator::new(&queue, progress(42.9, 3 * SPEED_STEP + 900, 121), 4)
        );
    }

    #[test]
    fn paused_and_idle_queues() {
        let paused = DownloadQueueSnapshot {
            active: vec![
                item(Runner::Gog, "a", DownloadState::Paused),
                item(Runner::Gog, "b", DownloadState::Queued),
            ],
            paused: true,
            ..Default::default()
        };
        let s = Indicator::new(&paused, progress(50.0, SPEED_STEP, 60), 5);
        assert!(s.paused);
        assert_eq!(s.percent, None, "a paused download shows no live numbers");
        assert_eq!(s.speed, None);
        assert_eq!(
            s.badge().as_deref(),
            Some("2"),
            "queue count wins over pending updates while paused"
        );
        let idle = Indicator::new(&DownloadQueueSnapshot::default(), None, 0);
        assert!(!idle.is_active());
        assert_eq!(idle.badge(), None);
        let idle_updates = Indicator::new(&DownloadQueueSnapshot::default(), None, 3);
        assert!(!idle_updates.is_active());
        assert_eq!(
            idle_updates.badge().as_deref(),
            Some("3"),
            "idle badge surfaces pending updates"
        );
    }

    #[gpui_kit::test]
    fn idle_badge_counts_available_irori_update(cx: &mut gpui_kit::TestAppContext) {
        use irori_services::{InstallKind, Release, SelfInstall};

        crate::ui_tests::setup(cx);
        let badge = cx.update(|cx| cx.new(DownloadsBadge::new));
        cx.update(|cx| {
            let app_update = models(cx).app_update.clone();
            app_update.update(cx, |m, cx| {
                m.status.kind = InstallKind::Manual;
                m.status.available = None;
                m.status.install = SelfInstall::Idle;
                cx.notify();
            });
        });
        assert_eq!(cx.read(|cx| badge.read(cx).state.badge()), None);

        cx.update(|cx| {
            let app_update = models(cx).app_update.clone();
            app_update.update(cx, |m, cx| {
                m.status.available = Some(Release {
                    version: "9.9.9".into(),
                    url: "https://github.com/irori-app/irori/releases/tag/v9.9.9".into(),
                });
                cx.notify();
            });
        });
        assert_eq!(
            cx.read(|cx| badge.read(cx).state.badge()).as_deref(),
            Some("1"),
            "idle Downloads badge should count an available Irori update"
        );

        // Staged Ready update still counts even if `available` is cleared.
        cx.update(|cx| {
            let app_update = models(cx).app_update.clone();
            app_update.update(cx, |m, cx| {
                m.status.available = None;
                m.status.install = SelfInstall::Ready {
                    version: "9.9.9".into(),
                };
                cx.notify();
            });
        });
        assert_eq!(
            cx.read(|cx| badge.read(cx).state.badge()).as_deref(),
            Some("1"),
            "idle Downloads badge should count a staged Ready Irori update"
        );

        cx.update(|cx| {
            let app_update = models(cx).app_update.clone();
            app_update.update(cx, |m, cx| {
                m.status.install = SelfInstall::Idle;
                cx.notify();
            });
        });
        assert_eq!(cx.read(|cx| badge.read(cx).state.badge()), None);
    }
}
