//! The main window: title bar, sidebar navigation and the routed content area.

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, Sizable as _, TitleBar, WindowExt as _, h_flex,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, ClickEvent, Context, Decorations, Entity, FocusHandle,
    Focusable, InteractiveElement, IntoElement, MouseButton, NavigationDirection,
    ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _,
    Subscription, Task, Window, div, rems,
};
use irori_core::{GameId, NoticeLevel, Runner, StatusKind};
use irori_ui::components::{BrandMark, BrandWordmark};
use irori_ui::glass::{self, Glass};
use irori_ui::i18n::t;
use irori_ui::models::{self, models};
use irori_ui::nav::{self, Route, navigator};

use crate::actions::*;
use crate::pages::Pages;
use crate::palette::Palette;
use crate::sidebar;

/// How far a newly shown page rises into place, in pixels.
const PAGE_RISE_PX: f32 = 8.0;

/// Guard window close when closing would quit the app (not hiding to the tray)
/// and transfers are active, offering Keep open, Minimize (when tray is
/// available), and Pause & quit.
fn install_close_guard(window: &mut Window, cx: &App) {
    let allow_close = std::rc::Rc::new(std::cell::Cell::new(false));
    window.on_window_should_close(cx, move |window, cx| {
        if allow_close.get() {
            return true;
        }
        let b = irori_ui::backend(cx);
        if crate::tray_hide::available(cx) || !b.has_active_transfers() {
            return true;
        }
        if window.has_active_dialog(cx) {
            return false;
        }
        let allow_quit = allow_close.clone();
        let show_minimize = crate::tray_hide::tray_running(cx);
        let width = window.rem_size() * 30.;
        window.open_dialog(cx, move |dialog, _, cx| {
            let allow_quit = allow_quit.clone();
            dialog
                .title(t("shell.quit_downloads.title"))
                .w(width)
                .child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child(t("shell.quit_downloads.body")),
                )
                .footer(
                    h_flex()
                        .w_full()
                        .items_center()
                        .justify_end()
                        .gap_2()
                        .child(
                            Button::new("quit-downloads-keep")
                                .ghost()
                                .label(t("shell.quit_downloads.keep_open"))
                                .on_click(|_, window, cx| window.close_dialog(cx)),
                        )
                        .when(show_minimize, |row| {
                            row.child(
                                Button::new("quit-downloads-minimize")
                                    .outline()
                                    .label(t("shell.quit_downloads.minimize"))
                                    .on_click(|_, window, cx| {
                                        window.close_dialog(cx);
                                        window.minimize_window();
                                    }),
                            )
                        })
                        .child(
                            Button::new("quit-downloads-pause-quit")
                                .danger()
                                .label(t("shell.quit_downloads.pause_and_quit"))
                                .on_click(move |_, window, cx| {
                                    let b = irori_ui::backend(cx);
                                    b.pause_all_downloads();
                                    allow_quit.set(true);
                                    window.close_dialog(cx);
                                    let shutdown_b = b.clone();
                                    let task = irori_ui::state::run_infallible(cx, async move {
                                        let _ = tokio::time::timeout(
                                            std::time::Duration::from_secs(5),
                                            shutdown_b.shutdown(),
                                        )
                                        .await;
                                    });
                                    cx.spawn(async move |cx| {
                                        task.await;
                                        cx.update(|cx| cx.quit());
                                    })
                                    .detach();
                                }),
                        ),
                )
        });
        false
    });
}

pub struct AppShell {
    focus: FocusHandle,
    palette: Palette,
    pages: Pages,
    sidebar_collapsed: bool,
    /// The sidebar was opened by hand while the window is narrow, where it
    /// otherwise collapses on its own. Not saved: a wide window forgets it.
    narrow_expanded: bool,
    recent: Entity<sidebar::RecentGames>,
    saved_views: Entity<sidebar::SavedViewsNav>,
    /// The title bar's downloads pill and the sidebar's Downloads badge,
    /// each redrawn on its own as progress arrives.
    downloads_pill: Entity<irori_manage::DownloadsPill>,
    downloads_badge: Entity<irori_manage::DownloadsBadge>,
    /// "Keep holding to open Big Picture" while a controller button is held.
    hold_hint: Entity<irori_bigpicture::gamepad::HoldHintView>,
    /// The start-with-system choice last written to the system.
    autostart: (bool, bool),
    /// Games we minimized the window for, to restore when they exit.
    minimized_for: Option<GameId>,
    /// The game seen playing, whose log was offered when it started.
    log_shown_for: Option<GameId>,
    /// Refreshes the title bar's elapsed play time while a game runs.
    playing_ticker: Option<Task<()>>,
    /// Appearance preferences last applied to the theme.
    applied_ui: Option<irori_core::settings::UiPrefs>,
    /// Glass last applied to the window, so it is changed only when the
    /// preference, route or fullscreen state calls for another.
    glass: Option<Glass>,
    /// Notices about Irori's own versions shown this session.
    app_update: crate::app_update::Announced,
    /// The first route was chosen knowing the library and the accounts.
    start_settled: bool,
    /// Saves the window's place once it has stopped moving.
    placement_save: Option<Task<()>>,
    /// Scrolling with the middle button held, when turned on.
    middle_scroll: irori_ui::middle_scroll::MiddleScroll,
    _subscriptions: Vec<Subscription>,
}

impl AppShell {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        // Written at every start, so the entry names the program running now.
        let autostart = {
            let app = &models(cx).settings.read(cx).app;
            (app.start_with_system, app.start_with_system_minimized)
        };
        crate::autostart::sync(autostart.0, autostart.1, crate::autostart::Trigger::Start);
        let m = models(cx).clone();
        let nav = navigator(cx);
        let subs = vec![
            cx.observe_in(&nav, window, |this, _, window, cx| {
                this.sync_glass(window, cx);
                cx.notify();
            }),
            cx.observe_in(&m.downloads, window, |this, _, window, cx| {
                this.sync_power_countdown(window, cx);
                cx.notify();
            }),
            cx.observe(&m.components, |_, _, cx| cx.notify()),
            cx.observe(&m.library, |_, _, cx| cx.notify()),
            cx.observe(&m.accounts, |this, _, cx| {
                this.settle_start(cx);
                cx.notify();
            }),
            cx.observe_in(&m.notices, window, |this, notices, window, cx| {
                this.show_notices(&notices, window, cx);
            }),
            cx.observe_in(&m.statuses, window, |this, _, window, cx| {
                this.show_log_on_launch(window, cx);
                this.minimize_on_launch(window, cx);
                this.sync_playing_ticker(cx);
            }),
            cx.observe_in(&m.sessions, window, |this, sessions, window, cx| {
                this.on_sessions(&sessions, window, cx);
            }),
            cx.observe_in(&m.app_update, window, |this, _, window, cx| {
                this.app_update.show(window, cx);
                cx.notify();
            }),
            cx.observe_in(&m.settings, window, |this, settings, window, cx| {
                let lang = settings.read(cx).app.ui_language.clone();
                if lang != irori_ui::i18n::language() {
                    irori_ui::i18n::set_language(&lang);
                    window.refresh();
                }
                let prefs = settings.read(cx).app.ui.clone();
                // GPUI holds every animation still while this is set.
                let reduce = irori_ui::style::reduce_motion(cx);
                cx.set_reduce_motion(reduce);
                // Settings notify for every change; re-theme only when the
                // appearance preferences did.
                if this.applied_ui.as_ref() != Some(&prefs) {
                    irori_ui::theme::apply(&prefs, Some(window), cx);
                    this.applied_ui = Some(prefs);
                }
                this.sync_glass(window, cx);
                let app = &settings.read(cx).app;
                let autostart = (app.start_with_system, app.start_with_system_minimized);
                if autostart != this.autostart {
                    this.autostart = autostart;
                    crate::autostart::sync(
                        autostart.0,
                        autostart.1,
                        crate::autostart::Trigger::Changed,
                    );
                }
            }),
            // Entering or leaving fullscreen; moving or resizing.
            cx.observe_window_bounds(window, |this, window, cx| {
                this.sync_glass(window, cx);
                this.save_placement(window, cx);
            }),
            // The desktop moved its window buttons (Linux).
            cx.observe_button_layout_changed(window, |_, _, cx| cx.notify()),
            // "Follow system" follows the system while Irori runs, too.
            cx.observe_window_appearance(window, |_, window, cx| {
                let prefs = models(cx).settings.read(cx).app.ui.clone();
                irori_ui::theme::apply(&prefs, Some(window), cx);
            }),
        ];
        // The root view exists once this effect cycle ends, before the first
        // frame is drawn.
        cx.defer_in(window, |this, window, cx| this.sync_glass(window, cx));
        install_close_guard(window, cx);
        // Notices and sessions that ended while no window was open (started
        // in the tray, or closed to it).
        cx.defer_in(window, |this, window, cx| {
            this.settle_start(cx);
            let m = models(cx).clone();
            this.show_notices(&m.notices, window, cx);
            this.on_sessions(&m.sessions, window, cx);
            this.app_update.show(window, cx);
            this.show_last_crash(window, cx);
            if *navigator(cx).read(cx).current() == Route::BigPicture {
                irori_ui::state::run_infallible(cx, irori_services::power::activate_window())
                    .detach();
            }
        });
        let collapsed = m.settings.read(cx).app.ui.sidebar_collapsed;
        let reduce = irori_ui::style::reduce_motion(cx);
        cx.set_reduce_motion(reduce);
        Self {
            focus: cx.focus_handle(),
            palette: Palette::default(),
            pages: Pages::default(),
            sidebar_collapsed: collapsed,
            narrow_expanded: false,
            recent: cx.new(sidebar::RecentGames::new),
            saved_views: cx.new(sidebar::SavedViewsNav::new),
            downloads_pill: cx.new(irori_manage::DownloadsPill::new),
            downloads_badge: cx.new(irori_manage::DownloadsBadge::new),
            hold_hint: irori_bigpicture::gamepad::HoldHintView::new(window, cx),
            autostart,
            minimized_for: None,
            log_shown_for: None,
            playing_ticker: None,
            applied_ui: Some(m.settings.read(cx).app.ui.clone()),
            glass: None,
            app_update: Default::default(),
            start_settled: false,
            placement_save: None,
            middle_scroll: Default::default(),
            _subscriptions: subs,
        }
    }

    /// The window opens before the stores have read their state, when
    /// nothing looks logged in and the library looks empty. Correct the first
    /// route once they have, but only then: logging in during onboarding
    /// must not end it.
    fn settle_start(&mut self, cx: &mut Context<Self>) {
        let backend = irori_ui::backend(cx);
        // The models follow the backend a moment later, and this reads them.
        let caught_up = models(cx).library.read(cx).snapshot.revision == backend.library().revision;
        if self.start_settled || !backend.stores_ready() || !caught_up {
            return;
        }
        self.start_settled = true;
        let nav = navigator(cx);
        if let Some(route) = route_once_loaded(nav.read(cx).current(), wants_onboarding(cx)) {
            nav.update(cx, |nav, cx| nav.replace(route, cx));
        }
    }

    fn show_last_crash(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(path) = crate::crash::take_pending(cx) else {
            return;
        };
        let click_path = path.clone();
        let action_path = path;
        let note = Notification::warning(t("shell.notice.crashed_last_time_message"))
            .title(t("shell.notice.crashed_last_time"))
            .id::<LastCrashNote>()
            .autohide(false)
            .on_click(move |_, window, cx| {
                irori_settings::open_crash_report_sheet(click_path.clone(), window, cx);
                window.remove_notification::<LastCrashNote>(cx);
            })
            .action(move |_, _, cx| {
                let action_path = action_path.clone();
                Button::new("crash-review-report")
                    .small()
                    .primary()
                    .label(t("shell.notice.crashed_review_action"))
                    .on_click(cx.listener(move |note, _, window, cx| {
                        irori_settings::open_crash_report_sheet(action_path.clone(), window, cx);
                        note.dismiss(window, cx);
                    }))
            });
        window.push_notification(note, cx);
    }

    fn show_notices(
        &mut self,
        notices: &gpui_kit::Entity<models::NoticeModel>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let pending = notices.update(cx, |n, _| n.drain());
        for n in pending {
            let (title, message) = irori_ui::system_notify::notice_text(&n);
            let mut note = match n.level {
                NoticeLevel::Info => Notification::info(title.clone()),
                NoticeLevel::Success => Notification::success(title.clone()),
                NoticeLevel::Warning => Notification::warning(title.clone()),
                NoticeLevel::Error => Notification::error(title.clone()),
            };
            if let Some(msg) = message {
                note = note.title(title).message(msg);
            }
            if let Some(route) = irori_ui::system_notify::notice_route(&n) {
                note = note.on_click(move |_, _, cx| nav::navigate(route.clone(), cx));
            }
            if !cfg!(windows) && has_wine_rollback_shortcut(&n) {
                note = note.action(|_, _, cx| {
                    Button::new("component-rollback-shortcut")
                        .small()
                        .ghost()
                        .label(t("shell.notice.manage_rollback"))
                        .on_click(cx.listener(|note, _, window, cx| {
                            nav::navigate(Route::WineManager, cx);
                            note.dismiss(window, cx);
                        }))
                });
            }
            if is_power_countdown(&n) {
                // Stays until the countdown ends or is cancelled, with the
                // way to cancel it right there.
                note = note
                    .id::<PowerCountdownNote>()
                    .autohide(false)
                    .on_click(|_, _, cx| nav::navigate(Route::Downloads, cx))
                    .action(|_, _, _| {
                        Button::new("power-countdown-keep-on")
                            .small()
                            .primary()
                            .label(t("shell.notice.keep_on"))
                            .on_click(|_, window, cx| {
                                irori_manage::keep_on(cx);
                                window.remove_notification::<PowerCountdownNote>(cx);
                            })
                    });
            }
            window.push_notification(note, cx);
        }
    }

    /// Take the countdown's notice away once the countdown is over.
    fn sync_power_countdown(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if models(cx).downloads.read(cx).queue.countdown.is_none() {
            window.remove_notification::<PowerCountdownNote>(cx);
        }
    }

    fn show_log_on_launch(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let enabled = models(cx).settings.read(cx).app.show_log_on_launch;
        let playing = models(cx).statuses.read(cx).playing().next().cloned();
        if let Some(id) =
            crate::playing::log_to_open(enabled, playing.as_ref(), self.log_shown_for.as_ref())
        {
            irori_game::open_log_sheet(id, window, cx);
        }
        self.log_shown_for = playing;
    }

    /// "Minimize when a game starts": close to the tray when "Close to tray"
    /// can bring the window back, else minimize.
    fn minimize_on_launch(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let enabled = models(cx).settings.read(cx).app.minimize_on_launch;
        let route = navigator(cx).read(cx).current().clone();
        let action = crate::tray_hide::on_launch(
            enabled,
            route == Route::BigPicture,
            self.minimized_for.is_some(),
            crate::tray_hide::available(cx),
        );
        match action {
            crate::tray_hide::OnLaunch::Stay => {}
            crate::tray_hide::OnLaunch::Minimize => {
                let playing = models(cx).statuses.read(cx).playing().next().cloned();
                if let Some(id) = playing {
                    self.minimized_for = Some(id);
                    window.minimize_window();
                }
            }
            crate::tray_hide::OnLaunch::HideToTray => {
                let playing: Vec<GameId> =
                    models(cx).statuses.read(cx).playing().cloned().collect();
                if playing.is_empty() {
                    return;
                }
                // The window opens again where it is now, even if it moved
                // within the last second.
                self.save_placement_now(window, cx);
                crate::tray_hide::note(playing, route, cx);
                window.remove_window();
            }
        }
    }

    fn on_sessions(
        &mut self,
        sessions: &gpui_kit::Entity<models::SessionModel>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let exited: Vec<_> = sessions.update(cx, |s, _| s.exited.drain(..).collect());
        let route = navigator(cx).read(cx).current().clone();
        for exit in exited {
            let activate = should_activate_on_exit(self.minimized_for.as_ref(), &exit.id, &route);
            if self.minimized_for.as_ref() == Some(&exit.id) {
                self.minimized_for = None;
            }
            if activate {
                window.activate_window();
                irori_ui::state::run_infallible(cx, irori_services::power::activate_window())
                    .detach();
            }
            if exit.crashed {
                irori_game::open_launch_problem_sheet(exit.id, exit.exit_code, window, cx);
            } else if let Some(note) = crate::playing::session_note(&exit.id, exit.seconds, cx) {
                window.push_notification(note, cx);
            }
        }
    }

    /// Run the title bar's play-time ticker exactly while a game is playing,
    /// so an idle launcher keeps using no CPU.
    fn sync_playing_ticker(&mut self, cx: &mut Context<Self>) {
        let playing = models(cx).statuses.read(cx).playing().next().is_some();
        if !playing {
            self.playing_ticker = None;
        } else if self.playing_ticker.is_none() {
            self.playing_ticker = Some(cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor().timer(crate::playing::TICK).await;
                    if this.update(cx, |_, cx| cx.notify()).is_err() {
                        break;
                    }
                }
            }));
        }
    }

    /// Bring the window's translucency in line with the preference, the route
    /// and fullscreen. Cheap when nothing changed, which is nearly always: it
    /// runs on every settings change, navigation and resize.
    fn sync_glass(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let glass = if window.is_fullscreen() {
            Glass::Opaque
        } else {
            initial_glass(navigator(cx).read(cx).current(), cx)
        };
        if self.glass == Some(glass) {
            glass::refit(window, cx);
        } else if glass::apply(glass, window, cx) {
            self.glass = Some(glass);
        }
    }

    /// Whether the window is too narrow for the full sidebar beside the page.
    fn is_narrow(window: &Window) -> bool {
        irori_ui::style::visible_size(window).width / window.rem_size() < sidebar::NARROW_WINDOW_REM
    }

    /// Whether the sidebar shows collapsed right now: the saved choice, except
    /// in narrow windows, where it collapses unless opened by hand.
    fn sidebar_collapsed_now(&self, window: &Window) -> bool {
        if Self::is_narrow(window) {
            !self.narrow_expanded
        } else {
            self.sidebar_collapsed
        }
    }

    /// Ctrl+B and the sidebar button. In a narrow window this only opens the
    /// sidebar for now; the saved preference is for wide windows.
    fn toggle_sidebar(&mut self, window: &Window, cx: &mut Context<Self>) {
        if Self::is_narrow(window) {
            self.narrow_expanded = !self.narrow_expanded;
            cx.notify();
        } else {
            let c = !self.sidebar_collapsed;
            self.set_sidebar_collapsed(c, cx);
        }
    }

    /// Save where the window is, a second after it stopped moving, so it
    /// reopens there (`crate::placement`).
    fn save_placement(&mut self, window: &Window, cx: &mut Context<Self>) {
        let previous = crate::placement::Saved::from_prefs(&models(cx).settings.read(cx).app.ui);
        let Some(saved) = crate::placement::current(window, &previous, cx) else {
            return;
        };
        let backend = irori_ui::backend(cx);
        let timer = cx
            .background_executor()
            .timer(std::time::Duration::from_secs(1));
        self.placement_save = Some(cx.background_spawn(async move {
            timer.await;
            let current = crate::placement::Saved::from_prefs(&backend.settings.app().ui);
            if current != saved {
                backend
                    .settings
                    .update_app(|s| crate::placement::store(&saved, &mut s.ui));
            }
        }));
    }

    /// Save the window's place at once, for a window about to close.
    fn save_placement_now(&mut self, window: &Window, cx: &mut Context<Self>) {
        self.placement_save = None;
        let previous = crate::placement::Saved::from_prefs(&models(cx).settings.read(cx).app.ui);
        let Some(saved) = crate::placement::current(window, &previous, cx) else {
            return;
        };
        if saved == previous {
            return;
        }
        let backend = irori_ui::backend(cx);
        cx.background_spawn(async move {
            backend
                .settings
                .update_app(|s| crate::placement::store(&saved, &mut s.ui));
        })
        .detach();
    }

    fn set_sidebar_collapsed(&mut self, collapsed: bool, cx: &mut Context<Self>) {
        self.sidebar_collapsed = collapsed;
        let b = irori_ui::backend(cx);
        b.settings
            .update_app(|s| s.ui.sidebar_collapsed = collapsed);
        cx.notify();
    }

    fn render_title_bar(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        // One flat tint, like the sidebar, instead of the title bar's gradient.
        let flat = glass::current(cx)
            .is_translucent()
            .then(|| glass::chrome(cx.theme().title_bar, cx));
        let open_palette = cx.listener(|this, _, window, cx| {
            this.palette.open(window, cx);
        });
        let content = title_bar_content(
            cx,
            open_palette,
            self.downloads_pill.clone().into_any_element(),
        );
        // The desktop's own button layout, where Irori draws the buttons.
        let desktop = cx
            .button_layout()
            .filter(|l| !crate::title_bar::is_kits_layout(l))
            .filter(|_| matches!(window.window_decorations(), Decorations::Client { .. }));
        if let Some(layout) = desktop {
            let bar = crate::title_bar::DesktopTitleBar::new(layout, content);
            return match flat {
                Some(color) => bar.flat(color),
                None => bar,
            }
            .into_any_element();
        }
        let mut title_bar = TitleBar::new();
        if let Some(color) = flat {
            title_bar = title_bar.bg(color);
        }
        title_bar.child(content).into_any_element()
    }

    fn render_sidebar(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let section = navigator(cx).read(cx).current().section();
        let downloads = {
            let d = models(cx).downloads.read(cx);
            d.queue.active.len() + d.web.active.len()
        };
        let updates = models(cx).library.read(cx).updates.len()
            + models(cx).components.read(cx).report.pending().count()
            + usize::from(models(cx).app_update.read(cx).status.has_update());
        let collapsed = self.sidebar_collapsed_now(window);
        let accounts_expired = models(cx).accounts.read(cx).any_expired();
        let theme = cx.theme();
        let item = |id, label: &'static str, icon, key: &str, route, suffix, cx: &App| {
            sidebar::nav_item(
                sidebar::NavItem {
                    id,
                    label: t(label),
                    icon,
                    route,
                    active: section == key,
                    suffix,
                },
                collapsed,
                cx,
            )
        };
        // Active download percent, else queue count, else pending updates count.
        let download_count =
            (downloads > 0 || updates > 0).then(|| self.downloads_badge.clone().into_any_element());
        let settings_dot = accounts_expired.then(|| {
            div()
                .flex_none()
                .size_2()
                .rounded_full()
                .bg(theme.warning)
                .into_any_element()
        });

        v_flex()
            .id("main-sidebar")
            .h_full()
            .w(rems(if collapsed {
                sidebar::COLLAPSED_REM
            } else {
                sidebar::WIDTH_REM
            }))
            .flex_shrink_0()
            .px_3()
            .pb_3()
            .gap_0p5()
            .bg(glass::chrome(theme.sidebar, cx))
            .border_r_1()
            .border_color(theme.sidebar_border)
            .child(
                h_flex()
                    .h(rems(4.5))
                    .items_center()
                    .gap_2()
                    .px_1()
                    .when(collapsed, |d| d.justify_center().px_0())
                    .child(BrandMark::new(if collapsed {
                        rems(2.)
                    } else {
                        rems(2.15)
                    }))
                    .when(!collapsed, |this| {
                        this.child(BrandWordmark::new(
                            rems(1.45),
                            irori_ui::style::SidebarPalette::new(cx).text,
                        ))
                    }),
            )
            .child(item(
                "nav-library",
                "shell.nav.library",
                IconName::LibraryBig,
                "library",
                Route::Library,
                None,
                cx,
            ))
            .child(item(
                "nav-downloads",
                "shell.nav.downloads",
                IconName::Download,
                "downloads",
                Route::Downloads,
                download_count,
                cx,
            ))
            .child(item(
                "nav-stores",
                "shell.nav.stores",
                IconName::Store,
                "stores",
                Route::Stores,
                None,
                cx,
            ))
            .child(item(
                "nav-stats",
                "shell.nav.stats",
                IconName::ChartColumn,
                "stats",
                Route::Stats,
                None,
                cx,
            ))
            .when(!collapsed, |d| {
                d.child(
                    v_flex()
                        .id("sidebar-scroll")
                        .flex_1()
                        .min_h_0()
                        .overflow_y_scroll()
                        .child(self.saved_views.clone())
                        .child(self.recent.clone()),
                )
            })
            .when(collapsed, |d| d.child(div().flex_1()))
            .child(item(
                "nav-settings",
                "shell.nav.settings",
                IconName::Settings,
                "settings",
                Route::Settings("general".into()),
                settings_dot,
                cx,
            ))
            .child(
                h_flex()
                    .pt_3()
                    .gap_1()
                    .when(collapsed, |d| d.flex_col())
                    .child(sidebar::big_picture_button(collapsed, cx))
                    .child(irori_ui::tour::mark(irori_ui::tour::anchors::BIG_PICTURE))
                    .child(
                        Button::new("toggle-sidebar")
                            .ghost()
                            .icon(if collapsed {
                                IconName::PanelLeftOpen
                            } else {
                                IconName::PanelLeftClose
                            })
                            .tooltip(t("shell.nav.toggle_sidebar"))
                            .on_click(
                                cx.listener(|this, _, window, cx| this.toggle_sidebar(window, cx)),
                            ),
                    ),
            )
            .child(irori_ui::tour::mark(irori_ui::tour::anchors::SIDEBAR))
    }
}

/// Names the notice of a countdown to sleeping or powering off, so it can be
/// taken away when the countdown ends.
struct PowerCountdownNote;
struct LastCrashNote;

/// The notice the backend raises when a finished queue starts counting down
/// (not the ones saying the computer was left on).
fn is_power_countdown(n: &irori_core::Notice) -> bool {
    n.title_key == Some(irori_services::DOWNLOADS_FINISHED_NOTICE)
        && matches!(
            n.message_key,
            Some("shell.notice.sleep_soon" | "shell.notice.power_off_soon")
        )
}

/// Whether `n` reports an in-place Wine/Proton component update that saved a
/// rollback build, so the toast can offer a direct shortcut to Wine Manager.
fn has_wine_rollback_shortcut(n: &irori_core::Notice) -> bool {
    n.title_key == Some("shell.notice.component_updated")
        && n.args
            .iter()
            .any(|(k, v)| *k == irori_services::COMPONENT_ROLLBACK_ARG && !v.is_empty())
}

/// The title bar's contents: navigation, the search button and the live status
/// group.
///
/// The three tracks grow equally from nothing, and the search button is centred
/// in the middle one, so its position depends only on the bar's width. A wider
/// status group (the playing pill, downloads, the offline mark) must not move
/// it. In a narrow window the tracks give the middle one a third of the bar, and
/// the button shortens its label rather than pushing the status group under it.
fn title_bar_content(
    cx: &mut App,
    open_palette: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    downloads: AnyElement,
) -> AnyElement {
    let nav = navigator(cx).read(cx);
    let (can_back, can_forward) = (nav.can_go_back(), nav.can_go_forward());
    let online = models(cx).accounts.read(cx).online;
    let working_offline = irori_ui::models::working_offline(cx);
    let playing = crate::playing::pill(cx);

    h_flex()
        .debug_selector(|| "title-bar-content".into())
        .w_full()
        .pr_2()
        .gap_1()
        .child(
            h_flex()
                .flex_1()
                .min_w_0()
                .gap_1()
                .child(
                    Button::new("nav-back")
                        .ghost()
                        .small()
                        .icon(IconName::ChevronLeft)
                        .tooltip(t("shell.nav.back"))
                        .disabled(!can_back)
                        .on_click(|_, _, cx| nav::go_back(cx)),
                )
                .child(
                    Button::new("nav-forward")
                        .ghost()
                        .small()
                        .icon(IconName::ChevronRight)
                        .tooltip(t("shell.nav.forward"))
                        .disabled(!can_forward)
                        .on_click(|_, _, cx| nav::go_forward(cx)),
                ),
        )
        .child(
            h_flex()
                .flex_1()
                .min_w_0()
                .justify_center()
                .child(
                    Button::new("open-palette")
                        .debug_selector(|| "open-palette".into())
                        .outline()
                        .small()
                        .w(rems(22.))
                        .min_w_0()
                        .flex_shrink(1.)
                        .icon(IconName::Search)
                        .label(t("shell.search.placeholder"))
                        .tooltip("Ctrl+K")
                        .on_click(open_palette),
                )
                .child(irori_ui::tour::mark(irori_ui::tour::anchors::PALETTE)),
        )
        .child(
            h_flex()
                .id("title-bar-status")
                .debug_selector(|| "title-bar-status".into())
                .flex_1()
                .min_w_0()
                .gap_1()
                .justify_end()
                .children(playing)
                .when(working_offline, |this| {
                    // The user's choice, so it is a button that undoes it.
                    this.child(
                        Button::new("working-offline")
                            .debug_selector(|| "working-offline".into())
                            .ghost()
                            .small()
                            .min_w_0()
                            .icon(IconName::WifiOff)
                            .label(t("shell.working_offline"))
                            .tooltip(t("shell.working_offline.tooltip"))
                            .on_click(|_, _, cx| irori_ui::models::set_working_offline(false, cx)),
                    )
                })
                .when(!online && !working_offline, |this| {
                    this.child(
                        h_flex()
                            .id("offline")
                            .min_w_0()
                            .gap_1()
                            .px_2()
                            .text_xs()
                            .text_color(cx.theme().warning)
                            .child(Icon::new(IconName::WifiOff).xsmall())
                            .child(div().min_w_0().truncate().child(t("shell.offline")))
                            .tooltip(|window, cx| {
                                gpui_kit::component::tooltip::Tooltip::new(t(
                                    "shell.offline.tooltip",
                                ))
                                .build(window, cx)
                            }),
                    )
                })
                .child(
                    div()
                        .debug_selector(|| "title-downloads".into())
                        .min_w_0()
                        .flex_shrink(1.)
                        .child(downloads),
                ),
        )
        .into_any_element()
}

impl Focusable for AppShell {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

/// Bind hardware mouse back/forward buttons to window navigation history.
fn bind_mouse_navigation<E: InteractiveElement>(el: E, enabled: bool) -> E {
    if !enabled {
        return el;
    }
    el.on_mouse_down(
        MouseButton::Navigate(NavigationDirection::Back),
        |_, _, cx| {
            nav::go_back(cx);
            cx.stop_propagation();
        },
    )
    .on_mouse_down(
        MouseButton::Navigate(NavigationDirection::Forward),
        |_, _, cx| {
            nav::go_forward(cx);
            cx.stop_propagation();
        },
    )
}

impl Render for AppShell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !Self::is_narrow(window) {
            self.narrow_expanded = false;
        }
        let route = navigator(cx).read(cx).current().clone();
        let fullscreen_mode = matches!(route, Route::BigPicture);
        let show_sidebar = !matches!(
            route,
            Route::BigPicture | Route::Onboarding | Route::YearInReview(_)
        );
        let content = self.pages.view(&route, window, cx);
        // Each new page settles in from just below; its id changes with the route.
        let content = irori_ui::motion::rise_in(
            page_transition_key(&route),
            div().size_full().child(content),
            PAGE_RISE_PX,
        );
        let modal_dim = irori_ui::theme::modal_dim(window, cx);
        // Big Picture has its own scrolling and no pointer to speak of.
        let middle_scroll =
            !fullscreen_mode && models(cx).settings.read(cx).app.ui.middle_click_scroll;

        v_flex()
            .id("irori-shell")
            .key_context(SHELL_CONTEXT)
            .track_focus(&self.focus)
            .relative()
            .size_full()
            // Translucent, the shell leaves the window clear and each region
            // paints its own surface over the backdrop.
            .when(!glass::current(cx).is_translucent(), |this| {
                this.bg(cx.theme().background)
            })
            .text_color(cx.theme().foreground)
            .on_action(cx.listener(|_, _: &GoBack, _, cx| nav::go_back(cx)))
            .on_action(cx.listener(|_, _: &GoForward, _, cx| nav::go_forward(cx)))
            .map(|this| bind_mouse_navigation(this, !fullscreen_mode))
            .on_action(cx.listener(|this, _: &OpenPalette, window, cx| {
                this.palette.open(window, cx);
            }))
            .on_action(cx.listener(|_, _: &GoLibrary, _, cx| nav::navigate(Route::Library, cx)))
            .on_action(cx.listener(|_, _: &GoDownloads, _, cx| nav::navigate(Route::Downloads, cx)))
            .on_action(cx.listener(|_, _: &GoStores, _, cx| nav::navigate(Route::Stores, cx)))
            .on_action(cx.listener(|_, _: &GoStats, _, cx| nav::navigate(Route::Stats, cx)))
            .on_action(cx.listener(|_, _: &GoWine, _, cx| nav::navigate(Route::WineManager, cx)))
            .on_action(cx.listener(|_, _: &GoEmulators, _, cx| nav::navigate(Route::Emulators, cx)))
            .on_action(cx.listener(|_, _: &GoAccounts, _, cx| nav::navigate(Route::Accounts, cx)))
            .on_action(cx.listener(|_, _: &GoSettings, _, cx| {
                nav::navigate(Route::Settings("general".into()), cx)
            }))
            .on_action(
                cx.listener(|_, _: &RefreshLibrary, _, cx| models::refresh_library(false, cx)),
            )
            .on_action(
                cx.listener(|this, _: &ToggleSidebar, window, cx| this.toggle_sidebar(window, cx)),
            )
            .on_action(cx.listener(|_, _: &ToggleFullscreen, window, _| window.toggle_fullscreen()))
            .on_action(
                cx.listener(|_, _: &EnterBigPicture, _, cx| nav::navigate(Route::BigPicture, cx)),
            )
            .on_action(cx.listener(|_, _: &PickRandomGame, window, cx| {
                irori_library::open_random_game(window, cx)
            }))
            .when(!fullscreen_mode, |this| {
                this.child(self.render_title_bar(window, cx))
            })
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .items_stretch()
                    .when(show_sidebar, |this| {
                        this.child(self.render_sidebar(window, cx))
                    })
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            // A faint warm glow at the top gives pages depth.
                            .bg(gpui_kit::linear_gradient(
                                180.,
                                gpui_kit::linear_color_stop(
                                    glass::page(cx.theme().secondary, cx),
                                    0.,
                                ),
                                gpui_kit::linear_color_stop(
                                    glass::page(cx.theme().background, cx),
                                    0.45,
                                ),
                            ))
                            .child(content),
                    ),
            )
            .when_some(modal_dim, |this, dim| {
                this.child(div().absolute().inset_0().bg(dim))
            })
            .child(self.hold_hint.clone())
            .child(self.middle_scroll.element(middle_scroll))
            .when(irori_ui::tour::running(cx), |this| {
                this.child(irori_ui::tour::tour(cx))
            })
    }
}

/// The glass the window should have on `route`, fullscreen aside. Big Picture
/// covers the screen with its own art, so it is opaque.
pub fn initial_glass(route: &Route, cx: &App) -> Glass {
    if matches!(route, Route::BigPicture) {
        Glass::Opaque
    } else {
        glass::resolve(models(cx).settings.read(cx).app.ui.translucency, cx)
    }
}

/// Launch a game, reporting failures as a notification.
pub fn launch_game(id: GameId, window: &mut Window, cx: &mut App) {
    let b = irori_ui::backend(cx);
    let busy = models(cx)
        .statuses
        .read(cx)
        .kind(&id)
        .is_some_and(StatusKind::is_busy);
    if busy {
        return;
    }
    let launch_id = id.clone();
    let task = irori_ui::run(
        cx,
        async move { b.launch(&launch_id, None, Vec::new()).await },
    );
    window
        .spawn(cx, async move |cx| {
            if let Err(e) = task.await {
                let _ = cx.update(|window, cx| {
                    if irori_ui::save_conflict::resolve_launch_error(
                        &id,
                        Default::default(),
                        &e,
                        window,
                        cx,
                    ) {
                        return;
                    }
                    window.push_notification(
                        Notification::error(format!("{e:#}")).title(t("shell.launch_failed")),
                        cx,
                    );
                });
            }
        })
        .detach();
}

/// Default route at startup: the welcome flow until the user finished or
/// skipped it, else the library.
pub fn initial_route(cx: &App) -> Route {
    if wants_onboarding(cx) {
        Route::Onboarding
    } else {
        Route::Library
    }
}

fn wants_onboarding(cx: &App) -> bool {
    let has_sideload = models(cx).library.read(cx).snapshot.count(Runner::Sideload) > 0;
    irori_manage::should_show_onboarding(cx) && !has_sideload
}

/// Whether a game exiting should bring Irori's window to the front: when
/// this game minimized it on launch, or while Big Picture is open.
fn should_activate_on_exit(minimized_for: Option<&GameId>, exited: &GameId, route: &Route) -> bool {
    minimized_for == Some(exited) || *route == Route::BigPicture
}

/// Key for the page entrance animation. Switching between sections inside
/// Settings reuses one key so the Settings sidebar stays steady.
fn page_transition_key(route: &Route) -> SharedString {
    if route.section() == "settings" {
        "page:settings".into()
    } else {
        format!("page:{route:?}").into()
    }
}

/// Where to go once the stores have loaded, if not where the user is.
fn route_once_loaded(current: &Route, wants_onboarding: bool) -> Option<Route> {
    (*current == Route::Onboarding && !wants_onboarding).then_some(Route::Library)
}

#[cfg(test)]
mod start_tests {
    use super::*;

    #[test]
    fn settings_routes_share_a_transition_key() {
        let key = page_transition_key(&Route::Settings("general".into()));
        assert_eq!(key, "page:settings");
        assert_eq!(page_transition_key(&Route::Settings("storage".into())), key);
        assert_eq!(page_transition_key(&Route::Accounts), key);
        assert_eq!(page_transition_key(&Route::WineManager), key);
        assert_eq!(page_transition_key(&Route::Emulators), key);
        assert_ne!(page_transition_key(&Route::Library), key);
        assert_ne!(page_transition_key(&Route::Stores), key);
    }

    #[test]
    fn onboarding_is_left_only_when_it_turns_out_not_to_be_needed() {
        assert_eq!(
            route_once_loaded(&Route::Onboarding, false),
            Some(Route::Library)
        );
        assert_eq!(route_once_loaded(&Route::Onboarding, true), None);
        assert_eq!(route_once_loaded(&Route::Library, false), None);
        assert_eq!(route_once_loaded(&Route::BigPicture, false), None);
    }

    #[test]
    fn only_the_countdown_notice_offers_to_keep_the_computer_on() {
        let notice = |message| {
            irori_core::Notice::new(NoticeLevel::Info, "Downloads finished").keys(
                irori_services::DOWNLOADS_FINISHED_NOTICE,
                Some(message),
                &[],
            )
        };
        assert!(is_power_countdown(&notice("shell.notice.sleep_soon")));
        assert!(is_power_countdown(&notice("shell.notice.power_off_soon")));
        assert!(!is_power_countdown(&notice(
            "shell.notice.power_off_skipped"
        )));
        assert!(!is_power_countdown(
            &irori_core::Notice::new(NoticeLevel::Info, "x").keys(
                "shell.notice.installed",
                Some("shell.notice.sleep_soon"),
                &[]
            )
        ));
    }

    #[test]
    fn only_in_place_wine_updates_with_rollback_offer_the_shortcut() {
        let with_rollback = irori_core::Notice::new(NoticeLevel::Success, "GE-Proton updated")
            .keys(
                "shell.notice.component_updated",
                None,
                &[
                    ("name", "GE-Proton"),
                    ("version", "2026-11-24"),
                    (irori_services::COMPONENT_ROLLBACK_ARG, "GE-Proton-latest"),
                ],
            );
        assert!(has_wine_rollback_shortcut(&with_rollback));

        let side_by_side = irori_core::Notice::new(NoticeLevel::Success, "GE-Proton updated").keys(
            "shell.notice.component_updated",
            None,
            &[("name", "GE-Proton"), ("version", "GE-Proton10-25")],
        );
        assert!(!has_wine_rollback_shortcut(&side_by_side));

        let other_notice = irori_core::Notice::new(NoticeLevel::Success, "Other").keys(
            "shell.notice.updated",
            None,
            &[(irori_services::COMPONENT_ROLLBACK_ARG, "GE-Proton-latest")],
        );
        assert!(!has_wine_rollback_shortcut(&other_notice));
    }

    #[test]
    fn big_picture_activates_on_exit() {
        let game = GameId::new(Runner::Gog, "123");
        let other = GameId::new(Runner::Legendary, "other");

        assert!(should_activate_on_exit(Some(&game), &game, &Route::Library));
        assert!(!should_activate_on_exit(
            Some(&game),
            &other,
            &Route::Library
        ));
        assert!(!should_activate_on_exit(None, &game, &Route::Library));
        assert!(should_activate_on_exit(None, &game, &Route::BigPicture));
        assert!(should_activate_on_exit(
            Some(&other),
            &game,
            &Route::BigPicture
        ));
    }
}

/// The title bar row laid out in a real window, so a change to the flex tracks
/// is measured where it ships.
#[cfg(test)]
pub(crate) mod title_bar_tests {
    use std::sync::{Arc, Mutex, Once};
    use std::time::{Duration, Instant};

    use super::*;
    use gpui_kit::{Bounds, Pixels, TestAppContext, VisualTestContext, px, size};
    use irori_core::{Paths, StatusKind};
    use irori_services::Backend;
    use irori_sideload::{SideloadApp, SideloadPlatform, SideloadStore};

    /// Long enough for the playing pill to reach its own truncation limit.
    const GAME_TITLE: &str = "A Deliberately Long Game Title, Part Two: The Return";

    /// The title bar's right padding, `pr_2` in rems.
    const RIGHT_PADDING: f32 = 8.;

    struct TitleBarHarness {
        downloads: Entity<irori_manage::DownloadsPill>,
    }

    impl Render for TitleBarHarness {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            title_bar_content(cx, |_, _, _| {}, self.downloads.clone().into_any_element())
        }
    }

    struct BarLayout {
        row: Bounds<Pixels>,
        search: Bounds<Pixels>,
        status: Bounds<Pixels>,
        pill: Option<Bounds<Pixels>>,
        downloads: Bounds<Pixels>,
    }

    /// Keep every file the backend writes in a private temporary directory.
    fn isolate_paths() {
        static ONCE: Once = Once::new();
        ONCE.call_once(|| {
            let root = tempfile::tempdir()
                .expect("a private test directory")
                .keep();
            // Another test in this binary must not have touched the real paths
            // first: this backend would then write to the user's own files.
            assert!(
                Paths::init(Paths::under(&root)),
                "the title bar test claims the paths first"
            );
        });
    }

    /// A real backend over a shelf with one manually added (browser) app.
    pub(crate) fn start(cx: &mut TestAppContext) -> (Backend, Arc<SideloadStore>) {
        isolate_paths();
        cx.executor().allow_parking();
        let made: Arc<Mutex<Option<Arc<SideloadStore>>>> = Arc::default();
        let slot = made.clone();
        let backend = Backend::start_for_tests(move |bus, settings| {
            let store = SideloadStore::new(bus, settings);
            *slot.lock().expect("the store slot") = Some(store.clone());
            vec![store]
        })
        .expect("the backend starts");
        let store = made
            .lock()
            .expect("the store slot")
            .clone()
            .expect("the store is built");
        cx.update(|cx| {
            gpui_kit::init(cx);
            irori_ui::init(backend.clone(), cx);
            // main.rs does this; without it the shell's keys render as keys.
            irori_ui::i18n::register(crate::strings::CATALOGS);
        });
        (backend, store)
    }

    /// Add the game whose title the pill shows, and wait for the library model.
    pub(crate) fn add_game(
        cx: &mut TestAppContext,
        backend: &Backend,
        store: &SideloadStore,
    ) -> GameId {
        let id = backend
            .runtime()
            .block_on(store.add_app(SideloadApp {
                title: GAME_TITLE.into(),
                platform: SideloadPlatform::Browser,
                browser_url: Some("https://example.test/a-long-game".into()),
                ..Default::default()
            }))
            .expect("the app is added");
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            cx.run_until_parked();
            if cx.read(|cx| models(cx).library.read(cx).snapshot.get(&id).is_some()) {
                return id;
            }
            assert!(
                Instant::now() < deadline,
                "the library never listed the added app"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// Wait until the status model shows (or stops showing) `id` as playing.
    pub(crate) fn wait_for_status(cx: &mut TestAppContext, id: &GameId, playing: bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            cx.run_until_parked();
            if cx.read(|cx| models(cx).statuses.read(cx).is_playing(id)) == playing {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "the status model never caught up"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// Draw the production title bar row in a window `width` wide and report
    /// what the layout engine placed.
    fn measure(cx: &mut TestAppContext, width: f32) -> BarLayout {
        let window = cx.open_window(size(px(width), px(34.)), |_, cx| TitleBarHarness {
            downloads: cx.new(irori_manage::DownloadsPill::new),
        });
        let mut cx = VisualTestContext::from_window(window.into(), cx);
        cx.update(|window, cx| {
            window.refresh();
            let _ = window.draw(cx);
        });
        cx.run_until_parked();
        let bounds = |cx: &mut VisualTestContext, selector: &'static str| -> Bounds<Pixels> {
            cx.debug_bounds(selector)
                .unwrap_or_else(|| panic!("{selector} is drawn"))
        };
        let row = bounds(&mut cx, "title-bar-content");
        let search = bounds(&mut cx, "open-palette");
        let status = bounds(&mut cx, "title-bar-status");
        let downloads = bounds(&mut cx, "title-downloads");
        let pill = cx.debug_bounds("playing-pill");
        BarLayout {
            row,
            search,
            status,
            pill,
            downloads,
        }
    }

    fn assert_same_place(before: Bounds<Pixels>, after: Bounds<Pixels>) {
        assert!(
            (before.left().as_f32() - after.left().as_f32()).abs() < 0.01
                && (before.size.width.as_f32() - after.size.width.as_f32()).abs() < 0.01,
            "the search button moved: {before:?} → {after:?}"
        );
    }

    #[gpui_kit::test]
    fn the_search_button_keeps_its_place_when_the_playing_pill_appears(cx: &mut TestAppContext) {
        let (backend, store) = start(cx);
        let id = add_game(cx, &backend, &store);

        // The reported width: adding the pill used to drag the search button
        // half the pill's width to the left.
        let wide_before = measure(cx, 2000.);
        assert!(wide_before.pill.is_none(), "nothing plays yet");
        assert!(
            (wide_before.search.center().x.as_f32()
                - (wide_before.row.left().as_f32()
                    + (wide_before.row.size.width.as_f32() - RIGHT_PADDING) / 2.))
                .abs()
                < 0.5,
            "the search button is centred in the bar's content box: {:?} in {:?}",
            wide_before.search,
            wide_before.row
        );

        backend.statuses.set(&id, StatusKind::Playing);
        wait_for_status(cx, &id, true);
        let wide_after = measure(cx, 2000.);
        assert!(wide_after.pill.is_some(), "the pill names the game");
        assert_same_place(wide_before.search, wide_after.search);

        // The window's smallest size: the tracks share the bar, so the search
        // button shortens instead of the groups overlapping each other.
        backend.statuses.clear(&id);
        wait_for_status(cx, &id, false);
        let narrow_before = measure(cx, 800.);
        backend.statuses.set(&id, StatusKind::Playing);
        wait_for_status(cx, &id, true);
        let narrow_after = measure(cx, 800.);

        assert_same_place(narrow_before.search, narrow_after.search);
        let pill = narrow_after
            .pill
            .expect("the pill is drawn in a narrow bar");
        assert!(
            pill.left() >= narrow_after.search.right(),
            "the pill stays clear of the search button: {pill:?} vs {:?}",
            narrow_after.search
        );
        let flush = |edge: Pixels| {
            (narrow_after.row.right().as_f32() - edge.as_f32() - RIGHT_PADDING).abs() < 0.5
        };
        assert!(
            flush(narrow_after.status.right()),
            "the status group ends at the bar's content edge: {:?} in {:?}",
            narrow_after.status,
            narrow_after.row
        );
        assert!(
            flush(narrow_after.downloads.right()),
            "the downloads button stays inside the bar: {:?} in {:?}",
            narrow_after.downloads,
            narrow_after.row
        );
        assert!(
            narrow_after.search.size.width > px(120.),
            "the search button stays usable: {:?}",
            narrow_after.search
        );
    }

    struct MouseNavHarness {
        enabled: bool,
        occluded: bool,
    }

    impl Render for MouseNavHarness {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            bind_mouse_navigation(
                div().id("mouse-nav-root").relative().size_full(),
                self.enabled,
            )
            .when(self.occluded, |this| {
                this.child(
                    div()
                        .id("mouse-nav-occluder")
                        .absolute()
                        .inset_0()
                        .occlude(),
                )
            })
        }
    }

    #[gpui_kit::test]
    fn mouse_side_buttons_navigate_history_unless_occluded_or_disabled(cx: &mut TestAppContext) {
        let _ = start(cx);
        let window = cx.open_window(size(px(400.), px(300.)), |_, _| MouseNavHarness {
            enabled: true,
            occluded: false,
        });
        let mut vcx = VisualTestContext::from_window(window.into(), cx);
        vcx.update(|window, cx| {
            nav::navigate(Route::Downloads, cx);
            window.refresh();
            let _ = window.draw(cx);
        });
        vcx.run_until_parked();

        let pos = gpui_kit::point(px(100.), px(100.));
        let mods = gpui_kit::Modifiers::default();

        vcx.simulate_mouse_down(pos, MouseButton::Navigate(NavigationDirection::Back), mods);
        vcx.run_until_parked();
        assert_eq!(
            vcx.read(|cx| navigator(cx).read(cx).current().clone()),
            Route::Library
        );

        vcx.simulate_mouse_down(
            pos,
            MouseButton::Navigate(NavigationDirection::Forward),
            mods,
        );
        vcx.run_until_parked();
        assert_eq!(
            vcx.read(|cx| navigator(cx).read(cx).current().clone()),
            Route::Downloads
        );

        // An occluded overlay (dialog, sheet, palette) must absorb pointer clicks.
        window
            .update(&mut vcx, |view, _, cx| {
                view.occluded = true;
                cx.notify();
            })
            .expect("update mouse nav harness");
        vcx.update(|window, cx| {
            window.refresh();
            let _ = window.draw(cx);
        });
        vcx.run_until_parked();
        vcx.simulate_mouse_down(pos, MouseButton::Navigate(NavigationDirection::Back), mods);
        vcx.run_until_parked();
        assert_eq!(
            vcx.read(|cx| navigator(cx).read(cx).current().clone()),
            Route::Downloads
        );

        // Disabled (Big Picture mode) ignores mouse side buttons.
        window
            .update(&mut vcx, |view, _, cx| {
                view.occluded = false;
                view.enabled = false;
                cx.notify();
            })
            .expect("disable mouse nav harness");
        vcx.update(|window, cx| {
            window.refresh();
            let _ = window.draw(cx);
        });
        vcx.run_until_parked();
        vcx.simulate_mouse_down(pos, MouseButton::Navigate(NavigationDirection::Back), mods);
        vcx.run_until_parked();
        assert_eq!(
            vcx.read(|cx| navigator(cx).read(cx).current().clone()),
            Route::Downloads
        );
    }
}
