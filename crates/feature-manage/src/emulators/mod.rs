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
use irori_engines::{EngineKind, EngineManifest, EngineRef, Host, Installation, Origin, systems};
use irori_services::EngineStatus;
use irori_services::engine_health::EngineHealth;
use irori_ui::components::{EmptyState, PageHeader, SectionTitle};
use irori_ui::i18n::{t, t_args};
use irori_ui::{backend, models};

use crate::common::{card, card_grid_capped, info_tip, notify_error, plain_tooltip};
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
const INSTALLED_CARD_REM: f32 = 26.;
/// Narrowest an available or manual emulator card gets before dropping a column, in rem.
const COMPACT_CARD_REM: f32 = 21.;
/// Narrowest a system card gets before dropping a column, in rem.
const SYSTEM_CARD_REM: f32 = 20.;
/// Maximum system chips shown inline on an emulator card before "+N".
const MAX_SYSTEM_CHIPS: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EngineFilter {
    All,
    Installed,
    Available,
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

fn filter_pill(
    id: &'static str,
    label: SharedString,
    count: usize,
    active: bool,
    on_click: impl Fn(&gpui_kit::ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    Button::new(id)
        .small()
        .label(format!("{label} ({count})"))
        .map(|b| if active { b.primary() } else { b.outline() })
        .on_click(on_click)
        .into_any_element()
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
    show_other_systems: bool,
    installed_grid_width: Rc<Cell<Pixels>>,
    available_grid_width: Rc<Cell<Pixels>>,
    manual_grid_width: Rc<Cell<Pixels>>,
    folders_grid_width: Rc<Cell<Pixels>>,
    library_systems_grid_width: Rc<Cell<Pixels>>,
    other_systems_grid_width: Rc<Cell<Pixels>>,
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
        let retro = cx.new(|cx| retro::RetroView::new(window, cx));
        let mut this = Self {
            retro,
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
            show_other_systems: false,
            installed_grid_width: Rc::new(Cell::new(px(0.))),
            available_grid_width: Rc::new(Cell::new(px(0.))),
            manual_grid_width: Rc::new(Cell::new(px(0.))),
            folders_grid_width: Rc::new(Cell::new(px(0.))),
            library_systems_grid_width: Rc::new(Cell::new(px(0.))),
            other_systems_grid_width: Rc::new(Cell::new(px(0.))),
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
        let folders: Vec<AnyElement> = app
            .rom_folders
            .iter()
            .enumerate()
            .map(|(ix, folder)| {
                let remove = folder.clone();
                h_flex()
                    .id(ElementId::Name(format!("rom-folder-{ix}").into()))
                    .gap_3()
                    .child(Icon::new(IconName::Folder).small().text_color(muted))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_sm()
                            .truncate()
                            .child(folder.clone()),
                    )
                    .child(
                        Button::new("remove-folder")
                            .ghost()
                            .small()
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
        let games: usize = self.counts.values().sum();
        let summary = match (games, self.counts.len()) {
            (0, _) => t("manage.emulators.folders.none_found"),
            (1, _) => t("manage.emulators.folders.found_one"),
            (n, systems) => t_args(
                "manage.emulators.folders.found",
                &[("games", &n.to_string()), ("systems", &systems.to_string())],
            ),
        };
        let bios = app.bios_folder.trim().to_string();
        let folders_card = card(cx)
            .child(
                SectionTitle::new(t("manage.emulators.folders.title")).trailing(
                    h_flex()
                        .gap_1()
                        .child(
                            Button::new("rescan")
                                .ghost()
                                .small()
                                .icon(IconName::RefreshCw)
                                .label(t("manage.emulators.folders.rescan"))
                                .loading(self.scanning)
                                .disabled(self.scanning)
                                .on_click(
                                    cx.listener(|this, _, window, cx| this.rescan(window, cx)),
                                ),
                        )
                        .child(
                            Button::new("add-folder")
                                .small()
                                .icon(IconName::FolderPlus)
                                .label(t("manage.emulators.folders.add"))
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.pick_folder(false, window, cx)
                                })),
                        ),
                ),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(muted)
                    .child(t("manage.emulators.folders.description")),
            )
            .children(folders)
            .child(div().flex_1())
            .child(div().text_sm().child(if self.scanning {
                t("manage.emulators.folders.scanning")
            } else {
                summary
            }))
            .into_any_element();
        let bios_card = card(cx)
            .child(
                SectionTitle::new(t("manage.emulators.bios.title")).trailing(
                    Button::new("choose-bios")
                        .small()
                        .icon(IconName::FolderOpen)
                        .label(t("manage.emulators.bios.choose"))
                        .on_click(
                            cx.listener(|this, _, window, cx| this.pick_folder(true, window, cx)),
                        ),
                ),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(muted)
                    .child(t("manage.emulators.bios.description")),
            )
            .child(div().flex_1())
            .child(div().text_sm().truncate().child(if bios.is_empty() {
                t("manage.emulators.bios.not_chosen")
            } else {
                bios.into()
            }))
            .into_any_element();
        card_grid_capped(
            &self.folders_grid_width,
            24.,
            2,
            vec![folders_card, bios_card],
            window,
        )
        .into_any_element()
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

    fn render_system_chips(&self, manifest: &EngineManifest, cx: &App) -> AnyElement {
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
        let shown = short_names.iter().take(MAX_SYSTEM_CHIPS).cloned();
        let overflow = short_names.len().saturating_sub(MAX_SYSTEM_CHIPS);
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

    fn render_engine(&self, status: &EngineStatus, cx: &mut Context<Self>) -> AnyElement {
        let m = &status.manifest;
        let opening = open::openable(cx).read(cx).opening.contains(&m.id);
        let (foreground, muted, border, radius, background, secondary) = {
            let theme = cx.theme();
            (
                theme.foreground,
                theme.muted_foreground,
                theme.border,
                theme.radius,
                theme.background,
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

        let primary: AnyElement = if op.is_some() {
            let k = key.clone();
            Button::new("cancel")
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
            let label = match newest.as_ref().and_then(|r| r.size).and_then(format::size) {
                Some(size) => t_args("manage.emulators.install_size", &[("size", &size)]),
                None => t("manage.emulators.install"),
            };
            Button::new("install")
                .primary()
                .small()
                .icon(IconName::Download)
                .label(label)
                .on_click(move |_, _, cx| {
                    start_install(
                        InstallTarget::Engine {
                            id: id.clone(),
                            release: None,
                            make_default: true,
                        },
                        cx,
                    )
                })
                .into_any_element()
        } else if !status.is_installed() && !m.custom {
            let id = m.id.clone();
            Button::new("use-program")
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

        let installs: Vec<AnyElement> = status
            .installations
            .iter()
            .enumerate()
            .map(|(ix, i)| self.render_installation(ix, status, i, cx))
            .collect();
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

        let avatar_icon = irori_ui::components::StoreLogo::of_engine(&m.id)
            .map(|logo| logo.icon().text_color(foreground))
            .unwrap_or_else(|| Icon::new(engine_category_icon(m)).small().text_color(muted));
        let show_dot = status.is_installed() || health.is_some_and(|h| !h.missing.is_empty());
        let is_retroarch = m.kind == EngineKind::Retroarch;
        let has_options = !m.options.is_empty();
        let can_pick_program = !m.custom && (status.is_installed() || m.downloadable_on(host));
        let has_overflow = (!status.is_installed() && has_options)
            || is_retroarch
            || can_pick_program
            || !m.homepage.is_empty()
            || m.custom;
        let sel_id = m.id.clone();
        let view = cx.entity().downgrade();
        let engine_id = m.id.clone();
        let engine_name = m.name.clone();
        let homepage = m.homepage.clone();
        let existing_custom = m.custom.then(|| custom_definition(status));

        card(cx)
            .id(ElementId::Name(format!("engine-{}", m.id).into()))
            .debug_selector(move || format!("engine-{sel_id}"))
            .child(
                h_flex()
                    .gap_3()
                    .items_start()
                    .child(
                        div()
                            .size(rems(2.))
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
                                    .gap_2()
                                    .items_center()
                                    .flex_wrap()
                                    .when(show_dot, |row| row.child(health_dot(status, health, cx)))
                                    .child(div().font_semibold().truncate().child(m.name.clone()))
                                    .when(status.is_installed(), |d| {
                                        d.child(
                                            Tag::success()
                                                .xsmall()
                                                .child(t("manage.emulators.tag.installed")),
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
                                    }),
                            )
                            .when(!m.systems.is_empty(), |col| {
                                col.child(self.render_system_chips(m, cx))
                            }),
                    )
                    .when(!info_text.is_empty(), |hdr| {
                        hdr.child(info_tip(format!("engine-{}", m.id), info_text.into(), cx))
                    }),
            )
            .child(
                v_flex()
                    .flex_1()
                    .gap_2()
                    .children(health_lines(health, cx))
                    .when_some(op, |d, live| d.child(render_op_wide(&live, cx)))
                    .when_some(release_error, |d, e| {
                        d.child(
                            div().text_xs().text_color(muted).child(t_args(
                                "manage.emulators.releases_failed",
                                &[("error", &e)],
                            )),
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
                    .when(is_retroarch, |d| {
                        let running = self.ops.read(cx);
                        let installed_cores =
                            m.cores.iter().filter(|c| status.has_core(&c.id)).count();
                        let busy = m
                            .cores
                            .iter()
                            .any(|c| running.running.contains_key(&core_key(&c.id)));
                        d.child(
                            h_flex()
                                .gap_2()
                                .items_center()
                                .justify_between()
                                .px_3()
                                .py_2()
                                .rounded(radius)
                                .bg(background)
                                .border_1()
                                .border_color(border)
                                .child(
                                    h_flex()
                                        .flex_1()
                                        .min_w_0()
                                        .gap_2()
                                        .items_center()
                                        .when(busy, |row| row.child(Spinner::new().small()))
                                        .child(div().text_xs().font_medium().truncate().child(
                                            t_args(
                                                "manage.emulators.cores.title",
                                                &[
                                                    ("installed", &installed_cores.to_string()),
                                                    ("total", &m.cores.len().to_string()),
                                                ],
                                            ),
                                        )),
                                )
                                .child(
                                    Button::new("manage-retroarch-cores")
                                        .debug_selector(|| "manage-retroarch-cores".into())
                                        .small()
                                        .outline()
                                        .label(t("manage.emulators.cores.manage"))
                                        .on_click(|_, window, cx| {
                                            cores::open_cores_dialog(window, cx)
                                        }),
                                ),
                        )
                    }),
            )
            .child(
                h_flex()
                    .w_full()
                    .gap_2()
                    .flex_wrap()
                    .items_center()
                    .justify_between()
                    .pt_2()
                    .border_t_1()
                    .border_color(border)
                    .child(primary)
                    .child(
                        h_flex()
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
                                        .on_click(move |_, window, cx| {
                                            open_engine_settings(&id, window, cx)
                                        }),
                                )
                            })
                            .when(has_overflow, |row| {
                                let show_cfg = !status.is_installed() && has_options;
                                let more_id =
                                    ElementId::Name(format!("engine-more-{}", m.id).into());
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
                                                    PopupMenuItem::new(t(
                                                        "manage.emulators.configure",
                                                    ))
                                                    .icon(IconName::SlidersHorizontal)
                                                    .on_click(move |_, window, cx| {
                                                        open_engine_settings(&id, window, cx)
                                                    }),
                                                );
                                            }
                                            if is_retroarch {
                                                menu = menu.item(
                                                    PopupMenuItem::new(t(
                                                        "manage.emulators.cores.manage",
                                                    ))
                                                    .icon(IconName::Package)
                                                    .on_click(|_, window, cx| {
                                                        cores::open_cores_dialog(window, cx)
                                                    }),
                                                );
                                            }
                                            if can_pick_program {
                                                let id = engine_id.clone();
                                                let view = view.clone();
                                                menu = menu.item(
                                                    PopupMenuItem::new(t(
                                                        "manage.emulators.use_program",
                                                    ))
                                                    .icon(IconName::FolderOpen)
                                                    .on_click(move |_, window, cx| {
                                                        let _ = view.update(cx, |this, cx| {
                                                            this.pick_program(
                                                                id.clone(),
                                                                window,
                                                                cx,
                                                            )
                                                        });
                                                    }),
                                                );
                                            }
                                            if !homepage.is_empty() {
                                                let url = homepage.clone();
                                                menu = menu.item(
                                                    PopupMenuItem::new(t(
                                                        "manage.emulators.homepage",
                                                    ))
                                                    .icon(IconName::ExternalLink)
                                                    .on_click(move |_, _, cx| cx.open_url(&url)),
                                                );
                                            }
                                            if let Some(existing) = existing_custom.clone() {
                                                let id = engine_id.clone();
                                                let name = engine_name.clone();
                                                menu = menu
                                                    .item(
                                                        PopupMenuItem::new(t(
                                                            "manage.emulators.custom.edit",
                                                        ))
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
                                                        PopupMenuItem::new(t(
                                                            "manage.emulators.custom.remove",
                                                        ))
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
                            }),
                    ),
            )
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
            return h_flex()
                .gap_2()
                .text_sm()
                .text_color(muted)
                .child(Spinner::new().small())
                .child(t("manage.emulators.loading"))
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
        let updates_count = statuses
            .iter()
            .filter(|s| self.latest_update(s).is_some())
            .count();

        let pills = h_flex()
            .gap_2()
            .flex_wrap()
            .child(filter_pill(
                "engines-filter-all",
                t("manage.emulators.filter.all"),
                all_count,
                self.engine_filter == EngineFilter::All,
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
                cx.listener(|this, _, _, cx| {
                    this.engine_filter = EngineFilter::Available;
                    cx.notify();
                }),
            ))
            .when(
                updates_count > 0 || self.engine_filter == EngineFilter::Updates,
                |row| {
                    row.child(filter_pill(
                        "engines-filter-updates",
                        t("manage.emulators.filter.updates"),
                        updates_count,
                        self.engine_filter == EngineFilter::Updates,
                        cx.listener(|this, _, _, cx| {
                            this.engine_filter = EngineFilter::Updates;
                            cx.notify();
                        }),
                    ))
                },
            );

        let mut installed_cards = Vec::new();
        let mut available_cards = Vec::new();
        let mut manual_cards = Vec::new();

        for status in statuses.iter().filter(matches_query) {
            let section = classify_engine(status, host);
            let matches_filter = match self.engine_filter {
                EngineFilter::All => true,
                EngineFilter::Installed => section == EngineSection::Installed,
                EngineFilter::Available => section == EngineSection::Available,
                EngineFilter::Updates => self.latest_update(status).is_some(),
            };
            if !matches_filter {
                continue;
            }
            let rendered = self.render_engine(status, cx);
            match section {
                EngineSection::Installed => installed_cards.push(rendered),
                EngineSection::Available => available_cards.push(rendered),
                EngineSection::Manual => manual_cards.push(rendered),
            }
        }

        let empty =
            installed_cards.is_empty() && available_cards.is_empty() && manual_cards.is_empty();

        v_flex()
            .gap_5()
            .child(pills)
            .when(empty, |col| {
                col.child(
                    div()
                        .text_sm()
                        .text_color(muted)
                        .child(t("manage.emulators.no_match")),
                )
            })
            .when(!installed_cards.is_empty(), |col| {
                col.child(
                    v_flex()
                        .gap_3()
                        .child(SectionTitle::new(t("manage.emulators.section.installed")))
                        .child(card_grid_capped(
                            &self.installed_grid_width,
                            INSTALLED_CARD_REM,
                            2,
                            installed_cards,
                            window,
                        )),
                )
            })
            .when(!available_cards.is_empty(), |col| {
                col.child(
                    v_flex()
                        .gap_3()
                        .child(SectionTitle::new(t("manage.emulators.section.available")))
                        .child(card_grid_capped(
                            &self.available_grid_width,
                            COMPACT_CARD_REM,
                            3,
                            available_cards,
                            window,
                        )),
                )
            })
            .when(!manual_cards.is_empty(), |col| {
                col.child(
                    v_flex()
                        .gap_3()
                        .child(SectionTitle::new(t("manage.emulators.section.manual")))
                        .child(card_grid_capped(
                            &self.manual_grid_width,
                            COMPACT_CARD_REM,
                            3,
                            manual_cards,
                            window,
                        )),
                )
            })
            .into_any_element()
    }

    fn render_systems(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let Some(statuses) = &self.statuses else {
            return Spinner::new().small().into_any_element();
        };
        let b = backend(cx);
        let query = self.query(cx);
        // Systems with games first, each group in catalog order.
        let mut ordered: Vec<&irori_engines::System> = systems::all().iter().collect();
        ordered.sort_by_key(|s| !self.counts.contains_key(&s.id));

        let mut all_count = 0usize;
        let mut with_games_count = 0usize;
        let mut attention_count = 0usize;
        let mut library_cards = Vec::new();
        let mut other_cards = Vec::new();

        for system in ordered {
            all_count += 1;
            let count = self.counts.get(&system.id).copied().unwrap_or(0);
            if count > 0 {
                with_games_count += 1;
            }
            let choice = b.system_engine(&system.id, statuses);
            let ready = choice.as_ref().is_some_and(|c| {
                statuses
                    .iter()
                    .any(|s| s.manifest.id == c.engine && s.plays(c))
            });
            let bios_line = self.bios.get(&system.id).and_then(bios_line);
            let bios_warn = bios_line.as_ref().is_some_and(|(warn, _)| *warn);
            let needs_attention = (count > 0 && !ready) || bios_warn;
            if needs_attention {
                attention_count += 1;
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

            let engine_installed = choice.as_ref().is_some_and(|c| {
                statuses
                    .iter()
                    .any(|s| s.manifest.id == c.engine && s.is_installed())
            });
            let state: AnyElement = if ready {
                Tag::success()
                    .xsmall()
                    .child(t("manage.emulators.system.ready"))
                    .into_any_element()
            } else {
                Tag::warning()
                    .xsmall()
                    .child(t("manage.emulators.system.no_engine"))
                    .into_any_element()
            };
            let fix: Option<AnyElement> = (!ready).then(|| {
                let choice = choice.clone();
                let name = choice.as_ref().map(choice_name).unwrap_or_default();
                Button::new("install-for-system")
                    .primary()
                    .small()
                    .icon(IconName::Download)
                    .label(t_args(
                        "manage.emulators.system.install_engine",
                        &[("engine", &name)],
                    ))
                    .on_click(move |_, _, cx| {
                        let Some(c) = choice.clone() else { return };
                        if !engine_installed {
                            start_install(
                                InstallTarget::Engine {
                                    id: c.engine.clone(),
                                    release: None,
                                    make_default: true,
                                },
                                cx,
                            );
                        }
                        if let Some(core) = c.core {
                            start_install(InstallTarget::Core { id: core }, cx);
                        }
                    })
                    .into_any_element()
            });
            let count_text = match count {
                0 => t("manage.emulators.system.no_games"),
                1 => t("manage.emulators.system.games_one"),
                n => t_args(
                    "manage.emulators.system.games_other",
                    &[("count", &n.to_string())],
                ),
            };
            let rendered = card(cx)
                .id(ElementId::Name(format!("system-{}", system.id).into()))
                .child(
                    h_flex()
                        .gap_2()
                        .items_start()
                        .child(
                            v_flex()
                                .flex_1()
                                .min_w_0()
                                .child(div().font_semibold().truncate().child(system.name.clone()))
                                .child(div().text_xs().text_color(muted).child(format!(
                                    "{} · {} · {count_text}",
                                    system.maker, system.year
                                ))),
                        )
                        .child(state),
                )
                .child(
                    v_flex()
                        .flex_1()
                        .gap_2()
                        .child(
                            v_flex()
                                .gap_1()
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(muted)
                                        .child(t("manage.emulators.system.default")),
                                )
                                .when_some(self.system_selects.get(&system.id), |d, select| {
                                    d.child(Select::new(select).small())
                                }),
                        )
                        .when_some(bios_line, |d, (warn, text)| {
                            d.child(
                                h_flex()
                                    .gap_2()
                                    .items_start()
                                    .text_xs()
                                    .child(
                                        Icon::new(if warn {
                                            IconName::TriangleAlert
                                        } else {
                                            IconName::CircleCheck
                                        })
                                        .xsmall()
                                        .text_color(
                                            if warn {
                                                cx.theme().warning
                                            } else {
                                                cx.theme().success
                                            },
                                        ),
                                    )
                                    .child(div().flex_1().min_w_0().child(text)),
                            )
                        }),
                )
                .when_some(fix, |d, fix| d.child(fix))
                .into_any_element();

            if count > 0 || bios_warn {
                library_cards.push(rendered);
            } else {
                other_cards.push(rendered);
            }
        }

        let pills = h_flex()
            .gap_2()
            .flex_wrap()
            .child(filter_pill(
                "systems-filter-all",
                t("manage.emulators.filter.all"),
                all_count,
                self.system_filter == SystemFilter::All,
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
                        cx.listener(|this, _, _, cx| {
                            this.system_filter = SystemFilter::NeedsAttention;
                            cx.notify();
                        }),
                    ))
                },
            );

        let has_library = !library_cards.is_empty();
        let has_other = !other_cards.is_empty();
        let can_collapse_others =
            has_library && query.is_empty() && self.system_filter == SystemFilter::All;
        let others_expanded = !can_collapse_others || self.show_other_systems;
        let other_count = other_cards.len();
        let empty = !has_library && !has_other;

        v_flex()
            .gap_5()
            .child(
                div()
                    .text_sm()
                    .text_color(muted)
                    .child(t("manage.emulators.systems.description")),
            )
            .child(self.render_folders(window, cx))
            .child(pills)
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
                        .child(card_grid_capped(
                            &self.library_systems_grid_width,
                            SYSTEM_CARD_REM,
                            3,
                            library_cards,
                            window,
                        )),
                )
            })
            .when(has_other, |col| {
                let title = if !has_library {
                    SectionTitle::new(t("manage.emulators.systems.section.other"))
                } else {
                    SectionTitle::new(t("manage.emulators.systems.section.other")).when(
                        can_collapse_others,
                        |s| {
                            s.trailing(
                                Button::new("toggle-other-systems")
                                    .ghost()
                                    .small()
                                    .icon(if others_expanded {
                                        IconName::ChevronUp
                                    } else {
                                        IconName::ChevronDown
                                    })
                                    .label(if others_expanded {
                                        t("manage.emulators.systems.hide_others")
                                    } else {
                                        t_args(
                                            "manage.emulators.systems.show_others",
                                            &[("count", &other_count.to_string())],
                                        )
                                    })
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.show_other_systems = !this.show_other_systems;
                                        cx.notify();
                                    })),
                            )
                        },
                    )
                };
                col.child(v_flex().gap_3().child(title).when(others_expanded, |sec| {
                    sec.child(card_grid_capped(
                        &self.other_systems_grid_width,
                        SYSTEM_CARD_REM,
                        3,
                        other_cards,
                        window,
                    ))
                }))
            })
            .into_any_element()
    }
}

/// The BIOS line of a system card: whether it is a warning, and its text.
/// `None` for systems that need no BIOS.
pub(crate) fn bios_line(status: &SystemBios) -> Option<(bool, SharedString)> {
    if status.files.is_empty() {
        return None;
    }
    let names = |state: FileState| {
        status
            .files
            .iter()
            .filter(|f| f.state == state)
            .map(|f| match &f.region {
                Some(r) => format!("{} ({r})", f.name),
                None => f.name.clone(),
            })
            .collect::<Vec<_>>()
            .join(", ")
    };
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
fn health_lines(health: Option<&EngineHealth>, cx: &App) -> Vec<AnyElement> {
    let Some(health) = health else {
        return Vec::new();
    };
    let theme = cx.theme();
    let mut lines = Vec::new();
    if health.games > 0 {
        let count = health.games.to_string();
        lines.push(
            div()
                .text_sm()
                .text_color(theme.muted_foreground)
                .child(if health.games == 1 {
                    t_args("manage.emulators.health.games_one", &[("count", &count)])
                } else {
                    t_args("manage.emulators.health.games_other", &[("count", &count)])
                })
                .into_any_element(),
        );
    }
    for program in &health.missing {
        lines.push(
            h_flex()
                .gap_1()
                .min_w_0()
                .text_sm()
                .text_color(theme.warning)
                .child(Icon::new(IconName::TriangleAlert).xsmall())
                .child(div().min_w_0().truncate().child(t_args(
                    "manage.emulators.health.missing",
                    &[("path", &program.display().to_string())],
                )))
                .into_any_element(),
        );
    }
    lines
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
        let installed_engines = self
            .statuses
            .as_ref()
            .map_or(0, |s| s.iter().filter(|st| st.is_installed()).count());
        let updates_count = self.statuses.as_ref().map_or(0, |s| {
            s.iter()
                .filter(|st| self.latest_update(st).is_some())
                .count()
        });
        let b = backend(cx);
        let systems_attention = self.statuses.as_ref().map_or(0, |statuses| {
            systems::all()
                .iter()
                .filter(|sys| {
                    let count = self.counts.get(&sys.id).copied().unwrap_or(0);
                    let ready = b.system_engine(&sys.id, statuses).is_some_and(|c| {
                        statuses
                            .iter()
                            .any(|s| s.manifest.id == c.engine && s.plays(&c))
                    });
                    let bios_warn = self
                        .bios
                        .get(&sys.id)
                        .and_then(bios_line)
                        .is_some_and(|(w, _)| w);
                    (count > 0 && !ready) || bios_warn
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
                    Tag::warning()
                        .xsmall()
                        .child(systems_attention.to_string())
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
                                    .child(
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
                                    .child(
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
                                    .when(updates_count > 0, |row| {
                                        row.child(
                                            Tag::warning()
                                                .xsmall()
                                                .child(updates_count.to_string()),
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
                            .when(self.tab_ix < 2, |bar| {
                                bar.child(
                                    div()
                                        .w(rems(18.))
                                        .flex_shrink_0()
                                        .pb_2()
                                        .debug_selector(|| "emulators-search".into())
                                        .child(Input::new(&self.search).small()),
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
}
