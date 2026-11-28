//! The Emulators page ([`Route::Emulators`](irori_ui::Route::Emulators)):
//! every emulator Irori knows with the installs found on this computer,
//! one-click installs and updates of the versions Irori manages, RetroArch
//! cores, each system's default emulator, the RetroAchievements account
//! ([`retro`]), starting an emulator without a game ([`open`]), and the
//! usual ROM folders found on this computer ([`found`]). Each emulator's
//! card says how it stands: a dot (ready, a program gone, not installed),
//! its mark where Irori has one (`StoreLogo::of_engine`), how many games
//! play with it and which program is gone.

mod configure;
mod cores;
mod custom;
mod found;
mod open;
mod ops;
mod retro;
mod setup;

use std::cell::Cell;
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::assets::IconName;
use gpui_kit::component::Icon;
use gpui_kit::component::IndexPath;
use gpui_kit::component::button::{Button, ButtonVariant, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::progress::Progress;
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::select::{Select, SelectEvent, SelectItem, SelectState};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::tag::Tag;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Sizable as _, StyledExt as _, WindowExt as _, h_flex,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, ElementId, Entity, InteractiveElement as _,
    IntoElement, ParentElement as _, PathPromptOptions, Pixels, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, Subscription, Task, Window, div, px, rems,
};
use irori_engines::bios::{FileState, SystemBios, SystemState};
use irori_engines::releases::{self, Release};
use irori_engines::{
    EngineKind, EngineManifest, EngineRef, Host, Installation, Origin, cheevos, systems,
};
use irori_services::EngineStatus;
use irori_services::engine_health::EngineHealth;
use irori_ui::components::{EmptyState, PageHeader, SectionTitle};
use irori_ui::i18n::{t, t_args};
use irori_ui::{backend, models};

use crate::common::{card, card_grid_natural_rows, info_tip, notify_error, plain_tooltip};
use crate::format;
pub use configure::open_engine_settings;
pub use custom::{confirm_remove_custom_engine, open_custom_engine_dialog};
pub use found::open_rom_sources_dialog;
pub use open::{OpenableEngine, open_engine, openable_engines};
use ops::{EngineOps, OpLive, cancel_install, core_key, engine_key, engine_ops};
pub use ops::{InstallTarget, start_install};
pub use setup::open_emulation_setup;

/// Translation keys of the built-in emulators' options.
#[cfg(test)]
pub(crate) fn option_keys() -> Vec<String> {
    configure::required_keys()
}

/// Repaint interval while installs report progress.
pub(super) const PROGRESS_TICK: Duration = Duration::from_millis(300);

/// Comfortable maximum width of the Emulators and Systems content, in rem.
const PAGE_MAX_WIDTH_REM: f32 = 80.;
/// Narrowest an installed emulator card gets before dropping a column, in rem.
const INSTALLED_CARD_REM: f32 = 25.;
/// Narrowest an available or manual emulator card gets before dropping a column, in rem.
const COMPACT_CARD_REM: f32 = 20.;
/// Narrowest a system card gets before dropping a column, in rem.
const SYSTEM_CARD_REM: f32 = 20.;
/// Maximum system chips shown inline on a standalone emulator card before "+N".
const MAX_SYSTEM_CHIPS: usize = 4;
/// Maximum system chips shown inline on the featured RetroArch hub card.
const MAX_HERO_SYSTEM_CHIPS: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EngineFilter {
    All,
    Installed,
    Available,
    Manual,
    Updates,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EngineSection {
    Installed,
    Available,
    Manual,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SystemFilter {
    All,
    WithGames,
    NeedsAttention,
}

/// Classify an emulator into Installed, Available (auto-downloadable and
/// non-experimental), or Manual (manual install, experimental, or missing
/// custom binary).
pub(crate) fn classify_engine(status: &EngineStatus, host: Host) -> EngineSection {
    if status.is_installed() {
        EngineSection::Installed
    } else if status.manifest.downloadable_on(host) && !status.manifest.experimental {
        EngineSection::Available
    } else {
        EngineSection::Manual
    }
}

/// Only systems with games in the library raise missing-emulator or BIOS alerts.
pub(crate) fn system_needs_attention(game_count: usize, ready: bool, bios_warn: bool) -> bool {
    game_count > 0 && (!ready || bios_warn)
}

/// Fallback icon for an emulator that does not ship a brand SVG.
pub(crate) fn engine_category_icon(manifest: &EngineManifest) -> IconName {
    match manifest.id.as_str() {
        "dosbox-staging" | "scummvm" => IconName::SquareTerminal,
        "mame" => IconName::Joystick,
        "ppsspp" | "mgba" | "melonds" | "desmume" | "azahar" | "vita3k" => {
            IconName::GamepadDirectional
        }
        _ => IconName::Gamepad2,
    }
}

pub(super) fn muted_xs(text: impl Into<SharedString>, cx: &App) -> gpui_kit::Div {
    div()
        .text_xs()
        .text_color(cx.theme().muted_foreground)
        .child(text.into())
}

pub(super) fn row_between() -> gpui_kit::Div {
    h_flex()
        .w_full()
        .gap_3()
        .flex_wrap()
        .items_center()
        .justify_between()
}

pub(super) fn install_engine_target(id: String) -> InstallTarget {
    InstallTarget::Engine {
        id,
        release: None,
        make_default: true,
    }
}

fn filter_pill(
    id: &'static str,
    label: SharedString,
    count: usize,
    active: bool,
    cx: &App,
    on_click: impl Fn(&gpui_kit::ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    let t = cx.theme();
    let (bg, fg) = if active {
        (t.primary, t.primary_foreground)
    } else {
        (t.secondary, t.muted_foreground)
    };
    let badge = div()
        .px_1()
        .rounded(t.radius)
        .text_xs()
        .font_semibold()
        .bg(bg)
        .text_color(fg)
        .child(count.to_string());
    Button::new(id)
        .small()
        .map(|b| {
            if active {
                b.secondary().border_color(t.primary)
            } else {
                b.outline()
            }
        })
        .child(h_flex().gap_1p5().items_center().child(label).child(badge))
        .on_click(on_click)
        .into_any_element()
}

fn tab_toolbar(pills: gpui_kit::Div, search: &Entity<InputState>) -> gpui_kit::Div {
    row_between().child(pills).child(
        div()
            .w(rems(17.))
            .flex_shrink_0()
            .debug_selector(|| "emulators-search".into())
            .child(Input::new(search).small()),
    )
}

fn collapse_button(
    id: &'static str,
    expanded: bool,
    count: usize,
    on_click: impl Fn(&gpui_kit::ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    let (icon, label) = if expanded {
        (
            IconName::ChevronUp,
            t("manage.emulators.systems.hide_others"),
        )
    } else {
        (
            IconName::ChevronDown,
            t_args(
                "manage.emulators.systems.show_others",
                &[("count", &count.to_string())],
            ),
        )
    };
    Button::new(id)
        .ghost()
        .small()
        .icon(icon)
        .label(label)
        .on_click(on_click)
        .into_any_element()
}

fn inset_row(cx: &App) -> gpui_kit::Div {
    let t = cx.theme();
    h_flex()
        .gap_2()
        .px_2p5()
        .py_1p5()
        .rounded(t.radius)
        .bg(t.background)
        .border_1()
        .border_color(t.border)
        .items_center()
}

fn system_monogram(short: &str, fixed_width: bool, cx: &App) -> gpui_kit::Div {
    let t = cx.theme();
    div()
        .when(fixed_width, |d| {
            d.w(rems(3.25)).py_0p5().flex().justify_center()
        })
        .when(!fixed_width, |d| d.px_2().py_1())
        .rounded(t.radius)
        .bg(t.secondary)
        .border_1()
        .border_color(t.border)
        .text_xs()
        .font_bold()
        .flex_shrink_0()
        .child(short.to_string())
}

/// One engine choice in a system's default picker.
#[derive(Debug, Clone, PartialEq)]
struct EngineChoice {
    value: SharedString,
    title: SharedString,
}

impl SelectItem for EngineChoice {
    type Value = SharedString;
    fn title(&self) -> SharedString {
        self.title.clone()
    }
    fn value(&self) -> &SharedString {
        &self.value
    }
}

/// The display name of an engine choice: the engine, or "RetroArch · Core".
pub fn choice_name(choice: &EngineRef) -> String {
    let Some(m) = irori_engines::engine(&choice.engine) else {
        return choice.to_string();
    };
    match choice.core.as_deref() {
        Some(core) => format!(
            "{} · {}",
            m.name,
            m.core(core).map_or(core, |c| c.name.as_str())
        ),
        None => m.name.clone(),
    }
}

/// Every emulator with its installs, and each system's default emulator.
pub struct EmulatorsView {
    tab_ix: usize,
    statuses: Option<Vec<EngineStatus>>,
    /// Each emulator's standing by engine id: its games, programs gone.
    health: HashMap<String, EngineHealth>,
    /// BIOS status by system id.
    bios: HashMap<String, SystemBios>,
    /// Emulated games in the library by system id.
    counts: BTreeMap<String, usize>,
    /// A ROM or BIOS scan started here is running.
    scanning: bool,
    /// Downloadable versions per engine (the cached list first).
    releases: HashMap<String, Result<Vec<Release>, SharedString>>,
    refreshing: bool,
    engine_filter: EngineFilter,
    system_filter: SystemFilter,
    show_manual: bool,
    show_other_systems: bool,
    installed_grid_width: Rc<Cell<Pixels>>,
    available_grid_width: Rc<Cell<Pixels>>,
    manual_grid_width: Rc<Cell<Pixels>>,
    folders_grid_width: Rc<Cell<Pixels>>,
    library_systems_grid_width: Rc<Cell<Pixels>>,
    search: Entity<InputState>,
    system_selects: HashMap<String, Entity<SelectState<Vec<EngineChoice>>>>,
    ops: Entity<EngineOps>,
    retro: Entity<retro::RetroView>,
    seen_revision: u64,
    ticking: bool,
    placeholder_language: String,
    placeholder_tab: usize,
    _ticker: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl EmulatorsView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        crate::i18n::register();
        let search =
            cx.new(|cx| InputState::new(window, cx).placeholder(t("manage.emulators.search")));
        let ops = engine_ops(cx);
        let settings = models(cx).settings.clone();
        let openable = open::openable(cx);
        let subscriptions = vec![
            cx.subscribe(&search, |_, _, event, cx| {
                if matches!(event, InputEvent::Change) {
                    cx.notify();
                }
            }),
            cx.observe_in(&ops, window, |this, ops, window, cx| {
                let revision = ops.read(cx).revision;
                if revision != this.seen_revision {
                    this.seen_revision = revision;
                    this.load(window, cx);
                }
                this.ensure_ticker(window, cx);
                cx.notify();
            }),
            // Engine changes (installs elsewhere, the default per system)
            // arrive as settings notifications.
            cx.observe_in(&settings, window, |this, _, window, cx| {
                this.load(window, cx)
            }),
            cx.observe(&models(cx).library.clone(), |this, _, cx| {
                this.counts = backend(cx).emulated_counts();
                cx.notify();
            }),
            cx.observe(&openable, |_, _, cx| cx.notify()),
        ];
        let seen_revision = ops.read(cx).revision;
        let mut this = Self {
            retro: cx.new(|cx| retro::RetroView::new(window, cx)),
            tab_ix: 0,
            statuses: None,
            health: HashMap::new(),
            bios: HashMap::new(),
            counts: BTreeMap::new(),
            scanning: false,
            releases: HashMap::new(),
            refreshing: false,
            engine_filter: EngineFilter::All,
            system_filter: SystemFilter::All,
            show_manual: false,
            show_other_systems: false,
            installed_grid_width: Rc::new(Cell::new(px(0.))),
            available_grid_width: Rc::new(Cell::new(px(0.))),
            manual_grid_width: Rc::new(Cell::new(px(0.))),
            folders_grid_width: Rc::new(Cell::new(px(0.))),
            library_systems_grid_width: Rc::new(Cell::new(px(0.))),
            search,
            system_selects: HashMap::new(),
            ops,
            seen_revision,
            ticking: false,
            placeholder_language: irori_ui::i18n::language(),
            placeholder_tab: 0,
            _ticker: None,
            _subscriptions: subscriptions,
        };
        this.load(window, cx);
        this.load_releases(false, window, cx);
        this.ensure_ticker(window, cx);
        this
    }

    /// Re-read installs from disk.
    fn load(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let b = backend(cx);
        self.counts = b.emulated_counts();
        let task = irori_ui::run(cx, async move {
            let statuses = b.engine_statuses().await;
            let health = b.engine_health(statuses.clone()).await;
            Ok((statuses, health, b.bios_statuses().await))
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok((statuses, health, bios)) = task.await else {
                return;
            };
            let _ = this.update_in(cx, |this, window, cx| {
                open::remember(&statuses, cx);
                this.statuses = Some(statuses);
                this.health = health;
                this.bios = bios.into_iter().map(|b| (b.system.clone(), b)).collect();
                this.sync_system_selects(window, cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// Version lists of every downloadable engine; `refresh` asks the
    /// projects even when the saved lists are fresh.
    fn load_releases(&mut self, refresh: bool, window: &mut Window, cx: &mut Context<Self>) {
        let host = Host::current();
        let ids: Vec<String> = irori_engines::engines()
            .iter()
            .filter(|m| m.downloadable_on(host))
            .map(|m| m.id.clone())
            .collect();
        if refresh {
            self.refreshing = true;
        }
        let b = backend(cx);
        let task = irori_ui::run(cx, async move {
            let mut out = Vec::new();
            for id in ids {
                let r = b.engine_releases(&id, refresh).await;
                out.push((id, r.map_err(|e| SharedString::from(format!("{e:#}")))));
            }
            Ok(out)
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(lists) = task.await else { return };
            let _ = this.update_in(cx, |this, _, cx| {
                this.refreshing = false;
                for (id, list) in lists {
                    this.releases.insert(id, list);
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn ensure_ticker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.ticking || self.ops.read(cx).running.is_empty() {
            return;
        }
        self.ticking = true;
        self._ticker = Some(cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor().timer(PROGRESS_TICK).await;
                let Ok(running) = this.update(cx, |this, cx| {
                    cx.notify();
                    !this.ops.read(cx).running.is_empty()
                }) else {
                    return;
                };
                if !running {
                    let _ = this.update(cx, |this, _| this.ticking = false);
                    return;
                }
            }
        }));
    }

    /// One select per system with the emulators that play it.
    fn sync_system_selects(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(statuses) = self.statuses.clone() else {
            return;
        };
        let b = backend(cx);
        for system in systems::all() {
            let choices = irori_engines::choices_for(&system.id);
            let current = b.system_engine(&system.id, &statuses);
            let recommended = choices.first().cloned();
            let items: Vec<EngineChoice> = choices
                .iter()
                .map(|c| {
                    let name = choice_name(c);
                    let title = if Some(c) == recommended.as_ref() {
                        t_args("manage.emulators.system.recommended", &[("engine", &name)])
                    } else {
                        name.into()
                    };
                    EngineChoice {
                        value: c.to_string().into(),
                        title,
                    }
                })
                .collect();
            let ix = current
                .as_ref()
                .and_then(|c| choices.iter().position(|x| x == c))
                .map(|row| IndexPath::default().row(row));
            match self.system_selects.get(&system.id) {
                Some(select) => select.update(cx, |s, cx| {
                    s.set_items(items, window, cx);
                    s.set_selected_index(ix, window, cx);
                }),
                None => {
                    let select = cx.new(|cx| SelectState::new(items, ix, window, cx));
                    let id = system.id.clone();
                    self._subscriptions.push(cx.subscribe_in(
                        &select,
                        window,
                        move |_, _, e: &SelectEvent<Vec<EngineChoice>>, _, cx| {
                            let SelectEvent::Confirm(Some(value)) = e else {
                                return;
                            };
                            let choice = EngineRef::parse(value);
                            let (b, id) = (backend(cx), id.clone());
                            drop(
                                b.clone().spawn_blocking(move || {
                                    b.set_system_engine(&id, choice.as_ref())
                                }),
                            );
                        },
                    ));
                    self.system_selects.insert(system.id.clone(), select);
                }
            }
        }
    }

    fn query(&self, cx: &App) -> String {
        self.search.read(cx).value().trim().to_lowercase()
    }

    fn pick_program(&mut self, engine: String, window: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: cfg!(target_os = "macos"),
            multiple: false,
            prompt: Some(t("manage.emulators.use_program")),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            let _ = this.update_in(cx, |this, window, cx| {
                this.set_program(engine, Some(path), window, cx)
            });
        })
        .detach();
    }

    fn set_program(
        &mut self,
        engine: String,
        path: Option<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let b = backend(cx);
        let id = engine.clone();
        let task = irori_ui::run(cx, async move {
            b.set_engine_program(&id, path.clone()).await?;
            if path.is_some() {
                b.set_engine_default(&id, "custom").await?;
            }
            Ok(())
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                if let Err(e) = result {
                    notify_error(t("manage.emulators.program_failed"), &e, window, cx);
                }
                this.load(window, cx);
            });
        })
        .detach();
    }

    fn make_default(
        &mut self,
        engine: String,
        installation: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let b = backend(cx);
        let task = irori_ui::run(cx, async move {
            b.set_engine_default(&engine, &installation).await
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                if let Err(e) = result {
                    notify_error(t("manage.emulators.default_failed"), &e, window, cx);
                }
                this.load(window, cx);
            });
        })
        .detach();
    }

    fn confirm_remove(
        &mut self,
        engine: String,
        name: String,
        version: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let view = cx.entity().downgrade();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let (view, engine, name, version) =
                (view.clone(), engine.clone(), name.clone(), version.clone());
            alert
                .confirm()
                .title(t_args(
                    "manage.emulators.remove_title",
                    &[("engine", &name), ("version", &version)],
                ))
                .description(t("manage.emulators.remove_message"))
                .ok_text(t("manage.emulators.remove_confirm"))
                .ok_variant(ButtonVariant::Danger)
                .on_ok(move |_, window, cx| {
                    let (engine, name, version) = (engine.clone(), name.clone(), version.clone());
                    let _ = view.update(cx, |this, cx| {
                        this.remove(engine, name, version, window, cx)
                    });
                    true
                })
        });
    }

    fn remove(
        &mut self,
        engine: String,
        name: String,
        version: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let b = backend(cx);
        let v = version.clone();
        let task = irori_ui::run(
            cx,
            async move { b.remove_engine_version(&engine, &v).await },
        );
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                if let Err(e) = result {
                    notify_error(
                        t_args(
                            "manage.emulators.remove_failed",
                            &[("engine", &name), ("version", &version)],
                        ),
                        &e,
                        window,
                        cx,
                    );
                }
                this.load(window, cx);
            });
        })
        .detach();
    }

    fn pick_folder(&mut self, bios: bool, window: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some(if bios {
                t("manage.emulators.bios.choose")
            } else {
                t("manage.emulators.folders.add")
            }),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            let _ = this.update_in(cx, |this, window, cx| {
                let b = backend(cx);
                this.run_scan(window, cx, async move {
                    if bios {
                        b.set_bios_folder(path).await.map(|_| ())
                    } else {
                        b.add_rom_folder(path).await
                    }
                })
            });
        })
        .detach();
    }

    /// Run a folder change or rescan, showing it as scanning.
    fn run_scan(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        work: impl std::future::Future<Output = anyhow::Result<()>> + Send + 'static,
    ) {
        self.scanning = true;
        cx.notify();
        let task = irori_ui::run(cx, work);
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.scanning = false;
                if let Err(e) = result {
                    notify_error(t("manage.emulators.folders.scan_failed"), &e, window, cx);
                }
                this.load(window, cx);
            });
        })
        .detach();
    }

    fn rescan(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let b = backend(cx);
        let bios = b.settings.app().bios_folder.clone();
        self.run_scan(window, cx, async move {
            b.refresh_store(irori_core::Runner::Emulation, false)
                .await?;
            if !bios.trim().is_empty() {
                b.rescan_bios(irori_core::paths::expand_home(&bios)).await?;
            }
            Ok(())
        });
    }

    fn render_folders(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let app = models(cx).settings.read(cx).app.clone();
        let has_folders = !app.rom_folders.is_empty();
        let folders: Vec<AnyElement> = app
            .rom_folders
            .iter()
            .enumerate()
            .map(|(ix, folder)| {
                let remove = folder.clone();
                inset_row(cx)
                    .id(ElementId::Name(format!("rom-folder-{ix}").into()))
                    .child(Icon::new(IconName::Folder).small().text_color(muted))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_xs()
                            .truncate()
                            .child(folder.clone()),
                    )
                    .child(
                        Button::new("remove-folder")
                            .ghost()
                            .xsmall()
                            .label(t("manage.emulators.folders.remove"))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                let (b, folder) = (backend(cx), remove.clone());
                                this.run_scan(window, cx, async move {
                                    b.remove_rom_folder(&folder).await
                                });
                            })),
                    )
                    .into_any_element()
            })
            .collect();
        let bios = app.bios_folder.trim().to_string();
        let req_bios: Vec<&SystemBios> = self
            .counts
            .keys()
            .filter_map(|id| self.bios.get(id))
            .filter(|b| !b.files.is_empty() && b.state != SystemState::Optional)
            .collect();
        let bios_ready = req_bios
            .iter()
            .filter(|b| b.state == SystemState::Ready)
            .count();
        let folders_card = card(cx)
            .child(
                SectionTitle::new(t("manage.emulators.folders.title")).trailing(
                    h_flex()
                        .gap_1p5()
                        .flex_wrap()
                        .child(
                            Button::new("find-roms")
                                .outline()
                                .small()
                                .icon(IconName::FolderSearch)
                                .label(t("manage.emulators.systems.find_roms"))
                                .disabled(self.scanning)
                                .on_click(|_, window, cx| {
                                    found::open_rom_sources_dialog(window, cx)
                                }),
                        )
                        .child(
                            Button::new("add-folder")
                                .outline()
                                .small()
                                .icon(IconName::FolderPlus)
                                .label(t("manage.emulators.folders.add"))
                                .disabled(self.scanning)
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.pick_folder(false, window, cx)
                                })),
                        )
                        .when(has_folders, |row| {
                            row.child(
                                Button::new("rescan")
                                    .ghost()
                                    .small()
                                    .icon(IconName::RefreshCw)
                                    .tooltip(t("manage.emulators.folders.rescan"))
                                    .accessibility_label(t("manage.emulators.folders.rescan"))
                                    .loading(self.scanning)
                                    .disabled(self.scanning)
                                    .on_click(
                                        cx.listener(|this, _, window, cx| this.rescan(window, cx)),
                                    ),
                            )
                        }),
                ),
            )
            .when(folders.is_empty(), |c| {
                c.child(muted_xs(t("manage.emulators.folders.description"), cx))
            })
            .children(folders)
            .when(self.scanning, |c| {
                c.child(muted_xs(t("manage.emulators.folders.scanning"), cx))
            })
            .into_any_element();
        let bios_card = card(cx)
            .child(
                SectionTitle::new(t("manage.emulators.bios.title")).trailing(
                    Button::new("choose-bios")
                        .outline()
                        .small()
                        .icon(IconName::FolderOpen)
                        .label(t("manage.emulators.bios.choose"))
                        .on_click(
                            cx.listener(|this, _, window, cx| this.pick_folder(true, window, cx)),
                        ),
                ),
            )
            .when(bios.is_empty(), |c| {
                c.child(muted_xs(t("manage.emulators.bios.description"), cx))
            })
            .when(!bios.is_empty(), |c| {
                c.child(
                    inset_row(cx)
                        .child(Icon::new(IconName::Cpu).small().text_color(muted))
                        .child(div().flex_1().min_w_0().text_xs().truncate().child(bios))
                        .when(!req_bios.is_empty(), |row| {
                            let label = t_args(
                                "manage.emulators.bios.count_ready",
                                &[
                                    ("ready", &bios_ready.to_string()),
                                    ("total", &req_bios.len().to_string()),
                                ],
                            );
                            let tag = if bios_ready == req_bios.len() {
                                Tag::success()
                            } else {
                                Tag::warning()
                            };
                            row.child(tag.xsmall().child(label))
                        }),
                )
            })
            .into_any_element();
        card_grid_natural_rows(
            &self.folders_grid_width,
            24.,
            2,
            vec![folders_card, bios_card],
            window,
        )
        .into_any_element()
    }

    fn needed_games_for_engine(&self, engine_id: &str, cx: &App) -> usize {
        let Some(statuses) = &self.statuses else {
            return 0;
        };
        let b = backend(cx);
        self.counts
            .iter()
            .filter(|(sys, n)| {
                **n > 0
                    && b.system_engine(sys, statuses)
                        .is_some_and(|c| c.engine == engine_id)
            })
            .map(|(_, n)| *n)
            .sum()
    }

    fn latest_update(&self, status: &EngineStatus) -> Option<Release> {
        let newest = match self.releases.get(&status.manifest.id) {
            Some(Ok(list)) => list.first().cloned(),
            _ => None,
        };
        newest.filter(|r| {
            status.newest_managed().is_some_and(|have| {
                releases::compare_versions(&r.version, have) == std::cmp::Ordering::Greater
            })
        })
    }

    fn render_system_chips(&self, manifest: &EngineManifest, max: usize, cx: &App) -> AnyElement {
        let theme = cx.theme();
        let (muted, bg, border, radius) = (
            theme.muted_foreground,
            theme.background,
            theme.border,
            theme.radius,
        );
        let short_names: Vec<String> = manifest
            .systems
            .iter()
            .filter_map(|s| systems::get(s).map(|sys| sys.short.clone()))
            .collect();
        let full_names: Vec<String> = manifest
            .systems
            .iter()
            .filter_map(|s| systems::get(s).map(|sys| sys.name.clone()))
            .collect();
        let shown = short_names.iter().take(max).cloned();
        let overflow = short_names.len().saturating_sub(max);
        let all_text = t_args(
            "manage.emulators.plays",
            &[("systems", &full_names.join(", "))],
        );
        h_flex()
            .flex_wrap()
            .gap_1()
            .children(shown.map(|label| {
                div()
                    .px_1p5()
                    .py_0p5()
                    .rounded(radius)
                    .bg(bg)
                    .border_1()
                    .border_color(border)
                    .text_xs()
                    .text_color(muted)
                    .child(label)
            }))
            .when(overflow > 0, |row| {
                let id = ElementId::Name(format!("systems-more-{}", manifest.id).into());
                row.child(
                    div()
                        .id(id)
                        .px_1p5()
                        .py_0p5()
                        .rounded(radius)
                        .bg(bg)
                        .border_1()
                        .border_color(border)
                        .text_xs()
                        .text_color(muted)
                        .tooltip(move |window, cx| plain_tooltip(all_text.clone(), window, cx))
                        .child(format!("+{overflow}")),
                )
            })
            .into_any_element()
    }

    fn render_engine(
        &self,
        status: &EngineStatus,
        featured: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let m = &status.manifest;
        let opening = open::openable(cx).read(cx).opening.contains(&m.id);
        let (foreground, muted, border, radius, secondary) = {
            let theme = cx.theme();
            (
                theme.foreground,
                theme.muted_foreground,
                theme.border,
                theme.radius,
                theme.secondary,
            )
        };
        let host = Host::current();
        let key = engine_key(&m.id);
        let op = self.ops.read(cx).running.get(&key).map(|o| o.snapshot());
        let newest = match self.releases.get(&m.id) {
            Some(Ok(list)) => list.first().cloned(),
            _ => None,
        };
        let update = self.latest_update(status);
        let needed_games = self.needed_games_for_engine(&m.id, cx);
        let supports_cheevos = cheevos::SUPPORTED.contains(&m.id.as_str());

        let primary: AnyElement = if op.is_some() {
            let k = key.clone();
            Button::new("cancel")
                .ghost()
                .small()
                .label(t("manage.emulators.cancel"))
                .on_click(move |_, _, cx| cancel_install(&k, cx))
                .into_any_element()
        } else if let Some(release) = update.clone() {
            let (id, label) = (
                m.id.clone(),
                t_args("manage.emulators.update", &[("version", &release.version)]),
            );
            Button::new("update")
                .primary()
                .small()
                .icon(IconName::RefreshCw)
                .label(label)
                .on_click(move |_, _, cx| {
                    start_install(
                        InstallTarget::Engine {
                            id: id.clone(),
                            release: Some(release.clone()),
                            make_default: true,
                        },
                        cx,
                    )
                })
                .into_any_element()
        } else if status.is_installed() && irori_engines::open::can_open(m) {
            let (id, name) = (m.id.clone(), SharedString::from(m.name.clone()));
            Button::new("open-engine")
                .secondary()
                .small()
                .icon(IconName::Play)
                .label(t("manage.emulators.open.button"))
                .tooltip(t_args(
                    "manage.emulators.open.tooltip",
                    &[("engine", &name)],
                ))
                .loading(opening)
                .on_click(move |_, window, cx| open_engine(id.clone(), name.clone(), window, cx))
                .into_any_element()
        } else if !status.is_installed() && m.downloadable_on(host) {
            let id = m.id.clone();
            Button::new("install")
                .map(|b| {
                    if needed_games > 0 {
                        b.primary()
                    } else {
                        b.outline()
                    }
                })
                .small()
                .icon(IconName::Download)
                .label(t("manage.emulators.install"))
                .on_click(move |_, _, cx| start_install(install_engine_target(id.clone()), cx))
                .into_any_element()
        } else if !status.is_installed() && !m.custom {
            let id = m.id.clone();
            Button::new("use-program")
                .outline()
                .small()
                .icon(IconName::FolderOpen)
                .label(t("manage.emulators.use_program"))
                .on_click(
                    cx.listener(move |this, _, window, cx| {
                        this.pick_program(id.clone(), window, cx)
                    }),
                )
                .into_any_element()
        } else if !status.is_installed() && m.custom {
            let existing = custom_definition(status);
            Button::new("edit-custom")
                .outline()
                .small()
                .icon(IconName::Pencil)
                .label(t("manage.emulators.custom.edit"))
                .on_click(move |_, window, cx| {
                    open_custom_engine_dialog(Some(existing.clone()), window, cx)
                })
                .into_any_element()
        } else {
            div().into_any_element()
        };

        let installs: Vec<AnyElement> = if status.installations.len() > 1 {
            status
                .installations
                .iter()
                .enumerate()
                .map(|(ix, i)| self.render_installation(ix, status, i, cx))
                .collect()
        } else {
            Vec::new()
        };
        let single = (status.installations.len() == 1).then(|| &status.installations[0]);
        let single_folder = single.and_then(|i| {
            i.dir
                .clone()
                .or_else(|| (i.origin != Origin::Flatpak).then(|| i.program.clone()))
        });
        let single_managed_ver = single
            .filter(|i| i.origin == Origin::Managed)
            .map(|i| i.version.clone().unwrap_or_default());
        let single_custom = single.is_some_and(|i| i.origin == Origin::Custom && !m.custom);
        let health = self.health.get(&m.id);
        let release_error = match self.releases.get(&m.id) {
            Some(Err(e)) if !status.is_installed() => Some(e.clone()),
            _ => None,
        };
        let info_text = [
            (!m.exit_hotkey.is_empty()).then(|| {
                t_args("manage.emulators.exit_hotkey", &[("keys", &m.exit_hotkey)]).to_string()
            }),
            (!m.license.is_empty()).then(|| {
                t_args("manage.emulators.license", &[("license", &m.license)]).to_string()
            }),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join("\n");
        let newest_size = newest.as_ref().and_then(|r| r.size).and_then(format::size);
        let subtitle = if status.is_installed() {
            single_install_meta(status).unwrap_or_else(|| system_summary(m))
        } else if featured {
            newest_size.unwrap_or_default()
        } else {
            match newest_size {
                Some(size) => format!("{} · {size}", system_summary(m)),
                None => system_summary(m),
            }
        };

        let avatar_icon = irori_ui::components::StoreLogo::of_engine(&m.id)
            .map(|logo| logo.icon().text_color(foreground))
            .unwrap_or_else(|| Icon::new(engine_category_icon(m)).small().text_color(muted));
        let show_dot = status.is_installed() || health.is_some_and(|h| !h.missing.is_empty());
        let is_retroarch = m.kind == EngineKind::Retroarch;
        let has_options = !m.options.is_empty();
        let can_pick_program = !m.custom && (status.is_installed() || m.downloadable_on(host));
        let has_overflow = (!status.is_installed() && has_options)
            || is_retroarch
            || single_folder.is_some()
            || single_managed_ver.is_some()
            || single_custom
            || can_pick_program
            || !m.homepage.is_empty()
            || m.custom;
        let sel_id = m.id.clone();
        let view = cx.entity().downgrade();
        let engine_id = m.id.clone();
        let engine_name = m.name.clone();
        let homepage = m.homepage.clone();
        let existing_custom = m.custom.then(|| custom_definition(status));

        let cores_btn = is_retroarch.then(|| {
            let inst = m.cores.iter().filter(|c| status.has_core(&c.id)).count();
            let busy = m
                .cores
                .iter()
                .any(|c| self.ops.read(cx).running.contains_key(&core_key(&c.id)));
            let (i_s, t_s) = (inst.to_string(), m.cores.len().to_string());
            let pairs = [("installed", i_s.as_str()), ("total", t_s.as_str())];
            Button::new("manage-retroarch-cores")
                .debug_selector(|| "manage-retroarch-cores".into())
                .small()
                .outline()
                .icon(IconName::Package)
                .loading(busy)
                .tooltip(t_args("manage.emulators.cores.title", &pairs))
                .label(t_args("manage.emulators.cores.button", &pairs))
                .on_click(|_, window, cx| cores::open_cores_dialog(window, cx))
        });

        let mut header = Some(
            h_flex()
                .gap_2p5()
                .items_start()
                .when(featured, |h| h.flex_1().min_w(rems(18.)))
                .child(
                    div()
                        .size(rems(if featured { 2.5 } else { 2. }))
                        .flex_shrink_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(radius)
                        .bg(secondary)
                        .border_1()
                        .border_color(border)
                        .child(avatar_icon),
                )
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .gap_1()
                        .child(
                            h_flex()
                                .gap_1p5()
                                .items_center()
                                .flex_wrap()
                                .when(show_dot, |row| row.child(health_dot(status, health, cx)))
                                .child(
                                    div()
                                        .map(|d| if featured { d.text_base() } else { d.text_sm() })
                                        .font_semibold()
                                        .truncate()
                                        .child(m.name.clone()),
                                )
                                .when(featured, |d| {
                                    d.child(
                                        Tag::info()
                                            .xsmall()
                                            .child(t("manage.emulators.retroarch.hub")),
                                    )
                                })
                                .when(update.is_some(), |d| {
                                    d.child(
                                        Tag::warning()
                                            .xsmall()
                                            .child(t("manage.emulators.tag.update")),
                                    )
                                })
                                .when(m.experimental, |d| {
                                    d.child(
                                        Tag::warning()
                                            .xsmall()
                                            .child(t("manage.emulators.tag.experimental")),
                                    )
                                })
                                .when(m.custom, |d| {
                                    d.child(
                                        Tag::secondary()
                                            .xsmall()
                                            .child(t("manage.emulators.tag.custom")),
                                    )
                                })
                                .when(!status.is_installed() && needed_games > 0, |d| {
                                    d.child(needed_games_tag(needed_games))
                                })
                                .when(featured && !subtitle.is_empty(), |d| {
                                    d.child(muted_xs(subtitle.clone(), cx).truncate())
                                }),
                        )
                        .when(!featured, |col| {
                            col.child(muted_xs(subtitle, cx).truncate())
                        })
                        .when(featured && !m.systems.is_empty(), |col| {
                            col.child(self.render_system_chips(m, MAX_HERO_SYSTEM_CHIPS, cx))
                        }),
                )
                .child(
                    h_flex()
                        .gap_0p5()
                        .items_center()
                        .flex_shrink_0()
                        .when(supports_cheevos, |r| r.child(cheevos_badge(&m.id, cx)))
                        .when(!info_text.is_empty(), |hdr| {
                            hdr.child(info_tip(format!("engine-{}", m.id), info_text.into(), cx))
                        }),
                ),
        );

        let trailing = h_flex()
            .gap_1()
            .items_center()
            .when(status.is_installed() && has_options, |row| {
                let id = m.id.clone();
                row.child(
                    Button::new("configure")
                        .small()
                        .outline()
                        .icon(IconName::SlidersHorizontal)
                        .label(t("manage.emulators.configure"))
                        .on_click(move |_, window, cx| open_engine_settings(&id, window, cx)),
                )
            })
            .when(has_overflow, |row| {
                let show_cfg = !status.is_installed() && has_options;
                let more_id = ElementId::Name(format!("engine-more-{}", m.id).into());
                row.child(
                    Button::new(more_id)
                        .ghost()
                        .small()
                        .icon(IconName::Ellipsis)
                        .accessibility_label(t("manage.emulators.more"))
                        .tooltip(t("manage.emulators.more"))
                        .dropdown_menu(move |mut menu, _, _| {
                            if show_cfg {
                                let id = engine_id.clone();
                                menu = menu.item(
                                    PopupMenuItem::new(t("manage.emulators.configure"))
                                        .icon(IconName::SlidersHorizontal)
                                        .on_click(move |_, window, cx| {
                                            open_engine_settings(&id, window, cx)
                                        }),
                                );
                            }
                            if is_retroarch {
                                menu = menu.item(
                                    PopupMenuItem::new(t("manage.emulators.cores.manage"))
                                        .icon(IconName::Package)
                                        .on_click(|_, window, cx| {
                                            cores::open_cores_dialog(window, cx)
                                        }),
                                );
                            }
                            if let Some(folder) = single_folder.clone() {
                                menu = menu.item(
                                    PopupMenuItem::new(t("manage.emulators.open_folder"))
                                        .icon(IconName::FolderOpen)
                                        .on_click(move |_, _, cx| cx.reveal_path(&folder)),
                                );
                            }
                            if can_pick_program {
                                let id = engine_id.clone();
                                let view = view.clone();
                                menu = menu.item(
                                    PopupMenuItem::new(t("manage.emulators.use_program"))
                                        .icon(IconName::FolderOpen)
                                        .on_click(move |_, window, cx| {
                                            let _ = view.update(cx, |this, cx| {
                                                this.pick_program(id.clone(), window, cx)
                                            });
                                        }),
                                );
                            }
                            if !homepage.is_empty() {
                                let url = homepage.clone();
                                menu = menu.item(
                                    PopupMenuItem::new(t("manage.emulators.homepage"))
                                        .icon(IconName::ExternalLink)
                                        .on_click(move |_, _, cx| cx.open_url(&url)),
                                );
                            }
                            if let Some(ver) = single_managed_ver.clone() {
                                let (id, name, view) =
                                    (engine_id.clone(), engine_name.clone(), view.clone());
                                menu = menu.item(
                                    PopupMenuItem::new(t("manage.emulators.remove"))
                                        .icon(IconName::Trash)
                                        .on_click(move |_, window, cx| {
                                            let _ = view.update(cx, |this, cx| {
                                                this.confirm_remove(
                                                    id.clone(),
                                                    name.clone(),
                                                    ver.clone(),
                                                    window,
                                                    cx,
                                                )
                                            });
                                        }),
                                );
                            }
                            if single_custom {
                                let (id, view) = (engine_id.clone(), view.clone());
                                menu = menu.item(
                                    PopupMenuItem::new(t("manage.emulators.stop_using"))
                                        .icon(IconName::Trash)
                                        .on_click(move |_, window, cx| {
                                            let _ = view.update(cx, |this, cx| {
                                                this.set_program(id.clone(), None, window, cx)
                                            });
                                        }),
                                );
                            }
                            if let Some(existing) = existing_custom.clone() {
                                let id = engine_id.clone();
                                let name = engine_name.clone();
                                menu = menu
                                    .item(
                                        PopupMenuItem::new(t("manage.emulators.custom.edit"))
                                            .icon(IconName::Pencil)
                                            .on_click(move |_, window, cx| {
                                                open_custom_engine_dialog(
                                                    Some(existing.clone()),
                                                    window,
                                                    cx,
                                                )
                                            }),
                                    )
                                    .item(
                                        PopupMenuItem::new(t("manage.emulators.custom.remove"))
                                            .icon(IconName::Trash)
                                            .on_click(move |_, window, cx| {
                                                confirm_remove_custom_engine(
                                                    id.clone(),
                                                    name.clone(),
                                                    window,
                                                    cx,
                                                )
                                            }),
                                    );
                            }
                            menu
                        }),
                )
            });

        let mut action_bar = Some(if let Some(live) = op {
            h_flex()
                .gap_2()
                .items_center()
                .when(featured, |h| h.min_w(rems(16.)))
                .when(!featured, |h| {
                    h.w_full().pt_2().border_t_1().border_color(border)
                })
                .child(div().flex_1().min_w_0().child(render_op_wide(&live, cx)))
                .child(primary)
        } else if featured {
            h_flex()
                .gap_1p5()
                .flex_wrap()
                .items_center()
                .children(cores_btn)
                .child(primary)
                .child(trailing)
        } else {
            row_between()
                .gap_2()
                .pt_2()
                .border_t_1()
                .border_color(border)
                .child(
                    h_flex()
                        .gap_1p5()
                        .items_center()
                        .child(primary)
                        .children(cores_btn),
                )
                .child(trailing)
        });

        card(cx)
            .id(ElementId::Name(format!("engine-{}", m.id).into()))
            .debug_selector(move || format!("engine-{sel_id}"))
            .when(featured, |c| {
                c.child(
                    row_between()
                        .gap_4()
                        .children(header.take())
                        .children(action_bar.take()),
                )
            })
            .when(!featured, |c| c.children(header.take()))
            .when(!featured && !m.systems.is_empty(), |c| {
                c.child(
                    row_between()
                        .gap_2()
                        .child(self.render_system_chips(m, MAX_SYSTEM_CHIPS, cx))
                        .children(health_games_line(health, cx)),
                )
            })
            .when(featured, |c| c.children(health_games_line(health, cx)))
            .children(health_missing_lines(health, cx))
            .when_some(release_error, |d, e| {
                d.child(
                    div()
                        .text_xs()
                        .text_color(muted)
                        .child(t_args("manage.emulators.releases_failed", &[("error", &e)])),
                )
            })
            .when(!status.is_installed() && !m.downloadable_on(host), |d| {
                d.child(div().text_xs().text_color(muted).child(if m.custom {
                    t_args("manage.emulators.custom.missing", &[("engine", &m.name)])
                } else {
                    not_downloadable_text(m, host)
                }))
            })
            .when(!installs.is_empty(), |d| {
                d.child(v_flex().gap_1().children(installs))
            })
            .children(action_bar)
            .into_any_element()
    }

    fn render_installation(
        &self,
        ix: usize,
        status: &EngineStatus,
        i: &Installation,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let m = &status.manifest;
        let is_default = status.default_installation().map(Installation::id) == Some(i.id());
        let (title, detail): (SharedString, String) = match i.origin {
            Origin::Managed => (
                t_args(
                    "manage.emulators.origin.managed",
                    &[("version", i.version.as_deref().unwrap_or("?"))],
                ),
                match format::size(i.size) {
                    Some(size) => {
                        t_args("manage.emulators.origin.managed_detail", &[("size", &size)])
                            .to_string()
                    }
                    None => t("manage.emulators.origin.managed_plain").to_string(),
                },
            ),
            Origin::Flatpak => (
                t("manage.emulators.origin.flatpak"),
                i.flatpak.clone().unwrap_or_default(),
            ),
            Origin::System | Origin::Windows => (
                t("manage.emulators.origin.system"),
                i.program.display().to_string(),
            ),
            Origin::App => (
                t("manage.emulators.origin.app"),
                i.program.display().to_string(),
            ),
            Origin::Custom => (
                t("manage.emulators.origin.custom"),
                i.program.display().to_string(),
            ),
        };
        let (engine, install_id) = (m.id.clone(), i.id());
        let folder = i
            .dir
            .clone()
            .or_else(|| (i.origin != Origin::Flatpak).then(|| i.program.clone()));
        h_flex()
            .id(ElementId::Name(format!("install-{}-{ix}", m.id).into()))
            .gap_3()
            .px_3()
            .py_1p5()
            .rounded(cx.theme().radius)
            .bg(cx.theme().background)
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .child(
                        h_flex()
                            .gap_2()
                            .child(div().font_medium().truncate().child(title))
                            .when(is_default, |d| {
                                d.child(
                                    Tag::info()
                                        .xsmall()
                                        .child(t("manage.emulators.tag.default")),
                                )
                            }),
                    )
                    .child(div().text_xs().text_color(muted).truncate().child(detail)),
            )
            .child(
                h_flex()
                    .gap_1()
                    .flex_shrink_0()
                    .when(!is_default && status.installations.len() > 1, |d| {
                        let (engine, install_id) = (engine.clone(), install_id.clone());
                        d.child(
                            Button::new("make-default")
                                .ghost()
                                .small()
                                .label(t("manage.emulators.make_default"))
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.make_default(
                                        engine.clone(),
                                        install_id.clone(),
                                        window,
                                        cx,
                                    )
                                })),
                        )
                    })
                    .when_some(folder, |d, folder| {
                        d.child(
                            Button::new("show")
                                .ghost()
                                .small()
                                .icon(IconName::FolderOpen)
                                .tooltip(t("manage.emulators.open_folder"))
                                .on_click(move |_, _, cx| cx.reveal_path(&folder)),
                        )
                    })
                    .when(i.origin == Origin::Managed, |d| {
                        let (engine, name, version) = (
                            engine.clone(),
                            m.name.clone(),
                            i.version.clone().unwrap_or_default(),
                        );
                        d.child(
                            Button::new("remove")
                                .ghost()
                                .small()
                                .label(t("manage.emulators.remove"))
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.confirm_remove(
                                        engine.clone(),
                                        name.clone(),
                                        version.clone(),
                                        window,
                                        cx,
                                    )
                                })),
                        )
                    })
                    // An emulator the user added is its program; it is
                    // changed or removed as a whole.
                    .when(i.origin == Origin::Custom && !m.custom, |d| {
                        let engine = engine.clone();
                        d.child(
                            Button::new("stop-using")
                                .ghost()
                                .small()
                                .label(t("manage.emulators.stop_using"))
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.set_program(engine.clone(), None, window, cx)
                                })),
                        )
                    }),
            )
            .into_any_element()
    }

    fn render_engines(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let Some(statuses) = &self.statuses else {
            return v_flex()
                .gap_5()
                .child(tab_toolbar(h_flex(), &self.search))
                .child(
                    h_flex()
                        .gap_2()
                        .text_sm()
                        .text_color(muted)
                        .child(Spinner::new().small())
                        .child(t("manage.emulators.loading")),
                )
                .into_any_element();
        };
        let host = Host::current();
        let query = self.query(cx);
        let matches_query = |s: &&EngineStatus| {
            query.is_empty()
                || s.manifest.name.to_lowercase().contains(&query)
                || s.manifest.systems.iter().any(|id| {
                    systems::get(id).is_some_and(|sys| {
                        sys.name.to_lowercase().contains(&query)
                            || sys.short.to_lowercase().contains(&query)
                    })
                })
        };

        let all_count = statuses.len();
        let installed_count = statuses.iter().filter(|s| s.is_installed()).count();
        let available_count = statuses
            .iter()
            .filter(|s| classify_engine(s, host) == EngineSection::Available)
            .count();
        let manual_count = statuses
            .iter()
            .filter(|s| classify_engine(s, host) == EngineSection::Manual)
            .count();
        let updates_count = statuses
            .iter()
            .filter(|s| self.latest_update(s).is_some())
            .count();

        let pills = h_flex()
            .gap_1p5()
            .flex_wrap()
            .child(filter_pill(
                "engines-filter-all",
                t("manage.emulators.filter.all"),
                all_count,
                self.engine_filter == EngineFilter::All,
                cx,
                cx.listener(|this, _, _, cx| {
                    this.engine_filter = EngineFilter::All;
                    cx.notify();
                }),
            ))
            .child(filter_pill(
                "engines-filter-installed",
                t("manage.emulators.filter.installed"),
                installed_count,
                self.engine_filter == EngineFilter::Installed,
                cx,
                cx.listener(|this, _, _, cx| {
                    this.engine_filter = EngineFilter::Installed;
                    cx.notify();
                }),
            ))
            .child(filter_pill(
                "engines-filter-available",
                t("manage.emulators.filter.available"),
                available_count,
                self.engine_filter == EngineFilter::Available,
                cx,
                cx.listener(|this, _, _, cx| {
                    this.engine_filter = EngineFilter::Available;
                    cx.notify();
                }),
            ))
            .when(
                manual_count > 0 || self.engine_filter == EngineFilter::Manual,
                |row| {
                    row.child(filter_pill(
                        "engines-filter-manual",
                        t("manage.emulators.filter.manual"),
                        manual_count,
                        self.engine_filter == EngineFilter::Manual,
                        cx,
                        cx.listener(|this, _, _, cx| {
                            this.engine_filter = EngineFilter::Manual;
                            cx.notify();
                        }),
                    ))
                },
            )
            .when(
                updates_count > 0 || self.engine_filter == EngineFilter::Updates,
                |row| {
                    row.child(filter_pill(
                        "engines-filter-updates",
                        t("manage.emulators.filter.updates"),
                        updates_count,
                        self.engine_filter == EngineFilter::Updates,
                        cx,
                        cx.listener(|this, _, _, cx| {
                            this.engine_filter = EngineFilter::Updates;
                            cx.notify();
                        }),
                    ))
                },
            );

        let (mut installed_hero, mut available_hero) = (None, None);
        let mut installed_cards = Vec::new();
        let mut available_cards = Vec::new();
        let mut manual_cards = Vec::new();
        let mut manual_names = Vec::new();

        for status in statuses.iter().filter(matches_query) {
            let section = classify_engine(status, host);
            let matches_filter = match self.engine_filter {
                EngineFilter::All => true,
                EngineFilter::Installed => section == EngineSection::Installed,
                EngineFilter::Available => section == EngineSection::Available,
                EngineFilter::Manual => section == EngineSection::Manual,
                EngineFilter::Updates => self.latest_update(status).is_some(),
            };
            if !matches_filter {
                continue;
            }
            let featured =
                status.manifest.kind == EngineKind::Retroarch && section != EngineSection::Manual;
            if section == EngineSection::Manual {
                manual_names.push(status.manifest.name.clone());
            }
            let rendered = self.render_engine(status, featured, cx);
            match (section, featured) {
                (EngineSection::Installed, true) => installed_hero = Some(rendered),
                (EngineSection::Installed, false) => installed_cards.push(rendered),
                (EngineSection::Available, true) => available_hero = Some(rendered),
                (EngineSection::Available, false) => available_cards.push(rendered),
                (EngineSection::Manual, _) => manual_cards.push(rendered),
            }
        }

        let has_installed = installed_hero.is_some() || !installed_cards.is_empty();
        let has_available = available_hero.is_some() || !available_cards.is_empty();
        let has_manual = !manual_cards.is_empty();
        let empty = !has_installed && !has_available && !has_manual;
        let can_collapse_manual = (has_installed || has_available)
            && query.is_empty()
            && self.engine_filter == EngineFilter::All;
        let manual_expanded = !can_collapse_manual || self.show_manual;
        let manual_shown = manual_cards.len();
        let manual_preview = preview_names(&manual_names, 7);

        v_flex()
            .gap_5()
            .child(tab_toolbar(pills, &self.search))
            .when(empty, |col| {
                col.child(
                    div()
                        .text_sm()
                        .text_color(muted)
                        .child(t("manage.emulators.no_match")),
                )
            })
            .when(has_installed, |col| {
                col.child(
                    v_flex()
                        .gap_3()
                        .child(SectionTitle::new(t("manage.emulators.section.installed")))
                        .children(installed_hero)
                        .when(!installed_cards.is_empty(), |sec| {
                            sec.child(card_grid_natural_rows(
                                &self.installed_grid_width,
                                INSTALLED_CARD_REM,
                                2,
                                installed_cards,
                                window,
                            ))
                        }),
                )
            })
            .when(has_available, |col| {
                col.child(
                    v_flex()
                        .gap_3()
                        .child(
                            SectionTitle::new(t("manage.emulators.section.available")).trailing(
                                muted_xs(t("manage.emulators.section.available_hint"), cx),
                            ),
                        )
                        .children(available_hero)
                        .when(!available_cards.is_empty(), |sec| {
                            sec.child(card_grid_natural_rows(
                                &self.available_grid_width,
                                COMPACT_CARD_REM,
                                3,
                                available_cards,
                                window,
                            ))
                        }),
                )
            })
            .when(has_manual, |col| {
                let title = SectionTitle::new(t("manage.emulators.section.manual")).when(
                    can_collapse_manual,
                    |s| {
                        s.trailing(collapse_button(
                            "toggle-manual-engines",
                            manual_expanded,
                            manual_shown,
                            cx.listener(|this, _, _, cx| {
                                this.show_manual = !this.show_manual;
                                cx.notify();
                            }),
                        ))
                    },
                );
                col.child(
                    v_flex()
                        .gap_3()
                        .child(title)
                        .when(manual_expanded, |sec| {
                            sec.child(card_grid_natural_rows(
                                &self.manual_grid_width,
                                COMPACT_CARD_REM,
                                3,
                                manual_cards,
                                window,
                            ))
                        })
                        .when(!manual_expanded, |sec| {
                            sec.child(
                                card(cx).py_3().child(
                                    row_between()
                                        .child(
                                            muted_xs(
                                                t_args(
                                                    "manage.emulators.manual_preview",
                                                    &[("engines", &manual_preview)],
                                                ),
                                                cx,
                                            )
                                            .flex_1()
                                            .min_w_0()
                                            .truncate(),
                                        )
                                        .child(
                                            Button::new("expand-manual-engines")
                                                .outline()
                                                .small()
                                                .label(t("manage.emulators.expand_catalog"))
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    this.show_manual = true;
                                                    cx.notify();
                                                })),
                                        ),
                                ),
                            )
                        }),
                )
            })
            .into_any_element()
    }

    fn system_standing(
        &self,
        system_id: &str,
        statuses: &[EngineStatus],
        cx: &App,
    ) -> (usize, Option<EngineRef>, bool, bool) {
        let count = self.counts.get(system_id).copied().unwrap_or(0);
        let choice = backend(cx).system_engine(system_id, statuses);
        let ready = choice.as_ref().is_some_and(|c| {
            statuses
                .iter()
                .any(|s| s.manifest.id == c.engine && s.plays(c))
        });
        let bios_warn = self
            .bios
            .get(system_id)
            .and_then(bios_line)
            .is_some_and(|(w, _)| w);
        (count, choice, ready, bios_warn)
    }

    fn render_systems(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let (muted, border, group_box, radius_lg, warning, success) = {
            let t = cx.theme();
            (
                t.muted_foreground,
                t.border,
                t.group_box,
                t.radius_lg,
                t.warning,
                t.success,
            )
        };
        let Some(statuses) = &self.statuses else {
            return v_flex()
                .gap_5()
                .child(self.render_folders(window, cx))
                .child(tab_toolbar(h_flex(), &self.search))
                .child(Spinner::new().small())
                .into_any_element();
        };
        let host = Host::current();
        let query = self.query(cx);
        let mut ordered: Vec<&irori_engines::System> = systems::all().iter().collect();
        ordered.sort_by_key(|s| !self.counts.contains_key(&s.id));

        let (mut all_count, mut with_games_count, mut attention_count) = (0, 0, 0);
        let (mut missing_lib, mut library_cards, mut other_rows) =
            (Vec::new(), Vec::new(), Vec::new());
        let bios_empty = models(cx)
            .settings
            .read(cx)
            .app
            .bios_folder
            .trim()
            .is_empty();

        for system in ordered {
            all_count += 1;
            let (count, choice, ready, bios_warn) = self.system_standing(&system.id, statuses, cx);
            if count > 0 {
                with_games_count += 1;
            }
            let needs_attention = system_needs_attention(count, ready, bios_warn);
            if needs_attention {
                attention_count += 1;
            }
            let can_download = choice
                .as_ref()
                .is_some_and(|c| engine_downloadable(statuses, &c.engine, host));
            if count > 0
                && !ready
                && can_download
                && let Some(c) = choice.clone()
            {
                missing_lib.push((system.short.clone(), c));
            }
            let matches_query = query.is_empty()
                || system.name.to_lowercase().contains(&query)
                || system.short.to_lowercase().contains(&query)
                || system.maker.to_lowercase().contains(&query);
            let matches_filter = match self.system_filter {
                SystemFilter::All => true,
                SystemFilter::WithGames => count > 0,
                SystemFilter::NeedsAttention => needs_attention,
            };
            if !matches_query || !matches_filter {
                continue;
            }

            let bios_entry = self.bios.get(&system.id);
            let engine_installed = choice.as_ref().is_some_and(|c| {
                statuses
                    .iter()
                    .any(|s| s.manifest.id == c.engine && s.is_installed())
            });
            let active_op = choice.as_ref().and_then(|c| {
                let r = &self.ops.read(cx).running;
                r.get(&engine_key(&c.engine))
                    .or_else(|| c.core.as_deref().and_then(|k| r.get(&core_key(k))))
                    .map(|o| o.snapshot())
            });

            if count > 0 {
                let state = if ready && !bios_warn {
                    Tag::success()
                        .xsmall()
                        .child(t("manage.emulators.system.ready"))
                } else if !ready {
                    Tag::warning()
                        .xsmall()
                        .child(t("manage.emulators.systems.needs_emulator"))
                } else {
                    Tag::warning()
                        .xsmall()
                        .child(t("manage.emulators.systems.missing_bios"))
                };
                let footer: Option<AnyElement> = if let Some(live) = active_op {
                    Some(render_op_wide(&live, cx).into_any_element())
                } else if !ready && can_download {
                    Some(system_install_btn(
                        "install-for-system",
                        choice.clone(),
                        engine_installed,
                        true,
                    ))
                } else if bios_warn && bios_empty {
                    Some(
                        Button::new("choose-bios-inline")
                            .outline()
                            .small()
                            .icon(IconName::FolderOpen)
                            .label(t("manage.emulators.bios.choose"))
                            .on_click(cx.listener(|this, _, w, cx| this.pick_folder(true, w, cx)))
                            .into_any_element(),
                    )
                } else {
                    None
                };
                let meta = format!(
                    "{} · {} · {}",
                    system.maker,
                    system.year,
                    games_label(count)
                );
                library_cards.push(
                    card(cx)
                        .id(ElementId::Name(format!("system-{}", system.id).into()))
                        .child(
                            h_flex()
                                .gap_2p5()
                                .items_start()
                                .child(system_monogram(&system.short, false, cx))
                                .child(
                                    v_flex()
                                        .flex_1()
                                        .min_w_0()
                                        .child(
                                            div()
                                                .text_sm()
                                                .font_semibold()
                                                .truncate()
                                                .child(system.name.clone()),
                                        )
                                        .child(muted_xs(meta, cx).truncate()),
                                )
                                .child(state),
                        )
                        .child(
                            v_flex()
                                .gap_1()
                                .child(muted_xs(t("manage.emulators.system.default"), cx))
                                .when_some(self.system_selects.get(&system.id), |d, select| {
                                    d.child(Select::new(select).small())
                                }),
                        )
                        .when_some(
                            bios_entry.and_then(bios_compact),
                            |c, (warn, short, full)| {
                                let tip_id =
                                    ElementId::Name(format!("bios-tip-{}", system.id).into());
                                let (icon, color) = if warn {
                                    (IconName::TriangleAlert, warning)
                                } else {
                                    (IconName::CircleCheck, success)
                                };
                                c.child(
                                    h_flex()
                                        .id(tip_id)
                                        .gap_1p5()
                                        .items_center()
                                        .text_xs()
                                        .tooltip(move |w, cx| plain_tooltip(full.clone(), w, cx))
                                        .child(Icon::new(icon).xsmall().text_color(color))
                                        .child(
                                            div()
                                                .flex_1()
                                                .min_w_0()
                                                .truncate()
                                                .text_color(if warn { warning } else { muted })
                                                .child(short),
                                        ),
                                )
                            },
                        )
                        .when_some(footer, |c, el| {
                            c.child(div().pt_2().border_t_1().border_color(border).child(el))
                        })
                        .into_any_element(),
                );
            } else {
                let divider = !other_rows.is_empty();
                let bios_tip = bios_entry
                    .filter(|b| !b.files.is_empty() && b.state != SystemState::Optional)
                    .and_then(bios_line)
                    .map(|(_, text)| text);
                let action: AnyElement = if active_op.is_some() {
                    h_flex()
                        .gap_1p5()
                        .items_center()
                        .child(Spinner::new().xsmall())
                        .child(muted_xs(t("manage.emulators.progress.starting"), cx))
                        .into_any_element()
                } else if ready {
                    Tag::success()
                        .xsmall()
                        .child(t("manage.emulators.system.ready"))
                        .into_any_element()
                } else if can_download {
                    system_install_btn(
                        ElementId::Name(format!("install-sys-{}", system.id).into()),
                        choice.clone(),
                        engine_installed,
                        false,
                    )
                } else {
                    Tag::secondary()
                        .xsmall()
                        .child(t("manage.emulators.filter.manual"))
                        .into_any_element()
                };
                other_rows.push(
                    row_between()
                        .id(ElementId::Name(format!("system-row-{}", system.id).into()))
                        .px_4()
                        .py_2p5()
                        .when(divider, |r| r.border_t_1().border_color(border))
                        .child(
                            h_flex()
                                .flex_1()
                                .min_w(rems(14.))
                                .gap_3()
                                .items_center()
                                .child(system_monogram(&system.short, true, cx))
                                .child(
                                    v_flex()
                                        .flex_1()
                                        .min_w_0()
                                        .child(
                                            h_flex()
                                                .gap_2()
                                                .items_center()
                                                .child(div().text_sm().font_medium().truncate().child(system.name.clone()))
                                                .when_some(bios_tip, |r, tip| {
                                                    let id = ElementId::Name(format!("sys-bios-req-{}", system.id).into());
                                                    r.child(
                                                        div()
                                                            .id(id)
                                                            .tooltip(move |w, cx| plain_tooltip(tip.clone(), w, cx))
                                                            .child(Tag::secondary().xsmall().child(t("manage.emulators.systems.bios_required"))),
                                                    )
                                                }),
                                        )
                                        .child(muted_xs(format!("{} · {}", system.maker, system.year), cx).truncate()),
                                ),
                        )
                        .child(
                            h_flex()
                                .gap_2p5()
                                .flex_wrap()
                                .items_center()
                                .when_some(self.system_selects.get(&system.id), |r, s| {
                                    r.child(div().w(rems(14.)).child(Select::new(s).small()))
                                })
                                .child(div().flex_shrink_0().child(action)),
                        )
                        .into_any_element(),
                );
            }
        }

        let pills = h_flex()
            .gap_1p5()
            .flex_wrap()
            .child(filter_pill(
                "systems-filter-all",
                t("manage.emulators.filter.all"),
                all_count,
                self.system_filter == SystemFilter::All,
                cx,
                cx.listener(|this, _, _, cx| {
                    this.system_filter = SystemFilter::All;
                    cx.notify();
                }),
            ))
            .child(filter_pill(
                "systems-filter-with-games",
                t("manage.emulators.systems.filter.with_games"),
                with_games_count,
                self.system_filter == SystemFilter::WithGames,
                cx,
                cx.listener(|this, _, _, cx| {
                    this.system_filter = SystemFilter::WithGames;
                    cx.notify();
                }),
            ))
            .when(
                attention_count > 0 || self.system_filter == SystemFilter::NeedsAttention,
                |row| {
                    row.child(filter_pill(
                        "systems-filter-attention",
                        t("manage.emulators.systems.filter.attention"),
                        attention_count,
                        self.system_filter == SystemFilter::NeedsAttention,
                        cx,
                        cx.listener(|this, _, _, cx| {
                            this.system_filter = SystemFilter::NeedsAttention;
                            cx.notify();
                        }),
                    ))
                },
            );

        let has_library = !library_cards.is_empty();
        let has_other = !other_rows.is_empty();
        let can_collapse_others =
            has_library && query.is_empty() && self.system_filter == SystemFilter::All;
        let others_expanded = !can_collapse_others || self.show_other_systems;
        let other_count = other_rows.len();
        let empty = !has_library && !has_other;

        v_flex()
            .gap_5()
            .child(self.render_folders(window, cx))
            .children(self.render_missing_banner(&missing_lib, statuses, warning, cx))
            .child(tab_toolbar(pills, &self.search))
            .when(empty, |col| {
                col.child(
                    div()
                        .text_sm()
                        .text_color(muted)
                        .child(t("manage.emulators.no_match")),
                )
            })
            .when(has_library, |col| {
                col.child(
                    v_flex()
                        .gap_3()
                        .child(SectionTitle::new(t(
                            "manage.emulators.systems.section.library",
                        )))
                        .child(card_grid_natural_rows(
                            &self.library_systems_grid_width,
                            SYSTEM_CARD_REM,
                            3,
                            library_cards,
                            window,
                        )),
                )
            })
            .when(has_other, |col| {
                let heading = if has_library {
                    t("manage.emulators.systems.section.other")
                } else {
                    t("manage.emulators.systems.section.supported")
                };
                let title = SectionTitle::new(heading).when(can_collapse_others, |s| {
                    s.trailing(collapse_button(
                        "toggle-other-systems",
                        others_expanded,
                        other_count,
                        cx.listener(|this, _, _, cx| {
                            this.show_other_systems = !this.show_other_systems;
                            cx.notify();
                        }),
                    ))
                });
                col.child(v_flex().gap_3().child(title).when(others_expanded, |sec| {
                    sec.child(
                        v_flex()
                            .w_full()
                            .rounded(radius_lg)
                            .border_1()
                            .border_color(border)
                            .bg(group_box)
                            .overflow_hidden()
                            .children(other_rows),
                    )
                }))
            })
            .into_any_element()
    }

    fn render_missing_banner(
        &self,
        missing: &[(String, EngineRef)],
        statuses: &[EngineStatus],
        warning: gpui_kit::Hsla,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let (mut engines, mut cores) = (Vec::new(), Vec::new());
        for (_, c) in missing {
            let inst = statuses
                .iter()
                .any(|s| s.manifest.id == c.engine && s.is_installed());
            if !inst && !engines.contains(&c.engine) {
                engines.push(c.engine.clone());
            }
            if let Some(core) = &c.core
                && !cores.contains(core)
            {
                cores.push(core.clone());
            }
        }
        let to_install = engines.len().max(cores.len());
        if to_install == 0 {
            return None;
        }
        let running = &self.ops.read(cx).running;
        let busy = engines
            .iter()
            .any(|id| running.contains_key(&engine_key(id)))
            || cores.iter().any(|id| running.contains_key(&core_key(id)));
        let sys_list = missing
            .iter()
            .map(|(s, _)| s.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        let msg = if missing.len() == 1 {
            t_args(
                "manage.emulators.systems.missing_banner_one",
                &[("systems", &sys_list)],
            )
        } else {
            t_args(
                "manage.emulators.systems.missing_banner_other",
                &[
                    ("count", &missing.len().to_string()),
                    ("systems", &sys_list),
                ],
            )
        };
        let label = if to_install == 1 {
            t("manage.emulators.systems.install_missing_one")
        } else {
            t_args(
                "manage.emulators.systems.install_missing_other",
                &[("count", &to_install.to_string())],
            )
        };
        Some(
            card(cx)
                .border_color(warning.opacity(0.45))
                .py_3()
                .child(
                    row_between()
                        .child(
                            h_flex()
                                .flex_1()
                                .min_w_0()
                                .gap_2()
                                .items_center()
                                .child(
                                    Icon::new(IconName::TriangleAlert)
                                        .small()
                                        .text_color(warning),
                                )
                                .child(div().flex_1().min_w_0().text_xs().child(msg)),
                        )
                        .child(
                            Button::new("install-missing-emulators")
                                .primary()
                                .small()
                                .icon(IconName::Download)
                                .label(label)
                                .disabled(busy)
                                .loading(busy)
                                .on_click(move |_, _, cx| {
                                    for id in &engines {
                                        start_install(install_engine_target(id.clone()), cx);
                                    }
                                    for id in &cores {
                                        start_install(InstallTarget::Core { id: id.clone() }, cx);
                                    }
                                }),
                        ),
                )
                .into_any_element(),
        )
    }
}

fn system_install_btn(
    id: impl Into<ElementId>,
    choice: Option<EngineRef>,
    engine_installed: bool,
    primary: bool,
) -> AnyElement {
    let name = choice.as_ref().map(choice_name).unwrap_or_default();
    Button::new(id)
        .small()
        .icon(IconName::Download)
        .map(|b| if primary { b.primary() } else { b.outline() })
        .label(t_args(
            "manage.emulators.system.install_engine",
            &[("engine", &name)],
        ))
        .on_click(move |_, _, cx| {
            let Some(c) = choice.clone() else { return };
            if !engine_installed {
                start_install(install_engine_target(c.engine), cx);
            }
            if let Some(core) = c.core {
                start_install(InstallTarget::Core { id: core }, cx);
            }
        })
        .into_any_element()
}

fn engine_downloadable(statuses: &[EngineStatus], id: &str, host: Host) -> bool {
    statuses
        .iter()
        .find(|s| s.manifest.id == id)
        .is_some_and(|s| s.is_installed() || s.manifest.downloadable_on(host))
}

fn single_install_meta(status: &EngineStatus) -> Option<String> {
    let i = status.default_installation()?;
    Some(match i.origin {
        Origin::Managed => {
            let ver = t_args(
                "manage.emulators.origin.managed",
                &[("version", i.version.as_deref().unwrap_or("?"))],
            );
            let detail = match format::size(i.size) {
                Some(size) => t_args("manage.emulators.origin.managed_detail", &[("size", &size)]),
                None => t("manage.emulators.origin.managed_plain"),
            };
            format!("{ver} · {detail}")
        }
        Origin::Flatpak => t("manage.emulators.origin.flatpak").to_string(),
        Origin::System | Origin::Windows => t("manage.emulators.origin.system").to_string(),
        Origin::App => t("manage.emulators.origin.app").to_string(),
        Origin::Custom => t("manage.emulators.origin.custom").to_string(),
    })
}

fn system_summary(m: &EngineManifest) -> String {
    let full: Vec<String> = m
        .systems
        .iter()
        .filter_map(|id| systems::get(id).map(|s| s.name.clone()))
        .collect();
    if full.len() <= 2 {
        return full.join(", ");
    }
    let short: Vec<String> = m
        .systems
        .iter()
        .filter_map(|id| systems::get(id).map(|s| s.short.clone()))
        .collect();
    preview_names(&short, 3)
}

fn preview_names(names: &[String], max: usize) -> String {
    let shown: Vec<&str> = names.iter().take(max).map(String::as_str).collect();
    let extra = names.len().saturating_sub(shown.len());
    if extra > 0 {
        format!("{} +{extra}", shown.join(", "))
    } else {
        shown.join(", ")
    }
}

fn cheevos_badge(engine_id: &str, cx: &App) -> AnyElement {
    let tip = t("manage.emulators.cheevos_tooltip");
    div()
        .id(ElementId::Name(format!("cheevos-tip-{engine_id}").into()))
        .p_1()
        .tooltip(move |w, cx| plain_tooltip(tip.clone(), w, cx))
        .child(
            Icon::new(IconName::Trophy)
                .xsmall()
                .text_color(cx.theme().muted_foreground),
        )
        .into_any_element()
}

fn games_label(count: usize) -> SharedString {
    if count == 1 {
        t("manage.emulators.system.games_one")
    } else {
        t_args(
            "manage.emulators.system.games_other",
            &[("count", &count.to_string())],
        )
    }
}

fn needed_games_tag(count: usize) -> AnyElement {
    Tag::warning()
        .xsmall()
        .child(games_label(count))
        .into_any_element()
}

fn bios_files(status: &SystemBios, state: FileState) -> Vec<String> {
    status
        .files
        .iter()
        .filter(|f| f.state == state)
        .map(|f| match &f.region {
            Some(r) => format!("{} ({r})", f.name),
            None => f.name.clone(),
        })
        .collect()
}

fn bios_compact(status: &SystemBios) -> Option<(bool, SharedString, SharedString)> {
    let (warn, full) = bios_line(status)?;
    let (state, key) = match status.state {
        SystemState::Ready => return Some((false, full.clone(), full)),
        SystemState::Unrecognized => (
            FileState::Unrecognized,
            "manage.emulators.bios.unrecognized",
        ),
        SystemState::Missing => (FileState::Missing, "manage.emulators.bios.missing"),
        SystemState::Optional => (FileState::Missing, "manage.emulators.bios.optional"),
    };
    let files = bios_files(status, state);
    let short_files = match files.split_first() {
        Some((first, [])) => first.clone(),
        Some((first, rest)) => format!("{first} +{}", rest.len()),
        None => return Some((warn, full.clone(), full)),
    };
    Some((warn, t_args(key, &[("files", &short_files)]), full))
}

/// The BIOS line of a system card: whether it is a warning, and its text.
/// `None` for systems that need no BIOS.
pub(crate) fn bios_line(status: &SystemBios) -> Option<(bool, SharedString)> {
    if status.files.is_empty() {
        return None;
    }
    let names = |state: FileState| bios_files(status, state).join(", ");
    Some(match status.state {
        SystemState::Ready => (false, t("manage.emulators.bios.ready")),
        SystemState::Unrecognized => (
            true,
            t_args(
                "manage.emulators.bios.unrecognized",
                &[("files", &names(FileState::Unrecognized))],
            ),
        ),
        SystemState::Missing => (
            true,
            t_args(
                "manage.emulators.bios.missing",
                &[("files", &names(FileState::Missing))],
            ),
        ),
        SystemState::Optional => (
            false,
            t_args(
                "manage.emulators.bios.optional",
                &[("files", &names(FileState::Missing))],
            ),
        ),
    })
}

/// Inline progress of a running install.
/// A dot saying how an emulator stands: ready, a program it was set up
/// with is gone, or not installed. The card's tags and lines say it in
/// words too.
fn health_dot(status: &EngineStatus, health: Option<&EngineHealth>, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let gone = health.is_some_and(|h| !h.missing.is_empty());
    let colour = if gone {
        theme.warning
    } else if status.is_installed() {
        theme.success
    } else {
        theme.muted_foreground.opacity(0.5)
    };
    div()
        .flex_shrink_0()
        .size(rems(0.625))
        .rounded_full()
        .bg(colour)
        .into_any_element()
}

/// The games that play with an emulator, and the programs it lost.
fn health_games_line(health: Option<&EngineHealth>, cx: &App) -> Option<AnyElement> {
    let games = health.filter(|h| h.games > 0)?.games;
    let count = games.to_string();
    Some(
        div()
            .text_xs()
            .text_color(cx.theme().muted_foreground)
            .child(if games == 1 {
                t_args("manage.emulators.health.games_one", &[("count", &count)])
            } else {
                t_args("manage.emulators.health.games_other", &[("count", &count)])
            })
            .into_any_element(),
    )
}

fn health_missing_lines(health: Option<&EngineHealth>, cx: &App) -> Vec<AnyElement> {
    let Some(health) = health else {
        return Vec::new();
    };
    let warning = cx.theme().warning;
    health
        .missing
        .iter()
        .map(|program| {
            h_flex()
                .gap_1()
                .min_w_0()
                .text_xs()
                .text_color(warning)
                .child(Icon::new(IconName::TriangleAlert).xsmall())
                .child(div().min_w_0().truncate().child(t_args(
                    "manage.emulators.health.missing",
                    &[("path", &program.display().to_string())],
                )))
                .into_any_element()
        })
        .collect()
}

pub(super) fn render_op(live: &OpLive, cx: &App) -> gpui_kit::Div {
    render_op_box(live, false, cx)
}

fn render_op_wide(live: &OpLive, cx: &App) -> gpui_kit::Div {
    render_op_box(live, true, cx)
}

fn render_op_box(live: &OpLive, full_width: bool, cx: &App) -> gpui_kit::Div {
    let progress = live.progress.unwrap_or_default();
    let unpacking = live.step.as_deref() == Some("Unpacking");
    let detail = if unpacking {
        t("manage.emulators.progress.unpacking").to_string()
    } else {
        [
            progress.percent.map(|p| {
                t_args(
                    "manage.emulators.progress.percent",
                    &[("percent", &format!("{p:.0}"))],
                )
                .to_string()
            }),
            Some(format::rate(progress.download_speed)).filter(|s| s != "—"),
            progress.eta_secs.filter(|e| *e > 0).map(|e| {
                t_args(
                    "manage.emulators.progress.time_left",
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
    let box_div = if full_width {
        v_flex().w_full()
    } else {
        v_flex().w(rems(12.)).flex_shrink_0()
    };
    box_div
        .gap_1()
        .child(
            Progress::new("engine-progress")
                .value(progress.percent.unwrap_or(0.0))
                .loading(unpacking || progress.percent.is_none()),
        )
        .child(
            div()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .truncate()
                .child(if detail.is_empty() {
                    t("manage.emulators.progress.starting").to_string()
                } else {
                    detail
                }),
        )
}

impl Render for EmulatorsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let language = irori_ui::i18n::language();
        if language != self.placeholder_language || self.tab_ix != self.placeholder_tab {
            self.placeholder_language = language;
            self.placeholder_tab = self.tab_ix;
            let placeholder = if self.tab_ix == 1 {
                t("manage.emulators.systems.search")
            } else {
                t("manage.emulators.search")
            };
            self.search
                .update(cx, |s, cx| s.set_placeholder(placeholder, window, cx));
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
        let host = Host::current();
        let installed_engines = self
            .statuses
            .as_ref()
            .map_or(0, |s| s.iter().filter(|st| st.is_installed()).count());
        let downloadable_engines = self.statuses.as_ref().map_or(0, |s| {
            s.iter()
                .filter(|st| st.manifest.downloadable_on(host) && !st.manifest.experimental)
                .count()
        });
        let updates_count = self.statuses.as_ref().map_or(0, |s| {
            s.iter()
                .filter(|st| self.latest_update(st).is_some())
                .count()
        });
        let systems_attention = self.statuses.as_ref().map_or(0, |statuses| {
            systems::all()
                .iter()
                .filter(|sys| {
                    let (count, _, ready, bios_warn) = self.system_standing(&sys.id, statuses, cx);
                    system_needs_attention(count, ready, bios_warn)
                })
                .count()
        });
        let needs_setup =
            installed_engines == 0 || models(cx).settings.read(cx).app.rom_folders.is_empty();

        let content = match self.tab_ix {
            0 => self.render_engines(window, cx),
            1 => self.render_systems(window, cx),
            _ => self.retro.clone().into_any_element(),
        };
        let empty = self.statuses.as_ref().is_some_and(|s| s.is_empty());

        let tabs = [
            (
                0usize,
                "engines",
                t("manage.emulators.tab.engines"),
                (installed_engines > 0).then(|| {
                    Tag::secondary()
                        .xsmall()
                        .child(installed_engines.to_string())
                        .into_any_element()
                }),
            ),
            (
                1usize,
                "systems",
                t("manage.emulators.tab.systems"),
                (systems_attention > 0).then(|| {
                    div()
                        .debug_selector(|| "systems-attention-badge".into())
                        .child(Tag::warning().xsmall().child(systems_attention.to_string()))
                        .into_any_element()
                }),
            ),
            (
                2usize,
                "achievements",
                t("manage.emulators.tab.achievements"),
                None,
            ),
        ];
        let total_games: usize = self.counts.values().sum();
        let tab_status: Option<SharedString> = match self.tab_ix {
            0 => Some(t_args(
                "manage.emulators.status_summary",
                &[
                    ("installed", &installed_engines.to_string()),
                    ("available", &downloadable_engines.to_string()),
                ],
            )),
            1 => Some(match (total_games, self.counts.len()) {
                (0, _) => t("manage.emulators.folders.none_found"),
                (1, _) => t("manage.emulators.folders.found_one"),
                (games, systems) => t_args(
                    "manage.emulators.folders.found",
                    &[
                        ("games", &games.to_string()),
                        ("systems", &systems.to_string()),
                    ],
                ),
            }),
            _ => None,
        };

        v_flex()
            .id("emulators-page")
            .size_full()
            .min_h_0()
            .overflow_y_scrollbar()
            .child(
                v_flex()
                    .id("emulators-content")
                    .debug_selector(|| "emulators-content".into())
                    .w_full()
                    .max_w(rems(PAGE_MAX_WIDTH_REM))
                    .px_6()
                    .py_6()
                    .gap_5()
                    .child(
                        PageHeader::new(t("manage.emulators.title"))
                            .description(t("manage.emulators.description"))
                            .action(
                                h_flex()
                                    .gap_2()
                                    .flex_wrap()
                                    .items_center()
                                    .when(self.tab_ix < 2, |row| {
                                        row.child(
                                            Button::new("setup-emulation")
                                                .icon(IconName::WandSparkles)
                                                .label(t("manage.setup.open"))
                                                .map(|b| {
                                                    if needs_setup {
                                                        b.primary()
                                                    } else {
                                                        b.outline()
                                                    }
                                                })
                                                .on_click(|_, window, cx| {
                                                    open_emulation_setup(window, cx)
                                                }),
                                        )
                                    })
                                    .when(self.tab_ix == 0, |row| {
                                        row.child(
                                            Button::new("add-emulator")
                                                .outline()
                                                .icon(IconName::Plus)
                                                .label(t("manage.emulators.custom.add"))
                                                .on_click(|_, window, cx| {
                                                    open_custom_engine_dialog(None, window, cx)
                                                }),
                                        )
                                        .child(
                                            Button::new("refresh-emulators")
                                                .ghost()
                                                .icon(IconName::RefreshCw)
                                                .label(t("manage.emulators.refresh"))
                                                .loading(self.refreshing)
                                                .disabled(self.refreshing)
                                                .on_click(cx.listener(|this, _, window, cx| {
                                                    this.load_releases(true, window, cx);
                                                    this.load(window, cx);
                                                })),
                                        )
                                        .when(
                                            updates_count > 0,
                                            |r| {
                                                r.child(
                                                    Tag::warning()
                                                        .xsmall()
                                                        .child(updates_count.to_string()),
                                                )
                                            },
                                        )
                                    }),
                            ),
                    )
                    .child(
                        h_flex()
                            .w_full()
                            .gap_4()
                            .flex_wrap()
                            .items_end()
                            .justify_between()
                            .border_b_1()
                            .border_color(border)
                            .child(h_flex().id("emulator-tabs").gap_8().children(
                                tabs.into_iter().map(|(ix, id, label, badge)| {
                                    let active = self.tab_ix == ix;
                                    v_flex()
                                        .id(ElementId::Name(format!("emulators-tab-{id}").into()))
                                        .debug_selector(move || format!("emulators-tab-{id}"))
                                        .cursor_pointer()
                                        .gap_2p5()
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            if this.tab_ix != ix {
                                                this.tab_ix = ix;
                                                cx.notify();
                                            }
                                        }))
                                        .child(
                                            h_flex()
                                                .gap_2()
                                                .items_center()
                                                .child(
                                                    div()
                                                        .text_xl()
                                                        .font_semibold()
                                                        .text_color(if active { fg } else { muted })
                                                        .hover(move |s| s.text_color(fg))
                                                        .child(label),
                                                )
                                                .children(badge),
                                        )
                                        .child(
                                            div()
                                                .h(px(3.))
                                                .w_full()
                                                .rounded_full()
                                                .when(active, |b| b.bg(primary)),
                                        )
                                }),
                            ))
                            .when_some(tab_status, |bar, status| {
                                bar.child(
                                    div()
                                        .pb_2p5()
                                        .text_xs()
                                        .text_color(muted)
                                        .truncate()
                                        .child(status),
                                )
                            }),
                    )
                    .child(if empty && self.tab_ix < 2 {
                        EmptyState::new(IconName::Joystick, t("manage.emulators.title"))
                            .into_any_element()
                    } else {
                        content
                    }),
            )
    }
}

/// What the user entered for the emulator they added in `status`, for the
/// edit dialog.
fn custom_definition(status: &EngineStatus) -> (String, irori_engines::custom::CustomEngine) {
    let m = &status.manifest;
    let program = status
        .installations
        .iter()
        .find(|i| i.origin == Origin::Custom)
        .map(|i| i.program.clone())
        .unwrap_or_default();
    (
        m.id.clone(),
        irori_engines::custom::CustomEngine {
            name: m.name.clone(),
            program,
            args: irori_engines::custom::args_line(&m.launch.args),
            systems: m.systems.clone(),
        },
    )
}

/// Why an engine has no Install button on this computer.
fn not_downloadable_text(m: &irori_engines::EngineManifest, host: Host) -> SharedString {
    // Engines Irori can find once the user installs them say how; engines
    // with builds only for other computers say which.
    let found_elsewhere = m.sources.iter().any(|s| !s.is_managed());
    let hosts = match irori_engines::EngineUnavailable::no_build(m, host).map(|e| e.reason) {
        Some(irori_engines::Unavailable::NoBuild { hosts }) => hosts,
        _ => Vec::new(),
    };
    if found_elsewhere || hosts.is_empty() {
        return t_args("manage.emulators.not_downloadable", &[("engine", &m.name)]);
    }
    let hosts: Vec<String> = hosts
        .iter()
        .map(|h| irori_engines::host::label(h))
        .collect();
    t_args(
        "manage.emulators.no_build",
        &[("engine", &m.name), ("hosts", &hosts.join(", "))],
    )
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::Arc;

    use irori_engines::Os;

    use super::*;

    fn built_in_manifest(id: &str) -> EngineManifest {
        let json = irori_engines::BUILT_IN
            .iter()
            .find(|(built_in_id, _)| *built_in_id == id)
            .map(|(_, json)| *json)
            .expect("built-in engine");
        EngineManifest::parse(json).expect("built-in manifest")
    }

    fn fake_status(
        id: &str,
        experimental: bool,
        installed: bool,
        with_sources: bool,
    ) -> EngineStatus {
        let mut manifest = built_in_manifest(id);
        manifest.experimental = experimental;
        if !with_sources {
            manifest.sources.clear();
        }
        let installations = if installed {
            vec![Installation {
                engine: id.into(),
                origin: Origin::Managed,
                version: Some("1.0".into()),
                dir: Some(PathBuf::from("/tmp/emu")),
                program: PathBuf::from("/tmp/emu/bin"),
                flatpak: None,
                size: 0,
            }]
        } else {
            Vec::new()
        };
        EngineStatus {
            manifest: Arc::new(manifest),
            installations,
            default: None,
            cores: Vec::new(),
        }
    }

    #[test]
    fn classify_engine_splits_installed_available_and_manual() {
        let linux = Host {
            os: Os::Linux,
            arch: "x86_64",
        };
        let installed = fake_status("duckstation", true, true, false);
        assert_eq!(classify_engine(&installed, linux), EngineSection::Installed);

        let available = fake_status("duckstation", false, false, true);
        assert_eq!(classify_engine(&available, linux), EngineSection::Available);

        let experimental = fake_status("duckstation", true, false, true);
        assert_eq!(classify_engine(&experimental, linux), EngineSection::Manual);

        let manual = fake_status("duckstation", false, false, false);
        assert_eq!(classify_engine(&manual, linux), EngineSection::Manual);
    }

    #[test]
    fn engine_category_icon_groups_systems() {
        let icon = |id: &str| engine_category_icon(&built_in_manifest(id));
        assert_eq!(icon("dosbox-staging"), IconName::SquareTerminal);
        assert_eq!(icon("scummvm"), IconName::SquareTerminal);
        assert_eq!(icon("mame"), IconName::Joystick);
        assert_eq!(icon("ppsspp"), IconName::GamepadDirectional);
        assert_eq!(icon("mgba"), IconName::GamepadDirectional);
        assert_eq!(icon("duckstation"), IconName::Gamepad2);
    }

    #[test]
    fn systems_attention_ignores_systems_without_games() {
        assert!(!system_needs_attention(0, false, true));
        assert!(!system_needs_attention(0, false, false));
        assert!(system_needs_attention(2, false, false));
        assert!(system_needs_attention(2, true, true));
        assert!(!system_needs_attention(2, true, false));
    }
}
