//! The Wine manager ([`Route::WineManager`](irori_ui::Route::WineManager)):
//! downloadable Proton/Wine builds per source, every build found on this
//! system with the user's custom Wine paths, and the Wine prefixes with the
//! games using them.

mod installed;
mod prefixes;
mod sources;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

use gpui_kit::assets::IconName;
use gpui_kit::component::alert::Alert;
use gpui_kit::component::button::{Button, ButtonVariant, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::link::Link;
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::progress::Progress;
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::tag::Tag;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, Sizable as _, StyledExt as _, WindowExt as _, h_flex,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, ElementId, Entity, InteractiveElement as _,
    IntoElement, ParentElement as _, PathPromptOptions, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, Subscription, Task, Window, div, px, rems,
};
use irori_core::{Event, Paths, WineInstallation};
use irori_runtime::manager::{self, Release, ReleaseKind};
use irori_runtime::release_checks;
use irori_ui::components::{EmptyState, PageHeader, SectionTitle, StoreLogo};
use irori_ui::i18n::{t, t_args};
use irori_ui::{backend, models};

use crate::common::{backend_op, card, notify_error};
use crate::format;
use installed::{WineOrigin, group, kind_label};
use irori_ui::wine_installs::{
    OpLive, WineOps, cancel_install, release_key, start_install, wine_ops,
};

/// Repaint interval while installs report progress.
const PROGRESS_TICK: Duration = Duration::from_millis(300);

fn source_label(kind: ReleaseKind) -> &'static str {
    kind.type_str()
}

/// Why a check didn't reach GitHub, and in how long it can.
fn rate_limited_text(limited: release_checks::RateLimited, now: u64) -> SharedString {
    t_args(
        "manage.wine.rate_limited",
        &[("minutes", &limited.minutes_left(now).to_string())],
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WineTab {
    Download,
    System,
    Prefixes,
}

#[derive(Clone)]
enum ListState {
    Loading,
    Loaded(Vec<Release>),
    Failed(SharedString),
}

/// What the "On this system" tab shows.
#[derive(Clone)]
enum SystemState {
    Loading,
    Loaded(Vec<(WineOrigin, Vec<WineInstallation>)>),
}

/// Downloadable builds per source plus the builds found on this system.
pub struct WineManagerView {
    active_tab: WineTab,
    selected_source: Option<ReleaseKind>,
    show_older: HashSet<ReleaseKind>,
    lists: HashMap<ReleaseKind, ListState>,
    refreshing: bool,
    /// GitHub's API budget is used up until this unix time.
    limited_until: Option<u64>,
    search: Entity<InputState>,
    prefix_search: Entity<InputState>,
    prefix_filter: prefixes::PrefixFilter,
    system: Option<SystemState>,
    /// The Prefixes tab, once opened.
    prefixes: Option<prefixes::PrefixState>,
    ops: Entity<WineOps>,
    seen_revision: u64,
    ticking: bool,
    /// Language the search placeholder was set in.
    placeholder_language: String,
    _ticker: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl WineManagerView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        crate::i18n::register();
        let search = cx.new(|cx| InputState::new(window, cx).placeholder(t("manage.wine.search")));
        let prefix_search =
            cx.new(|cx| InputState::new(window, cx).placeholder(t("manage.wine.prefixes.search")));
        let ops = wine_ops(cx);
        let settings = models(cx).settings.clone();
        let subscriptions = vec![
            cx.subscribe(&search, |_, _, event, cx| {
                if matches!(event, InputEvent::Change) {
                    cx.notify();
                }
            }),
            cx.subscribe(&prefix_search, |this, _, event, cx| {
                if matches!(event, InputEvent::Change) {
                    this.refresh_prefix_filter(cx);
                }
            }),
            cx.observe_in(&ops, window, |this, ops, window, cx| {
                let revision = ops.read(cx).revision;
                if revision != this.seen_revision {
                    this.seen_revision = revision;
                    this.reload_after_change(window, cx);
                }
                this.ensure_ticker(window, cx);
                cx.notify();
            }),
            cx.observe_in(&settings, window, |this, _, window, cx| {
                if this.system.is_some() {
                    this.load_system(window, cx);
                }
                if this.prefixes.is_some() {
                    this.load_prefixes(window, cx);
                }
            }),
            // Installs and uninstalls change who uses a prefix.
            cx.observe_in(
                &models(cx).library.clone(),
                window,
                |this, _, window, cx| {
                    if this.prefixes.is_some() {
                        this.load_prefixes(window, cx);
                    }
                },
            ),
            // In-place Wine updates and rollbacks change the saved previous builds.
            cx.observe(&models(cx).components.clone(), |_, _, cx| cx.notify()),
        ];
        let seen_revision = ops.read(cx).revision;
        let selected_source = ReleaseKind::available().first().copied();
        let active_tab = if selected_source.is_some() {
            WineTab::Download
        } else {
            WineTab::System
        };
        let mut this = Self {
            active_tab,
            selected_source,
            show_older: HashSet::new(),
            lists: HashMap::new(),
            refreshing: false,
            limited_until: None,
            search,
            prefix_search,
            prefix_filter: prefixes::PrefixFilter::All,
            system: None,
            prefixes: None,
            ops,
            seen_revision,
            ticking: false,
            placeholder_language: irori_ui::i18n::language(),
            _ticker: None,
            _subscriptions: subscriptions,
        };
        if let Some(kind) = selected_source {
            this.load_list(kind, false, window, cx);
        } else {
            this.load_system(window, cx);
        }
        this.ensure_ticker(window, cx);
        this
    }

    fn active_source(&self) -> Option<ReleaseKind> {
        (self.active_tab == WineTab::Download)
            .then_some(self.selected_source)
            .flatten()
    }

    /// The Prefixes tab, after the downloads and "On this system".
    fn on_prefixes(&self) -> bool {
        self.active_tab == WineTab::Prefixes
    }

    fn select_tab(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let has_downloads = !ReleaseKind::available().is_empty();
        if has_downloads && ix == 0 {
            self.active_tab = WineTab::Download;
            if let Some(kind) = self.selected_source
                && !self.lists.contains_key(&kind)
            {
                self.load_list(kind, false, window, cx);
            }
        } else if ix == usize::from(has_downloads) {
            self.active_tab = WineTab::System;
            if self.system.is_none() {
                self.load_system(window, cx);
            }
        } else {
            self.active_tab = WineTab::Prefixes;
            if self.prefixes.is_none() {
                self.load_prefixes(window, cx);
            }
        }
        cx.notify();
    }

    fn select_source(&mut self, kind: ReleaseKind, window: &mut Window, cx: &mut Context<Self>) {
        self.active_tab = WineTab::Download;
        if self.selected_source != Some(kind) {
            self.selected_source = Some(kind);
            self.search.update(cx, |s, cx| s.set_value("", window, cx));
        }
        if !self.lists.contains_key(&kind) {
            self.load_list(kind, false, window, cx);
        }
        cx.notify();
    }

    fn toggle_older(&mut self, kind: ReleaseKind, cx: &mut Context<Self>) {
        if !self.show_older.remove(&kind) {
            self.show_older.insert(kind);
        }
        cx.notify();
    }

    fn reload_after_change(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let kinds: Vec<ReleaseKind> = self.lists.keys().copied().collect();
        for kind in kinds {
            self.load_list(kind, false, window, cx);
        }
        if self.system.is_some() {
            self.load_system(window, cx);
        }
    }

    fn load_list(
        &mut self,
        kind: ReleaseKind,
        fetch: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.lists.entry(kind).or_insert(ListState::Loading);
        if fetch {
            self.refreshing = true;
        }
        cx.notify();
        let task = irori_ui::run(cx, async move {
            let list = async {
                if fetch {
                    manager::update_releases(true, &[kind]).await?;
                }
                manager::available_releases(kind).await
            }
            .await;
            let now = irori_core::util::unix_now();
            Ok((list, release_checks::exhausted_until(now)))
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok((result, limited_until)) = task.await else {
                return;
            };
            let _ = this.update_in(cx, |this, window, cx| {
                if fetch {
                    this.refreshing = false;
                }
                this.limited_until = limited_until;
                match result {
                    Ok(list) => {
                        this.lists.insert(kind, ListState::Loaded(list));
                    }
                    Err(e) => {
                        let now = irori_core::util::unix_now();
                        let limited = release_checks::rate_limited(&e);
                        let message = match limited {
                            Some(l) => rate_limited_text(l, now),
                            None => {
                                t_args("manage.wine.fetch_failed", &[("error", &format!("{e:#}"))])
                            }
                        };
                        if let Some(l) = limited {
                            this.limited_until = Some(l.reset);
                        }
                        match this.lists.get(&kind) {
                            Some(ListState::Loaded(_)) if limited.is_some() => {
                                window.push_notification(
                                    Notification::warning(message).title(t_args(
                                        "manage.wine.refresh_failed",
                                        &[("source", source_label(kind))],
                                    )),
                                    cx,
                                );
                            }
                            Some(ListState::Loaded(_)) => notify_error(
                                t_args(
                                    "manage.wine.refresh_failed",
                                    &[("source", source_label(kind))],
                                ),
                                &e,
                                window,
                                cx,
                            ),
                            _ => {
                                this.lists.insert(kind, ListState::Failed(message));
                            }
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn load_system(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.system.is_none() {
            self.system = Some(SystemState::Loading);
        }
        let app = backend(cx).settings.app();
        let custom = app.custom_wine_paths.clone();
        let task = irori_ui::run(cx, async move {
            Ok(irori_runtime::wine_installations(&app).await)
        });
        cx.spawn_in(window, async move |this, cx| {
            let list = task.await.unwrap_or_default();
            let own = vec![Paths::get().tools_dir()];
            let grouped = group(&list, &custom, &own);
            let _ = this.update_in(cx, |this, _, cx| {
                this.system = Some(SystemState::Loaded(grouped));
                cx.notify();
            });
        })
        .detach();
    }

    fn refresh(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.active_source() {
            Some(kind) => self.load_list(kind, true, window, cx),
            None if self.on_prefixes() => self.load_prefixes(window, cx),
            None => self.load_system(window, cx),
        }
    }

    /// Repaint while installs run so progress moves; stops when none is left.
    fn ensure_ticker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.ticking || self.ops.read(cx).running.is_empty() {
            return;
        }
        self.ticking = true;
        self._ticker = Some(cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor().timer(PROGRESS_TICK).await;
                let keep = this.update_in(cx, |this, _, cx| {
                    cx.notify();
                    let running = !this.ops.read(cx).running.is_empty();
                    if !running {
                        this.ticking = false;
                    }
                    running
                });
                if !matches!(keep, Ok(true)) {
                    break;
                }
            }
        }));
    }

    fn confirm_remove(&mut self, release: Release, window: &mut Window, cx: &mut Context<Self>) {
        let default_bin = models(cx)
            .settings
            .read(cx)
            .app
            .game
            .wine_version
            .bin
            .clone();
        let is_default = release.install_dir.as_deref().is_some_and(|d| {
            !d.is_empty() && !default_bin.is_empty() && default_bin.starts_with(d)
        });
        let view = cx.entity().downgrade();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let view = view.clone();
            let release = release.clone();
            let description = if is_default {
                t("manage.wine.remove_confirm.default_description")
            } else {
                t("manage.wine.remove_confirm.description")
            };
            alert
                .confirm()
                .title(t_args(
                    "manage.wine.remove_confirm.title",
                    &[("version", &release.version)],
                ))
                .description(description)
                .ok_text(t("manage.wine.remove_confirm.ok"))
                .ok_variant(ButtonVariant::Danger)
                .on_ok(move |_, window, cx| {
                    let release = release.clone();
                    let _ = view.update(cx, |this, cx| this.remove(release, window, cx));
                    true
                })
        });
    }

    fn remove(&mut self, release: Release, window: &mut Window, cx: &mut Context<Self>) {
        let b = backend(cx);
        let version = release.version.clone();
        let task = irori_ui::run(cx, async move {
            manager::remove(&release).await?;
            b.bus.emit(Event::WineVersionsChanged);
            Ok(())
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                if let Err(e) = result {
                    notify_error(
                        t_args("manage.wine.remove_failed", &[("version", &version)]),
                        &e,
                        window,
                        cx,
                    );
                }
                this.reload_after_change(window, cx);
            });
        })
        .detach();
    }

    /// Ask, then restore the build replaced by the last in-place update of `version`.
    fn confirm_go_back(&self, version: String, window: &mut Window, cx: &mut Context<Self>) {
        let view = cx.entity().downgrade();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let version = version.clone();
            let view = view.clone();
            alert
                .title(t_args(
                    "manage.updates.go_back_title",
                    &[("name", &version)],
                ))
                .description(t_args(
                    "manage.updates.go_back_description",
                    &[("name", &version)],
                ))
                .show_cancel(true)
                .cancel_text(t("manage.updates.cancel"))
                .ok_text(t("manage.updates.go_back"))
                .ok_variant(ButtonVariant::Danger)
                .on_ok(move |_, window, cx| {
                    let (b, name) = (backend(cx), version.clone());
                    let task = irori_ui::run(cx, async move { b.roll_back_build(&name).await });
                    let name = version.clone();
                    let view = view.clone();
                    window
                        .spawn(cx, async move |cx| {
                            let result = task.await;
                            let _ = cx.update(|window, cx| match result {
                                Ok(()) => {
                                    window.push_notification(
                                        Notification::success(t_args(
                                            "manage.updates.went_back",
                                            &[("name", &name)],
                                        )),
                                        cx,
                                    );
                                    let _ = view.update(cx, |this, cx| {
                                        this.reload_after_change(window, cx)
                                    });
                                }
                                Err(e) => notify_error(
                                    t_args("manage.updates.go_back_failed", &[("name", &name)]),
                                    &e,
                                    window,
                                    cx,
                                ),
                            });
                        })
                        .detach();
                    true
                })
        });
    }

    /// Saved builds from previous in-place Wine/Proton updates, offered for rollback.
    fn render_previous_builds(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let previous = models(cx).components.read(cx).report.previous.clone();
        if previous.is_empty() {
            return None;
        }
        let muted = cx.theme().muted_foreground;
        let rows: Vec<AnyElement> = previous
            .into_iter()
            .map(|p| {
                let version = p.version;
                let target = version.clone();
                let selector = version.clone();
                h_flex()
                    .debug_selector(move || format!("rollback-{selector}"))
                    .w_full()
                    .gap_3()
                    .items_center()
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap_0p5()
                            .child(div().font_medium().truncate().child(version.clone()))
                            .child(
                                div()
                                    .text_xs()
                                    .truncate()
                                    .text_color(muted)
                                    .child(t("manage.updates.previous_detail")),
                            ),
                    )
                    .child(
                        Button::new(ElementId::Name(format!("go-back-{version}").into()))
                            .small()
                            .outline()
                            .label(t("manage.updates.go_back"))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.confirm_go_back(target.clone(), window, cx)
                            })),
                    )
                    .into_any_element()
            })
            .collect();
        Some(
            v_flex()
                .gap_2()
                .child(SectionTitle::new(t("manage.updates.section.previous")))
                .child(card(cx).children(rows))
                .into_any_element(),
        )
    }

    fn add_custom_path(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some(t("manage.wine.custom.prompt")),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            let path = path.to_string_lossy().into_owned();
            let _ = this.update_in(cx, |_, _, cx| {
                backend_op(cx, move |b| {
                    b.settings.update_app(|s| {
                        if !s.custom_wine_paths.contains(&path) {
                            s.custom_wine_paths.push(path);
                        }
                    });
                    b.bus.emit(Event::WineVersionsChanged);
                });
            });
        })
        .detach();
    }

    fn render_release(&self, release: &Release, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let key = release_key(release);
        let op = self.ops.read(cx).running.get(&key).map(|op| op.snapshot());
        let installed = release.installed();
        let latest = sources::is_latest_release(release);
        let has_update = installed && release.has_update == Some(true);
        let size = if installed && release.disksize > 0 {
            release.disksize
        } else {
            release.downsize
        };
        let meta = [
            (!release.date.is_empty()).then(|| {
                t_args(
                    "manage.wine.released",
                    &[("date", &irori_ui::dates::localized_date(&release.date))],
                )
                .to_string()
            }),
            format::size(size),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" · ");

        let actions: AnyElement = match (&op, installed) {
            (Some(_), _) => {
                let cancel_key = key.clone();
                Button::new("cancel")
                    .label(t("manage.wine.cancel"))
                    .on_click(move |_, _, cx| cancel_install(&cancel_key, cx))
                    .into_any_element()
            }
            (None, true) => {
                let (dir, upd, rem) = (
                    release.install_dir.clone().unwrap_or_default(),
                    release.clone(),
                    release.clone(),
                );
                h_flex()
                    .gap_1()
                    .child(
                        Button::new("open-folder")
                            .ghost()
                            .small()
                            .icon(IconName::FolderOpen)
                            .tooltip(t("manage.wine.open_folder"))
                            .on_click(move |_, _, cx| cx.open_with_system(Path::new(&dir))),
                    )
                    .when(has_update, |this| {
                        this.child(
                            Button::new("update")
                                .small()
                                .icon(IconName::RefreshCw)
                                .label(t("manage.wine.update"))
                                .on_click(move |_, _, cx| {
                                    start_install(upd.clone(), false, cx);
                                }),
                        )
                    })
                    .child(
                        Button::new("remove")
                            .ghost()
                            .small()
                            .label(t("manage.wine.remove"))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.confirm_remove(rem.clone(), window, cx)
                            })),
                    )
                    .into_any_element()
            }
            (None, false) => {
                let r = release.clone();
                Button::new("install")
                    .small()
                    .icon(IconName::Download)
                    .label(t("manage.wine.install"))
                    .when(latest, |b| b.primary())
                    .on_click(move |_, _, cx| {
                        start_install(r.clone(), false, cx);
                    })
                    .into_any_element()
            }
        };

        h_flex()
            .id(ElementId::Name(format!("release-{key}").into()))
            .gap_4()
            .px_3()
            .py_2()
            .rounded(theme.radius)
            .when(latest, |row| row.bg(theme.secondary.opacity(0.3)))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_0p5()
                    .child(
                        h_flex()
                            .gap_2()
                            .child(
                                div()
                                    .font_medium()
                                    .truncate()
                                    .child(release.version.clone()),
                            )
                            .when(latest, |this| {
                                this.child(
                                    Tag::primary()
                                        .xsmall()
                                        .child(t("manage.wine.tag.recommended")),
                                )
                            })
                            .when(installed, |this| {
                                this.child(
                                    Tag::success()
                                        .xsmall()
                                        .child(t("manage.wine.tag.installed")),
                                )
                            })
                            .when(has_update, |this| {
                                this.child(
                                    Tag::warning()
                                        .xsmall()
                                        .child(t("manage.wine.tag.update_available")),
                                )
                            }),
                    )
                    .child(
                        h_flex()
                            .gap_3()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(meta)
                            .when_some(
                                irori_ui::web_url(&release.release_notes_link),
                                |this, url| {
                                    this.child(
                                        Link::new("notes")
                                            .href(url)
                                            .child(t("manage.wine.release_notes")),
                                    )
                                },
                            ),
                    ),
            )
            .when_some(op, |this, live| this.child(render_op(&live, cx)))
            .child(div().flex_shrink_0().child(actions))
            .into_any_element()
    }

    fn render_source(&self, kind: ReleaseKind, cx: &mut Context<Self>) -> AnyElement {
        let (muted, radius, summary_bg, border) = {
            let theme = cx.theme();
            (
                theme.muted_foreground,
                theme.radius,
                theme.secondary.opacity(0.35),
                theme.border,
            )
        };
        let query = self.search.read(cx).value().trim().to_string();
        let available = ReleaseKind::available();
        let quick = sources::quick_sources(available);
        let secondary = sources::secondary_quick_sources(available);
        let other_selected = !quick.contains(&kind) && !secondary.contains(&kind);
        let has_more = available.len() > quick.len() + secondary.len();
        let expanded = self.show_older.contains(&kind);

        let body: AnyElement = match self.lists.get(&kind) {
            None | Some(ListState::Loading) => h_flex()
                .gap_2()
                .text_sm()
                .text_color(muted)
                .child(Spinner::new().small())
                .child(t("manage.wine.loading"))
                .into_any_element(),
            Some(ListState::Failed(message)) => {
                Alert::error("wine-list-error", message.clone()).into_any_element()
            }
            Some(ListState::Loaded(list)) if list.is_empty() => {
                EmptyState::new(IconName::Wine, t("manage.wine.empty.title"))
                    .description(t("manage.wine.empty.description"))
                    .into_any_element()
            }
            Some(ListState::Loaded(list)) => {
                let visible = sources::visible_releases(list, &query, expanded);
                if visible.items.is_empty() {
                    div()
                        .text_sm()
                        .text_color(muted)
                        .child(t("manage.wine.no_match"))
                        .into_any_element()
                } else {
                    let rows: Vec<AnyElement> = visible
                        .items
                        .into_iter()
                        .map(|r| self.render_release(r, cx))
                        .collect();
                    let show_fewer = visible.hidden_count == 0
                        && expanded
                        && query.is_empty()
                        && sources::visible_releases(list, "", false).hidden_count > 0;
                    v_flex()
                        .gap_1()
                        .children(rows)
                        .when(visible.hidden_count > 0, |col| {
                            col.child(
                                div().pt_2().child(
                                    Button::new("show-older-wine")
                                        .ghost()
                                        .small()
                                        .icon(IconName::ChevronDown)
                                        .label(t_args(
                                            "manage.wine.show_older",
                                            &[("count", &visible.hidden_count.to_string())],
                                        ))
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.toggle_older(kind, cx)
                                        })),
                                ),
                            )
                        })
                        .when(show_fewer, |col| {
                            col.child(
                                div().pt_2().child(
                                    Button::new("show-fewer-wine")
                                        .ghost()
                                        .small()
                                        .icon(IconName::ChevronUp)
                                        .label(t("manage.wine.show_fewer"))
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.toggle_older(kind, cx)
                                        })),
                                ),
                            )
                        })
                        .into_any_element()
                }
            }
        };

        let selector = h_flex()
            .gap_3()
            .flex_wrap()
            .items_center()
            .justify_between()
            .child(
                h_flex()
                    .gap_2()
                    .flex_wrap()
                    .items_center()
                    .children(quick.iter().copied().map(|q| {
                        Button::new(ElementId::Name(
                            format!("wine-source-{}", q.type_str()).into(),
                        ))
                        .label(source_label(q))
                        .map(|b| if q == kind { b.primary() } else { b.outline() })
                        .on_click(
                            cx.listener(move |this, _, window, cx| {
                                this.select_source(q, window, cx)
                            }),
                        )
                    }))
                    .when(!secondary.is_empty(), |row| {
                        row.child(
                            div()
                                .w(px(1.))
                                .h(rems(1.25))
                                .mx_1()
                                .flex_shrink_0()
                                .bg(border),
                        )
                        .children(secondary.iter().copied().map(|q| {
                            Button::new(ElementId::Name(
                                format!("wine-source-{}", q.type_str()).into(),
                            ))
                            .label(source_label(q))
                            .map(|b| {
                                if q == kind {
                                    b.primary()
                                } else {
                                    b.outline().text_color(muted)
                                }
                            })
                            .on_click(cx.listener(
                                move |this, _, window, cx| this.select_source(q, window, cx),
                            ))
                        }))
                    })
                    .when(has_more, |row| {
                        let groups = sources::grouped_sources(available);
                        let view = cx.entity().downgrade();
                        row.child(
                            Button::new("wine-more-builds")
                                .dropdown_caret(true)
                                .map(|b| {
                                    if other_selected {
                                        b.primary().label(source_label(kind))
                                    } else {
                                        b.outline().label(t("manage.wine.more_builds"))
                                    }
                                })
                                .dropdown_menu(move |mut menu, _, _| {
                                    for (ix, (group, items)) in groups.iter().enumerate() {
                                        if ix > 0 {
                                            menu = menu.separator();
                                        }
                                        menu = menu.label(group.label());
                                        for &source in items {
                                            let view = view.clone();
                                            menu = menu.item(
                                                PopupMenuItem::new(source_label(source))
                                                    .checked(source == kind)
                                                    .on_click(move |_, window, cx| {
                                                        let _ = view.update(cx, |this, cx| {
                                                            this.select_source(source, window, cx)
                                                        });
                                                    }),
                                            );
                                        }
                                    }
                                    menu
                                }),
                        )
                    }),
            )
            .child(
                div()
                    .w(rems(20.))
                    .flex_shrink_0()
                    .child(Input::new(&self.search)),
            );

        let summary = v_flex()
            .gap_1()
            .p_3()
            .rounded(radius)
            .bg(summary_bg)
            .border_1()
            .border_color(border)
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(div().font_semibold().truncate().child(source_label(kind)))
                    .when_some(sources::source_badge(kind), |this, badge| {
                        this.child(badge.tag())
                    }),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(muted)
                    .child(sources::source_description(kind)),
            );

        let matched_sources = sources::matching_sources(available, kind, &query);

        v_flex()
            .gap_4()
            .child(selector)
            .child(summary)
            .when(!matched_sources.is_empty(), |col| {
                col.child(
                    h_flex()
                        .gap_2()
                        .flex_wrap()
                        .items_center()
                        .child(
                            div()
                                .text_sm()
                                .text_color(muted)
                                .child(t("manage.wine.matching_sources")),
                        )
                        .children(matched_sources.into_iter().map(|source| {
                            Button::new(ElementId::Name(
                                format!("wine-match-{}", source.type_str()).into(),
                            ))
                            .small()
                            .outline()
                            .label(source_label(source))
                            .on_click(cx.listener(
                                move |this, _, window, cx| this.select_source(source, window, cx),
                            ))
                        })),
                )
            })
            .when_some(
                self.limited_until.filter(|reset| {
                    *reset > irori_core::util::unix_now() && release_checks::is_github(kind.url())
                }),
                |col, reset| {
                    let limited = release_checks::RateLimited { reset };
                    col.child(
                        div()
                            .text_xs()
                            .text_color(muted)
                            .child(rate_limited_text(limited, irori_core::util::unix_now())),
                    )
                },
            )
            .child(body)
            .into_any_element()
    }

    fn render_system(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let (muted, border, background, radius) = (
            theme.muted_foreground,
            theme.border,
            theme.background,
            theme.radius,
        );
        let custom_paths = models(cx).settings.read(cx).app.custom_wine_paths.clone();
        let has_custom = !custom_paths.is_empty();
        let origin_icon = |origin: WineOrigin| -> Icon {
            match origin {
                WineOrigin::Steam => StoreLogo::Steam.icon().text_color(muted),
                WineOrigin::Lutris => StoreLogo::Lutris.icon().text_color(muted),
                WineOrigin::Custom => Icon::new(IconName::Folder).small().text_color(muted),
                WineOrigin::System => Icon::new(IconName::HardDrive).small().text_color(muted),
                WineOrigin::Irori | WineOrigin::Bottles | WineOrigin::Other => {
                    Icon::new(IconName::Wine).small().text_color(muted)
                }
            }
        };
        let builds: AnyElement = match &self.system {
            None | Some(SystemState::Loading) => h_flex()
                .gap_2()
                .text_sm()
                .text_color(muted)
                .child(Spinner::new().small())
                .child(t("manage.wine.system.loading"))
                .into_any_element(),
            Some(SystemState::Loaded(groups)) if groups.is_empty() => {
                EmptyState::new(IconName::Wine, t("manage.wine.system.empty.title"))
                    .description(t("manage.wine.system.empty.description"))
                    .into_any_element()
            }
            Some(SystemState::Loaded(groups)) => v_flex()
                .gap_4()
                .children(groups.iter().map(|(origin, list)| {
                    let origin = *origin;
                    card(cx)
                        .child(
                            h_flex()
                                .gap_2()
                                .items_center()
                                .child(origin_icon(origin))
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .font_semibold()
                                        .truncate()
                                        .child(origin.label()),
                                )
                                .child(Tag::secondary().xsmall().child(list.len().to_string())),
                        )
                        .child(
                            v_flex()
                                .w_full()
                                .rounded(radius)
                                .bg(background)
                                .border_1()
                                .border_color(border)
                                .overflow_hidden()
                                .children(list.iter().enumerate().map(|(ix, w)| {
                                    let reveal = Path::new(&w.bin)
                                        .parent()
                                        .map(Path::to_path_buf)
                                        .unwrap_or_else(|| PathBuf::from(&w.bin));
                                    h_flex()
                                        .w_full()
                                        .gap_3()
                                        .px_3()
                                        .py_2()
                                        .items_center()
                                        .when(ix > 0, |row| row.border_t_1().border_color(border))
                                        .child(
                                            v_flex()
                                                .flex_1()
                                                .min_w_0()
                                                .gap_0p5()
                                                .child(
                                                    div()
                                                        .font_medium()
                                                        .truncate()
                                                        .child(w.name.clone()),
                                                )
                                                .child(
                                                    div()
                                                        .text_xs()
                                                        .text_color(muted)
                                                        .truncate()
                                                        .child(w.bin.clone()),
                                                ),
                                        )
                                        .child(
                                            h_flex()
                                                .gap_1p5()
                                                .flex_shrink_0()
                                                .items_center()
                                                .child(
                                                    Tag::secondary()
                                                        .xsmall()
                                                        .child(kind_label(w.kind)),
                                                )
                                                .child(
                                                    Button::new(ElementId::Name(
                                                        format!("system-open-{origin:?}-{ix}")
                                                            .into(),
                                                    ))
                                                    .ghost()
                                                    .small()
                                                    .icon(IconName::FolderOpen)
                                                    .tooltip(t("manage.wine.open_folder"))
                                                    .accessibility_label(t(
                                                        "manage.wine.open_folder",
                                                    ))
                                                    .on_click(move |_, _, cx| {
                                                        cx.reveal_path(&reveal)
                                                    }),
                                                ),
                                        )
                                })),
                        )
                }))
                .into_any_element(),
        };
        let paths_card = card(cx)
            .child(
                SectionTitle::new(t("manage.wine.custom.title")).trailing(
                    Button::new("add-path")
                        .small()
                        .icon(IconName::FolderPlus)
                        .label(t("manage.wine.custom.add"))
                        .on_click(
                            cx.listener(|this, _, window, cx| this.add_custom_path(window, cx)),
                        ),
                ),
            )
            .when(!has_custom, |this| {
                this.child(
                    div()
                        .text_sm()
                        .text_color(muted)
                        .child(t("manage.wine.custom.hint")),
                )
            })
            .when(has_custom, |this| {
                this.child(
                    v_flex()
                        .w_full()
                        .rounded(radius)
                        .bg(background)
                        .border_1()
                        .border_color(border)
                        .overflow_hidden()
                        .children(custom_paths.into_iter().enumerate().map(|(ix, p)| {
                            let remove = p.clone();
                            h_flex()
                                .id(ElementId::Name(format!("custom-{p}").into()))
                                .w_full()
                                .gap_3()
                                .px_3()
                                .py_1p5()
                                .items_center()
                                .when(ix > 0, |row| row.border_t_1().border_color(border))
                                .child(Icon::new(IconName::Folder).small().text_color(muted))
                                .child(div().flex_1().min_w_0().text_sm().truncate().child(p))
                                .child(
                                    Button::new("remove-path")
                                        .ghost()
                                        .small()
                                        .label(t("manage.wine.custom.remove"))
                                        .on_click(move |_, _, cx| {
                                            let remove = remove.clone();
                                            backend_op(cx, move |b| {
                                                b.settings.update_app(|s| {
                                                    s.custom_wine_paths.retain(|x| x != &remove)
                                                });
                                                b.bus.emit(Event::WineVersionsChanged);
                                            });
                                        }),
                                )
                        })),
                )
            });
        if has_custom {
            v_flex()
                .gap_4()
                .child(paths_card)
                .child(builds)
                .into_any_element()
        } else {
            v_flex()
                .gap_4()
                .child(builds)
                .child(paths_card)
                .into_any_element()
        }
    }
}

/// Inline progress of a running install.
fn render_op(live: &OpLive, cx: &App) -> impl IntoElement {
    let progress = live.progress.unwrap_or_default();
    let extracting = live.extracting();
    let detail = if extracting {
        t("manage.wine.progress.extracting").to_string()
    } else {
        [
            progress.percent.map(|p| {
                t_args(
                    "manage.wine.progress.percent",
                    &[("percent", &format!("{p:.0}"))],
                )
                .to_string()
            }),
            Some(format::rate(progress.download_speed)).filter(|s| s != "—"),
            progress.eta_secs.filter(|e| *e > 0).map(|e| {
                t_args(
                    "manage.wine.progress.time_left",
                    &[("time", &format::eta(Some(e)))],
                )
                .to_string()
            }),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" · ")
    };
    v_flex()
        .w(rems(14.))
        .flex_shrink_0()
        .gap_1()
        .child(
            Progress::new("wine-progress")
                .value(progress.percent.unwrap_or(0.0))
                .loading(extracting || progress.percent.is_none()),
        )
        .child(
            div()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .truncate()
                .child(if detail.is_empty() {
                    t("manage.wine.progress.starting").to_string()
                } else {
                    detail
                }),
        )
}

impl Render for WineManagerView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let language = irori_ui::i18n::language();
        if language != self.placeholder_language {
            self.placeholder_language = language;
            self.search.update(cx, |s, cx| {
                s.set_placeholder(t("manage.wine.search"), window, cx)
            });
            self.prefix_search.update(cx, |s, cx| {
                s.set_placeholder(t("manage.wine.prefixes.search"), window, cx)
            });
        }
        let (fg, muted, border, primary) = {
            let theme = cx.theme();
            (
                theme.foreground,
                theme.muted_foreground,
                theme.border,
                theme.primary,
            )
        };
        let running = self.ops.read(cx).running.len();
        let has_downloads = !ReleaseKind::available().is_empty();
        let selected_tab = match self.active_tab {
            WineTab::Download if has_downloads => 0,
            WineTab::Prefixes => usize::from(has_downloads) + 1,
            _ => usize::from(has_downloads),
        };
        let mut tabs: Vec<(usize, &'static str, SharedString)> = Vec::with_capacity(3);
        if has_downloads {
            tabs.push((0, "download", t("manage.wine.tab.download")));
        }
        let sys_ix = usize::from(has_downloads);
        tabs.push((sys_ix, "system", t("manage.wine.tab.system")));
        tabs.push((sys_ix + 1, "prefixes", t("manage.wine.tab.prefixes")));
        let on_prefixes = self.on_prefixes();
        let content = match self.active_source() {
            Some(kind) => self.render_source(kind, cx),
            None if on_prefixes => self.render_prefixes(cx),
            None => self.render_system(cx),
        };
        // The prefix list is virtual and scrolls itself.
        let previous = (!on_prefixes)
            .then(|| self.render_previous_builds(cx))
            .flatten();
        let body = if on_prefixes {
            content
        } else {
            div()
                .id("wine-body")
                .flex_1()
                .min_h_0()
                .overflow_y_scrollbar()
                .child(
                    v_flex()
                        .id("wine-content")
                        .debug_selector(|| "wine-content".into())
                        .w_full()
                        .px_6()
                        .py_4()
                        .gap_6()
                        .children(previous)
                        .child(content),
                )
                .into_any_element()
        };
        let description = if cfg!(target_os = "macos") {
            t("manage.wine.description_macos")
        } else {
            match running {
                0 => t("manage.wine.description"),
                1 => t("manage.wine.description_running_one"),
                n => t_args(
                    "manage.wine.description_running_other",
                    &[("count", &n.to_string())],
                ),
            }
        };
        v_flex()
            .size_full()
            .min_h_0()
            .child(
                v_flex()
                    .id("wine-header")
                    .debug_selector(|| "wine-header".into())
                    .w_full()
                    .px_6()
                    .pt_6()
                    .gap_4()
                    .child(
                        PageHeader::new(t("manage.wine.title"))
                            .description(description)
                            .action(
                                Button::new("refresh-wine")
                                    .icon(IconName::RefreshCw)
                                    .label(t("manage.wine.refresh"))
                                    .loading(self.refreshing)
                                    .disabled(self.refreshing)
                                    .on_click(
                                        cx.listener(|this, _, window, cx| this.refresh(window, cx)),
                                    ),
                            ),
                    )
                    .child(
                        h_flex()
                            .id("wine-tabs")
                            .w_full()
                            .gap_8()
                            .border_b_1()
                            .border_color(border)
                            .children(tabs.into_iter().map(|(ix, id, label)| {
                                let active = selected_tab == ix;
                                v_flex()
                                    .id(ElementId::Name(format!("wine-tab-{id}").into()))
                                    .debug_selector(move || format!("wine-tab-{id}"))
                                    .cursor_pointer()
                                    .gap_2p5()
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.select_tab(ix, window, cx)
                                    }))
                                    .child(
                                        div()
                                            .text_xl()
                                            .font_semibold()
                                            .text_color(if active { fg } else { muted })
                                            .hover(move |s| s.text_color(fg))
                                            .child(label),
                                    )
                                    .child(
                                        div()
                                            .h(px(3.))
                                            .w_full()
                                            .rounded_full()
                                            .when(active, |b| b.bg(primary)),
                                    )
                            })),
                    ),
            )
            .child(body)
    }
}
