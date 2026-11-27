//! The install modal (new installs) and its Modify mode (language, branch,
//! build and DLCs of an installed game), centered with game artwork, live
//! storage metrics and progressive disclosure.

mod chrome;
mod fetch;
mod proton;
mod sections;
mod submit;
#[cfg(test)]
mod tests;

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

use gpui_kit::component::WindowExt as _;
use gpui_kit::component::dialog::DialogEntrance;
use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::component::select::SelectEvent;
use gpui_kit::component::skeleton::Skeleton;
use gpui_kit::component::v_flex;
use gpui_kit::{
    App, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement, ParentElement,
    Pixels, Render, SharedString, Styled, Subscription, Task, Window, px, rems,
};
use irori_core::{GameId, GameInfo, InstallInfo, InstallPlatform, Runner, WineInstallation};
use irori_ui::backend;
use irori_ui::i18n::{t, t_args};
use irori_ui::style::{reduce_motion, visible_size};
use irori_ui::wine_installs;

pub use self::chrome::InstallFooter;
use self::chrome::InstallHeader;
use self::proton::ProtonDownload;
use crate::choice::{Choice, ChoiceSelect, choice_select};
use crate::hooks::CompatibilityInfo;
use crate::load::Load;
use crate::ops::notify_error;
use crate::sizes::{DiskSpace, InstallSize, drive_label};

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
    is_default: bool,
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
                    let is_default = f.is_default();
                    let label: SharedString = if is_default {
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
                        is_default,
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
