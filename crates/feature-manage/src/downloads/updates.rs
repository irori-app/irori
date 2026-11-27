//! Updates inside the unified downloads page: Irori self-update banner,
//! game updates (which enqueue into the download queue), and in-place or
//! side-by-side component updates (Wine/Proton, translation layers, runtimes,
//! emulators and RetroArch cores).

use std::sync::Arc;
use std::time::Duration;

use gpui_kit::assets::IconName;
use gpui_kit::component::alert::Alert;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::progress::Progress;
use gpui_kit::component::tag::Tag;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, Sizable as _, StyledExt as _, WindowExt as _, h_flex,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, ClipboardItem, Context, ElementId, InteractiveElement as _, IntoElement,
    ParentElement as _, SharedString, StatefulInteractiveElement as _, Styled as _, Window, div,
    rems,
};
use irori_core::{GameId, GameInfo, Runner};
use irori_services::{
    AutoUpdateHold, ComponentKind, ComponentReport, ComponentUpdate, SelfInstall,
};
use irori_ui::app_update::{can_install, install_update, restart_into_update};
use irori_ui::components::{BrandMark, GameCover, SectionTitle, StoreBadge};
use irori_ui::i18n::{t, t_args};
use irori_ui::state::run_infallible;
use irori_ui::{backend, models};

use super::{DownloadsView, list_card, queue};
use crate::common::{card, notify_error};

/// Game update rows rendered per "Show more" step, as a library can hold hundreds.
pub(super) const GAMES_PAGE: usize = 50;

/// Short label for a component's kind.
pub(super) fn kind_label(kind: ComponentKind) -> SharedString {
    match kind {
        ComponentKind::Wine => t("manage.updates.kind.wine"),
        ComponentKind::Translation => t("manage.updates.kind.translation"),
        ComponentKind::Runtime => t("manage.updates.kind.runtime"),
        ComponentKind::Emulator => t("manage.updates.kind.emulator"),
        ComponentKind::Cores => t("manage.updates.kind.cores"),
    }
}

/// Icon representing a component's kind.
fn kind_icon(kind: ComponentKind) -> IconName {
    match kind {
        ComponentKind::Wine => IconName::Wine,
        ComponentKind::Translation => IconName::Layers,
        ComponentKind::Runtime => IconName::Cpu,
        ComponentKind::Emulator => IconName::Joystick,
        ComponentKind::Cores => IconName::Package,
    }
}

/// What automatic updates are waiting for, in a short notice.
pub(super) fn hold_text(hold: &AutoUpdateHold) -> SharedString {
    match hold {
        AutoUpdateHold::Window(from, until) => t_args(
            "manage.updates.waiting_window",
            &[
                ("from", &format!("{from:02}:00")),
                ("until", &format!("{until:02}:00")),
            ],
        ),
        AutoUpdateHold::Metered => t("manage.updates.waiting_metered"),
    }
}

/// Games with an available update, resolved to their library info and sorted by title.
pub(super) fn sorted_games(
    ids: impl Iterator<Item = GameId>,
    cx: &App,
) -> Vec<(GameId, Arc<GameInfo>)> {
    let b = backend(cx);
    let snapshot = models(cx).library.read(cx).snapshot.clone();
    let mut games: Vec<(GameId, Arc<GameInfo>)> = ids
        .map(|id| {
            let game = snapshot
                .get(&id)
                .cloned()
                .or_else(|| b.game(&id))
                .unwrap_or_else(|| queue::placeholder_game(&id, &id.app_name));
            (id, game)
        })
        .collect();
    games.sort_by_key(|(_, g)| g.title.to_lowercase());
    games
}

impl DownloadsView {
    /// Ask the stores, component sources and GitHub now.
    pub(super) fn check_now(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.checking {
            return;
        }
        self.checking = true;
        cx.notify();
        let b = backend(cx);
        let task = irori_ui::run(cx, async move {
            b.refresh_updates().await;
            b.check_components().await;
            if b.app_update().kind.can_check_manually() {
                b.check_app_update().await?;
            }
            Ok(())
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.checking = false;
                if let Err(e) = result {
                    notify_error(t("manage.updates.check_failed"), &e, window, cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn update_game(&self, id: GameId, title: String, cx: &mut Context<Self>) {
        backend(cx).enqueue_update(id, title, Default::default());
    }

    fn ignore_game(&self, id: &GameId, title: &str, window: &mut Window, cx: &mut Context<Self>) {
        backend(cx).ignore_game_updates(id);
        window.push_notification(
            Notification::info(t_args("manage.updates.ignored", &[("title", title)])),
            cx,
        );
    }

    fn update_component(&self, id: String, cx: &mut Context<Self>) {
        let b = backend(cx);
        // Outcomes reach the user as a notice (with rollback shortcut for
        // in-place Wine updates) and inline on the component's row.
        run_infallible(cx, async move {
            let _ = b.update_component(&id).await;
        })
        .detach();
    }

    fn skip_component(&self, id: String, skip: bool, cx: &mut Context<Self>) {
        let b = backend(cx);
        run_infallible(cx, async move { b.skip_component_update(&id, skip).await }).detach();
    }

    fn copy_irori_command(&mut self, command: &str, window: &mut Window, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(command.to_string()));
        self.irori_copied = true;
        cx.notify();
        self.irori_copy_reset = Some(cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(Duration::from_secs(2)).await;
            let _ = this.update(cx, |this, cx| {
                this.irori_copied = false;
                this.irori_copy_reset = None;
                cx.notify();
            });
        }));
    }

    #[cfg(test)]
    pub(crate) fn trigger_restart_irori_for_test(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        restart_into_update(window, cx).detach();
    }

    /// Alerts from auto-update scheduling or failed store/component checks.
    pub(super) fn render_update_alerts(
        &self,
        report: &ComponentReport,
        failed_stores: &[Runner],
    ) -> Vec<AnyElement> {
        let mut alerts = Vec::new();
        if let Some(hold) = report.auto_hold.as_ref() {
            alerts.push(Alert::info("auto-hold", hold_text(hold)).into_any_element());
        }
        for (ix, runner) in failed_stores.iter().enumerate() {
            alerts.push(
                Alert::warning(
                    ElementId::Name(format!("update-check-{ix}").into()),
                    t_args(
                        "manage.updates.store_check_failed",
                        &[("store", runner.store_name())],
                    ),
                )
                .into_any_element(),
            );
        }
        if let Some(reset) = report.rate_limited_until {
            let minutes = irori_core::github_releases::RateLimited { reset }
                .minutes_left(irori_core::util::unix_now());
            alerts.push(
                Alert::warning(
                    "component-rate-limit",
                    t_args(
                        "manage.wine.rate_limited",
                        &[("minutes", &minutes.to_string())],
                    ),
                )
                .title(t("manage.updates.component_check_failed"))
                .into_any_element(),
            );
        }
        if let Some(err) = report.check_errors.first() {
            alerts.push(
                Alert::warning("component-check", err.clone())
                    .title(t("manage.updates.component_check_failed"))
                    .into_any_element(),
            );
        }
        alerts
    }

    /// Dedicated top banner for an Irori self-update (kept apart from
    /// "Update all" because applying it restarts the app).
    pub(super) fn render_irori(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let model = models(cx).app_update.read(cx);
        let status = model.status.clone();
        let restarting = model.restarting;
        let show = status.has_update()
            || matches!(
                status.install,
                SelfInstall::Downloading { .. }
                    | SelfInstall::Failed { .. }
                    | SelfInstall::RolledBack { .. }
            );
        if !show {
            return None;
        }

        let version = match &status.install {
            SelfInstall::Ready { version } => Some(version.clone()),
            _ => status
                .available
                .as_ref()
                .filter(|_| status.kind.can_check_manually())
                .map(|r| r.version.clone())
                .or_else(|| match &status.install {
                    SelfInstall::RolledBack { version } => Some(version.clone()),
                    _ => None,
                }),
        }?;
        let url = status
            .available
            .as_ref()
            .filter(|r| r.version == version)
            .map(|r| r.url.clone())
            .unwrap_or_else(|| irori_services::app_update::release_page(&version));
        let theme = cx.theme();
        let command = status.kind.update_command();
        let installable = can_install(&status);
        let ready = matches!(status.install, SelfInstall::Ready { .. });
        let downloading_percent = match status.install {
            SelfInstall::Downloading { percent } => Some(percent),
            _ => None,
        };
        let error = match &status.install {
            SelfInstall::Failed { error } => Some(error.clone()),
            _ => None,
        };
        let rollback_warning = match &status.install {
            SelfInstall::RolledBack { version } => Some(t_args(
                "manage.updates.irori_rolled_back",
                &[("version", version), ("current", &status.current)],
            )),
            _ => None,
        };

        let title = match &status.install {
            SelfInstall::Ready { version } => {
                t_args("manage.updates.irori_ready_title", &[("version", version)])
            }
            SelfInstall::RolledBack { version } if status.available.is_none() => t_args(
                "manage.updates.irori_rolled_back_title",
                &[("version", version)],
            ),
            _ => t_args("manage.updates.irori_available", &[("version", &version)]),
        };
        let detail: Option<SharedString> = match &status.install {
            SelfInstall::Downloading { percent: Some(p) } => Some(t_args(
                "manage.updates.irori_downloading_percent",
                &[("percent", &p.to_string())],
            )),
            SelfInstall::Downloading { percent: None } => {
                Some(t("manage.updates.irori_downloading"))
            }
            SelfInstall::Ready { .. } => Some(t("manage.updates.irori_ready_detail")),
            _ => Some(match command {
                Some(command) => command.into(),
                None if installable => t_args(
                    "manage.updates.irori_running",
                    &[("current", &status.current)],
                ),
                None => format!("v{}", status.current).into(),
            }),
        };

        let has_primary = command.is_some() || ready || installable;
        let mut actions: Vec<AnyElement> = vec![
            Button::new("irori-release-notes")
                .debug_selector(|| "irori-release-notes".into())
                .map(|b| if has_primary { b.ghost() } else { b.primary() })
                .small()
                .icon(IconName::ExternalLink)
                .label(t("manage.updates.release_notes"))
                .on_click(move |_, _, cx| cx.open_url(&url))
                .into_any_element(),
        ];
        if let Some(command) = command {
            actions.push(
                Button::new("irori-copy-command")
                    .debug_selector(|| "irori-copy-command".into())
                    .small()
                    .primary()
                    .icon(if self.irori_copied {
                        IconName::Check
                    } else {
                        IconName::Copy
                    })
                    .label(if self.irori_copied {
                        t("manage.updates.irori_copied")
                    } else {
                        t("manage.updates.irori_copy_command")
                    })
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.copy_irori_command(command, window, cx)
                    }))
                    .into_any_element(),
            );
        } else if ready {
            actions.push(
                Button::new("irori-restart-update")
                    .debug_selector(|| "irori-restart-update".into())
                    .small()
                    .primary()
                    .icon(IconName::RotateCw)
                    .label(t("manage.updates.irori_restart"))
                    .loading(restarting)
                    .disabled(restarting)
                    .on_click(|_, window, cx| restart_into_update(window, cx).detach())
                    .into_any_element(),
            );
        } else if installable {
            let downloading = downloading_percent.is_some();
            actions.push(
                Button::new("irori-install-update")
                    .debug_selector(|| "irori-install-update".into())
                    .small()
                    .primary()
                    .icon(IconName::Download)
                    .label(t("manage.updates.irori_install"))
                    .loading(downloading)
                    .disabled(downloading)
                    .on_click(|_, _, cx| install_update(cx))
                    .into_any_element(),
            );
        }

        let accent = if error.is_some() {
            theme.danger
        } else if rollback_warning.is_some() {
            theme.warning
        } else {
            theme.primary
        };

        Some(
            v_flex()
                .id("irori-self-update-banner")
                .debug_selector(|| "irori-self-update-banner".into())
                .w_full()
                .gap_2()
                .px_4()
                .py_3()
                .rounded(theme.radius_lg)
                .border_1()
                .border_color(accent.opacity(0.4))
                .bg(accent.opacity(0.08))
                .child(
                    h_flex()
                        .w_full()
                        .gap_3()
                        .items_center()
                        .child(div().flex_shrink_0().child(BrandMark::new(rems(1.5))))
                        .child(
                            v_flex()
                                .flex_1()
                                .min_w_0()
                                .gap_0p5()
                                .child(div().font_semibold().truncate().child(title))
                                .when_some(detail, |this, detail| {
                                    this.child(
                                        div()
                                            .text_xs()
                                            .truncate()
                                            .text_color(theme.muted_foreground)
                                            .child(detail),
                                    )
                                }),
                        )
                        .child(h_flex().flex_shrink_0().gap_2().children(actions)),
                )
                .when_some(error, |this, error| {
                    this.child(
                        div()
                            .debug_selector(|| "irori-update-error".into())
                            .w_full()
                            .text_xs()
                            .text_color(theme.danger)
                            .child(error),
                    )
                })
                .when_some(rollback_warning, |this, warning| {
                    this.child(
                        div()
                            .debug_selector(|| "irori-rollback-warning".into())
                            .w_full()
                            .text_xs()
                            .text_color(theme.warning)
                            .child(warning),
                    )
                })
                .when_some(downloading_percent, |this, percent| {
                    this.child(
                        div()
                            .debug_selector(|| "irori-update-progress".into())
                            .child(irori_ui::motion::gliding_progress(
                                "irori-update-progress",
                                percent.map(f32::from),
                            )),
                    )
                })
                .into_any_element(),
        )
    }

    fn render_game_update(
        &self,
        ix: usize,
        id: GameId,
        game: Arc<GameInfo>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let queued = models(cx)
            .downloads
            .read(cx)
            .queue
            .active
            .iter()
            .any(|i| i.id == id);
        let theme = cx.theme();
        let border = theme.border;
        let key = id.key();
        let open_key = format!("game-update-open-{key}");
        let open_id = id.clone();
        let title = game.title.clone();
        let actions: Vec<AnyElement> = if queued {
            vec![
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(t("manage.updates.in_queue"))
                    .into_any_element(),
            ]
        } else {
            let (update_id, update_title) = (id.clone(), title.clone());
            let (ignore_id, ignore_title) = (id.clone(), title.clone());
            vec![
                Button::new(ElementId::Name(format!("ignore-{key}").into()))
                    .ghost()
                    .small()
                    .label(t("manage.updates.ignore"))
                    .tooltip(t("manage.updates.ignore_tooltip"))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.ignore_game(&ignore_id, &ignore_title, window, cx)
                    }))
                    .into_any_element(),
                Button::new(ElementId::Name(format!("queue-update-{key}").into()))
                    .small()
                    .outline()
                    .label(t("manage.updates.update"))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.update_game(update_id.clone(), update_title.clone(), cx)
                    }))
                    .into_any_element(),
            ]
        };
        let detail = queue::store_label(id.runner);
        h_flex()
            .id(ElementId::Name(format!("game-update-{key}").into()))
            .debug_selector(move || format!("update-{key}"))
            .w_full()
            .gap_3()
            .px_4()
            .py_3()
            .items_center()
            .when(ix > 0, |d| d.border_t_1().border_color(border))
            .hover(|s| s.bg(theme.muted.opacity(0.35)))
            .child(
                h_flex()
                    .id(ElementId::Name(open_key.into()))
                    .flex_1()
                    .min_w_0()
                    .items_center()
                    .gap_3()
                    .cursor_pointer()
                    .on_click(move |_, _, cx| {
                        irori_ui::navigate(irori_ui::Route::Game(open_id.clone()), cx)
                    })
                    .child(GameCover::new(game).width(rems(3.5)))
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
                                        div().text_base().font_semibold().truncate().child(title),
                                    )
                                    .child(StoreBadge::new(id.runner).medium()),
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .truncate()
                                    .text_color(theme.muted_foreground)
                                    .child(detail),
                            ),
                    ),
            )
            .child(h_flex().flex_shrink_0().gap_2().children(actions))
            .into_any_element()
    }

    pub(super) fn render_game_updates(
        &self,
        games: Vec<(GameId, Arc<GameInfo>)>,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if games.is_empty() {
            return None;
        }
        let total = games.len();
        let shown = total.min(self.games_shown);
        let more = total > shown;
        let rows: Vec<AnyElement> = games
            .into_iter()
            .take(shown)
            .enumerate()
            .map(|(ix, (id, game))| self.render_game_update(ix, id, game, cx))
            .collect();
        let heading = format!("{} · {total}", t("manage.updates.section.games"));
        Some(
            v_flex()
                .gap_2()
                .child(SectionTitle::new(heading))
                .child(list_card(cx).children(rows))
                .when(more, |this| {
                    this.child(
                        h_flex().justify_center().child(
                            Button::new("updates-more")
                                .ghost()
                                .small()
                                .label(t("manage.updates.show_more"))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.games_shown += GAMES_PAGE;
                                    cx.notify();
                                })),
                        ),
                    )
                })
                .into_any_element(),
        )
    }

    fn render_component(
        &self,
        update: &ComponentUpdate,
        running: bool,
        percent: Option<f32>,
        error: Option<String>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme();
        let id = update.id.clone();
        let element = |name: &str| ElementId::Name(format!("{name}-{id}").into());

        let icon_box = div()
            .p_2()
            .rounded(theme.radius)
            .bg(theme.muted)
            .flex_shrink_0()
            .child(
                Icon::new(kind_icon(update.kind))
                    .small()
                    .text_color(theme.muted_foreground),
            );

        if update.external {
            let detail = format!(
                "{} · {}",
                t("manage.updates.updated_by_flatpak"),
                update.latest
            );
            return h_flex()
                .w_full()
                .gap_3()
                .items_center()
                .child(icon_box)
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .gap_0p5()
                        .child(
                            h_flex()
                                .gap_2()
                                .items_center()
                                .min_w_0()
                                .child(div().font_medium().truncate().child(update.name.clone()))
                                .child(Tag::secondary().xsmall().child(kind_label(update.kind))),
                        )
                        .child(
                            div()
                                .text_xs()
                                .truncate()
                                .text_color(theme.muted_foreground)
                                .child(detail),
                        ),
                )
                .into_any_element();
        }

        let mut actions: Vec<AnyElement> = Vec::new();
        if let Some(url) = update.notes_url.clone() {
            actions.push(
                Button::new(element("notes"))
                    .ghost()
                    .small()
                    .icon(IconName::ExternalLink)
                    .tooltip(t("manage.updates.release_notes"))
                    .accessibility_label(t("manage.updates.release_notes"))
                    .on_click(move |_, _, cx| {
                        if let Some(url) = irori_ui::web_url(&url) {
                            cx.open_url(&url);
                        }
                    })
                    .into_any_element(),
            );
        }
        if running {
            let target = id.clone();
            actions.push(
                Button::new(element("cancel"))
                    .ghost()
                    .small()
                    .label(t("manage.updates.cancel"))
                    .on_click(move |_, _, cx| backend(cx).cancel_component_update(&target))
                    .into_any_element(),
            );
            actions.push(
                Button::new(element("updating"))
                    .small()
                    .loading(true)
                    .label(match percent {
                        Some(p) => t_args(
                            "manage.updates.updating_percent",
                            &[("percent", &format!("{p:.0}"))],
                        ),
                        None => t("manage.updates.updating"),
                    })
                    .into_any_element(),
            );
        } else if update.skipped {
            let target = id.clone();
            actions.push(
                Button::new(element("unskip"))
                    .ghost()
                    .small()
                    .label(t("manage.updates.unskip"))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.skip_component(target.clone(), false, cx)
                    }))
                    .into_any_element(),
            );
        } else {
            let (skip, install) = (id.clone(), id.clone());
            actions.push(
                Button::new(element("skip"))
                    .ghost()
                    .small()
                    .label(t("manage.updates.skip"))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.skip_component(skip.clone(), true, cx)
                    }))
                    .into_any_element(),
            );
            actions.push(
                Button::new(element("update"))
                    .small()
                    .outline()
                    .label(t("manage.updates.update"))
                    .on_click(
                        cx.listener(move |this, _, _, cx| {
                            this.update_component(install.clone(), cx)
                        }),
                    )
                    .into_any_element(),
            );
        }

        let mut detail = format!("{} → {}", update.installed, update.latest);
        if update.in_place {
            detail = format!("{detail} · {}", t("manage.updates.replaces_build"));
        }
        if update.skipped {
            detail = format!("{detail} · {}", t("manage.updates.skipped"));
        }

        let selector_id = id.clone();
        v_flex()
            .debug_selector(move || format!("component-update-{selector_id}"))
            .gap_2()
            .child(
                h_flex()
                    .w_full()
                    .gap_3()
                    .items_center()
                    .child(icon_box)
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap_0p5()
                            .child(
                                h_flex()
                                    .gap_2()
                                    .items_center()
                                    .min_w_0()
                                    .child(
                                        div().font_medium().truncate().child(update.name.clone()),
                                    )
                                    .child(
                                        Tag::secondary().xsmall().child(kind_label(update.kind)),
                                    ),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .truncate()
                                    .text_color(theme.muted_foreground)
                                    .child(detail),
                            )
                            .when_some(error, |this, e| {
                                this.child(div().text_xs().text_color(theme.danger).child(e))
                            }),
                    )
                    .child(h_flex().flex_shrink_0().gap_2().children(actions)),
            )
            .when_some(percent.filter(|_| running), |this, p| {
                this.child(Progress::new(element("progress")).value(p))
            })
            .into_any_element()
    }

    pub(super) fn render_component_updates(
        &self,
        report: &ComponentReport,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if report.updates.is_empty() {
            return None;
        }
        let muted = cx.theme().muted_foreground;
        let show_side_by_side_note = report
            .updates
            .iter()
            .any(|u| u.kind == ComponentKind::Wine && !u.in_place);
        let rows: Vec<AnyElement> = report
            .updates
            .iter()
            .map(|u| {
                let running = report.running.contains(&u.id);
                let percent = report.progress.get(&u.id).copied();
                let error = report.failed.get(&u.id).cloned();
                self.render_component(u, running, percent, error, cx)
            })
            .collect();
        let heading = format!(
            "{} · {}",
            t("manage.updates.section.components"),
            report.updates.len()
        );
        Some(
            v_flex()
                .gap_2()
                .child(SectionTitle::new(heading))
                .when(show_side_by_side_note, |this| {
                    this.child(
                        div()
                            .text_xs()
                            .text_color(muted)
                            .child(t("manage.updates.components_note")),
                    )
                })
                .child(card(cx).children(rows))
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_labels_use_concise_copy() {
        crate::i18n::register();
        assert_eq!(kind_label(ComponentKind::Wine), "Wine / Proton");
        assert_eq!(kind_label(ComponentKind::Translation), "Translation");
        assert_eq!(kind_label(ComponentKind::Cores), "Cores");
        assert_eq!(kind_label(ComponentKind::Emulator), "Emulator");
        assert_eq!(kind_label(ComponentKind::Runtime), "Runtime");
    }

    #[test]
    fn hold_texts_format_window_and_metered() {
        crate::i18n::register();
        assert_eq!(
            hold_text(&AutoUpdateHold::Metered),
            "Auto-updates paused on metered connection"
        );
        assert_eq!(
            hold_text(&AutoUpdateHold::Window(2, 6)),
            "Auto-updates scheduled for 02:00–06:00"
        );
    }
}
