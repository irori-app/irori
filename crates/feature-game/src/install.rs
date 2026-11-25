//! The install modal (new installs) and its Modify mode (language, branch,
//! build and DLCs of an installed game), centered with game artwork, live
//! storage metrics and progressive disclosure.

use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::Arc;

use futures::StreamExt as _;
use gpui_kit::StatefulInteractiveElement as _;
use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::dialog::{Confirm, DialogEntrance};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::progress::Progress;
use gpui_kit::component::radio::RadioGroup;
use gpui_kit::component::select::{Select, SelectEvent};
use gpui_kit::component::skeleton::Skeleton;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::tag::Tag;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, Sizable as _, StyledExt as _, WindowExt as _,
    alert::Alert, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, BoxShadow, Context, Entity, Hsla, InteractiveElement as _,
    IntoElement, ParentElement, Pixels, Render, Role, SharedString, Styled, Subscription, Task,
    Window, div, linear_color_stop, linear_gradient, point, px, relative, rems,
};
use irori_core::util::format_bytes;
use irori_core::{
    GameId, GameInfo, InstallInfo, InstallInfoQuery, InstallPlatform, InstallRequest, Runner,
    UpdateRequest, WineInstallation,
};
use irori_extras::anticheat::InstallGate;
use irori_runtime::manager::{self, ReleaseKind};
use irori_ui::components::{GameCover, StoreLogo};
use irori_ui::i18n::{t, t_args};
use irori_ui::style::{
    ICON_CONTRAST, KeyboardFocusable as _, TEXT_CONTRAST, art_tint, legible, reduce_motion,
    visible_size,
};
use irori_ui::theme::reduce_transparency;
use irori_ui::wine_choice::build_label;
use irori_ui::wine_installs::{self, Outcome};
use irori_ui::{Route, backend, navigate};

use crate::choice::{Choice, ChoiceSelect, choice_select, set_choices};
use crate::hooks::{self, AnticheatStatus, CompatibilityInfo, SteamDeckStatus};
use crate::i18n::platform_label;
use crate::load::{Load, language_name, preferred_language};
use crate::ops::{notify_error, pick_path};
use crate::sizes::{
    DiskSpace, DiskVerdict, InstallSize, bar_summary, detect_extra_drives, disk_bar_fractions,
    disk_space, drive_label, free_space, install_button_label, install_size, modify_disk_verdict,
    same_dir,
};
use crate::state::is_unsupported_third_party;

/// Repaint interval while GE-Proton downloads.
const PROTON_TICK: std::time::Duration = std::time::Duration::from_millis(300);

/// The GE-Proton download offered when no Wine or Proton build exists.
#[derive(Debug, Clone, PartialEq)]
enum ProtonDownload {
    /// Offered, not asked for.
    Offered,
    /// Looking up the newest release.
    Finding,
    /// Installing through the Wine manager, by its install key.
    Running(String),
    Failed(SharedString),
}

/// Whether the modal installs a new game or modifies an installed one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallMode {
    Install,
    Modify,
}

/// A selectable destination chip (configured library folder or auto-detected drive).
#[derive(Debug, Clone)]
struct InstallTarget {
    label: SharedString,
    base_path: String,
    free: Option<u64>,
}

/// State of the install / modify modal. Owns its inputs and selects; the hero
/// header and footer are separate views observing this one so they stay pinned
/// while the body scrolls.
pub struct InstallSheet {
    id: GameId,
    game: Arc<GameInfo>,
    mode: InstallMode,
    platforms: Vec<InstallPlatform>,
    platform: InstallPlatform,
    /// Accounts owning the game (id, name), when more than one does.
    owners: Vec<(String, String)>,
    /// The account to install with; the first owner by default.
    account: Option<String>,
    path: Entity<InputState>,
    /// Configured library folders plus extra mounted drives detected in background.
    install_targets: Vec<InstallTarget>,
    info: Load<InstallInfo>,
    info_rev: u64,
    dlc_sizes: HashMap<String, InstallSize>,
    selected_dlcs: BTreeSet<String>,
    language: Option<String>,
    language_select: Entity<ChoiceSelect>,
    sdl: BTreeSet<String>,
    branch: Option<String>,
    branch_select: Entity<ChoiceSelect>,
    pin_build: bool,
    build: Option<String>,
    build_select: Entity<ChoiceSelect>,
    password: Entity<InputState>,
    password_busy: bool,
    wines: Load<Vec<WineInstallation>>,
    wine: Option<WineInstallation>,
    wine_select: Entity<ChoiceSelect>,
    proton: ProtonDownload,
    _proton_ticker: Option<Task<()>>,
    prefix: Entity<InputState>,
    disk: Load<DiskSpace>,
    disk_rev: u64,
    /// Defaults for DLCs and components were applied once.
    selection_made: bool,
    /// Whether the collapsible "More options" card is expanded.
    options_expanded: bool,
    compat: Option<CompatibilityInfo>,
    anticheat_ack: bool,
    submitting: bool,
    _subscriptions: Vec<Subscription>,
}

/// Open the install modal of a game. Defaults to the native Linux build when
/// the store offers one.
pub fn open_install_sheet(id: GameId, window: &mut Window, cx: &mut App) {
    open_sheet(id, InstallMode::Install, window, cx);
}

/// Open the modal that changes language, branch, build and DLCs of an
/// installed game.
pub fn open_modify_sheet(id: GameId, window: &mut Window, cx: &mut App) {
    open_sheet(id, InstallMode::Modify, window, cx);
}

fn open_sheet(id: GameId, mode: InstallMode, window: &mut Window, cx: &mut App) {
    // Steam installs its own games, with its own dialog.
    if !id.runner.manages_installs() {
        if mode == InstallMode::Install {
            crate::ops::client_action(&id, irori_core::ClientAction::Install, window, cx);
        }
        return;
    }
    let Some(game) = backend(cx).game(&id) else {
        notify_error(
            window,
            cx,
            t("game.install.open_failed"),
            &anyhow::anyhow!(t_args(
                "game.install.not_in_library",
                &[("id", &id.app_name)]
            )),
        );
        return;
    };
    let sheet = cx.new(|cx| InstallSheet::new(game, mode, window, cx));
    let header = cx.new(|cx| InstallHeader::new(sheet.clone(), cx));
    let footer = cx.new(|cx| InstallFooter::new(sheet.clone(), cx));
    window.open_dialog(cx, move |d, window, cx| {
        let entrance = if reduce_motion(cx) {
            DialogEntrance::None
        } else {
            DialogEntrance::FadeSlide
        };
        d.width(modal_width(window))
            .margin_top(modal_margin_top(window))
            .p_0()
            .gap_0()
            .overflow_hidden()
            .close_button(false)
            .entrance(entrance)
            .on_ok(|_, _, _| false)
            .title(header.clone())
            .child(sheet.clone())
            .footer(footer.clone())
    });
}

/// Whether the store's answer leaves nothing to download (a GOG goodie
/// pack, say). A game another launcher installs (EA app, Ubisoft Connect)
/// has no sizes either, yet Irori installs it by starting that launcher, so
/// it never counts (#167).
pub(crate) fn nothing_to_install(info: &InstallInfo, game: &GameInfo) -> bool {
    let launcher = info
        .third_party_managed_app
        .as_ref()
        .or(game.third_party_managed_app.as_ref());
    launcher.is_none()
        && info.download_size == 0
        && info.disk_size == 0
        && info.language_sizes.is_empty()
}

/// The explanation for [`nothing_to_install`]: only GOG's mentions GOG's
/// bonus content.
pub(crate) fn nothing_to_install_text(runner: Runner) -> &'static str {
    match runner {
        Runner::Gog => "game.install.nothing_to_install",
        _ => "game.install.nothing_to_install_other",
    }
}

/// The modal's width: 48 rem, at most 92% of the visible window.
fn modal_width(window: &Window) -> Pixels {
    let visible = visible_size(window).width;
    (window.rem_size() * 48.).min(visible * 0.92)
}

/// Top offset that centers compact modals nicely while letting taller ones snap up.
fn modal_margin_top(window: &Window) -> Pixels {
    let visible = visible_size(window).height;
    (visible * 0.16).clamp(px(36.), px(160.))
}

/// Whether the "More options" card should start or become expanded.
fn should_auto_expand_options(
    mode: InstallMode,
    needs_wine: bool,
    wines_empty: bool,
    proton: &ProtonDownload,
) -> bool {
    mode == InstallMode::Modify
        || (needs_wine
            && (wines_empty
                || matches!(
                    proton,
                    ProtonDownload::Finding
                        | ProtonDownload::Running(_)
                        | ProtonDownload::Failed(_)
                )))
}

/// Platforms the user can install on this host, native Linux first.
pub fn installable_platforms(game: &GameInfo) -> Vec<InstallPlatform> {
    let mut out = Vec::new();
    let native = irori_core::HostPlatform::current().native();
    if native != InstallPlatform::Windows
        && (game.platforms.contains(&native)
            || (native == InstallPlatform::Linux && game.is_linux_native)
            || (native == InstallPlatform::Mac && game.is_mac_native))
    {
        out.push(native);
    }
    if game.platforms.is_empty() || game.platforms.contains(&InstallPlatform::Windows) {
        out.push(InstallPlatform::Windows);
    }
    if game.platforms.contains(&InstallPlatform::Browser) {
        out.push(InstallPlatform::Browser);
    }
    if out.is_empty() {
        out.push(InstallPlatform::Windows);
    }
    out
}

impl InstallSheet {
    pub fn new(
        game: Arc<GameInfo>,
        mode: InstallMode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let b = backend(cx);
        let id = game.id();
        let app = b.settings.app();
        let settings = b.settings.game(&id);
        let platforms = match (mode, game.installed_platform()) {
            (InstallMode::Modify, Some(p)) => vec![p],
            _ => installable_platforms(&game),
        };
        let platform = platforms.first().copied().unwrap_or_default();
        let install = game.install.as_ref();
        let owners: Vec<(String, String)> = if mode == InstallMode::Install && !game.is_installed {
            let accounts = irori_ui::models(cx).accounts.read(cx);
            let accounts = accounts.accounts(id.runner);
            b.library()
                .owners(&id)
                .iter()
                .map(|owner| {
                    let name = accounts
                        .iter()
                        .find(|a| a.account_id == *owner)
                        .map(|a| a.display_name.clone())
                        .filter(|n| !n.trim().is_empty())
                        .unwrap_or_else(|| owner.clone());
                    (owner.clone(), name)
                })
                .collect()
        } else {
            Vec::new()
        };
        let owners = if owners.len() > 1 { owners } else { Vec::new() };
        let account = owners.first().map(|(id, _)| id.clone());

        let default_path = app.install_folder(id.runner);
        let path = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(t("game.install.path_placeholder"))
                .default_value(default_path)
        });
        let install_targets: Vec<InstallTarget> = if mode == InstallMode::Install {
            app.library_folders()
                .into_iter()
                .filter(|f| !f.path.trim().is_empty())
                .map(|f| {
                    let expanded = irori_core::paths::expand_home(f.path.trim());
                    let base_label = irori_ui::library_folders::label(&f);
                    let label: SharedString = if f.is_default() {
                        let drive = drive_label(&expanded);
                        if drive.is_empty() {
                            base_label
                        } else {
                            format!("{base_label} ({drive})").into()
                        }
                    } else {
                        base_label
                    };
                    InstallTarget {
                        label,
                        base_path: f.path,
                        free: None,
                    }
                })
                .collect()
        } else {
            Vec::new()
        };
        let password = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(t("game.install.password_placeholder"))
                .masked(true)
        });
        let prefix_default = if settings.wine_prefix.is_empty() {
            irori_runtime::default_prefix(&app, &game)
                .to_string_lossy()
                .into_owned()
        } else {
            settings.wine_prefix.clone()
        };
        let prefix = cx.new(|cx| InputState::new(window, cx).default_value(prefix_default));

        let language_select = choice_select(Vec::new(), None, window, cx);
        let branch_select = choice_select(Vec::new(), None, window, cx);
        let build_select = choice_select(Vec::new(), None, window, cx);
        let wine_select = choice_select(Vec::new(), None, window, cx);

        let subscriptions = vec![
            cx.subscribe_in(&path, window, |this, _, e: &InputEvent, window, cx| {
                if matches!(e, InputEvent::Change) {
                    this.refresh_disk(window, cx);
                }
            }),
            cx.subscribe_in(
                &language_select,
                window,
                |this, _, e: &SelectEvent<Vec<Choice>>, _, cx| {
                    let SelectEvent::Confirm(v) = e;
                    this.language = v.as_ref().map(|v| v.to_string());
                    cx.notify();
                },
            ),
            cx.subscribe_in(
                &branch_select,
                window,
                |this, _, e: &SelectEvent<Vec<Choice>>, window, cx| {
                    let SelectEvent::Confirm(v) = e;
                    let next = v.as_ref().map(|v| v.to_string()).filter(|v| !v.is_empty());
                    if next != this.branch {
                        this.branch = next;
                        this.build = None;
                        this.fetch_info(window, cx);
                    }
                },
            ),
            cx.subscribe_in(
                &build_select,
                window,
                |this, _, e: &SelectEvent<Vec<Choice>>, window, cx| {
                    let SelectEvent::Confirm(v) = e;
                    let next = v.as_ref().map(|v| v.to_string());
                    if next != this.build {
                        this.build = next;
                        // Sizes and languages differ between builds.
                        this.fetch_info(window, cx);
                    }
                },
            ),
            cx.subscribe_in(
                &wine_select,
                window,
                |this, _, e: &SelectEvent<Vec<Choice>>, _, cx| {
                    let SelectEvent::Confirm(v) = e;
                    if let (Some(bin), Some(list)) = (v, this.wines.ready()) {
                        this.wine = list.iter().find(|w| w.bin == bin.as_ref()).cloned();
                    }
                    cx.notify();
                },
            ),
        ];

        let options_expanded = should_auto_expand_options(
            mode,
            platform.needs_wine(),
            false,
            &ProtonDownload::Offered,
        );

        let mut this = Self {
            id,
            mode,
            platforms,
            platform,
            owners,
            account,
            path,
            install_targets,
            info: Load::Loading,
            info_rev: 0,
            dlc_sizes: HashMap::new(),
            selected_dlcs: BTreeSet::new(),
            language: install.and_then(|i| i.language.clone()),
            language_select,
            sdl: BTreeSet::new(),
            branch: install.and_then(|i| i.branch.clone()),
            branch_select,
            pin_build: install.is_some_and(|i| i.pinned_version),
            build: install.and_then(|i| i.build_id.clone()),
            build_select,
            password,
            password_busy: false,
            wines: Load::Idle,
            wine: settings
                .wine_version
                .is_set()
                .then(|| settings.wine_version.clone()),
            wine_select,
            proton: ProtonDownload::Offered,
            _proton_ticker: None,
            prefix,
            disk: Load::Idle,
            disk_rev: 0,
            selection_made: false,
            options_expanded,
            compat: None,
            anticheat_ack: false,
            submitting: false,
            game,
            _subscriptions: subscriptions,
        };
        let installs = wine_installs::wine_ops(cx);
        this._subscriptions
            .push(cx.observe_in(&installs, window, |this, _, window, cx| {
                this.proton_progressed(window, cx)
            }));
        this.fetch_info(window, cx);
        this.fetch_wines(window, cx);
        this.fetch_compat(window, cx);
        this.refresh_disk(window, cx);
        this.probe_library_folders(window, cx);
        this.check_prefix(window, cx);
        this
    }

    fn is_modify(&self) -> bool {
        self.mode == InstallMode::Modify
    }

    fn sync_auto_expand(&mut self) {
        let wines_empty = self.wines.ready().is_some_and(|w| w.is_empty());
        if should_auto_expand_options(
            self.mode,
            self.platform.needs_wine(),
            wines_empty,
            &self.proton,
        ) {
            self.options_expanded = true;
        }
    }

    fn set_platform(
        &mut self,
        platform: InstallPlatform,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if platform != self.platform {
            self.platform = platform;
            self.branch = None;
            self.build = None;
            self.sync_auto_expand();
            self.fetch_info(window, cx);
        }
    }

    fn select_install_target(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(target) = self.install_targets.get(index) else {
            return;
        };
        let app = irori_ui::models(cx).settings.read(cx).app.clone();
        let dir = app.install_folder_in(&target.base_path, self.id.runner);
        self.path
            .update(cx, |state, cx| state.set_value(dir, window, cx));
        self.refresh_disk(window, cx);
    }

    fn probe_library_folders(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_modify() {
            return;
        }
        let configured: Vec<PathBuf> = self
            .install_targets
            .iter()
            .map(|t| irori_core::paths::expand_home(t.base_path.trim()))
            .collect();
        let task = cx.background_spawn(async move {
            let spaces: Vec<(usize, u64)> = configured
                .iter()
                .enumerate()
                .filter_map(|(ix, path)| free_space(path).ok().map(|free| (ix, free)))
                .collect();
            let extras = detect_extra_drives(&configured);
            (spaces, extras)
        });
        cx.spawn_in(window, async move |this, cx| {
            let (spaces, extras) = task.await;
            let _ = this.update(cx, |this, cx| {
                for (ix, free) in spaces {
                    if let Some(target) = this.install_targets.get_mut(ix) {
                        target.free = Some(free);
                    }
                }
                for drive in extras {
                    let already = this.install_targets.iter().any(|t| {
                        same_dir(
                            &irori_core::paths::expand_home(t.base_path.trim()),
                            &drive.mount,
                        )
                    });
                    if !already {
                        this.install_targets.push(InstallTarget {
                            label: drive.label.into(),
                            base_path: drive.suggested_base,
                            free: Some(drive.free),
                        });
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// The store offers packages to choose between rather than builds to
    /// pin (the Microsoft Store's PC and Xbox packages, editions).
    fn package_choice(&self) -> bool {
        self.id.runner == Runner::Xbox && !self.is_modify()
    }

    /// The build to install or describe: a pinned one, or the chosen package.
    fn chosen_build(&self) -> Option<String> {
        self.build
            .clone()
            .filter(|_| self.pin_build || self.package_choice())
    }

    fn fetch_info(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.info_rev += 1;
        let rev = self.info_rev;
        self.info = Load::Loading;
        self.dlc_sizes.clear();
        cx.notify();
        let b = backend(cx);
        let id = self.id.clone();
        let platform = self.platform;
        let query = InstallInfoQuery {
            branch: self.branch.clone(),
            build: self.chosen_build(),
            language: None,
        };
        let task = irori_ui::run(cx, async move {
            let store = b
                .store(id.runner)
                .ok_or_else(|| crate::i18n::store_unavailable(id.runner))?;
            store.install_info(&id.app_name, platform, &query).await
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                if this.info_rev == rev {
                    this.apply_info(result, window, cx);
                }
            });
        })
        .detach();
    }

    fn apply_info(
        &mut self,
        result: anyhow::Result<InstallInfo>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let info = match result {
            Ok(info) => info,
            Err(e) => {
                self.info = Load::Failed(format!("{e:#}").into());
                cx.notify();
                return;
            }
        };
        let ui_language = backend(cx).settings.app().ui_language.clone();
        self.language = preferred_language(&info.languages, self.language.as_deref(), &ui_language);
        let mut languages = info.languages.clone();
        languages.sort();
        let items = languages
            .iter()
            .map(|l| Choice::new(l.clone(), language_name(l)))
            .collect();
        set_choices(
            &self.language_select,
            items,
            self.language.as_deref(),
            window,
            cx,
        );

        let branches = std::iter::once(Choice::new("", t("game.install.branch_default")))
            .chain(
                info.branches
                    .iter()
                    .filter(|b| !b.is_empty())
                    .map(|b| Choice::new(b.clone(), b.clone())),
            )
            .collect();
        set_choices(
            &self.branch_select,
            branches,
            Some(self.branch.as_deref().unwrap_or("")),
            window,
            cx,
        );

        let builds: Vec<Choice> = info
            .builds
            .iter()
            .filter(|b| b.branch == self.branch)
            .map(|b| {
                let date = b.date.chars().take(10).collect::<String>();
                let label = if date.is_empty() {
                    b.version_name.clone()
                } else {
                    format!("{} · {date}", b.version_name)
                };
                Choice::new(b.build_id.clone(), label)
            })
            .collect();
        if self.build.is_none()
            || !builds
                .iter()
                .any(|c| Some(c.value_str()) == self.build.as_deref())
        {
            self.build = builds.first().map(|c| c.value_str().to_string());
        }
        set_choices(
            &self.build_select,
            builds,
            self.build.as_deref(),
            window,
            cx,
        );

        let owned: BTreeSet<String> = info.dlcs.iter().map(|d| d.app_name.clone()).collect();
        if self.selection_made {
            // A refetch (branch, build, password) keeps the user's choices.
            let sdl_ids: BTreeSet<String> = info.sdl.iter().map(|o| o.id.clone()).collect();
            self.sdl.retain(|id| sdl_ids.contains(id));
            self.selected_dlcs.retain(|d| owned.contains(d));
        } else {
            self.selection_made = true;
            self.sdl = info
                .sdl
                .iter()
                .filter(|o| o.mandatory || o.default_selected)
                .map(|o| o.id.clone())
                .collect();
            self.selected_dlcs = self.default_dlcs(&owned);
        }
        let dlc_ids: Vec<String> = info.dlcs.iter().map(|d| d.app_name.clone()).collect();
        self.info = Load::Ready(info);
        self.fetch_dlc_sizes(dlc_ids, window, cx);
        cx.notify();
    }

    /// Owned DLCs preselected for a new install; installed ones when modifying.
    fn default_dlcs(&self, owned: &BTreeSet<String>) -> BTreeSet<String> {
        match self.mode {
            InstallMode::Install => owned.clone(),
            InstallMode::Modify => self
                .game
                .install
                .as_ref()
                .map(|i| {
                    i.installed_dlcs
                        .iter()
                        .filter(|d| owned.contains(*d))
                        .cloned()
                        .collect()
                })
                .unwrap_or_default(),
        }
    }

    fn fetch_dlc_sizes(&mut self, dlcs: Vec<String>, window: &mut Window, cx: &mut Context<Self>) {
        if dlcs.is_empty() {
            return;
        }
        let rev = self.info_rev;
        let b = backend(cx);
        let runner = self.id.runner;
        let platform = self.platform;
        let task = irori_ui::run(cx, async move {
            let store = b
                .store(runner)
                .ok_or_else(|| crate::i18n::store_unavailable(runner))?;
            let sizes: Vec<(String, InstallSize)> = futures::stream::iter(dlcs)
                .map(|dlc| {
                    let store = store.clone();
                    async move {
                        let info = store
                            .install_info(&dlc, platform, &InstallInfoQuery::default())
                            .await
                            .ok()?;
                        Some((dlc, InstallSize::new(info.download_size, info.disk_size)))
                    }
                })
                .buffer_unordered(4)
                .filter_map(|x| async move { x })
                .collect()
                .await;
            Ok(sizes)
        });
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(sizes) = task.await {
                let _ = this.update(cx, |this, cx| {
                    if this.info_rev == rev {
                        this.dlc_sizes
                            .extend(sizes.into_iter().filter(|(_, s)| s.is_known()));
                        cx.notify();
                    }
                });
            }
        })
        .detach();
    }

    /// Replace a saved prefix that no longer exists with the one the
    /// defaults give, unless the user already typed another.
    fn check_prefix(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_modify() {
            return;
        }
        let b = backend(cx);
        let app = b.settings.app();
        let settings = b.settings.game(&self.id);
        if settings.wine_prefix.is_empty() {
            return;
        }
        let shown = self.prefix.read(cx).value().to_string();
        let game = self.game.clone();
        let task = irori_ui::run(cx, async move {
            Ok(irori_runtime::install_prefix(&app, &settings, &game))
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(suggested) = task.await else { return };
            let _ = this.update_in(cx, |this, window, cx| {
                let suggested = suggested.to_string_lossy().into_owned();
                if this.prefix.read(cx).value() == shown && suggested != shown {
                    this.prefix
                        .update(cx, |s, cx| s.set_value(suggested, window, cx));
                }
            });
        })
        .detach();
    }

    fn fetch_wines(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_modify() {
            return;
        }
        self.wines = Load::Loading;
        let app = backend(cx).settings.app();
        let task = irori_ui::run(cx, async move {
            Ok(irori_runtime::wine_installations_with_sources(&app).await)
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                let mut found = result.unwrap_or_default();
                if let Some(current) = &this.wine
                    && !found.iter().any(|(w, _)| w.bin == current.bin)
                {
                    found.insert(0, (current.clone(), None));
                }
                if this.wine.is_none() {
                    this.wine = found.first().map(|(w, _)| w.clone());
                }
                let items = found
                    .iter()
                    .map(|(w, source)| Choice::new(w.bin.clone(), build_label(w, *source)))
                    .collect();
                let list: Vec<WineInstallation> = found.into_iter().map(|(w, _)| w).collect();
                let selected = this.wine.as_ref().map(|w| w.bin.clone());
                set_choices(&this.wine_select, items, selected.as_deref(), window, cx);
                this.wines = Load::Ready(list);
                this.sync_auto_expand();
                cx.notify();
            });
        })
        .detach();
    }

    /// Download the newest GE-Proton through the Wine manager, which makes it
    /// the default Wine once installed. Asked for by the user's click; the
    /// download goes on when the modal closes.
    fn download_proton(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if matches!(
            self.proton,
            ProtonDownload::Finding | ProtonDownload::Running(_)
        ) {
            return;
        }
        self.proton = ProtonDownload::Finding;
        self.sync_auto_expand();
        cx.notify();
        let task = irori_ui::run(cx, manager::newest_release(ReleaseKind::GeProton));
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                match result {
                    Ok(release) => {
                        let key = wine_installs::start_install(release, true, cx);
                        this.proton = ProtonDownload::Running(key);
                        this.tick_proton(window, cx);
                    }
                    Err(e) => this.proton = ProtonDownload::Failed(format!("{e:#}").into()),
                }
                this.sync_auto_expand();
                cx.notify();
            });
        })
        .detach();
    }

    /// Follow the GE-Proton install: pick the build when it is there.
    fn proton_progressed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let ProtonDownload::Running(key) = &self.proton else {
            return;
        };
        let installs = wine_installs::wine_ops(cx);
        let outcome = installs.read(cx).finished.get(key).cloned();
        match outcome {
            None => {}
            Some(Outcome::Installed(wine)) => {
                self.proton = ProtonDownload::Offered;
                self.wine = Some(wine);
                self.fetch_wines(window, cx);
            }
            Some(Outcome::Failed(message)) => {
                self.proton = ProtonDownload::Failed(message.into());
                self.sync_auto_expand();
            }
            Some(Outcome::Cancelled) => self.proton = ProtonDownload::Offered,
        }
        cx.notify();
    }

    /// Repaint while GE-Proton downloads so its progress moves.
    fn tick_proton(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self._proton_ticker = Some(cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor().timer(PROTON_TICK).await;
                let keep = this.update(cx, |this, cx| {
                    cx.notify();
                    matches!(this.proton, ProtonDownload::Running(_))
                });
                if !matches!(keep, Ok(true)) {
                    break;
                }
            }
        }));
    }

    fn render_proton(&self, cx: &mut Context<Self>) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let open_manager = Button::new("open-wine-manager")
            .small()
            .outline()
            .label(t("game.install.open_wine_manager"))
            .on_click(|_, window, cx| {
                window.close_dialog(cx);
                navigate(Route::WineManager, cx);
            });
        // GE-Proton is offered only where the Wine manager has it (Linux).
        if !ReleaseKind::available().contains(&ReleaseKind::GeProton) {
            return v_flex()
                .gap_2()
                .child(Alert::warning("no-wine", t("game.install.no_wine")))
                .child(open_manager)
                .into_any_element();
        }
        let download = |label: SharedString, cx: &mut Context<Self>| {
            Button::new("download-ge-proton")
                .small()
                .primary()
                .icon(IconName::Download)
                .label(label)
                .on_click(cx.listener(|this, _, window, cx| this.download_proton(window, cx)))
        };
        match &self.proton {
            ProtonDownload::Offered => v_flex()
                .gap_2()
                .child(Alert::warning("no-wine", t("game.install.ge_proton_offer")))
                .child(
                    h_flex()
                        .gap_2()
                        .flex_wrap()
                        .child(download(t("game.install.download_ge_proton"), cx))
                        .child(open_manager),
                )
                .into_any_element(),
            ProtonDownload::Finding => h_flex()
                .gap_2()
                .text_sm()
                .child(Spinner::new().small())
                .child(t("game.install.ge_proton_finding"))
                .into_any_element(),
            ProtonDownload::Running(key) => {
                let installs = wine_installs::wine_ops(cx);
                let (version, live) = installs
                    .read(cx)
                    .running
                    .get(key)
                    .map(|op| (op.version.clone(), op.snapshot()))
                    .unwrap_or_default();
                let extracting = live.extracting();
                let progress = live.progress.unwrap_or_default();
                let text = if extracting {
                    t_args("game.install.ge_proton_unpacking", &[("version", &version)])
                } else if let Some(p) = progress.percent {
                    t_args(
                        "game.install.ge_proton_progress",
                        &[("version", &version), ("percent", &format!("{p:.0}"))],
                    )
                } else {
                    t_args("game.install.ge_proton_starting", &[("version", &version)])
                };
                let cancel_key = key.clone();
                v_flex()
                    .gap_2()
                    .child(
                        h_flex()
                            .gap_3()
                            .child(
                                v_flex()
                                    .flex_1()
                                    .min_w_0()
                                    .gap_1()
                                    .child(div().text_sm().truncate().child(text))
                                    .child(
                                        Progress::new("ge-proton-progress")
                                            .value(progress.percent.unwrap_or(0.0))
                                            .loading(extracting || progress.percent.is_none()),
                                    ),
                            )
                            .child(
                                Button::new("cancel-ge-proton")
                                    .small()
                                    .ghost()
                                    .label(t("game.common.cancel"))
                                    .disabled(extracting)
                                    .on_click(move |_, _, cx| {
                                        wine_installs::cancel_install(&cancel_key, cx)
                                    }),
                            ),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(muted)
                            .child(t("game.install.ge_proton_meanwhile")),
                    )
                    .into_any_element()
            }
            ProtonDownload::Failed(message) => v_flex()
                .gap_2()
                .child(Alert::error(
                    "ge-proton-failed",
                    t_args("game.install.ge_proton_failed", &[("error", message)]),
                ))
                .child(
                    h_flex()
                        .gap_2()
                        .flex_wrap()
                        .child(download(t("game.install.ge_proton_retry"), cx))
                        .child(open_manager),
                )
                .into_any_element(),
        }
    }

    fn fetch_compat(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(task) = hooks::fetch_compatibility(self.game.clone(), cx) else {
            return;
        };
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                this.compat = result.ok();
                cx.notify();
            });
        })
        .detach();
    }

    fn install_dir(&self, cx: &App) -> PathBuf {
        let raw = self.path.read(cx).value();
        irori_core::paths::expand_home(raw.trim())
    }

    fn disk_probe_dir(&self, cx: &App) -> Option<PathBuf> {
        if self.is_modify() {
            self.game.install_path()
        } else {
            let dir = self.install_dir(cx);
            (!dir.as_os_str().is_empty()).then_some(dir)
        }
    }

    fn refresh_disk(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.disk_rev += 1;
        let rev = self.disk_rev;
        let Some(dir) = self.disk_probe_dir(cx) else {
            self.disk = Load::Idle;
            cx.notify();
            return;
        };
        self.disk = Load::Loading;
        cx.notify();
        let task = cx.background_spawn(async move { disk_space(&dir) });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                if this.disk_rev == rev {
                    this.disk = Load::from_result(result);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn browse_install_dir(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let pick = pick_path(window, cx, true, t("game.common.select"));
        cx.spawn_in(window, async move |this, cx| {
            if let Some(path) = pick.await {
                let _ = this.update_in(cx, |this, window, cx| {
                    let value = path.to_string_lossy().into_owned();
                    this.path.update(cx, |s, cx| s.set_value(value, window, cx));
                    this.refresh_disk(window, cx);
                });
            }
        })
        .detach();
    }

    fn browse_prefix(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let pick = pick_path(window, cx, true, t("game.common.select"));
        cx.spawn_in(window, async move |this, cx| {
            if let Some(path) = pick.await {
                let _ = this.update_in(cx, |this, window, cx| {
                    let value = path.to_string_lossy().into_owned();
                    this.prefix
                        .update(cx, |s, cx| s.set_value(value, window, cx));
                });
            }
        })
        .detach();
    }

    fn apply_password(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let password = self.password.read(cx).value();
        self.password_busy = true;
        cx.notify();
        let task = hooks::set_branch_password(self.id.clone(), password, cx);
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.password_busy = false;
                match result {
                    Ok(()) => this.fetch_info(window, cx),
                    Err(e) => notify_error(window, cx, t("game.install.password_failed"), &e),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Bytes this choice downloads and occupies.
    pub fn size(&self) -> InstallSize {
        self.info
            .ready()
            .map(|info| {
                install_size(
                    info,
                    self.language.as_deref(),
                    &self.selected_dlcs,
                    &self.dlc_sizes,
                )
            })
            .unwrap_or_default()
    }

    /// Disk verdict for the current mode. In Modify mode, accounts for bytes
    /// the installed game already occupies on disk so the base game is never
    /// double-counted against free space.
    fn disk_verdict(&self) -> DiskVerdict {
        let space = self.disk.ready().copied();
        let size = self.size();
        if !self.is_modify() {
            return DiskVerdict::evaluate(space, size.disk);
        }
        let installed_disk = self
            .game
            .install
            .as_ref()
            .map_or(0, |i| i.install_size_bytes);
        let installed_dlcs: BTreeSet<&str> = self
            .game
            .install
            .as_ref()
            .map(|i| i.installed_dlcs.iter().map(String::as_str).collect())
            .unwrap_or_default();
        let mut added_dlc_disk = 0u64;
        for dlc in &self.selected_dlcs {
            if !installed_dlcs.contains(dlc.as_str())
                && let Some(s) = self.dlc_sizes.get(dlc)
            {
                added_dlc_disk = added_dlc_disk.saturating_add(s.disk);
            }
        }
        let mut removed_dlc_disk = 0u64;
        for dlc in &installed_dlcs {
            if !self.selected_dlcs.contains(*dlc)
                && let Some(s) = self.dlc_sizes.get(*dlc)
            {
                removed_dlc_disk = removed_dlc_disk.saturating_add(s.disk);
            }
        }
        modify_disk_verdict(
            space,
            size.disk,
            installed_disk,
            added_dlc_disk,
            removed_dlc_disk,
        )
    }

    fn third_party(&self) -> Option<String> {
        self.info
            .ready()
            .and_then(|i| i.third_party_managed_app.clone())
            .or_else(|| self.game.third_party_managed_app.clone())
    }

    fn anticheat_gate(&self, cx: &App) -> InstallGate {
        hooks::install_gate(
            self.compat.as_ref().and_then(|c| c.anticheat.as_ref()),
            &backend(cx).settings.app(),
        )
    }

    fn not_installable(&self) -> bool {
        self.info
            .ready()
            .is_some_and(|i| nothing_to_install(i, &self.game))
    }

    /// Why the primary button is disabled, if it is (a `game.install.blocker`
    /// key).
    fn blocker(&self, cx: &App) -> Option<&'static str> {
        if self.submitting {
            return Some("game.install.blocker.working");
        }
        if !matches!(self.info, Load::Ready(_)) {
            return Some("game.install.blocker.loading");
        }
        if self.not_installable() {
            return Some("game.install.blocker.not_installable");
        }
        if is_unsupported_third_party(&self.game) {
            return Some("game.install.blocker.unsupported_launcher");
        }
        match self.anticheat_gate(cx) {
            InstallGate::Blocked => return Some("game.install.blocker.anticheat"),
            InstallGate::Confirm if !self.anticheat_ack => {
                return Some("game.install.blocker.anticheat");
            }
            _ => {}
        }
        if self.is_modify() {
            return (!self.has_changes()).then_some("game.install.blocker.no_changes");
        }
        if self.path.read(cx).value().trim().is_empty() {
            return Some("game.install.blocker.choose_folder");
        }
        if matches!(
            self.disk,
            Load::Ready(DiskSpace {
                writable: false,
                ..
            })
        ) {
            return Some("game.install.blocker.not_writable");
        }
        None
    }

    fn has_changes(&self) -> bool {
        let Some(install) = self.game.install.as_ref() else {
            return false;
        };
        let installed: BTreeSet<String> = install.installed_dlcs.iter().cloned().collect();
        installed != self.selected_dlcs
            || (self.language.is_some() && install.language != self.language)
            || install.branch != self.branch
            || self.pin_build != install.pinned_version
            || (self.pin_build && install.build_id != self.build)
    }

    /// Primary button label: `Install · 12.3 GiB` or `Save changes`.
    pub fn primary_label(&self) -> String {
        match self.mode {
            InstallMode::Install => {
                let resumable = self.info.ready().is_some_and(|i| i.resumable);
                let verb = if resumable {
                    t("game.install.continue")
                } else {
                    t("game.install.install")
                };
                install_button_label(&verb, self.size().download)
            }
            InstallMode::Modify => t("game.install.apply").to_string(),
        }
    }

    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.blocker(cx).is_some() {
            return;
        }
        match self.mode {
            InstallMode::Install => self.submit_install(window, cx),
            InstallMode::Modify => self.submit_modify(window, cx),
        }
    }

    fn selected_sdl_tags(&self, info: &InstallInfo) -> Vec<String> {
        info.sdl
            .iter()
            .filter(|o| o.mandatory || self.sdl.contains(&o.id))
            .flat_map(|o| o.tags.iter().cloned())
            .collect()
    }

    fn submit_install(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(info) = self.info.ready().cloned() else {
            return;
        };
        let b = backend(cx);
        let id = self.id.clone();
        if self.platform.needs_wine() {
            self.persist_wine(cx);
        }
        let size = self.size();
        let request = InstallRequest {
            path: self.install_dir(cx),
            platform: self.platform,
            dlcs: info
                .dlcs
                .iter()
                .filter(|d| self.selected_dlcs.contains(&d.app_name))
                .map(|d| d.app_name.clone())
                .collect(),
            sdl: self.selected_sdl_tags(&info),
            language: self.language.clone(),
            branch: self.branch.clone(),
            build: self.chosen_build(),
            dependencies: info.dependencies.clone(),
            installer: None,
            account: self.account.clone(),
        };
        let title = self.game.title.clone();
        self.submitting = true;
        cx.notify();
        let dir = request.path.clone();
        let task = irori_ui::run(cx, async move {
            tokio::fs::create_dir_all(&dir).await.map_err(|e| {
                anyhow::anyhow!(t_args(
                    "game.install.create_dir_failed",
                    &[
                        ("path", &dir.display().to_string()),
                        ("error", &e.to_string())
                    ]
                ))
            })?;
            b.enqueue_install(id, title, request, size.download);
            Ok(())
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.submitting = false;
                match result {
                    Ok(()) => window.close_dialog(cx),
                    Err(e) => notify_error(window, cx, t("game.install.start_failed"), &e),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Store the chosen Wine build and prefix as per-game overrides when they
    /// differ from what the game would use anyway.
    fn persist_wine(&self, cx: &App) {
        let b = backend(cx);
        let effective = b.settings.game(&self.id);
        if let Some(wine) = &self.wine
            && *wine != effective.wine_version
            && let Ok(v) = serde_json::to_value(wine)
        {
            b.settings.set_game_value(&self.id, "wineVersion", Some(v));
        }
        let prefix = self.prefix.read(cx).value().trim().to_string();
        let app = b.settings.app();
        let current = irori_runtime::prefix_path(&app, &effective, &self.game);
        let inherited = irori_runtime::prefix_path(
            &app,
            &irori_core::GameSettings {
                wine_prefix: app.game.wine_prefix.clone(),
                ..effective.clone()
            },
            &self.game,
        );
        if prefix.is_empty() {
            return;
        }
        let chosen = irori_core::paths::expand_home(&prefix);
        if chosen == inherited && chosen != current {
            // The saved prefix was gone and the defaults' one was taken.
            b.settings.set_game_value(&self.id, "winePrefix", None);
        } else if chosen != current {
            b.settings.set_game_value(
                &self.id,
                "winePrefix",
                Some(serde_json::Value::String(prefix)),
            );
        }
    }

    fn submit_modify(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(install) = self.game.install.clone() else {
            return;
        };
        let b = backend(cx);
        let installed: BTreeSet<String> = install.installed_dlcs.iter().cloned().collect();
        // Epic DLCs are separate installs; queue each change on its own.
        if self.id.runner == Runner::Legendary
            && let Some(info) = self.info.ready()
        {
            for dlc in &info.dlcs {
                let want = self.selected_dlcs.contains(&dlc.app_name);
                if want != installed.contains(&dlc.app_name) {
                    crate::dlc::toggle_dlc(&self.game, dlc, want, window, cx);
                }
            }
        }
        let dlcs_changed = installed != self.selected_dlcs && self.id.runner != Runner::Legendary;
        let request = UpdateRequest {
            dlcs: dlcs_changed.then(|| self.selected_dlcs.iter().cloned().collect()),
            language: self
                .language
                .clone()
                .filter(|l| install.language.as_ref() != Some(l)),
            branch: (install.branch != self.branch)
                .then(|| self.branch.clone().unwrap_or_default()),
            build: (self.pin_build && install.build_id != self.build)
                .then(|| self.build.clone())
                .flatten(),
            dependencies: None,
        };
        let needs_update = request.dlcs.is_some()
            || request.language.is_some()
            || request.branch.is_some()
            || request.build.is_some();
        if needs_update {
            b.enqueue_update(self.id.clone(), self.game.title.clone(), request);
        }
        if self.pin_build != install.pinned_version {
            let id = self.id.clone();
            let pinned = self.pin_build;
            crate::ops::run_reporting(
                window,
                cx,
                t("game.install.pin_failed"),
                async move {
                    let store = b
                        .store(id.runner)
                        .ok_or_else(|| crate::i18n::store_unavailable(id.runner))?;
                    store.set_version_pinned(&id.app_name, pinned).await
                },
                |_, _, _| {},
            )
            .detach();
        }
        window.close_dialog(cx);
    }

    fn card(cx: &App) -> gpui_kit::Div {
        let theme = cx.theme();
        v_flex()
            .w_full()
            .rounded(theme.radius_lg)
            .bg(theme.secondary.opacity(0.45))
            .border_1()
            .border_color(theme.border)
            .overflow_hidden()
    }

    fn field_label(label: SharedString, cx: &App) -> AnyElement {
        div()
            .text_xs()
            .font_medium()
            .text_color(cx.theme().muted_foreground)
            .child(label)
            .into_any_element()
    }

    fn render_notices(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mut out = Vec::new();
        if let Load::Failed(e) = &self.info {
            out.push(
                v_flex()
                    .gap_2()
                    .child(Alert::error(
                        "install-info-error",
                        t_args("game.install.info_failed", &[("error", e.as_ref())]),
                    ))
                    .child(
                        Button::new("retry-install-info")
                            .small()
                            .outline()
                            .icon(IconName::RefreshCw)
                            .label(t("game.common.retry"))
                            .on_click(
                                cx.listener(|this, _, window, cx| this.fetch_info(window, cx)),
                            ),
                    )
                    .into_any_element(),
            );
        }
        if self.not_installable() {
            out.push(
                Alert::warning(
                    "not-installable",
                    t(nothing_to_install_text(self.game.runner())),
                )
                .into_any_element(),
            );
        }
        if let Some(app) = self.third_party() {
            if is_unsupported_third_party(&self.game) {
                out.push(
                    Alert::error(
                        "third-party",
                        t_args("game.install.third_party_unsupported", &[("app", &app)]),
                    )
                    .into_any_element(),
                );
            } else {
                out.push(
                    Alert::info(
                        "third-party",
                        t_args("game.install.third_party", &[("app", &app)]),
                    )
                    .into_any_element(),
                );
            }
        }
        let gate = self.anticheat_gate(cx);
        if let Some(report) = self.compat.as_ref().and_then(|c| c.anticheat.as_ref())
            && gate != InstallGate::Allowed
        {
            let allowed = gate == InstallGate::Confirm;
            let mut col = v_flex().gap_2().child(Alert::warning(
                "anticheat",
                anticheat_warning(report.status, &report.anticheats),
            ));
            if allowed {
                col = col.child(
                    Checkbox::new("anticheat-ack")
                        .label(t("game.install.install_anyway"))
                        .checked(self.anticheat_ack)
                        .on_change(cx.listener(|this, v: &bool, _, cx| {
                            this.anticheat_ack = *v;
                            cx.notify();
                        })),
                );
            } else {
                col = col
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child(t("game.install.anticheat_disabled")),
                    )
                    .child(
                        Button::new("open-advanced")
                            .small()
                            .outline()
                            .label(t("game.install.open_settings"))
                            .on_click(|_, window, cx| {
                                window.close_dialog(cx);
                                navigate(Route::Settings("advanced".into()), cx);
                            }),
                    );
            }
            out.push(col.into_any_element());
        } else if let Some(note) = self
            .compat
            .as_ref()
            .and_then(|c| c.anticheat.as_ref())
            .and_then(|r| r.online_only_note())
        {
            out.push(Alert::info("anticheat-online-only", note).into_any_element());
        }
        out
    }

    fn platform_chip_label(platform: InstallPlatform) -> (IconName, SharedString) {
        let native = irori_core::HostPlatform::current().native();
        match platform {
            InstallPlatform::Linux if native == InstallPlatform::Linux => (
                IconName::Terminal,
                t_args(
                    "game.install.native",
                    &[("platform", &platform_label(platform))],
                ),
            ),
            InstallPlatform::Mac if native == InstallPlatform::Mac => (
                IconName::Monitor,
                t_args(
                    "game.install.native",
                    &[("platform", &platform_label(platform))],
                ),
            ),
            InstallPlatform::Windows if platform.needs_wine() => {
                (IconName::AppWindow, t("game.install.windows_wine"))
            }
            InstallPlatform::Windows => (IconName::AppWindow, platform_label(platform)),
            InstallPlatform::Browser => (IconName::Globe, platform_label(platform)),
            _ => (IconName::Monitor, platform_label(platform)),
        }
    }

    fn render_platform(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.platforms.len() < 2 || self.is_modify() {
            return None;
        }
        let theme = cx.theme();
        let active_bg = theme.primary.opacity(0.14);
        let active_border = theme.primary;
        let hover_bg = theme.secondary.opacity(0.8);
        let fg = theme.foreground;
        let muted = theme.muted_foreground;
        let radius = theme.radius;
        let border = theme.border;
        let seg_bg = theme.secondary.opacity(0.6);

        let buttons = self.platforms.iter().enumerate().map(|(ix, &p)| {
            let active = p == self.platform;
            let (icon, label) = Self::platform_chip_label(p);
            h_flex()
                .id(("install-platform-opt", ix))
                .role(Role::Button)
                .keyboard_focusable(cx)
                .cursor_pointer()
                .items_center()
                .gap_1p5()
                .px_2p5()
                .py_1()
                .rounded(radius)
                .border_1()
                .text_xs()
                .when(active, |el| {
                    el.bg(active_bg)
                        .border_color(active_border)
                        .text_color(fg)
                        .font_semibold()
                })
                .when(!active, |el| {
                    el.border_color(gpui_kit::transparent_black())
                        .text_color(muted)
                        .hover(move |s| s.bg(hover_bg).text_color(fg))
                })
                .child(Icon::new(icon).xsmall())
                .child(label)
                .on_action(cx.listener(move |this, _: &Confirm, window, cx| {
                    this.set_platform(p, window, cx);
                }))
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.set_platform(p, window, cx);
                }))
        });

        Some(
            h_flex()
                .w_full()
                .items_center()
                .justify_between()
                .gap_3()
                .flex_wrap()
                .child(
                    h_flex()
                        .items_center()
                        .gap_2()
                        .child(Icon::new(IconName::Gamepad2).small().text_color(muted))
                        .child(
                            div()
                                .text_sm()
                                .font_semibold()
                                .child(t("game.common.platform")),
                        ),
                )
                .child(
                    h_flex()
                        .p_0p5()
                        .gap_1()
                        .rounded(radius)
                        .bg(seg_bg)
                        .border_1()
                        .border_color(border)
                        .children(buttons),
                )
                .into_any_element(),
        )
    }

    /// Which account installs a game several accounts own. Updates and
    /// launches use the same account afterwards.
    fn render_account(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.owners.is_empty() {
            return None;
        }
        let selected = self
            .owners
            .iter()
            .position(|(id, _)| Some(id) == self.account.as_ref());
        let owners = self.owners.clone();
        let muted = cx.theme().muted_foreground;
        Some(
            Self::card(cx)
                .debug_selector(|| "install-account".into())
                .p_3p5()
                .gap_2p5()
                .child(
                    h_flex()
                        .items_center()
                        .gap_2()
                        .child(Icon::new(IconName::Users).small().text_color(muted))
                        .child(
                            div()
                                .text_sm()
                                .font_semibold()
                                .child(t("game.info.account")),
                        ),
                )
                .child(
                    RadioGroup::horizontal("install-account")
                        .children(owners.iter().map(|(_, name)| name.clone()))
                        .selected_index(selected)
                        .on_change(cx.listener(move |this, ix: &usize, _, cx| {
                            if let Some((id, _)) = owners.get(*ix) {
                                this.account = Some(id.clone());
                                cx.notify();
                            }
                        })),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(muted)
                        .child(t("game.install.account_hint")),
                )
                .into_any_element(),
        )
    }

    fn render_location(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.is_modify() {
            return None;
        }
        let size = self.size();
        let space = self.disk.ready().copied();
        let verdict = DiskVerdict::evaluate(space, size.disk);
        let dir = self.install_dir(cx);
        let label = drive_label(&dir);

        let theme = cx.theme();
        let border = theme.border;
        let muted = theme.muted_foreground;
        let fg = theme.foreground;
        let primary = theme.primary;
        let primary_text = legible(
            primary,
            &[theme.background, theme.secondary],
            TEXT_CONTRAST,
            cx,
        );
        let danger = theme.danger;
        let success = legible(theme.success, &[theme.background], TEXT_CONTRAST, cx);
        let danger_text = legible(theme.danger, &[theme.background], TEXT_CONTRAST, cx);
        let radius = theme.radius;
        let chip_active_bg = primary.opacity(0.14);
        let chip_idle_bg = theme.background.opacity(0.55);
        let chip_hover_bg = theme.secondary;

        let app = irori_ui::models(cx).settings.read(cx).app.clone();
        let chips = (self.install_targets.len() > 1).then(|| {
            h_flex().items_center().gap_1p5().flex_wrap().children(
                self.install_targets.iter().enumerate().map(|(ix, target)| {
                    let resolved = irori_core::paths::expand_home(
                        &app.install_folder_in(&target.base_path, self.id.runner),
                    );
                    let active = same_dir(&resolved, &dir);
                    let name = target.label.clone();
                    let free_text = target.free.map(|free| {
                        t_args("game.disk.chip_free", &[("free", &format_bytes(free))])
                    });
                    h_flex()
                        .id(("library-folder-chip", ix))
                        .debug_selector(move || format!("library-folder-chip-{ix}"))
                        .role(Role::Button)
                        .keyboard_focusable(cx)
                        .cursor_pointer()
                        .items_center()
                        .gap_1p5()
                        .px_2p5()
                        .py_1()
                        .rounded(radius)
                        .border_1()
                        .text_xs()
                        .when(active, |el| {
                            el.bg(chip_active_bg)
                                .border_color(primary)
                                .text_color(fg)
                                .font_semibold()
                        })
                        .when(!active, |el| {
                            el.bg(chip_idle_bg)
                                .border_color(border)
                                .text_color(muted)
                                .hover(move |s| s.bg(chip_hover_bg).text_color(fg))
                        })
                        .child(name)
                        .when_some(free_text, |el, free| {
                            el.child(
                                div()
                                    .text_color(if active { primary_text } else { muted })
                                    .child(free),
                            )
                        })
                        .on_action(cx.listener(move |this, _: &Confirm, window, cx| {
                            this.select_install_target(ix, window, cx);
                        }))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.select_install_target(ix, window, cx);
                        }))
                }),
            )
        });

        let bar: AnyElement = match space {
            Some(s) => {
                let (used, install) = disk_bar_fractions(s, size.disk);
                let install_color = match verdict {
                    DiskVerdict::TooSmall { .. } | DiskVerdict::NotWritable => danger,
                    _ => primary,
                };
                h_flex()
                    .id("disk-bar")
                    .w_full()
                    .h(px(7.))
                    .rounded_full()
                    .overflow_hidden()
                    .bg(theme.muted)
                    .child(div().h_full().w(relative(used)).bg(muted.opacity(0.45)))
                    .child(
                        div()
                            .h_full()
                            .w(relative(install))
                            .when(size.disk > 0 && s.free > 0, |el| el.min_w(px(3.)))
                            .bg(install_color),
                    )
                    .into_any_element()
            }
            None if self.disk.is_loading() => Skeleton::new()
                .w_full()
                .h(px(7.))
                .rounded_full()
                .into_any_element(),
            None => div()
                .w_full()
                .h(px(7.))
                .rounded_full()
                .bg(theme.muted)
                .into_any_element(),
        };

        let (strip_summary, strip_color, error_line): (String, Hsla, Option<String>) = match &self
            .disk
        {
            Load::Failed(e) => (String::new(), danger_text, Some(e.to_string())),
            _ => match verdict {
                DiskVerdict::Fits { .. } => (bar_summary(verdict, space, size.disk), success, None),
                DiskVerdict::TooSmall { missing } => (
                    format!("{} {}", t("game.install.short_by"), format_bytes(missing)),
                    danger_text,
                    Some(bar_summary(verdict, space, size.disk)),
                ),
                DiskVerdict::NotWritable => (
                    String::new(),
                    danger_text,
                    Some(bar_summary(verdict, space, size.disk)),
                ),
                DiskVerdict::Unknown => (bar_summary(verdict, space, size.disk), muted, None),
            },
        };

        Some(
            Self::card(cx)
                .child(
                    v_flex()
                        .p_4()
                        .gap_3()
                        .child(
                            h_flex()
                                .w_full()
                                .items_center()
                                .justify_between()
                                .gap_2()
                                .flex_wrap()
                                .child(
                                    h_flex()
                                        .items_center()
                                        .gap_2()
                                        .child(
                                            Icon::new(IconName::Folder).small().text_color(muted),
                                        )
                                        .child(
                                            div()
                                                .text_sm()
                                                .font_semibold()
                                                .child(t("game.install.location_heading")),
                                        ),
                                )
                                .children(chips),
                        )
                        .child(
                            h_flex()
                                .w_full()
                                .gap_2()
                                .child(div().flex_1().min_w_0().child(Input::new(&self.path)))
                                .child(
                                    Button::new("browse-install-dir")
                                        .outline()
                                        .icon(IconName::FolderOpen)
                                        .label(t("game.common.browse"))
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.browse_install_dir(window, cx)
                                        })),
                                ),
                        ),
                )
                .child(
                    v_flex()
                        .w_full()
                        .border_t_1()
                        .border_color(border)
                        .bg(theme.background.opacity(0.45))
                        .px_4()
                        .py_2p5()
                        .gap_1p5()
                        .child(
                            h_flex()
                                .w_full()
                                .items_center()
                                .gap_3()
                                .child(
                                    h_flex()
                                        .flex_shrink_0()
                                        .items_center()
                                        .gap_1p5()
                                        .child(
                                            Icon::new(IconName::HardDrive)
                                                .xsmall()
                                                .text_color(muted),
                                        )
                                        .when(!label.is_empty(), |row| {
                                            row.child(
                                                div()
                                                    .text_xs()
                                                    .font_medium()
                                                    .text_color(muted)
                                                    .child(label),
                                            )
                                        }),
                                )
                                .child(div().flex_1().min_w(rems(4.)).child(bar))
                                .when(!strip_summary.is_empty(), |row| {
                                    row.child(
                                        div()
                                            .flex_shrink_0()
                                            .text_xs()
                                            .font_medium()
                                            .text_color(strip_color)
                                            .child(strip_summary),
                                    )
                                }),
                        )
                        .when_some(error_line, |col, msg| {
                            col.child(div().text_xs().text_color(danger_text).child(msg))
                        }),
                )
                .into_any_element(),
        )
    }

    fn render_sdl(&self, info: &InstallInfo, cx: &mut Context<Self>) -> Option<AnyElement> {
        // Optional components are chosen at install time only.
        if info.sdl.is_empty() || self.is_modify() {
            return None;
        }
        let theme = cx.theme();
        let border = theme.border;
        let muted = theme.muted_foreground;
        let selected_count = info
            .sdl
            .iter()
            .filter(|o| o.mandatory || self.sdl.contains(&o.id))
            .count();
        let total_count = info.sdl.len();
        let mut rows = v_flex();
        for (ix, opt) in info.sdl.iter().enumerate() {
            let id = opt.id.clone();
            rows = rows.child(
                h_flex()
                    .w_full()
                    .px_3p5()
                    .py_2()
                    .when(ix > 0, |r| r.border_t_1().border_color(border.opacity(0.5)))
                    .child(
                        Checkbox::new(SharedString::from(format!("sdl-{}", opt.id)))
                            .label(opt.name.clone())
                            .checked(opt.mandatory || self.sdl.contains(&opt.id))
                            .disabled(opt.mandatory)
                            .on_change(cx.listener(move |this, v: &bool, _, cx| {
                                if *v {
                                    this.sdl.insert(id.clone());
                                } else {
                                    this.sdl.remove(&id);
                                }
                                cx.notify();
                            })),
                    ),
            );
        }
        Some(
            Self::card(cx)
                .child(
                    h_flex()
                        .w_full()
                        .px_3p5()
                        .py_2p5()
                        .items_center()
                        .justify_between()
                        .gap_2()
                        .border_b_1()
                        .border_color(border)
                        .child(
                            h_flex()
                                .items_center()
                                .gap_2()
                                .child(Icon::new(IconName::Layers).small().text_color(muted))
                                .child(
                                    div()
                                        .text_sm()
                                        .font_semibold()
                                        .child(t("game.install.optional_components")),
                                )
                                .child(
                                    Tag::secondary()
                                        .xsmall()
                                        .outline()
                                        .child(format!("{selected_count} / {total_count}")),
                                ),
                        ),
                )
                .child(rows)
                .into_any_element(),
        )
    }

    fn render_dlcs(&self, info: &InstallInfo, cx: &mut Context<Self>) -> Option<AnyElement> {
        if info.dlcs.is_empty() {
            return None;
        }
        let theme = cx.theme();
        let border = theme.border;
        let muted = theme.muted_foreground;
        let all: BTreeSet<String> = info.dlcs.iter().map(|d| d.app_name.clone()).collect();
        let all_selected = all.is_subset(&self.selected_dlcs);
        let selected_count = info
            .dlcs
            .iter()
            .filter(|d| self.selected_dlcs.contains(&d.app_name))
            .count();
        let total_count = info.dlcs.len();

        let mut rows = v_flex();
        for (ix, dlc) in info.dlcs.iter().enumerate() {
            let app = dlc.app_name.clone();
            let size = self
                .dlc_sizes
                .get(&dlc.app_name)
                .map(|s| format_bytes(s.download))
                .unwrap_or_default();
            rows = rows.child(
                h_flex()
                    .w_full()
                    .px_3p5()
                    .py_2()
                    .gap_3()
                    .items_center()
                    .justify_between()
                    .when(ix > 0, |r| r.border_t_1().border_color(border.opacity(0.5)))
                    .child(
                        div().flex_1().min_w_0().child(
                            Checkbox::new(SharedString::from(format!("dlc-{}", dlc.app_name)))
                                .label(dlc.title.clone())
                                .checked(self.selected_dlcs.contains(&dlc.app_name))
                                .on_change(cx.listener(move |this, v: &bool, _, cx| {
                                    if *v {
                                        this.selected_dlcs.insert(app.clone());
                                    } else {
                                        this.selected_dlcs.remove(&app);
                                    }
                                    cx.notify();
                                })),
                        ),
                    )
                    .when(!size.is_empty(), |r| {
                        r.child(
                            div()
                                .flex_shrink_0()
                                .text_xs()
                                .text_color(muted)
                                .child(size),
                        )
                    }),
            );
        }

        Some(
            Self::card(cx)
                .child(
                    h_flex()
                        .w_full()
                        .px_3p5()
                        .py_2p5()
                        .items_center()
                        .justify_between()
                        .gap_2()
                        .flex_wrap()
                        .border_b_1()
                        .border_color(border)
                        .child(
                            h_flex()
                                .items_center()
                                .gap_2()
                                .child(Icon::new(IconName::Package).small().text_color(muted))
                                .child(div().text_sm().font_semibold().child(t("game.install.dlc")))
                                .child(
                                    Tag::secondary()
                                        .xsmall()
                                        .outline()
                                        .child(format!("{selected_count} / {total_count}")),
                                ),
                        )
                        .child(
                            Checkbox::new("dlc-all")
                                .label(t("game.install.dlc_all"))
                                .checked(all_selected)
                                .on_change(cx.listener(move |this, v: &bool, _, cx| {
                                    if *v {
                                        this.selected_dlcs.extend(all.iter().cloned());
                                    } else {
                                        this.selected_dlcs.clear();
                                    }
                                    cx.notify();
                                })),
                        ),
                )
                .child(rows)
                .into_any_element(),
        )
    }

    fn options_summary(&self, cx: &App) -> Option<String> {
        let mut parts: Vec<String> = Vec::new();
        if let Some(info) = self.info.ready() {
            if info.languages.len() > 1
                && let Some(lang) = &self.language
            {
                parts.push(language_name(lang));
            }
            let has_branches = !info.branches.is_empty();
            let builds = info
                .builds
                .iter()
                .filter(|b| b.branch == self.branch)
                .count();
            if self.package_choice() {
                if let Some(name) = self.selected_build_name(info) {
                    parts.push(name);
                }
            } else if has_branches || builds > 0 {
                if let Some(branch) = &self.branch {
                    parts.push(branch.clone());
                } else if has_branches {
                    parts.push(t("game.install.branch_default").to_string());
                }
                if self.pin_build {
                    if let Some(name) = self.selected_build_name(info) {
                        parts.push(name);
                    }
                } else if builds > 0 && !has_branches {
                    parts.push(t("game.install.version_latest").to_string());
                }
            } else if self.is_modify() && matches!(self.id.runner, Runner::Legendary | Runner::Itch)
            {
                let label = if self.pin_build {
                    t("game.install.skip_updates")
                } else {
                    t("game.install.version_latest")
                };
                parts.push(label.to_string());
            }
        }
        if !self.is_modify()
            && self.platform.needs_wine()
            && let Some(wine) = &self.wine
            && !wine.name.trim().is_empty()
        {
            parts.push(wine.name.clone());
        }
        let _ = cx;
        (!parts.is_empty()).then(|| parts.join(" · "))
    }

    fn selected_build_name(&self, info: &InstallInfo) -> Option<String> {
        let id = self.build.as_deref()?;
        info.builds
            .iter()
            .find(|b| b.build_id == id)
            .map(|b| b.version_name.clone())
    }

    fn render_more_options(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let info = self.info.ready();
        let has_language = info.is_some_and(|i| i.languages.len() > 1);
        let has_branches = info.is_some_and(|i| !i.branches.is_empty());
        let builds = info.map_or(0, |i| {
            i.builds.iter().filter(|b| b.branch == self.branch).count()
        });
        let has_password =
            info.is_some() && hooks::has_branch_password(cx) && self.id.runner == Runner::Gog;
        let has_package = self.package_choice() && builds > 0;
        let has_version = !self.package_choice() && (has_branches || builds > 0 || has_password);
        let has_skip_updates = info.is_some()
            && !self.package_choice()
            && !has_version
            && self.is_modify()
            && matches!(self.id.runner, Runner::Legendary | Runner::Itch);
        let has_wine = !self.is_modify() && self.platform.needs_wine();

        if !(has_language || has_package || has_version || has_skip_updates || has_wine) {
            return None;
        }

        let theme = cx.theme();
        let border = theme.border;
        let muted = theme.muted_foreground;
        let hover_bg = theme.secondary.opacity(0.75);
        let summary = self.options_summary(cx);
        let chevron = if self.options_expanded {
            IconName::ChevronUp
        } else {
            IconName::ChevronDown
        };

        let ring = theme.ring;
        let header = h_flex()
            .id("more-options-toggle")
            .debug_selector(|| "more-options-toggle".into())
            .role(Role::Button)
            // Inside `Self::card` (which clips with `overflow_hidden`), draw an
            // inset focus ring so keyboard focus stays visible on every edge.
            .tab_index(0)
            .focus_visible(move |style| {
                style.shadow(vec![BoxShadow {
                    color: ring,
                    offset: point(px(0.), px(0.)),
                    blur_radius: px(0.),
                    spread_radius: px(2.),
                    inset: true,
                }])
            })
            .cursor_pointer()
            .w_full()
            .px_3p5()
            .py_2p5()
            .items_center()
            .justify_between()
            .gap_3()
            .hover(move |s| s.bg(hover_bg))
            .on_action(cx.listener(|this, _: &Confirm, _, cx| {
                this.options_expanded = !this.options_expanded;
                cx.notify();
            }))
            .on_click(cx.listener(|this, _, _, cx| {
                this.options_expanded = !this.options_expanded;
                cx.notify();
            }))
            .child(
                h_flex()
                    .items_center()
                    .gap_2()
                    .flex_shrink_0()
                    .child(
                        Icon::new(IconName::SlidersHorizontal)
                            .small()
                            .text_color(muted),
                    )
                    .child(
                        div()
                            .text_sm()
                            .font_semibold()
                            .child(t("game.install.more_options")),
                    ),
            )
            .child(
                h_flex()
                    .min_w_0()
                    .items_center()
                    .gap_2()
                    .when_some(summary, |row, text| {
                        row.child(
                            Tag::secondary()
                                .xsmall()
                                .outline()
                                .max_w(rems(18.))
                                .child(div().min_w_0().truncate().child(text)),
                        )
                    })
                    .child(Icon::new(chevron).small().text_color(muted)),
            );

        let mut card = Self::card(cx).child(header);
        if self.options_expanded {
            let mut body = v_flex()
                .debug_selector(|| "more-options-body".into())
                .w_full()
                .px_3p5()
                .pt_3()
                .pb_3p5()
                .gap_3()
                .border_t_1()
                .border_color(border);

            // Language & Branch sit side-by-side when both are available.
            if has_language && has_version && has_branches {
                body = body.child(
                    h_flex()
                        .w_full()
                        .gap_3()
                        .items_start()
                        .child(
                            v_flex()
                                .flex_1()
                                .min_w_0()
                                .gap_1p5()
                                .child(Self::field_label(t("game.common.language"), cx))
                                .child(
                                    Select::new(&self.language_select)
                                        .placeholder(t("game.common.language")),
                                ),
                        )
                        .child(
                            v_flex()
                                .flex_1()
                                .min_w_0()
                                .gap_1p5()
                                .child(Self::field_label(t("game.install.branch"), cx))
                                .child(
                                    Select::new(&self.branch_select)
                                        .placeholder(t("game.install.branch")),
                                ),
                        ),
                );
            } else {
                if has_language {
                    body = body.child(
                        v_flex()
                            .w_full()
                            .gap_1p5()
                            .child(Self::field_label(t("game.common.language"), cx))
                            .child(
                                Select::new(&self.language_select)
                                    .placeholder(t("game.common.language")),
                            ),
                    );
                }
                if has_version && has_branches {
                    body = body.child(
                        v_flex()
                            .w_full()
                            .gap_1p5()
                            .child(Self::field_label(t("game.install.branch"), cx))
                            .child(
                                Select::new(&self.branch_select)
                                    .placeholder(t("game.install.branch")),
                            ),
                    );
                }
            }

            if has_package {
                body = body.child(
                    v_flex()
                        .w_full()
                        .gap_1p5()
                        .child(Self::field_label(t("game.install.package"), cx))
                        .child(
                            Select::new(&self.build_select)
                                .placeholder(t("game.install.package"))
                                .disabled(builds < 2),
                        ),
                );
            } else if has_version {
                if builds > 0 {
                    body = body
                        .child(
                            Checkbox::new("pin-build")
                                .label(t("game.install.pin"))
                                .checked(self.pin_build)
                                .on_change(cx.listener(|this, v: &bool, window, cx| {
                                    this.pin_build = *v;
                                    this.fetch_info(window, cx);
                                })),
                        )
                        .when(self.pin_build, |c| {
                            c.child(
                                Select::new(&self.build_select)
                                    .placeholder(t("game.common.version"))
                                    .disabled(builds < 2),
                            )
                        });
                }
                if has_password {
                    body = body.child(
                        h_flex()
                            .w_full()
                            .gap_2()
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .child(Input::new(&self.password).mask_toggle()),
                            )
                            .child(
                                Button::new("apply-branch-password")
                                    .outline()
                                    .label(t("game.install.unlock"))
                                    .loading(self.password_busy)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.apply_password(window, cx)
                                    })),
                            ),
                    );
                }
            } else if has_skip_updates {
                body = body.child(
                    Checkbox::new("skip-updates")
                        .label(t("game.install.skip_updates"))
                        .checked(self.pin_build)
                        .on_change(cx.listener(|this, v: &bool, _, cx| {
                            this.pin_build = *v;
                            cx.notify();
                        })),
                );
            }

            if has_wine {
                let has_above = has_language || has_package || has_version || has_skip_updates;
                let empty = self.wines.ready().is_some_and(|w| w.is_empty());
                let wine_block = v_flex()
                    .w_full()
                    .gap_2()
                    .when(has_above, |b| b.pt_3().border_t_1().border_color(border))
                    .child(Self::field_label(t("game.install.compatibility_tool"), cx))
                    .map(|col| {
                        if self.wines.is_loading() {
                            col.child(
                                h_flex()
                                    .gap_2()
                                    .text_sm()
                                    .child(Spinner::new().small())
                                    .child(t("game.install.looking_for_wine")),
                            )
                        } else if empty || matches!(self.proton, ProtonDownload::Running(_)) {
                            col.child(self.render_proton(cx))
                        } else {
                            col.child(
                                Select::new(&self.wine_select)
                                    .placeholder(t("game.install.wine_version")),
                            )
                        }
                    })
                    .child(Self::field_label(t("game.install.wine_folder"), cx))
                    .child(
                        h_flex()
                            .w_full()
                            .gap_2()
                            .child(div().flex_1().min_w_0().child(Input::new(&self.prefix)))
                            .child(
                                Button::new("browse-prefix")
                                    .outline()
                                    .icon(IconName::FolderOpen)
                                    .label(t("game.common.browse"))
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.browse_prefix(window, cx)
                                    })),
                            ),
                    );
                body = body.child(wine_block);
            }

            card = card.child(body);
        }

        Some(card.into_any_element())
    }
}

/// Install warning for a gated anticheat, naming the products when known.
fn anticheat_warning(status: AnticheatStatus, products: &[String]) -> SharedString {
    let key = match (status, products.is_empty()) {
        (AnticheatStatus::Broken, false) => "game.install.anticheat.broken",
        (AnticheatStatus::Broken, true) => "game.install.anticheat.broken_unnamed",
        (AnticheatStatus::Denied, false) => "game.install.anticheat.denied",
        (AnticheatStatus::Denied, true) => "game.install.anticheat.denied_unnamed",
        (_, false) => "game.install.anticheat.unknown",
        (_, true) => "game.install.anticheat.unknown_unnamed",
    };
    t_args(key, &[("products", &products.join(", "))])
}

impl Choice {
    fn value_str(&self) -> &str {
        use gpui_kit::component::select::SelectItem as _;
        self.value().as_ref()
    }
}

impl Render for InstallSheet {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let notices = self.render_notices(cx);
        let platform = self.render_platform(cx);
        let account = self.render_account(cx);
        let location = self.render_location(cx);
        let sdl = self.info.ready().and_then(|info| self.render_sdl(info, cx));
        let dlcs = self
            .info
            .ready()
            .and_then(|info| self.render_dlcs(info, cx));
        let more_options = self.render_more_options(cx);
        let modify_loading = (self.is_modify()
            && self.info.is_loading()
            && notices.is_empty()
            && dlcs.is_none()
            && more_options.is_none())
        .then(|| {
            Self::card(cx)
                .p_3p5()
                .gap_2p5()
                .child(Skeleton::new().h_4().w(rems(10.)).rounded_sm())
                .child(Skeleton::new().h_8().w_full().rounded_sm())
                .into_any_element()
        });

        v_flex()
            .id("install-sheet")
            .debug_selector(|| "install-sheet".into())
            .w_full()
            .px_6()
            .pt_3()
            .pb_5()
            .gap_4()
            .children(notices)
            .children(platform)
            .children(account)
            .children(location)
            .children(sdl)
            .children(dlcs)
            .children(more_options)
            .children(modify_loading)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CompatBadgeTone {
    Success,
    Warning,
    Danger,
    Neutral,
}

fn protondb_tone(tier: &str) -> CompatBadgeTone {
    match tier.trim().to_ascii_lowercase().as_str() {
        "platinum" | "native" => CompatBadgeTone::Success,
        "gold" | "silver" | "bronze" => CompatBadgeTone::Warning,
        "borked" => CompatBadgeTone::Danger,
        _ => CompatBadgeTone::Neutral,
    }
}

/// Pinned Hero Header of the install / modify modal: artwork cover, title,
/// badges, ambient art tint, close button and live 3-metric storage summary.
struct InstallHeader {
    sheet: Entity<InstallSheet>,
    _observe: Vec<Subscription>,
}

impl InstallHeader {
    fn new(sheet: Entity<InstallSheet>, cx: &mut Context<Self>) -> Self {
        let art = irori_ui::art::art_store(cx);
        let observe = vec![
            cx.observe(&sheet, |_, _, cx| cx.notify()),
            cx.observe(&art, |_, _, cx| cx.notify()),
        ];
        Self {
            sheet,
            _observe: observe,
        }
    }

    fn render_store_pill(runner: Runner, cx: &App) -> AnyElement {
        let theme = cx.theme();
        let store_name: SharedString = runner.store_name().into();
        let pill = h_flex()
            .id("install-store-badge")
            .items_center()
            .justify_center()
            .px_2()
            .py_1()
            .rounded(theme.radius)
            .bg(theme.secondary.opacity(0.65))
            .border_1()
            .border_color(theme.border)
            .tooltip(move |window, cx| Tooltip::new(store_name.clone()).build(window, cx));
        match StoreLogo::of_runner(runner) {
            Some(logo) => pill
                .child(logo.icon().size(px(20.)).text_color(theme.foreground))
                .into_any_element(),
            None => pill
                .child(
                    div()
                        .px_0p5()
                        .text_xs()
                        .font_semibold()
                        .text_color(theme.foreground)
                        .child(runner.short_name()),
                )
                .into_any_element(),
        }
    }

    fn render_metric(
        icon: IconName,
        accent: Hsla,
        label: SharedString,
        value: Option<(SharedString, Hsla)>,
        cx: &App,
    ) -> AnyElement {
        let theme = cx.theme();
        let accent_ink = legible(
            accent,
            &[theme.background, theme.secondary],
            ICON_CONTRAST,
            cx,
        );
        h_flex()
            .flex_1()
            .min_w(rems(8.))
            .items_center()
            .gap_3()
            .px_3p5()
            .py_2p5()
            .rounded(theme.radius)
            .bg(theme.secondary.opacity(0.45))
            .border_1()
            .border_color(theme.border.opacity(0.75))
            .child(
                h_flex()
                    .size(px(32.))
                    .flex_shrink_0()
                    .items_center()
                    .justify_center()
                    .rounded(theme.radius)
                    .bg(accent.opacity(0.14))
                    .border_1()
                    .border_color(accent.opacity(0.28))
                    .child(Icon::new(icon).size(px(16.)).text_color(accent_ink)),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_0p5()
                    .child(
                        div()
                            .text_xs()
                            .font_semibold()
                            .text_color(theme.muted_foreground)
                            .truncate()
                            .child(SharedString::from(label.to_uppercase())),
                    )
                    .child(match value {
                        Some((text, color)) => div()
                            .text_lg()
                            .font_bold()
                            .text_color(color)
                            .truncate()
                            .child(text)
                            .into_any_element(),
                        None => Skeleton::new()
                            .h_5()
                            .w(rems(5.))
                            .rounded_sm()
                            .into_any_element(),
                    }),
            )
            .into_any_element()
    }
}

impl Render for InstallHeader {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let state = self.sheet.read(cx);
        let game = state.game.clone();
        let modify = state.is_modify();
        let info_loading = state.info.is_loading();
        let disk_loading = state.disk.is_loading();
        let size = state.size();
        let space = state.disk.ready().copied();
        let verdict = state.disk_verdict();
        let compat = state.compat.clone();

        let tint = (!reduce_transparency(cx))
            .then(|| {
                game.hero_art()
                    .and_then(|url| irori_ui::art::ambient(url, cx))
                    .map(|a| art_tint(a.rgb, cx))
            })
            .flatten();

        let theme = cx.theme();
        let dark = theme.mode.is_dark();
        let bg = theme.background;
        let fg = theme.foreground;
        let muted = theme.muted_foreground;
        let border = theme.border;
        let info_color = theme.info;
        let primary_color = theme.primary;
        let success_color = legible(theme.success, &[bg], TEXT_CONTRAST, cx);
        let danger_color = legible(theme.danger, &[bg], TEXT_CONTRAST, cx);

        let subtitle: Option<SharedString> = if modify {
            let base = t("game.install.subtitle_modify");
            Some(
                match game
                    .install
                    .as_ref()
                    .map(|i| i.version.trim())
                    .filter(|v| !v.is_empty())
                {
                    Some(ver) => format!("{base} · {ver}").into(),
                    None => base,
                },
            )
        } else {
            game.developer
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .or_else(|| {
                    game.publisher
                        .as_deref()
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                })
                .map(|s| SharedString::from(s.to_string()))
        };

        let mut compat_pills: Vec<AnyElement> = Vec::new();
        if cfg!(target_os = "linux")
            && let Some(c) = &compat
        {
            if let Some(tier) = &c.protondb_tier {
                let tier_text = crate::page::compat::tier_label(tier);
                let tip = t_args("game.header.protondb", &[("tier", &tier_text)]);
                let tag = match protondb_tone(tier) {
                    CompatBadgeTone::Success => Tag::success(),
                    CompatBadgeTone::Warning => Tag::warning(),
                    CompatBadgeTone::Danger => Tag::danger(),
                    CompatBadgeTone::Neutral => Tag::secondary(),
                };
                compat_pills.push(
                    div()
                        .id("install-protondb-badge")
                        .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
                        .child(tag.xsmall().outline().font_medium().child(tier_text))
                        .into_any_element(),
                );
            }
            if let Some(deck) = c.steam_deck.filter(|d| *d != SteamDeckStatus::Unknown) {
                let deck_label = deck.label();
                let tip = t_args("game.header.deck", &[("status", &deck_label)]);
                let tag = match deck {
                    SteamDeckStatus::Verified => Tag::success(),
                    SteamDeckStatus::Playable => Tag::warning(),
                    SteamDeckStatus::Unsupported => Tag::danger(),
                    SteamDeckStatus::Unknown => Tag::secondary(),
                };
                compat_pills.push(
                    div()
                        .id("install-deck-badge")
                        .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
                        .child(
                            tag.xsmall()
                                .outline()
                                .font_medium()
                                .child(format!("Deck {deck_label}")),
                        )
                        .into_any_element(),
                );
            }
        }

        let download_value = (!info_loading).then(|| {
            let text = if size.download == 0 {
                t("game.install.unknown")
            } else {
                format_bytes(size.download).into()
            };
            (text, fg)
        });
        let disk_value = (!info_loading).then(|| {
            let text = if size.disk == 0 {
                t("game.install.unknown")
            } else {
                format_bytes(size.disk).into()
            };
            (text, fg)
        });
        let (third_icon, third_accent, third_label, third_value) = if info_loading || disk_loading {
            (
                IconName::CircleCheck,
                theme.success,
                t("game.install.free_after"),
                None,
            )
        } else {
            match verdict {
                DiskVerdict::Fits { left_after } => (
                    IconName::CircleCheck,
                    theme.success,
                    t("game.install.free_after"),
                    Some((format_bytes(left_after).into(), success_color)),
                ),
                DiskVerdict::TooSmall { missing } => (
                    IconName::TriangleAlert,
                    theme.danger,
                    t("game.install.short_by"),
                    Some((format_bytes(missing).into(), danger_color)),
                ),
                DiskVerdict::NotWritable => (
                    IconName::TriangleAlert,
                    theme.danger,
                    t("game.install.free_after"),
                    Some((
                        space
                            .map(|s| format_bytes(s.free).into())
                            .unwrap_or_else(|| t("game.install.unknown")),
                        danger_color,
                    )),
                ),
                DiskVerdict::Unknown => (
                    IconName::CircleCheck,
                    if space.is_some() {
                        theme.success
                    } else {
                        muted
                    },
                    t("game.install.free_after"),
                    Some((
                        space
                            .map(|s| format_bytes(s.free).into())
                            .unwrap_or_else(|| t("game.install.unknown")),
                        if space.is_some() {
                            success_color
                        } else {
                            muted
                        },
                    )),
                ),
            }
        };

        div()
            .id("install-header")
            .debug_selector(|| "install-header".into())
            .relative()
            .w_full()
            .bg(bg)
            .border_b_1()
            .border_color(border)
            .overflow_hidden()
            .when_some(tint, |el, tint| {
                let top_alpha = if dark { 0.24 } else { 0.13 };
                el.child(div().absolute().inset_0().bg(linear_gradient(
                    140.,
                    linear_color_stop(tint.opacity(top_alpha), 0.),
                    linear_color_stop(tint.opacity(0.), 0.78),
                )))
            })
            .child(
                h_flex()
                    .relative()
                    .w_full()
                    .px_6()
                    .py_5()
                    .gap_5()
                    .items_stretch()
                    .child(
                        div()
                            .flex_shrink_0()
                            .rounded(theme.radius)
                            .border_1()
                            .border_color(border)
                            .overflow_hidden()
                            .shadow_md()
                            .child(
                                GameCover::new(game.clone())
                                    .width(rems(6.))
                                    .radius(theme.radius),
                            ),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .justify_between()
                            .gap_3()
                            .child(
                                h_flex()
                                    .w_full()
                                    .items_start()
                                    .justify_between()
                                    .gap_3()
                                    .child(
                                        v_flex()
                                            .flex_1()
                                            .min_w_0()
                                            .gap_1p5()
                                            .child(
                                                h_flex()
                                                    .items_center()
                                                    .gap_2p5()
                                                    .flex_wrap()
                                                    .child(
                                                        div()
                                                            .min_w_0()
                                                            .text_2xl()
                                                            .font_extrabold()
                                                            .text_color(fg)
                                                            .line_clamp(2)
                                                            .child(game.title.clone()),
                                                    )
                                                    .child(Self::render_store_pill(
                                                        game.runner(),
                                                        cx,
                                                    ))
                                                    .when(modify, |row| {
                                                        row.child(
                                                            Tag::primary()
                                                                .xsmall()
                                                                .outline()
                                                                .font_semibold()
                                                                .child(t(
                                                                    "game.install.modify_badge",
                                                                )),
                                                        )
                                                    }),
                                            )
                                            .when(
                                                !compat_pills.is_empty() || subtitle.is_some(),
                                                |col| {
                                                    col.child(
                                                        h_flex()
                                                            .min_w_0()
                                                            .items_center()
                                                            .gap_2()
                                                            .flex_wrap()
                                                            .children(compat_pills)
                                                            .when_some(subtitle, |row, sub| {
                                                                row.child(
                                                                    div()
                                                                        .min_w_0()
                                                                        .text_sm()
                                                                        .font_medium()
                                                                        .text_color(muted)
                                                                        .truncate()
                                                                        .child(sub),
                                                                )
                                                            }),
                                                    )
                                                },
                                            ),
                                    )
                                    .child(
                                        Button::new("install-modal-close")
                                            .flex_shrink_0()
                                            .small()
                                            .ghost()
                                            .icon(IconName::X)
                                            .accessibility_label(t("game.install.close"))
                                            .tooltip(t("game.install.close"))
                                            .on_click(|_, window, cx| window.close_dialog(cx)),
                                    ),
                            )
                            .child(
                                h_flex()
                                    .w_full()
                                    .items_stretch()
                                    .gap_3()
                                    .child(Self::render_metric(
                                        IconName::CloudDownload,
                                        info_color,
                                        t("game.install.download"),
                                        download_value,
                                        cx,
                                    ))
                                    .child(Self::render_metric(
                                        IconName::HardDriveDownload,
                                        primary_color,
                                        t("game.install.size_on_disk"),
                                        disk_value,
                                        cx,
                                    ))
                                    .child(Self::render_metric(
                                        third_icon,
                                        third_accent,
                                        third_label,
                                        third_value,
                                        cx,
                                    )),
                            ),
                    ),
            )
    }
}

/// Modal footer: secondary actions and the primary `Install · <size>` / `Save changes` button.
pub struct InstallFooter {
    sheet: Entity<InstallSheet>,
    _observe: Subscription,
}

impl InstallFooter {
    fn new(sheet: Entity<InstallSheet>, cx: &mut Context<Self>) -> Self {
        let observe = cx.observe(&sheet, |_, _, cx| cx.notify());
        Self {
            sheet,
            _observe: observe,
        }
    }
}

impl Render for InstallFooter {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let state = self.sheet.read(cx);
        let blocker = state.blocker(cx);
        let label = state.primary_label();
        let modify = state.is_modify();
        let has_changes = state.has_changes();
        let submitting = state.submitting;
        let id = state.id.clone();
        let sheet = self.sheet.clone();
        let theme = cx.theme();

        h_flex()
            .w_full()
            .px_6()
            .py_4()
            .gap_3()
            .flex_wrap()
            .items_center()
            .justify_between()
            .border_t_1()
            .border_color(theme.border)
            .bg(theme.background)
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .when(!modify, |row| {
                        row.child(
                            Button::new("import-instead")
                                .debug_selector(|| "import-instead".into())
                                .ghost()
                                .icon(IconName::FolderInput)
                                .label(t("game.install.import_existing"))
                                .on_click(move |_, window, cx| {
                                    window.close_dialog(cx);
                                    crate::manage::open_import_dialog(id.clone(), window, cx);
                                }),
                        )
                    })
                    .when(modify && has_changes && blocker.is_none(), |row| {
                        row.child(
                            div()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .child(t("game.install.changes_ready")),
                        )
                    }),
            )
            .child(
                h_flex()
                    .min_w_0()
                    .items_center()
                    .gap_2()
                    .when_some(blocker.filter(|_| !submitting), |row, reason| {
                        row.child(
                            div()
                                .min_w_0()
                                .truncate()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .child(t(reason)),
                        )
                    })
                    .when(modify, |row| {
                        row.child(
                            Button::new("install-cancel")
                                .outline()
                                .label(t("game.common.cancel"))
                                .on_click(|_, window, cx| window.close_dialog(cx)),
                        )
                    })
                    .child(
                        Button::new("install-submit")
                            .primary()
                            .when(!modify, |b| b.icon(Icon::new(IconName::Download)))
                            .label(label)
                            .loading(submitting)
                            .disabled(blocker.is_some())
                            .on_click(move |_, window, cx| {
                                sheet.update(cx, |s, cx| s.submit(window, cx));
                            }),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Epic games the EA app or Ubisoft Connect installs have no sizes; the
    /// modal must offer Install, not "nothing to install" (#167).
    #[test]
    fn launcher_games_have_something_to_install() {
        let ea = GameInfo {
            runner: Some(Runner::Legendary),
            third_party_managed_app: Some("The EA App".into()),
            is_ea_managed: true,
            ..Default::default()
        };
        let info = InstallInfo {
            third_party_managed_app: Some("The EA App".into()),
            ..Default::default()
        };
        assert!(!nothing_to_install(&info, &ea));
        assert!(!nothing_to_install(&InstallInfo::default(), &ea));

        let goodies = GameInfo {
            runner: Some(Runner::Gog),
            ..Default::default()
        };
        assert!(nothing_to_install(&InstallInfo::default(), &goodies));
        let sized = InstallInfo {
            download_size: 1,
            ..Default::default()
        };
        assert!(!nothing_to_install(&sized, &goodies));
    }

    #[test]
    fn only_gog_mentions_gog_bonus_content() {
        assert_eq!(
            nothing_to_install_text(Runner::Gog),
            "game.install.nothing_to_install"
        );
        for runner in [Runner::Legendary, Runner::Nile, Runner::Zoom, Runner::Itch] {
            assert_eq!(
                nothing_to_install_text(runner),
                "game.install.nothing_to_install_other"
            );
        }
    }

    #[test]
    fn native_linux_is_the_default_platform() {
        let game = GameInfo {
            is_linux_native: true,
            platforms: vec![InstallPlatform::Windows, InstallPlatform::Linux],
            ..Default::default()
        };
        assert_eq!(
            installable_platforms(&game),
            if cfg!(target_os = "linux") {
                vec![InstallPlatform::Linux, InstallPlatform::Windows]
            } else {
                vec![InstallPlatform::Windows]
            }
        );
        let windows_only = GameInfo {
            platforms: vec![InstallPlatform::Windows, InstallPlatform::Mac],
            ..Default::default()
        };
        assert_eq!(
            installable_platforms(&windows_only),
            if cfg!(target_os = "macos") {
                vec![InstallPlatform::Mac, InstallPlatform::Windows]
            } else {
                vec![InstallPlatform::Windows]
            }
        );
        assert_eq!(
            installable_platforms(&GameInfo::default()),
            vec![InstallPlatform::Windows]
        );
    }

    #[test]
    fn more_options_auto_expands_for_modify_or_missing_wine() {
        assert!(should_auto_expand_options(
            InstallMode::Modify,
            false,
            false,
            &ProtonDownload::Offered
        ));
        assert!(!should_auto_expand_options(
            InstallMode::Install,
            false,
            false,
            &ProtonDownload::Offered
        ));
        assert!(!should_auto_expand_options(
            InstallMode::Install,
            true,
            false,
            &ProtonDownload::Offered
        ));
        assert!(should_auto_expand_options(
            InstallMode::Install,
            true,
            true,
            &ProtonDownload::Offered
        ));
        assert!(should_auto_expand_options(
            InstallMode::Install,
            true,
            false,
            &ProtonDownload::Finding
        ));
        assert!(should_auto_expand_options(
            InstallMode::Install,
            true,
            false,
            &ProtonDownload::Running("ge".into())
        ));
        assert!(should_auto_expand_options(
            InstallMode::Install,
            true,
            false,
            &ProtonDownload::Failed("network".into())
        ));
        // On Windows (needs_wine == false), missing wine does not force expand.
        assert!(!should_auto_expand_options(
            InstallMode::Install,
            false,
            true,
            &ProtonDownload::Offered
        ));
    }

    #[test]
    fn protondb_tone_is_case_insensitive() {
        assert_eq!(protondb_tone("Platinum"), CompatBadgeTone::Success);
        assert_eq!(protondb_tone("native"), CompatBadgeTone::Success);
        assert_eq!(protondb_tone("Gold"), CompatBadgeTone::Warning);
        assert_eq!(protondb_tone("gold"), CompatBadgeTone::Warning);
        assert_eq!(protondb_tone("Silver"), CompatBadgeTone::Warning);
        assert_eq!(protondb_tone("Bronze"), CompatBadgeTone::Warning);
        assert_eq!(protondb_tone("Borked"), CompatBadgeTone::Danger);
        assert_eq!(protondb_tone("Unknown"), CompatBadgeTone::Neutral);
    }
}
