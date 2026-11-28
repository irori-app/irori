//! The Wine manager ([`Route::WineManager`](irori_ui::Route::WineManager)):
//! a 2-view workspace with a unified `Builds` view (quick defaults and
//! integrations, installed & system builds, and downloadable Proton/Wine
//! sources) plus a dedicated virtualized `Prefixes` view.

mod installed;
mod installed_pane;
mod prefixes;
mod sources;
mod ui;

use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::assets::IconName;
use gpui_kit::component::IndexPath;
use gpui_kit::component::alert::Alert;
use gpui_kit::component::button::{Button, ButtonVariant, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::link::Link;
use gpui_kit::component::notification::Notification;
use gpui_kit::component::popover::Popover;
use gpui_kit::component::progress::Progress;
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::select::{Select, SelectEvent, SelectItem, SelectState};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, Sizable as _, StyledExt as _, WindowExt as _, h_flex,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    Anchor, AnyElement, App, AppContext as _, Context, Entity, InteractiveElement as _,
    IntoElement, ParentElement as _, PathPromptOptions, Pixels, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, Subscription, Task, Window, canvas, div, px,
    rems,
};
use irori_core::{Event, Paths, WineInstallation};
use irori_runtime::manager::{self, Release, ReleaseKind};
use irori_runtime::release_checks;
use irori_ui::components::EmptyState;
use irori_ui::i18n::{t, t_args};
use irori_ui::{backend, models};

use crate::common::{backend_op, card, info_tip, notify_error, notify_success};
use crate::format;
use installed::{OriginFilter, WineOrigin, exclude_managed, group, natural_desc};
use irori_ui::wine_installs::{
    OpLive, WineOps, cancel_install, release_key, start_install, wine_ops,
};
use ui::{
    PAGE_MAX_WIDTH_REM, PillTone, choice_pill, control_switch, eid, icon_tile, muted_xs,
    pane_shell, pill, seg_button, seg_group,
};

const PROGRESS_TICK: Duration = Duration::from_millis(300);
const SPLIT_MIN_WIDTH_REM: f32 = 58.;

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
    Builds,
    Prefixes,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NarrowPane {
    Installed,
    Download,
}

#[derive(Clone)]
enum ListState {
    Loading,
    Loaded(Vec<Release>),
    Failed(SharedString),
}

#[derive(Clone)]
pub(super) enum SystemState {
    Loading,
    Loaded {
        managed: Vec<Release>,
        detected: Vec<(WineOrigin, Vec<WineInstallation>)>,
        all_installs: Vec<WineInstallation>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DefaultBuildChoice {
    value: SharedString,
    title: SharedString,
}

impl SelectItem for DefaultBuildChoice {
    type Value = SharedString;
    fn title(&self) -> SharedString {
        self.title.clone()
    }
    fn value(&self) -> &SharedString {
        &self.value
    }
}

pub(super) fn release_is_default(release: &Release, default_bin: &str) -> bool {
    !default_bin.is_empty()
        && (release.installation().is_some_and(|w| w.bin == default_bin)
            || release
                .install_dir
                .as_deref()
                .is_some_and(|d| !d.is_empty() && default_bin.starts_with(d)))
}

/// Unified Wine manager view: 2 top-level screens (`Builds` and `Prefixes`).
pub struct WineManagerView {
    active_tab: WineTab,
    narrow_pane: NarrowPane,
    selected_source: Option<ReleaseKind>,
    more_sources_open: bool,
    show_older: HashSet<ReleaseKind>,
    lists: HashMap<ReleaseKind, ListState>,
    refreshing: bool,
    /// GitHub's API budget is used up until this unix time.
    limited_until: Option<u64>,
    search: Entity<InputState>,
    pub(super) installed_search: Entity<InputState>,
    prefix_search: Entity<InputState>,
    pub(super) origin_filter: OriginFilter,
    pub(super) collapsed_origins: HashSet<WineOrigin>,
    prefix_filter: prefixes::PrefixFilter,
    default_select: Entity<SelectState<Vec<DefaultBuildChoice>>>,
    viewport_width: Rc<Cell<Pixels>>,
    pub(super) system: Option<SystemState>,
    prefixes: Option<prefixes::PrefixState>,
    pub(super) ops: Entity<WineOps>,
    seen_revision: u64,
    ticking: bool,
    placeholder_language: String,
    _ticker: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl WineManagerView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        crate::i18n::register();
        let search = cx.new(|cx| InputState::new(window, cx).placeholder(t("manage.wine.search")));
        let installed_search =
            cx.new(|cx| InputState::new(window, cx).placeholder(t("manage.wine.installed.search")));
        let prefix_search =
            cx.new(|cx| InputState::new(window, cx).placeholder(t("manage.wine.prefixes.search")));
        let default_select =
            cx.new(|cx| SelectState::new(Vec::<DefaultBuildChoice>::new(), None, window, cx));
        let ops = wine_ops(cx);
        let settings = models(cx).settings.clone();
        let subscriptions = vec![
            cx.subscribe(&search, |_, _, event, cx| {
                if matches!(event, InputEvent::Change) {
                    cx.notify();
                }
            }),
            cx.subscribe(&installed_search, |_, _, event, cx| {
                if matches!(event, InputEvent::Change) {
                    cx.notify();
                }
            }),
            cx.subscribe(&prefix_search, |this, _, event, cx| {
                if matches!(event, InputEvent::Change) {
                    this.refresh_prefix_filter(cx);
                }
            }),
            cx.subscribe_in(
                &default_select,
                window,
                |this, _, event: &SelectEvent<Vec<DefaultBuildChoice>>, window, cx| {
                    if let SelectEvent::Confirm(Some(bin)) = event
                        && let Some(wine) = this.find_installation_by_bin(bin.as_ref())
                    {
                        this.set_default_wine(wine, window, cx);
                    }
                },
            ),
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
                this.load_system(window, cx);
                if this.prefixes.is_some() {
                    this.load_prefixes(window, cx);
                }
            }),
            cx.observe_in(
                &models(cx).library.clone(),
                window,
                |this, _, window, cx| {
                    if this.prefixes.is_some() {
                        this.load_prefixes(window, cx);
                    }
                },
            ),
            cx.observe(&models(cx).components.clone(), |_, _, cx| cx.notify()),
        ];
        let seen_revision = ops.read(cx).revision;
        let selected_source = ReleaseKind::available().first().copied();
        let mut this = Self {
            active_tab: WineTab::Builds,
            narrow_pane: NarrowPane::Installed,
            selected_source,
            more_sources_open: false,
            show_older: HashSet::new(),
            lists: HashMap::new(),
            refreshing: false,
            limited_until: None,
            search,
            installed_search,
            prefix_search,
            origin_filter: OriginFilter::All,
            collapsed_origins: HashSet::new(),
            prefix_filter: prefixes::PrefixFilter::All,
            default_select,
            viewport_width: Rc::new(Cell::new(px(0.))),
            system: None,
            prefixes: None,
            ops,
            seen_revision,
            ticking: false,
            placeholder_language: irori_ui::i18n::language(),
            _ticker: None,
            _subscriptions: subscriptions,
        };
        this.load_system(window, cx);
        if let Some(kind) = selected_source {
            this.load_list(kind, false, window, cx);
        }
        this.load_prefixes(window, cx);
        this.ensure_ticker(window, cx);
        this
    }

    fn select_tab(&mut self, tab: WineTab, window: &mut Window, cx: &mut Context<Self>) {
        if self.active_tab == tab {
            return;
        }
        self.active_tab = tab;
        self.more_sources_open = false;
        if tab == WineTab::Prefixes && self.prefixes.is_none() {
            self.load_prefixes(window, cx);
        }
        cx.notify();
    }

    fn select_source(&mut self, kind: ReleaseKind, window: &mut Window, cx: &mut Context<Self>) {
        self.more_sources_open = false;
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
        self.load_system(window, cx);
    }

    fn find_installation_by_bin(&self, bin: &str) -> Option<WineInstallation> {
        let Some(SystemState::Loaded {
            managed,
            all_installs,
            ..
        }) = &self.system
        else {
            return None;
        };
        all_installs
            .iter()
            .find(|w| w.bin == bin)
            .cloned()
            .or_else(|| {
                managed
                    .iter()
                    .filter_map(Release::installation)
                    .find(|w| w.bin == bin)
            })
    }

    pub(super) fn set_default_wine(
        &mut self,
        wine: WineInstallation,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if models(cx).settings.read(cx).app.game.wine_version.bin == wine.bin {
            return;
        }
        let name = wine.name.clone();
        backend_op(cx, move |b| {
            b.settings.update_app(move |s| s.game.wine_version = wine);
            b.bus.emit(Event::WineVersionsChanged);
        });
        notify_success(
            t_args("manage.wine.controls.default_changed", &[("name", &name)]),
            window,
            cx,
        );
    }

    fn sync_default_select(&self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(SystemState::Loaded {
            managed, detected, ..
        }) = &self.system
        else {
            return;
        };
        let default_bin = models(cx)
            .settings
            .read(cx)
            .app
            .game
            .wine_version
            .bin
            .clone();
        let (mut items, mut seen) = (Vec::new(), HashSet::new());
        for r in managed {
            if let Some(w) = r.installation()
                && seen.insert(w.bin.clone())
            {
                items.push(DefaultBuildChoice {
                    value: w.bin.into(),
                    title: format!("{} (Irori)", r.version).into(),
                });
            }
        }
        for (origin, list) in detected {
            let label = origin.label();
            for w in list {
                if seen.insert(w.bin.clone()) {
                    items.push(DefaultBuildChoice {
                        value: w.bin.clone().into(),
                        title: format!("{} ({label})", w.name).into(),
                    });
                }
            }
        }
        let ix = items
            .iter()
            .position(|i| i.value.as_ref() == default_bin)
            .map(|r| IndexPath::default().row(r));
        self.default_select.update(cx, |sel, cx| {
            sel.set_items(items, window, cx);
            sel.set_selected_index(ix, window, cx);
        });
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
                        this.load_system(window, cx);
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
        let available: HashSet<&'static str> = ReleaseKind::available()
            .iter()
            .map(|k| k.type_str())
            .collect();
        let task = irori_ui::run(cx, async move {
            let installs = irori_runtime::wine_installations(&app).await;
            let cached = tokio::task::spawn_blocking(manager::cached_releases)
                .await
                .unwrap_or_default();
            let mut managed: Vec<Release> = cached
                .into_iter()
                .filter(|r| r.installed() && available.contains(r.kind.as_str()))
                .collect();
            managed.sort_by(|a, b| {
                b.is_latest()
                    .cmp(&a.is_latest())
                    .then_with(|| {
                        b.has_update
                            .unwrap_or(false)
                            .cmp(&a.has_update.unwrap_or(false))
                    })
                    .then_with(|| natural_desc(&a.version, &b.version))
            });
            let own = [Paths::get().tools_dir()];
            let detected = group(&exclude_managed(&installs, &managed), &custom, &own);
            Ok((managed, detected, installs))
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok((managed, detected, all_installs)) = task.await else {
                return;
            };
            let _ = this.update_in(cx, |this, window, cx| {
                this.system = Some(SystemState::Loaded {
                    managed,
                    detected,
                    all_installs,
                });
                this.sync_default_select(window, cx);
                cx.notify();
            });
        })
        .detach();
    }

    fn refresh(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.active_tab {
            WineTab::Builds => {
                self.load_system(window, cx);
                if let Some(kind) = self.selected_source {
                    self.load_list(kind, true, window, cx);
                }
            }
            WineTab::Prefixes => self.load_prefixes(window, cx),
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

    pub(super) fn confirm_remove(
        &mut self,
        release: Release,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let default_bin = models(cx)
            .settings
            .read(cx)
            .app
            .game
            .wine_version
            .bin
            .clone();
        let is_default = release_is_default(&release, &default_bin);
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
    pub(super) fn confirm_go_back(
        &self,
        version: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
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

    pub(super) fn add_custom_path(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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

    fn render_release(
        &self,
        kind: ReleaseKind,
        release: &Release,
        searching: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme();
        let key = release_key(release);
        let op = self.ops.read(cx).running.get(&key).map(|op| op.snapshot());
        let installed = release.installed();
        let latest = sources::is_latest_release(release);
        let featured = latest && !searching;
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
                    .small()
                    .outline()
                    .label(t("manage.wine.cancel"))
                    .on_click(move |_, _, cx| cancel_install(&cancel_key, cx))
                    .into_any_element()
            }
            (None, true) => {
                let (upd, rem) = (release.clone(), release.clone());
                h_flex()
                    .gap_1()
                    .items_center()
                    .when(has_update, |this| {
                        this.child(
                            Button::new("update")
                                .primary()
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
                    .map(|b| if latest { b.primary() } else { b.outline() })
                    .on_click(move |_, _, cx| {
                        start_install(r.clone(), false, cx);
                    })
                    .into_any_element()
            }
        };

        let mut head = h_flex().gap_2().flex_wrap().items_center().child(
            div()
                .font_semibold()
                .truncate()
                .map(|d| if featured { d.text_sm() } else { d.text_xs() })
                .child(release.version.clone()),
        );
        if latest {
            head = head.child(pill(
                t("manage.wine.tag.rolling_channel"),
                PillTone::Accent,
                false,
                cx,
            ));
        }
        if installed && !(featured && has_update) {
            head = head.child(pill(
                t("manage.wine.tag.installed"),
                PillTone::Success,
                true,
                cx,
            ));
        }
        if has_update {
            head = head.child(pill(
                t("manage.wine.tag.update_available"),
                PillTone::Warning,
                true,
                cx,
            ));
        }

        let meta_row = h_flex()
            .gap_3()
            .text_xs()
            .text_color(theme.muted_foreground)
            .child(meta)
            .children(irori_ui::web_url(&release.release_notes_link).map(|url| {
                Link::new("notes")
                    .href(url)
                    .child(t("manage.wine.release_notes"))
            }));
        let info = v_flex()
            .flex_1()
            .min_w_0()
            .gap_1()
            .child(head)
            .when(featured, |c| {
                c.child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(t_args(
                            "manage.wine.latest_hint",
                            &[("source", source_label(kind))],
                        )),
                )
            })
            .child(meta_row);

        let top = h_flex()
            .gap_3()
            .items_center()
            .child(info)
            .when(!featured, |r| {
                r.children(op.as_ref().map(|l| render_op(l, cx)))
            })
            .child(div().flex_shrink_0().child(actions));

        let wrap = v_flex()
            .id(eid("release", &key))
            .w_full()
            .gap_2()
            .child(top)
            .when(featured, |c| {
                c.children(op.as_ref().map(|l| render_op(l, cx)))
            });
        if featured {
            let bc = if has_update {
                theme.warning.opacity(0.35)
            } else {
                theme.border
            };
            wrap.p_3p5()
                .rounded(theme.radius_lg)
                .bg(theme.secondary.opacity(0.32))
                .border_1()
                .border_color(bc)
                .into_any_element()
        } else {
            wrap.px_3().py_2().rounded(theme.radius).into_any_element()
        }
    }

    fn render_more_sources_popover(
        &self,
        kind: ReleaseKind,
        available: &[ReleaseKind],
        other_selected: bool,
        cx: &mut Context<Self>,
    ) -> Popover {
        let groups = sources::grouped_sources(available);
        let (view, toggle) = (cx.entity().downgrade(), cx.entity().downgrade());
        let trigger = Button::new("wine-more-builds")
            .small()
            .dropdown_caret(true)
            .map(|b| {
                if other_selected {
                    b.primary().label(source_label(kind))
                } else {
                    b.outline().label(t("manage.wine.more_builds"))
                }
            });
        Popover::new("wine-more-builds-popover")
            .anchor(Anchor::TopRight)
            .open(self.more_sources_open)
            .on_open_change(move |open, _, cx| {
                let open = *open;
                let _ = toggle.update(cx, |this, cx| {
                    this.more_sources_open = open;
                    cx.notify();
                });
            })
            .trigger(trigger)
            .content(move |_, _, cx| {
                let t = cx.theme();
                let (fg, border, primary, hover, radius) = (
                    t.foreground,
                    t.border,
                    t.primary,
                    t.secondary.opacity(0.4),
                    t.radius,
                );
                let mut list = v_flex()
                    .w(rems(20.))
                    .max_h(rems(26.))
                    .overflow_y_scrollbar()
                    .gap_1p5();
                for (gix, (group, items)) in groups.iter().enumerate() {
                    let mut sec = v_flex()
                        .gap_1()
                        .when(gix > 0, |s| s.pt_1p5().border_t_1().border_color(border))
                        .child(muted_xs(group.label(), cx).px_2().font_semibold());
                    for &source in items {
                        let (sel, v) = (source == kind, view.clone());
                        let item = v_flex()
                            .id(eid("more-src", source.type_str()))
                            .px_2()
                            .py_1p5()
                            .gap_0p5()
                            .rounded(radius)
                            .cursor_pointer()
                            .map(|d| {
                                if sel {
                                    d.bg(primary.opacity(0.13))
                                } else {
                                    d.hover(move |s| s.bg(hover))
                                }
                            })
                            .on_click(move |_, win, cx| {
                                let _ =
                                    v.update(cx, |this, cx| this.select_source(source, win, cx));
                            })
                            .child(
                                h_flex()
                                    .gap_2()
                                    .items_center()
                                    .justify_between()
                                    .child(
                                        div()
                                            .text_xs()
                                            .font_semibold()
                                            .text_color(if sel { primary } else { fg })
                                            .child(source_label(source)),
                                    )
                                    .children(sources::source_badge(source).map(|b| b.pill(cx))),
                            )
                            .child(muted_xs(sources::source_description(source), cx));
                        sec = sec.child(item);
                    }
                    list = list.child(sec);
                }
                list
            })
    }

    fn render_source(&self, kind: ReleaseKind, cx: &mut Context<Self>) -> AnyElement {
        let (muted, radius, bg, border, warning) = {
            let theme = cx.theme();
            (
                theme.muted_foreground,
                theme.radius,
                theme.background,
                theme.border,
                theme.warning,
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
                .p_4()
                .gap_2()
                .text_sm()
                .text_color(muted)
                .child(Spinner::new().small())
                .child(t("manage.wine.loading"))
                .into_any_element(),
            Some(ListState::Failed(message)) => div()
                .p_3()
                .child(Alert::error("wine-list-error", message.clone()))
                .into_any_element(),
            Some(ListState::Loaded(list)) if list.is_empty() => div()
                .p_4()
                .child(
                    EmptyState::new(IconName::Wine, t("manage.wine.empty.title"))
                        .description(t("manage.wine.empty.description")),
                )
                .into_any_element(),
            Some(ListState::Loaded(list)) => {
                let visible = sources::visible_releases(list, &query, expanded);
                if visible.items.is_empty() {
                    div()
                        .p_6()
                        .text_sm()
                        .text_color(muted)
                        .child(t("manage.wine.no_match"))
                        .into_any_element()
                } else {
                    let rows: Vec<AnyElement> = visible
                        .items
                        .into_iter()
                        .map(|r| self.render_release(kind, r, !query.is_empty(), cx))
                        .collect();
                    let show_fewer = visible.hidden_count == 0
                        && expanded
                        && query.is_empty()
                        && sources::visible_releases(list, "", false).hidden_count > 0;
                    v_flex()
                        .p_3()
                        .gap_1p5()
                        .children(rows)
                        .when(visible.hidden_count > 0, |col| {
                            col.child(
                                h_flex().justify_center().pt_1().child(
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
                                h_flex().justify_center().pt_1().child(
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

        let mut source_bar = h_flex().gap_1p5().flex_wrap().items_center();
        for &q in &quick {
            source_bar = source_bar.child(choice_pill(
                eid("wine-source", q.type_str()),
                source_label(q),
                q == kind,
                cx,
                cx.listener(move |this, _, w, cx| this.select_source(q, w, cx)),
            ));
        }
        if !secondary.is_empty() {
            source_bar = source_bar.child(div().w(px(1.)).h(rems(1.1)).mx_0p5().bg(border));
            for &q in &secondary {
                source_bar = source_bar.child(choice_pill(
                    eid("wine-source", q.type_str()),
                    source_label(q),
                    q == kind,
                    cx,
                    cx.listener(move |this, _, w, cx| this.select_source(q, w, cx)),
                ));
            }
        }
        if has_more {
            source_bar = source_bar.child(self.render_more_sources_popover(
                kind,
                available,
                other_selected,
                cx,
            ));
        }

        let summary = v_flex()
            .gap_1()
            .px_3()
            .py_2p5()
            .rounded(radius)
            .bg(bg)
            .border_1()
            .border_color(border)
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(
                        div()
                            .text_xs()
                            .font_semibold()
                            .truncate()
                            .child(source_label(kind)),
                    )
                    .when_some(sources::source_badge(kind), |this, badge| {
                        this.child(badge.pill(cx))
                    }),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(muted)
                    .child(sources::source_description(kind)),
            );

        let matched_sources = sources::matching_sources(available, kind, &query);
        let header = v_flex()
            .child(
                h_flex()
                    .w_full()
                    .gap_2()
                    .flex_wrap()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_sm()
                            .font_semibold()
                            .truncate()
                            .child(t("manage.wine.pane.download")),
                    )
                    .child(
                        div()
                            .w(rems(16.))
                            .flex_shrink_0()
                            .child(Input::new(&self.search).small()),
                    ),
            )
            .child(source_bar)
            .child(summary)
            .when(!matched_sources.is_empty(), |col| {
                col.child(
                    h_flex()
                        .gap_2()
                        .flex_wrap()
                        .items_center()
                        .child(muted_xs(t("manage.wine.matching_sources"), cx))
                        .children(matched_sources.into_iter().map(|source| {
                            Button::new(eid("wine-match", source.type_str()))
                                .small()
                                .outline()
                                .label(source_label(source))
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.select_source(source, window, cx)
                                }))
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
                        h_flex()
                            .gap_2()
                            .px_3()
                            .py_2()
                            .rounded(radius)
                            .bg(warning.opacity(0.12))
                            .border_1()
                            .border_color(warning.opacity(0.30))
                            .items_center()
                            .child(Icon::new(IconName::Clock).small().text_color(warning))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .text_xs()
                                    .text_color(warning)
                                    .child(rate_limited_text(
                                        limited,
                                        irori_core::util::unix_now(),
                                    )),
                            ),
                    )
                },
            );
        pane_shell("wine-catalog-pane", "wine-catalog-scroll", header, body, cx)
    }

    fn build_counts(&self) -> (usize, usize, usize) {
        let Some(SystemState::Loaded {
            managed, detected, ..
        }) = &self.system
        else {
            return (0, 0, 0);
        };
        let det: usize = detected.iter().map(|(_, l)| l.len()).sum();
        let upd = managed
            .iter()
            .filter(|r| r.has_update == Some(true))
            .count();
        let pr = managed
            .iter()
            .filter(|r| r.release_kind().is_some_and(ReleaseKind::is_proton))
            .count();
        (managed.len() + det, upd, pr)
    }

    fn render_controls_strip(&self, irori_protons: usize, cx: &mut Context<Self>) -> AnyElement {
        let border = cx.theme().border;
        let app = models(cx).settings.read(cx).app.clone();
        let picker: AnyElement = if self.build_counts().0 > 0 {
            div()
                .w(rems(15.))
                .child(Select::new(&self.default_select).small())
                .into_any_element()
        } else {
            muted_xs(t("manage.wine.controls.default_none"), cx).into_any_element()
        };
        let mut row = h_flex()
            .w_full()
            .gap_x_6()
            .gap_y_3()
            .flex_wrap()
            .items_center()
            .child(
                h_flex()
                    .gap_2p5()
                    .items_center()
                    .child(icon_tile(IconName::Star, Some(PillTone::Accent), cx))
                    .child(
                        v_flex()
                            .gap_0p5()
                            .child(muted_xs(t("manage.wine.controls.default_label"), cx))
                            .child(picker),
                    ),
            );
        if cfg!(target_os = "linux") {
            let on = app.download_proton_to_steam;
            let meta = if on {
                t_args(
                    "manage.wine.controls.share_steam_on",
                    &[("count", &irori_protons.to_string())],
                )
            } else {
                t("manage.wine.controls.share_steam_off")
            };
            let tip = info_tip(
                "wine-steam-share",
                t("manage.wine.controls.share_steam_info"),
                cx,
            );
            row = row
                .child(div().w(px(1.)).h(rems(2.)).bg(border))
                .child(control_switch(
                    "wine-share-steam",
                    on,
                    t("manage.wine.controls.share_steam"),
                    meta,
                    Some(tip),
                    cx,
                    |v, _, cx| {
                        let v = *v;
                        backend_op(cx, move |b| {
                            b.settings
                                .update_app(move |s| s.download_proton_to_steam = v);
                        });
                    },
                ));
        }
        if !ReleaseKind::available().is_empty() {
            let on = app.auto_update_components;
            let meta = if on {
                t("manage.wine.controls.auto_update_on")
            } else {
                t("manage.wine.controls.auto_update_off")
            };
            row = row
                .child(div().w(px(1.)).h(rems(2.)).bg(border))
                .child(control_switch(
                    "wine-auto-update",
                    on,
                    t("manage.wine.controls.auto_update"),
                    meta,
                    None,
                    cx,
                    |v, _, cx| {
                        let v = *v;
                        backend_op(cx, move |b| {
                            b.settings.update_app(move |s| s.auto_update_components = v);
                        });
                    },
                ));
        }
        card(cx)
            .id("wine-controls")
            .debug_selector(|| "wine-controls".into())
            .py_3()
            .child(row)
            .into_any_element()
    }

    fn render_builds(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let (total_installed, _, irori_protons) = self.build_counts();
        let measured = self.viewport_width.get();
        let is_narrow = measured > px(0.) && (measured / window.rem_size()) < SPLIT_MIN_WIDTH_REM;
        let controls = self.render_controls_strip(irori_protons, cx);

        let workbench: AnyElement = match (self.selected_source, is_narrow) {
            (None, _) => div()
                .flex_1()
                .min_h_0()
                .w_full()
                .child(self.render_installed_pane(cx))
                .into_any_element(),
            (Some(kind), false) => h_flex()
                .flex_1()
                .min_h_0()
                .w_full()
                .gap_4()
                .items_stretch()
                .child(
                    div()
                        .w(rems(28.))
                        .min_w(rems(22.))
                        .h_full()
                        .min_h_0()
                        .flex_shrink_0()
                        .child(self.render_installed_pane(cx)),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .h_full()
                        .min_h_0()
                        .child(self.render_source(kind, cx)),
                )
                .into_any_element(),
            (Some(kind), true) => {
                let pane_el = match self.narrow_pane {
                    NarrowPane::Installed => self.render_installed_pane(cx),
                    NarrowPane::Download => self.render_source(kind, cx),
                };
                let bar = seg_group(cx)
                    .w_full()
                    .child(
                        seg_button(
                            "wine-pane-installed",
                            self.narrow_pane == NarrowPane::Installed,
                            cx,
                            cx.listener(|this, _, _, cx| {
                                this.narrow_pane = NarrowPane::Installed;
                                cx.notify();
                            }),
                        )
                        .debug_selector(|| "wine-pane-installed".into())
                        .flex_1()
                        .child(t_args(
                            "manage.wine.pane.installed_tab",
                            &[("count", &total_installed.to_string())],
                        )),
                    )
                    .child(
                        seg_button(
                            "wine-pane-download",
                            self.narrow_pane == NarrowPane::Download,
                            cx,
                            cx.listener(|this, _, _, cx| {
                                this.narrow_pane = NarrowPane::Download;
                                cx.notify();
                            }),
                        )
                        .debug_selector(|| "wine-pane-download".into())
                        .flex_1()
                        .child(t("manage.wine.pane.download")),
                    );
                v_flex()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .gap_3()
                    .child(bar)
                    .child(div().flex_1().min_h_0().w_full().child(pane_el))
                    .into_any_element()
            }
        };

        v_flex()
            .id("wine-content")
            .debug_selector(|| "wine-content".into())
            .w_full()
            .max_w(rems(PAGE_MAX_WIDTH_REM))
            .mx_auto()
            .flex_1()
            .min_h_0()
            .px_6()
            .py_4()
            .gap_4()
            .child(controls)
            .child(workbench)
            .into_any_element()
    }
}

/// Inline progress of a running install.
pub(super) fn render_op(live: &OpLive, cx: &App) -> impl IntoElement {
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
            self.installed_search.update(cx, |s, cx| {
                s.set_placeholder(t("manage.wine.installed.search"), window, cx)
            });
            self.prefix_search.update(cx, |s, cx| {
                s.set_placeholder(t("manage.wine.prefixes.search"), window, cx)
            });
        }
        let (fg, muted, border, muted_bg, warning) = {
            let theme = cx.theme();
            (
                theme.foreground,
                theme.muted_foreground,
                theme.border,
                theme.muted.opacity(0.6),
                theme.warning,
            )
        };
        let running = self.ops.read(cx).running.len();
        let (total_installed, updates_count, _) = self.build_counts();
        let prefix_counts = self.prefix_counts();

        let summary_text = [
            (total_installed > 0).then(|| {
                t_args(
                    "manage.wine.summary.installed",
                    &[("count", &total_installed.to_string())],
                )
                .to_string()
            }),
            match updates_count {
                0 => None,
                1 => Some(t("manage.wine.summary.updates_one").to_string()),
                n => Some(
                    t_args(
                        "manage.wine.summary.updates_other",
                        &[("count", &n.to_string())],
                    )
                    .to_string(),
                ),
            },
            prefix_counts.map(|(n, _)| {
                t_args("manage.wine.summary.prefixes", &[("count", &n.to_string())]).to_string()
            }),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" · ");

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

        let badge = |n: usize| {
            div()
                .px_1p5()
                .rounded_full()
                .bg(muted_bg)
                .text_xs()
                .text_color(muted)
                .child(n.to_string())
        };
        let switcher = seg_group(cx)
            .id("wine-tabs")
            .child(
                seg_button(
                    "wine-tab-builds",
                    self.active_tab == WineTab::Builds,
                    cx,
                    cx.listener(|this, _, w, cx| this.select_tab(WineTab::Builds, w, cx)),
                )
                .debug_selector(|| "wine-tab-builds".into())
                .child(t("manage.wine.tab.builds"))
                .when(total_installed > 0, |b| b.child(badge(total_installed))),
            )
            .child(
                seg_button(
                    "wine-tab-prefixes",
                    self.active_tab == WineTab::Prefixes,
                    cx,
                    cx.listener(|this, _, w, cx| this.select_tab(WineTab::Prefixes, w, cx)),
                )
                .debug_selector(|| "wine-tab-prefixes".into())
                .child(t("manage.wine.tab.prefixes"))
                .when_some(prefix_counts, |b, (total, unused)| {
                    b.child(badge(total)).when(unused > 0, |b| {
                        b.child(div().w_1p5().h_1p5().rounded_full().bg(warning))
                    })
                }),
            );

        let body = match self.active_tab {
            WineTab::Builds => self.render_builds(window, cx),
            WineTab::Prefixes => self.render_prefixes(cx),
        };
        let width_cell = self.viewport_width.clone();
        let measure = canvas(
            move |bounds, window, _| {
                if width_cell.replace(bounds.size.width) != bounds.size.width {
                    window.refresh();
                }
            },
            |_, _, _, _| {},
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full();

        v_flex()
            .relative()
            .size_full()
            .min_h_0()
            .child(measure)
            .child(
                v_flex()
                    .id("wine-header")
                    .debug_selector(|| "wine-header".into())
                    .w_full()
                    .max_w(rems(PAGE_MAX_WIDTH_REM))
                    .mx_auto()
                    .px_6()
                    .pt_6()
                    .pb_4()
                    .border_b_1()
                    .border_color(border)
                    .child(
                        h_flex()
                            .w_full()
                            .gap_4()
                            .flex_wrap()
                            .items_start()
                            .justify_between()
                            .child(
                                v_flex()
                                    .flex_1()
                                    .min_w_0()
                                    .gap_1()
                                    .child(
                                        h_flex()
                                            .gap_2p5()
                                            .flex_wrap()
                                            .items_center()
                                            .child(
                                                div()
                                                    .text_2xl()
                                                    .font_bold()
                                                    .text_color(fg)
                                                    .child(t("manage.wine.title")),
                                            )
                                            .when(!summary_text.is_empty(), |r| {
                                                r.child(pill(
                                                    summary_text.into(),
                                                    PillTone::Muted,
                                                    false,
                                                    cx,
                                                ))
                                            }),
                                    )
                                    .child(div().text_sm().text_color(muted).child(description)),
                            )
                            .child(
                                h_flex()
                                    .gap_2p5()
                                    .flex_shrink_0()
                                    .items_center()
                                    .child(switcher)
                                    .child(
                                        Button::new("refresh-wine")
                                            .outline()
                                            .small()
                                            .icon(IconName::RefreshCw)
                                            .label(t("manage.wine.refresh"))
                                            .loading(self.refreshing)
                                            .disabled(self.refreshing)
                                            .on_click(cx.listener(|this, _, window, cx| {
                                                this.refresh(window, cx)
                                            })),
                                    ),
                            ),
                    ),
            )
            .child(body)
    }
}
