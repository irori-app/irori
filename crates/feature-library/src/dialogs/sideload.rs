//! Adding and editing apps that are not from a store ("sideloaded" apps):
//! native and Windows programs, and web apps.
//!
//! Compared with Heroic: the platform can be changed when editing, launch
//! arguments are part of the form, the title is not truncated or stripped of
//! characters, addresses are completed only on save (so `http://` survives),
//! and no third-party image lookup runs behind the user's back (art is found
//! on SteamGridDB only when the user asks).
//!
//! "Run installer…" runs a Windows setup program in the app's Wine prefix
//! before its executable is known. A new app is added first (with the
//! installer as its program) so it has settings and a prefix; the prefix
//! path is pinned so renaming the app later keeps the installed files. If
//! the dialog is closed without saving, that provisional entry is removed.
//!
//! The form asks first for what playing needs (title, platform, program,
//! and for Windows programs the Wine or Proton build and prefix); the rest
//! waits under "More details", and Add stays in the dialog's footer.

use std::cell::Cell;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::assets::IconName;
use gpui_kit::component::alert::Alert;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::form::{field, v_form};
use gpui_kit::component::input::{Input, InputEvent, InputState, Textarea, TextareaState};
use gpui_kit::component::list::ListItem;
use gpui_kit::component::notification::Notification;
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, Sizable as _, StyledExt as _, WindowExt as _, h_flex,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    Animation, AnimationExt as _, App, AppContext as _, BoxShadow, Context, ElementId, Entity,
    Focusable as _, Hsla, InteractiveElement as _, IntoElement, ObjectFit, ParentElement as _,
    Render, SharedString, StatefulInteractiveElement as _, Styled as _, StyledImage as _,
    Subscription, Task, Window, div, point, px, rems, transparent_black,
};
use irori_core::process::CancellationToken;
use irori_core::{GameId, GameInfo, GameMode, Runner, Store as _};
use irori_extras::shortcuts::{self, ShortcutTargets};
use irori_sideload::scan::{
    DetectionOutcome, InstalledCandidate, PrefixSnapshot, detect_installed_executables,
};
use irori_sideload::{SideloadApp, SideloadPlatform};
use irori_ui::components::{art_img, physical_width};
use irori_ui::i18n::{t, t_args};
use irori_ui::style::{KeyboardFocusable as _, TEXT_CONTRAST, legible};
use irori_ui::wine_choice::{WineChoice, WineSetup};
use irori_ui::{Route, backend, navigate};

use super::steamgrid::{ArtKind, ArtPicker, ArtPickerEvent};
use super::{dialog_width, optional_text, pick_path, text};

const PLATFORMS: &[SideloadPlatform] = if cfg!(target_os = "macos") {
    &[
        SideloadPlatform::Mac,
        SideloadPlatform::Windows,
        SideloadPlatform::Browser,
    ]
} else if cfg!(windows) {
    &[SideloadPlatform::Windows, SideloadPlatform::Browser]
} else {
    &[
        SideloadPlatform::Windows,
        SideloadPlatform::Linux,
        SideloadPlatform::Browser,
    ]
};

/// Name of an app type choice.
fn platform_label(platform: SideloadPlatform) -> SharedString {
    match platform {
        SideloadPlatform::Windows => t("library.sideload.platform.pc_windows"),
        SideloadPlatform::Linux => t("library.sideload.platform.pc_linux"),
        SideloadPlatform::Mac => t("library.sideload.platform.pc_mac"),
        SideloadPlatform::Browser => t("library.sideload.platform.web"),
    }
}

fn platform_icon(platform: SideloadPlatform) -> IconName {
    match platform {
        SideloadPlatform::Browser => IconName::Globe,
        SideloadPlatform::Windows | SideloadPlatform::Linux | SideloadPlatform::Mac => {
            IconName::AppWindow
        }
    }
}

fn active_accent(cx: &App) -> Hsla {
    let theme = cx.theme();
    legible(theme.primary, &[theme.background], TEXT_CONTRAST, cx)
}

const FIND_ONLINE_FLASH_DURATION: Duration = Duration::from_millis(1300);

/// Smooth two-pulse curve in `0.0..=1.0` (peaks at `0.25` and `0.75`, `0.0` at `1.0`).
fn double_flash_strength(t: f32) -> f32 {
    (t.clamp(0.0, 1.0) * std::f32::consts::TAU).sin().powi(2)
}

/// Active tab inside the expanded "More details" section.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum DetailsTab {
    #[default]
    Artwork,
    Details,
}

/// Title suggested from an executable's file name (`/x/Hollow_Knight.exe` →
/// `Hollow Knight`), or from its parent folder when the executable has a
/// generic installer name (`/x/Void Reaver [FitGirl Repack]/setup.exe` →
/// `Void Reaver`).
pub fn title_from_executable(path: &Path) -> Option<String> {
    let stem = path.file_stem()?.to_str()?;
    let lower = stem.trim().to_ascii_lowercase();
    let generic = matches!(
        lower.as_str(),
        "setup" | "install" | "installer" | "autorun"
    );
    let raw = if generic {
        path.parent()
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
            .filter(|n| !n.trim().is_empty())
            .unwrap_or(stem)
    } else {
        stem
    };
    let stripped = strip_bracket_tags(raw);
    let source = if stripped.trim().is_empty() {
        raw
    } else {
        &stripped
    };
    let title = source.replace(['_', '.'], " ");
    let title = title.split_whitespace().collect::<Vec<_>>().join(" ");
    (!title.is_empty()).then_some(title)
}

/// Remove `[FitGirl Repack]`-style bracketed tags from a folder name.
fn strip_bracket_tags(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut depth = 0usize;
    for ch in s.chars() {
        match ch {
            '[' => depth += 1,
            ']' => depth = depth.saturating_sub(1),
            _ if depth == 0 => out.push(ch),
            _ => {}
        }
    }
    out
}

/// The add/edit form.
pub struct SideloadDialog {
    /// Library id of the app being edited; `None` when adding.
    editing: Option<String>,
    title: Entity<InputState>,
    executable: Entity<InputState>,
    arguments: Entity<InputState>,
    /// A command line that starts a Linux or macOS game instead of its
    /// program.
    command: Entity<InputState>,
    install_folder: Entity<InputState>,
    url: Entity<InputState>,
    user_agent: Entity<InputState>,
    cover: Entity<InputState>,
    background: Entity<InputState>,
    pub(crate) description: Entity<TextareaState>,
    developer: Entity<InputState>,
    release_date: Entity<InputState>,
    genres: Entity<InputState>,
    /// The app's modes as PCGamingWiki says.
    known_modes: Vec<GameMode>,
    /// The app's modes as ticked; kept as the user's own (`GameOverrides`).
    modes: BTreeSet<GameMode>,
    platform: SideloadPlatform,
    fullscreen: bool,
    /// The link another launcher starts the app from (Playnite's games);
    /// the form doesn't show it, so it is kept as it was.
    launch_link: Option<String>,
    /// The ROM library entry this new app replaces ("Play as a PC game…"):
    /// what the user keeps about it moves to the app, and the ROM leaves
    /// the library.
    replaces: Option<GameId>,
    saving: bool,
    /// Saved successfully (a provisional entry is then kept).
    saved: bool,
    /// Library id of an app added only to run its installer; it becomes the
    /// new app on save and is removed if the dialog closes without saving.
    provisional: Option<String>,
    installer: InstallerState,
    error: Option<SharedString>,
    /// The Wine or Proton build and prefix of a Windows program; `None` on
    /// Windows, which runs it itself.
    wine: Option<Entity<WineChoice>>,
    /// The optional fields are shown.
    more: bool,
    /// Which tab is active inside "More details".
    details_tab: DetailsTab,
    /// One-click keyless Steam artwork fetch is currently running.
    fetching_art: bool,
    _fetch_art_task: Option<Task<()>>,
    /// Active 2x border-flash generation on "Find online…" after a Steam miss.
    find_online_flash: Option<u64>,
    next_flash_seq: u64,
    /// The SteamGridDB picker, shown instead of the form while open.
    picker: Option<Entity<ArtPicker>>,
    _picker_subscription: Option<Subscription>,
    _subscriptions: Vec<Subscription>,
}

/// Prefix and baseline snapshot for an installer started from this dialog.
#[derive(Debug, Clone)]
pub(crate) struct InstallerRun {
    pub(crate) prefix: PathBuf,
    pub(crate) snapshot: Arc<PrefixSnapshot>,
    pub(crate) installer: PathBuf,
    pub(crate) extra_dir: Option<PathBuf>,
}

/// Progress of "Run installer…".
#[derive(Debug, Clone)]
enum InstallerState {
    Idle,
    /// Preparing the prefix and starting the setup program.
    Starting,
    /// The installer is running; Irori waits for it to exit and then scans.
    Running {
        run: InstallerRun,
        cancel: CancellationToken,
    },
    /// One executable was auto-filled (or chosen by the user from a list).
    Picked {
        run: InstallerRun,
        picked: InstalledCandidate,
        alternatives: Vec<InstalledCandidate>,
    },
    /// Multiple executables were found; ask the user to choose one.
    Choose {
        run: InstallerRun,
        candidates: Vec<InstalledCandidate>,
    },
    /// No executable was detected; show the manual Browse… guidance.
    Manual {
        run: InstallerRun,
    },
}

impl InstallerState {
    fn is_busy(&self) -> bool {
        matches!(self, Self::Starting | Self::Running { .. })
    }
}

/// How saving the form applies to the sideload store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SaveMode {
    /// Add a new app.
    Add,
    /// Turn the entry created for an installer into the new app.
    Finish(String),
    /// Change an existing app.
    Edit(String),
}

/// Pick the save mode from the dialog's state.
pub fn save_mode(editing: Option<&str>, provisional: Option<&str>) -> SaveMode {
    match (editing, provisional) {
        (Some(name), _) => SaveMode::Edit(name.to_string()),
        (None, Some(name)) => SaveMode::Finish(name.to_string()),
        (None, None) => SaveMode::Add,
    }
}

impl SideloadDialog {
    /// The dialog for `editing`, or for a new app filled in from `prefill`
    /// (a file dropped on the library).
    pub(crate) fn with_app(
        editing: Option<String>,
        prefill: Option<SideloadApp>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let stored = editing
            .as_deref()
            .and_then(|name| crate::sideload_store(cx).and_then(|s| s.app(name)));
        let given = stored.is_some() || prefill.is_some();
        let existing = stored.or(prefill).unwrap_or_default();
        let path_text = |p: &Option<PathBuf>| {
            p.as_ref()
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_default()
        };
        let mut input = |placeholder: SharedString, value: String| {
            cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder(placeholder)
                    .default_value(value)
            })
        };
        let title = input(
            t("library.sideload.placeholder.title"),
            existing.title.clone(),
        );
        let executable = input("/path/to/game".into(), path_text(&existing.executable));
        let arguments = input(
            t("library.sideload.placeholder.arguments"),
            existing.launcher_args.clone(),
        );
        let command = input(
            t("library.sideload.placeholder.command"),
            existing.command.clone().unwrap_or_default(),
        );
        let install_folder = input(
            t("library.sideload.placeholder.install_folder"),
            path_text(&existing.install_folder),
        );
        let url = input(
            "https://example.com/game".into(),
            existing.browser_url.clone().unwrap_or_default(),
        );
        let user_agent = input(
            t("library.sideload.placeholder.optional"),
            existing.custom_user_agent.clone().unwrap_or_default(),
        );
        let cover = input(
            t("library.sideload.placeholder.image"),
            existing.cover_art.clone().unwrap_or_default(),
        );
        let background = input(
            t("library.sideload.placeholder.image"),
            existing.background_art.clone().unwrap_or_default(),
        );
        let developer = input(
            t("library.sideload.placeholder.optional"),
            existing.developer.clone().unwrap_or_default(),
        );
        let release_date = input(
            "2024-05-30".into(),
            existing.release_date.clone().unwrap_or_default(),
        );
        let genres = input(
            t("library.sideload.placeholder.optional"),
            existing.genres.join(", "),
        );
        let description = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(3, 8)
                .placeholder(t("library.sideload.placeholder.optional"))
                .default_value(existing.description.clone().unwrap_or_default())
        });

        let (known_modes, modes) = match &editing {
            Some(name) => {
                let id = GameId::new(Runner::Sideload, name.as_str());
                let known = backend(cx).known_game_modes(&id);
                let own = irori_ui::models(cx)
                    .library
                    .read(cx)
                    .user
                    .overrides
                    .get(&id.key())
                    .and_then(|o| o.game_modes.clone());
                let ticked = own.unwrap_or_else(|| known.clone()).into_iter().collect();
                (known, ticked)
            }
            None => (Vec::new(), BTreeSet::new()),
        };

        let wine =
            (irori_core::HostPlatform::current() != irori_core::HostPlatform::Windows).then(|| {
                let stored = editing.as_deref().and_then(|name| {
                    let game = crate::sideload_store(cx)?.game(name)?;
                    Some((GameId::new(Runner::Sideload, name), game))
                });
                cx.new(|cx| match stored {
                    Some((id, game)) => WineChoice::for_game(&id, &game, window, cx),
                    None => {
                        let prefix = new_app_prefix(&existing.title, cx);
                        let mut choice = WineChoice::new(&prefix, window, cx);
                        if let Some(detected) = existing
                            .executable
                            .as_deref()
                            .and_then(irori_runtime::enclosing_wine_prefix)
                        {
                            choice.set_prefix(&detected, window, cx);
                        }
                        choice
                    }
                })
            });
        let more = editing.is_some()
            && (!existing.launcher_args.is_empty()
                || existing.command.is_some()
                || existing.install_folder.is_some()
                || existing.custom_user_agent.is_some()
                || existing
                    .description
                    .as_deref()
                    .is_some_and(|d| !d.is_empty())
                || existing.developer.is_some()
                || existing.release_date.is_some()
                || existing.genres.iter().any(|g| !g.trim().is_empty())
                || existing.cover_art.is_some()
                || existing.background_art.is_some()
                || !modes.is_empty());

        let art = irori_ui::art::art_store(cx);
        let mut subscriptions = vec![cx.observe(&art, |_, _, cx| cx.notify())];
        // A new app's prefix is named after it.
        if editing.is_none()
            && let Some(wine) = &wine
        {
            let wine = wine.clone();
            subscriptions.push(cx.subscribe_in(
                &title,
                window,
                move |_, state, event, window, cx| {
                    if matches!(event, InputEvent::Change) {
                        let title = state.read(cx).value().trim().to_string();
                        let prefix = new_app_prefix(&title, cx);
                        wine.update(cx, |w, cx| w.set_default_prefix(&prefix, window, cx));
                    }
                },
            ));
        }
        for state in [&title, &executable, &url] {
            subscriptions.push(
                cx.subscribe_in(state, window, |this, _, event, window, cx| match event {
                    InputEvent::Change => {
                        this.error = None;
                        cx.notify();
                    }
                    InputEvent::PressEnter { .. } => this.save(window, cx),
                    _ => {}
                }),
            );
        }
        for state in [&cover, &background] {
            subscriptions.push(cx.subscribe(state, |_, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    cx.notify();
                }
            }));
        }

        let details_tab =
            if existing.cover_art.is_none() && existing.background_art.is_none() && more {
                DetailsTab::Details
            } else {
                DetailsTab::Artwork
            };

        Self {
            platform: if given {
                existing.platform
            } else {
                SideloadPlatform::default()
            },
            fullscreen: existing.launch_fullscreen,
            launch_link: existing.launch_link.clone(),
            replaces: None,
            editing,
            title,
            executable,
            arguments,
            command,
            install_folder,
            url,
            user_agent,
            cover,
            background,
            description,
            developer,
            release_date,
            genres,
            known_modes,
            modes,
            saving: false,
            saved: false,
            provisional: None,
            installer: InstallerState::Idle,
            error: None,
            wine,
            more,
            details_tab,
            fetching_art: false,
            _fetch_art_task: None,
            find_online_flash: None,
            next_flash_seq: 0,
            picker: None,
            _picker_subscription: None,
            _subscriptions: subscriptions,
        }
    }

    /// The Wine choice to apply to the app, when it is a Windows program
    /// played through Wine.
    fn wine_setup(&self, cx: &App) -> Option<WineSetup> {
        (self.platform == SideloadPlatform::Windows)
            .then(|| self.wine.as_ref().map(|w| w.read(cx).setup(cx)))
            .flatten()
    }

    fn dialog_title(&self, cx: &App) -> SharedString {
        match (&self.picker, &self.editing) {
            (Some(p), _) => p.read(cx).kind().title(),
            (None, Some(_)) => t("library.sideload.title.edit"),
            (None, None) => t("library.sideload.title.add"),
        }
    }

    /// Fetch official Steam cover + background art for the entered title
    /// without requiring any API key. Only runs when explicitly clicked.
    fn fetch_art(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.fetching_art {
            return;
        }
        let title = text(&self.title, cx);
        if title.is_empty() {
            return;
        }
        self.fetching_art = true;
        cx.notify();
        let query = title.clone();
        let task = irori_ui::run(cx, async move {
            let matches = irori_extras::wiki::search_steam_artwork(&query).await?;
            let wanted = irori_services::search::fold(&query);
            let picked = matches
                .iter()
                .find(|m| irori_services::search::fold(&m.name) == wanted)
                .or_else(|| {
                    matches.iter().find(|m| {
                        let lower = m.name.to_ascii_lowercase();
                        !lower.contains("soundtrack") && !lower.contains(" demo")
                    })
                })
                .or_else(|| matches.first());
            let Some(picked) = picked else {
                return Ok(None);
            };
            let (cover, hero) = irori_extras::wiki::resolve_steam_artwork(picked.app_id).await;
            Ok(Some((picked.name.clone(), cover, hero)))
        });
        self._fetch_art_task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.apply_fetch_art_result(&title, result, window, cx);
            });
        }));
    }

    /// Apply the outcome of a keyless Steam artwork fetch to the dialog.
    pub(crate) fn apply_fetch_art_result(
        &mut self,
        title: &str,
        result: anyhow::Result<Option<(String, String, String)>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.fetching_art = false;
        match result {
            Ok(Some((name, cover_url, hero_url))) => {
                self.cover
                    .update(cx, |s, cx| s.set_value(cover_url, window, cx));
                self.background
                    .update(cx, |s, cx| s.set_value(hero_url, window, cx));
                self.more = true;
                self.details_tab = DetailsTab::Artwork;
                self.find_online_flash = None;
                window.push_notification(
                    Notification::success(t_args(
                        "library.sideload.fetch_art_found",
                        &[("name", &name)],
                    )),
                    cx,
                );
            }
            Ok(None) => {
                self.more = true;
                self.details_tab = DetailsTab::Artwork;
                self.next_flash_seq = self.next_flash_seq.wrapping_add(1).max(1);
                self.find_online_flash = Some(self.next_flash_seq);
                window.push_notification(
                    Notification::info(t_args(
                        "library.sideload.fetch_art_none",
                        &[("title", title)],
                    )),
                    cx,
                );
            }
            Err(e) => {
                crate::notify_error(t("library.sideload.fetch_art_failed"), &e, window, cx);
            }
        }
        cx.notify();
    }

    /// Remove the entry created for an installer when the app was not saved.
    fn discard_provisional(&mut self, cx: &mut App) {
        if let InstallerState::Running { cancel, .. } = &self.installer {
            cancel.cancel();
        }
        let Some(name) = self.provisional.take().filter(|_| !self.saved) else {
            return;
        };
        let Some(store) = crate::sideload_store(cx) else {
            return;
        };
        let b = backend(cx);
        irori_ui::state::run_infallible(cx, async move {
            if let Err(e) = store.remove_app(&name, false).await {
                tracing::warn!("removing unsaved app {name}: {e:#}");
            }
            b.settings
                .remove_game(&GameId::new(Runner::Sideload, name.as_str()));
        })
        .detach();
    }

    fn find_art(&mut self, kind: ArtKind, window: &mut Window, cx: &mut Context<Self>) {
        self.find_online_flash = None;
        let title = text(&self.title, cx);
        let picker = cx.new(|cx| ArtPicker::for_sideload(kind, &title, window, cx));
        picker.update(cx, |p, cx| p.focus(window, cx));
        self._picker_subscription = Some(cx.subscribe_in(
            &picker,
            window,
            move |this, _, event: &ArtPickerEvent, window, cx| {
                if let ArtPickerEvent::Picked(url) = event {
                    let input = if kind == ArtKind::Background {
                        &this.background
                    } else {
                        &this.cover
                    };
                    let url = url.clone();
                    input.update(cx, |s, cx| s.set_value(url, window, cx));
                }
                this.picker = None;
                this._picker_subscription = None;
                this.title.focus_handle(cx).focus(window, cx);
                cx.notify();
            },
        ));
        self.picker = Some(picker);
        cx.notify();
    }

    /// Pick a Windows installer and run it in the app's prefix.
    fn run_installer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.installer.is_busy() {
            return;
        }
        let picked = pick_path(false, t("library.sideload.picker.run_installer"), cx);
        cx.spawn_in(window, async move |this, cx| {
            let Some(installer) = picked.await else {
                return;
            };
            this.update_in(cx, |this, window, cx| {
                this.start_installer(installer, window, cx)
            })
            .ok();
        })
        .detach();
    }

    fn start_installer(&mut self, installer: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let current_title = text(&self.title, cx);
        if (current_title.is_empty()
            || matches!(
                current_title.to_ascii_lowercase().as_str(),
                "setup" | "install" | "installer"
            ))
            && let Some(suggested) = title_from_executable(&installer)
        {
            self.title
                .update(cx, |s, cx| s.set_value(suggested, window, cx));
        }
        if text(&self.title, cx).is_empty() {
            self.error = Some(t("library.sideload.error.title_first"));
            cx.notify();
            return;
        }
        let Some(store) = crate::sideload_store(cx) else {
            self.error = Some(t("library.sideload.error.unavailable"));
            cx.notify();
            return;
        };
        let existing = self.editing.clone().or_else(|| self.provisional.clone());
        let mut app = self.collect(cx);
        let extra_dir = app.install_folder.clone();
        app.platform = SideloadPlatform::Windows;
        if app.executable.is_none() {
            app.executable = Some(installer.clone());
        }
        let setup = self.wine.as_ref().map(|w| w.read(cx).setup(cx));
        self.installer = InstallerState::Starting;
        self.error = None;
        cx.notify();

        let b = backend(cx);
        let extra_for_snap = extra_dir.clone();
        let prep = irori_ui::run(cx, async move {
            let (id, created) = match existing {
                Some(name) => (GameId::new(Runner::Sideload, name.as_str()), None),
                None => {
                    let id = store.add_app(app).await?;
                    (id.clone(), Some(id.app_name.to_string()))
                }
            };
            let game = store.game(&id.app_name).ok_or_else(|| {
                anyhow::anyhow!(t("library.sideload.error.not_found").to_string())
            })?;
            // The installer runs with the build and in the prefix chosen.
            if let Some(setup) = &setup {
                setup.apply(&b, &id, &game);
            }
            let custom_prefix = setup
                .as_ref()
                .map(|s| s.prefix.trim())
                .filter(|p| !p.is_empty());
            let prefix = match custom_prefix {
                Some(p) => irori_core::paths::expand_home(p),
                None => irori_runtime::default_prefix(&b.settings.app(), &game),
            };
            if created.is_some() {
                b.settings.set_game_value(
                    &id,
                    "winePrefix",
                    Some(serde_json::Value::String(
                        prefix.to_string_lossy().into_owned(),
                    )),
                );
            }
            let snap_prefix = prefix.clone();
            let snapshot = b
                .spawn_blocking(move || {
                    PrefixSnapshot::capture(&snap_prefix, extra_for_snap.as_deref())
                })
                .await?;
            Ok((id, created, prefix, Arc::new(snapshot)))
        });
        cx.spawn_in(window, async move |this, cx| {
            let prepared = prep.await;
            let Some((run_info, cancel, run_task)) = this
                .update_in(cx, |this, window, cx| match prepared {
                    Ok((id, created, prefix, snapshot)) => {
                        if created.is_some() {
                            this.provisional = created;
                        }
                        // The prefix is pinned now; saving keeps it.
                        if let Some(wine) = &this.wine {
                            wine.update(cx, |w, cx| w.set_prefix(&prefix, window, cx));
                        }
                        let cancel = CancellationToken::new();
                        let run_info = InstallerRun {
                            prefix,
                            snapshot,
                            installer: installer.clone(),
                            extra_dir: extra_dir.clone(),
                        };
                        this.installer = InstallerState::Running {
                            run: run_info.clone(),
                            cancel: cancel.clone(),
                        };
                        let b = backend(cx);
                        let installer_for_run = installer.clone();
                        let cancel_for_run = cancel.clone();
                        let run_task = irori_ui::run(cx, async move {
                            b.run_installer_in_prefix(&id, &installer_for_run, cancel_for_run)
                                .await
                        });
                        cx.notify();
                        Some((run_info, cancel, run_task))
                    }
                    Err(e) => {
                        this.installer = InstallerState::Idle;
                        this.error = Some(installer_error(&e));
                        cx.notify();
                        None
                    }
                })
                .ok()
                .flatten()
            else {
                return;
            };

            let run = run_task.await;

            this.update_in(cx, |this, window, cx| {
                if cancel.is_cancelled() {
                    return;
                }
                match run {
                    Ok(_) => {
                        this.scan_prefix_after_installer(run_info, window, cx);
                    }
                    Err(e) => {
                        this.installer = InstallerState::Idle;
                        this.error = Some(installer_error(&e));
                        cx.notify();
                    }
                }
            })
            .ok();
        })
        .detach();
    }

    /// Scan the prefix now for executables installed since `snapshot`.
    fn scan_prefix_after_installer(
        &mut self,
        run_info: InstallerRun,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let title = text(&self.title, cx);
        let scan_info = run_info.clone();
        let scan = backend(cx).spawn_blocking(move || {
            detect_installed_executables(
                &scan_info.prefix,
                &title,
                &scan_info.installer,
                scan_info.extra_dir.as_deref(),
                &scan_info.snapshot,
            )
        });
        let task = irori_ui::run(cx, async move { Ok(scan.await?) });
        cx.spawn_in(window, async move |this, cx| {
            let outcome = task.await.unwrap_or(DetectionOutcome::None);
            this.update_in(cx, |this, window, cx| {
                this.apply_detection(run_info, outcome, window, cx);
            })
            .ok();
        })
        .detach();
    }

    /// Apply the outcome of a post-installer scan to the form.
    pub(crate) fn apply_detection(
        &mut self,
        run: InstallerRun,
        outcome: DetectionOutcome,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match outcome {
            DetectionOutcome::Picked(picked) => {
                self.apply_candidate(&picked, window, cx);
                self.installer = InstallerState::Picked {
                    run,
                    picked,
                    alternatives: Vec::new(),
                };
            }
            DetectionOutcome::Ambiguous(candidates) => {
                self.installer = InstallerState::Choose { run, candidates };
            }
            DetectionOutcome::None => {
                self.installer = InstallerState::Manual { run };
            }
        }
        cx.notify();
    }

    /// Fill the form from a detected or user-chosen installed executable.
    fn apply_candidate(
        &mut self,
        candidate: &InstalledCandidate,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let exe = candidate.executable.to_string_lossy().into_owned();
        self.executable
            .update(cx, |s, cx| s.set_value(exe, window, cx));
        if text(&self.arguments, cx).is_empty() && !candidate.args.is_empty() {
            let args = candidate.args.clone();
            self.arguments
                .update(cx, |s, cx| s.set_value(args, window, cx));
        }
        if text(&self.install_folder, cx).is_empty()
            && let Some(folder) = &candidate.install_folder
        {
            let folder = folder.to_string_lossy().into_owned();
            self.install_folder
                .update(cx, |s, cx| s.set_value(folder, window, cx));
        }
        self.error = None;
    }

    /// Choose candidate `ix` from [`InstallerState::Choose`].
    pub(crate) fn choose_installer_candidate(
        &mut self,
        ix: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let InstallerState::Choose { run, candidates } = &self.installer else {
            return;
        };
        let Some(picked) = candidates.get(ix).cloned() else {
            return;
        };
        let (run, alternatives) = (run.clone(), candidates.clone());
        self.apply_candidate(&picked, window, cx);
        self.installer = InstallerState::Picked {
            run,
            picked,
            alternatives,
        };
        cx.notify();
    }

    /// Re-open the candidate list when more than one executable was found.
    fn reopen_candidate_chooser(&mut self, cx: &mut Context<Self>) {
        if let InstallerState::Picked {
            run, alternatives, ..
        } = &self.installer
            && alternatives.len() > 1
        {
            self.installer = InstallerState::Choose {
                run: run.clone(),
                candidates: alternatives.clone(),
            };
            cx.notify();
        }
    }

    /// Stop waiting for the installer process and scan immediately.
    fn scan_installer_now(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let run = match &self.installer {
            InstallerState::Running { run, cancel } => {
                cancel.cancel();
                Some(run.clone())
            }
            InstallerState::Manual { run } | InstallerState::Choose { run, .. } => {
                Some(run.clone())
            }
            _ => None,
        };
        if let Some(run) = run {
            self.scan_prefix_after_installer(run, window, cx);
        }
    }

    /// Stop waiting for the installer process and fall back to manual Browse….
    fn stop_waiting_installer(&mut self, cx: &mut Context<Self>) {
        if let InstallerState::Running { run, cancel } = &self.installer {
            cancel.cancel();
            self.installer = InstallerState::Manual { run: run.clone() };
            cx.notify();
        }
    }

    pub(crate) fn collect(&self, cx: &App) -> SideloadApp {
        let path = |input: &Entity<InputState>| optional_text(input, cx).map(PathBuf::from);
        SideloadApp {
            title: text(&self.title, cx),
            executable: path(&self.executable),
            platform: self.platform,
            browser_url: optional_text(&self.url, cx),
            custom_user_agent: optional_text(&self.user_agent, cx),
            launch_fullscreen: self.fullscreen,
            cover_art: optional_text(&self.cover, cx),
            background_art: optional_text(&self.background, cx),
            install_folder: path(&self.install_folder),
            launcher_args: text(&self.arguments, cx),
            description: Some(self.description.read(cx).value().to_string()),
            developer: optional_text(&self.developer, cx),
            release_date: optional_text(&self.release_date, cx),
            genres: text(&self.genres, cx)
                .split(',')
                .map(str::to_string)
                .collect(),
            launch_link: self.launch_link.clone(),
            command: matches!(
                self.platform,
                SideloadPlatform::Linux | SideloadPlatform::Mac
            )
            .then(|| optional_text(&self.command, cx))
            .flatten(),
        }
    }

    fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.saving {
            return;
        }
        let app = self.collect(cx);
        if let Err(e) = app.clone().normalized() {
            self.error = Some(format!("{e:#}").into());
            cx.notify();
            return;
        }
        let Some(store) = crate::sideload_store(cx) else {
            self.error = Some(t("library.sideload.error.unavailable"));
            cx.notify();
            return;
        };
        let title = app.title.trim().to_string();
        let mode = save_mode(self.editing.as_deref(), self.provisional.as_deref());
        let b = backend(cx);
        let settings = b.settings.app();
        let modes = super::chosen_modes(&self.known_modes, &self.modes);
        let setup = self.wine_setup(cx);
        let replaces = self.replaces.clone();
        self.saving = true;
        self.error = None;
        cx.notify();

        let task = irori_ui::run(cx, async move {
            let mut app = app;
            if matches!(mode, SaveMode::Add | SaveMode::Finish(_))
                && app.cover_art.is_none()
                && !cfg!(test)
                && let Some((cover, hero)) =
                    irori_extras::wiki::steam_art_for_title(&app.title).await
            {
                app.cover_art = Some(cover);
                if app.background_art.is_none() {
                    app.background_art = Some(hero);
                }
            }
            let (id, added) = match mode {
                SaveMode::Edit(name) => {
                    store.edit_app(&name, app).await?;
                    (GameId::new(Runner::Sideload, name.as_str()), false)
                }
                SaveMode::Finish(name) => {
                    store.edit_app(&name, app).await?;
                    (GameId::new(Runner::Sideload, name.as_str()), true)
                }
                SaveMode::Add => (store.add_app(app).await?, true),
            };
            if let Some(setup) = &setup
                && let Some(game) = store.game(&id.app_name)
            {
                setup.apply(&b, &id, &game);
            }
            let mut own = b
                .userdata
                .snapshot()
                .overrides
                .get(&id.key())
                .cloned()
                .unwrap_or_default();
            if own.game_modes != modes {
                own.game_modes = modes;
                b.userdata.set_overrides(&id, own);
            }
            if !added {
                return Ok(false);
            }
            if let Some(rom) = &replaces {
                b.userdata.carry_over(rom, &id);
                // A ROM leaves the library by being hidden there; its file stays.
                if let Err(e) = b.uninstall(rom, Default::default()).await {
                    tracing::warn!("taking {rom} out of the ROM library: {e:#}");
                }
            }
            let targets = ShortcutTargets::from_settings(&settings);
            if (targets.desktop || targets.app_menu || targets.steam)
                && let Some(game) = store.game(&id.app_name)
                && let Err(e) = shortcuts::add(&game, &settings, targets).await
            {
                tracing::warn!("adding shortcuts for {id}: {e:#}");
            }
            Ok(true)
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            this.update_in(cx, |this, window, cx| {
                this.saving = false;
                match result {
                    Ok(added) => {
                        this.saved = true;
                        window.close_dialog(cx);
                        let message = if added {
                            t_args("library.sideload.notify.added", &[("title", &title)])
                        } else {
                            t_args("library.sideload.notify.saved", &[("title", &title)])
                        };
                        window.push_notification(Notification::success(message), cx);
                    }
                    Err(e) => this.error = Some(format!("{e:#}").into()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn browse(
        &mut self,
        target: fn(&Self) -> &Entity<InputState>,
        directories: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let picked = pick_path(directories, t("library.sideload.picker.select"), cx);
        cx.spawn_in(window, async move |this, cx| {
            let Some(path) = picked.await else { return };
            this.update_in(cx, |this, window, cx| {
                let value = path.to_string_lossy().into_owned();
                let input = target(this).clone();
                input.update(cx, |s, cx| s.set_value(value, window, cx));
                // Suggest a title from the executable when none was entered.
                if std::ptr::eq(target(this), &this.executable) {
                    if text(&this.title, cx).is_empty()
                        && let Some(title) = title_from_executable(&path)
                    {
                        this.title
                            .update(cx, |s, cx| s.set_value(title, window, cx));
                    }
                    if this.editing.is_none()
                        && let Some(wine) = &this.wine
                        && wine.read(cx).setup(cx).prefix.is_empty()
                        && let Some(detected) = irori_runtime::enclosing_wine_prefix(&path)
                    {
                        wine.update(cx, |w, cx| w.set_prefix(&detected, window, cx));
                    }
                    if matches!(
                        this.installer,
                        InstallerState::Choose { .. } | InstallerState::Manual { .. }
                    ) {
                        this.installer = InstallerState::Idle;
                    }
                }
                this.error = None;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn path_row(
        &self,
        id: &'static str,
        target: fn(&Self) -> &Entity<InputState>,
        directories: bool,
        cx: &mut Context<Self>,
    ) -> gpui_kit::Div {
        h_flex()
            .w_full()
            .gap_2()
            .child(div().flex_1().min_w_0().child(Input::new(target(self))))
            .child(
                Button::new(id)
                    .outline()
                    .icon(IconName::FolderOpen)
                    .label(t("library.common.browse"))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.browse(target, directories, window, cx)
                    })),
            )
    }

    /// An art field: path or URL, plus "Find online…".
    fn art_row(
        &self,
        id: &'static str,
        target: fn(&Self) -> &Entity<InputState>,
        kind: ArtKind,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let (find_id, flash_name) = if kind == ArtKind::Background {
            ("find-background", "find-background-flash")
        } else {
            ("find-cover", "find-cover-flash")
        };
        let (base_border, accent) = (cx.theme().input, cx.theme().ring);
        let button = Button::new(find_id)
            .debug_selector(move || find_id.into())
            .outline()
            .icon(IconName::Globe)
            .label(t("library.common.find_online"))
            .on_click(cx.listener(move |this, _, window, cx| this.find_art(kind, window, cx)));
        let find_btn = match self.find_online_flash {
            Some(seq) => button
                .with_animation(
                    ElementId::NamedInteger(flash_name.into(), seq),
                    Animation::new(FIND_ONLINE_FLASH_DURATION),
                    move |btn, delta| {
                        let s = double_flash_strength(delta);
                        if s <= 0.01 {
                            return btn;
                        }
                        btn.border_color(base_border.blend(accent.opacity(s)))
                            .shadow(vec![BoxShadow {
                                color: accent.opacity(0.45 * s),
                                offset: point(px(0.), px(0.)),
                                blur_radius: px(2. * s),
                                spread_radius: px(1.5 * s),
                                inset: false,
                            }])
                    },
                )
                .into_any_element(),
            None => button.into_any_element(),
        };
        self.path_row(id, target, false, cx).child(find_btn)
    }
}

impl SideloadDialog {
    /// The system a picked file is a game of, when it is a ROM rather than
    /// a program (DOS programs count as programs here).
    fn rom_system(&self, cx: &App) -> Option<&'static irori_engines::System> {
        let path = PathBuf::from(text(&self.executable, cx));
        let ext = path.extension()?.to_str()?.to_ascii_lowercase();
        if matches!(
            ext.as_str(),
            "exe"
                | "com"
                | "bat"
                | "cmd"
                | "ps1"
                | "vbs"
                | "sh"
                | "appimage"
                | "msi"
                | "lnk"
                | "desktop"
                | "app"
        ) {
            return None;
        }
        match irori_engines::systems::by_extension(&ext).as_slice() {
            [one] => Some(one),
            _ => None,
        }
    }

    /// Put the picked ROM in the ROM library and open its page.
    fn add_as_rom(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(store) = crate::emulation_store(cx) else {
            return;
        };
        let path = PathBuf::from(text(&self.executable, cx));
        let title = optional_text(&self.title, cx);
        self.saving = true;
        cx.notify();
        let task = irori_ui::run(cx, async move { store.add_game(&path, None, title).await });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            this.update_in(cx, |this, window, cx| {
                this.saving = false;
                match result {
                    Ok(id) => {
                        this.discard_provisional(cx);
                        window.close_dialog(cx);
                        navigate(Route::Game(id), cx);
                    }
                    Err(e) => this.error = Some(format!("{e:#}").into()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}

impl SideloadDialog {
    /// The hints above the form: a console game's file, or store games.
    fn render_hints(
        &self,
        rom_system: Option<&irori_engines::System>,
        cx: &mut Context<Self>,
    ) -> Vec<gpui_kit::AnyElement> {
        let mut out = Vec::new();
        if let Some(system) = rom_system {
            out.push(
                Alert::info(
                    "sideload-rom-hint",
                    t_args(
                        "library.sideload.rom_hint.body",
                        &[("system", &system.name)],
                    ),
                )
                .title(t("library.sideload.rom_hint.title"))
                .into_any_element(),
            );
            out.push(
                h_flex()
                    .justify_end()
                    .child(
                        Button::new("sideload-add-rom")
                            .primary()
                            .small()
                            .icon(IconName::Joystick)
                            .label(t("library.sideload.rom_hint.add"))
                            .loading(self.saving)
                            .on_click(
                                cx.listener(|this, _, window, cx| this.add_as_rom(window, cx)),
                            ),
                    )
                    .into_any_element(),
            );
        } else if self.editing.is_none() {
            let theme = cx.theme();
            let muted = theme.muted_foreground;
            let active_fg = active_accent(cx);
            let shortcuts = h_flex()
                .items_center()
                .flex_wrap()
                .gap_1p5()
                .min_w_0()
                .child(
                    Button::new("sideload-folder")
                        .xsmall()
                        .outline()
                        .icon(IconName::FolderPlus)
                        .label(t("library.sideload.scan_folder"))
                        .tooltip(t("library.folder.open_tooltip"))
                        .on_click(|_, window, cx| {
                            window.close_dialog(cx);
                            super::folder::open_add_folder_dialog(window, cx);
                        }),
                )
                .when(cfg!(target_os = "linux"), |el| {
                    el.child(
                        Button::new("sideload-lutris")
                            .xsmall()
                            .outline()
                            .icon(IconName::ScrollText)
                            .label(t("library.sideload.lutris_short"))
                            .tooltip(t("library.sideload.lutris_script_tooltip"))
                            .on_click(|_, window, cx| {
                                window.close_dialog(cx);
                                super::lutris::open_lutris_installer(None, window, cx);
                            }),
                    )
                });
            out.push(
                h_flex()
                    .id("sideload-quick-bar")
                    .debug_selector(|| "sideload-quick-bar".into())
                    .w_full()
                    .items_center()
                    .justify_between()
                    .flex_wrap()
                    .gap_2()
                    .px_2p5()
                    .py_1p5()
                    .rounded(theme.radius)
                    .bg(theme.secondary.opacity(0.35))
                    .border_1()
                    .border_color(theme.border.opacity(0.7))
                    .child(shortcuts)
                    .child(
                        h_flex()
                            .items_center()
                            .gap_1()
                            .flex_shrink_0()
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(muted)
                                    .child(t("library.sideload.store_game_prompt")),
                            )
                            .child(
                                Button::new("sideload-accounts")
                                    .xsmall()
                                    .ghost()
                                    .text_color(active_fg)
                                    .label(t("library.sideload.accounts_link"))
                                    .child(Icon::new(IconName::ArrowRight).xsmall())
                                    .tooltip(t("library.sideload.store_hint.body"))
                                    .on_click(|_, window, cx| {
                                        window.close_dialog(cx);
                                        navigate(Route::Accounts, cx);
                                    }),
                            ),
                    )
                    .into_any_element(),
            );
        }
        out
    }

    /// Live thumbnail previews for cover (2:3) and background (wide) art.
    fn render_art_previews(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        let cover_w = rems(5.5);
        let preview_h = rems(8.25);
        let cover_url = optional_text(&self.cover, cx);
        let cover_img = cover_url
            .as_deref()
            .and_then(|u| art_img(u, physical_width(cover_w, window), false, cx));
        let bg_url = optional_text(&self.background, cx);
        let bg_img = bg_url
            .as_deref()
            .and_then(|u| art_img(u, physical_width(rems(16.), window), false, cx));

        let theme = cx.theme();
        let radius = theme.radius;
        let muted_bg = theme.muted;
        let muted_fg = theme.muted_foreground;
        let border = theme.border;
        let card_bg = theme.secondary.opacity(0.3);

        let cover_box = div()
            .w(cover_w)
            .h(preview_h)
            .flex_shrink_0()
            .overflow_hidden()
            .rounded(radius)
            .bg(muted_bg)
            .border_1()
            .border_color(border.opacity(0.75))
            .map(|frame| match cover_img {
                Some(img) => {
                    frame.child(img.size_full().rounded(radius).object_fit(ObjectFit::Cover))
                }
                None => frame.child(
                    v_flex()
                        .size_full()
                        .items_center()
                        .justify_center()
                        .gap_1()
                        .text_color(muted_fg)
                        .child(Icon::new(IconName::Image).large())
                        .child(
                            div()
                                .text_xs()
                                .px_1()
                                .truncate()
                                .child(t("library.sideload.preview.cover")),
                        ),
                ),
            });

        let bg_box = div()
            .flex_1()
            .min_w_0()
            .h(preview_h)
            .overflow_hidden()
            .rounded(radius)
            .bg(muted_bg)
            .border_1()
            .border_color(border.opacity(0.75))
            .map(|frame| match bg_img {
                Some(img) => {
                    frame.child(img.size_full().rounded(radius).object_fit(ObjectFit::Cover))
                }
                None => frame.child(
                    v_flex()
                        .size_full()
                        .items_center()
                        .justify_center()
                        .gap_1()
                        .text_color(muted_fg)
                        .child(Icon::new(IconName::Image).large())
                        .child(
                            div()
                                .text_xs()
                                .px_1()
                                .truncate()
                                .child(t("library.sideload.preview.background")),
                        ),
                ),
            });

        h_flex()
            .w_full()
            .items_center()
            .gap_3()
            .p_2p5()
            .rounded(radius)
            .bg(card_bg)
            .border_1()
            .border_color(border.opacity(0.65))
            .child(cover_box)
            .child(bg_box)
            .into_any_element()
    }

    /// Accent-highlighted 2-tab switcher inside "More details".
    fn render_details_tabs(&self, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        let theme = cx.theme();
        let primary = theme.primary;
        let active_fg = active_accent(cx);
        let fg = theme.foreground;
        let muted = theme.muted_foreground;
        let rail_bg = theme.secondary.opacity(0.45);
        let hover_bg = theme.secondary.opacity(0.85);
        let border = theme.border;
        let radius = theme.radius;
        let active_tab = self.details_tab;

        let tabs = [
            (
                DetailsTab::Artwork,
                "sideload-tab-artwork",
                IconName::Image,
                t("library.sideload.tab.artwork"),
            ),
            (
                DetailsTab::Details,
                "sideload-tab-details",
                IconName::SlidersHorizontal,
                t("library.sideload.tab.details"),
            ),
        ];

        h_flex()
            .w_full()
            .p_0p5()
            .gap_1()
            .rounded(radius)
            .bg(rail_bg)
            .border_1()
            .border_color(border)
            .children(tabs.into_iter().map(|(tab, id, icon, label)| {
                let selected = active_tab == tab;
                h_flex()
                    .id(id)
                    .debug_selector(move || id.into())
                    .flex_1()
                    .min_w_0()
                    .justify_center()
                    .items_center()
                    .gap_1p5()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .border_1()
                    .cursor_pointer()
                    .keyboard_focusable(cx)
                    .text_xs()
                    .when(selected, |el| {
                        el.bg(primary.opacity(0.18))
                            .border_color(primary)
                            .text_color(active_fg)
                            .font_semibold()
                    })
                    .when(!selected, |el| {
                        el.border_color(transparent_black())
                            .text_color(muted)
                            .hover(move |s| s.bg(hover_bg).text_color(fg))
                    })
                    .child(Icon::new(icon).xsmall().flex_shrink_0())
                    .child(div().min_w_0().truncate().child(label))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if this.details_tab != tab {
                            this.details_tab = tab;
                            this.find_online_flash = None;
                            cx.notify();
                        }
                    }))
            }))
            .into_any_element()
    }

    /// The optional fields behind "More details", split cleanly into Artwork
    /// (with live thumbnail previews) and Details & Launch.
    fn render_details(&self, window: &mut Window, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        let browser = self.platform == SideloadPlatform::Browser;
        let native = matches!(
            self.platform,
            SideloadPlatform::Linux | SideloadPlatform::Mac
        );
        let body = match self.details_tab {
            DetailsTab::Artwork => v_flex()
                .debug_selector(|| "sideload-panel-artwork".into())
                .gap_3()
                .child(self.render_art_previews(window, cx))
                .child(
                    v_form()
                        .child(
                            field()
                                .label(t("library.common.cover_art"))
                                .description(t("library.sideload.art_hint"))
                                .child(self.art_row(
                                    "browse-cover",
                                    |this| &this.cover,
                                    ArtKind::Cover,
                                    cx,
                                )),
                        )
                        .child(field().label(t("library.sideload.field.background")).child(
                            self.art_row(
                                "browse-background",
                                |this| &this.background,
                                ArtKind::Background,
                                cx,
                            ),
                        )),
                )
                .into_any_element(),
            DetailsTab::Details => div()
                .debug_selector(|| "sideload-panel-details".into())
                .child(
                    v_form()
                        .when(!browser, |form| {
                            form.child(
                                field()
                                    .label(t("library.sideload.field.arguments"))
                                    .child(Input::new(&self.arguments)),
                            )
                            .when(native, |form| {
                                form.child(
                                    field()
                                        .label(t("library.sideload.field.command"))
                                        .description(t("library.sideload.command_hint"))
                                        .child(Input::new(&self.command)),
                                )
                            })
                            .child(
                                field()
                                    .label(t("library.sideload.field.install_folder"))
                                    .description(t("library.sideload.install_folder_hint"))
                                    .child(self.path_row(
                                        "browse-folder",
                                        |this| &this.install_folder,
                                        true,
                                        cx,
                                    )),
                            )
                        })
                        .when(browser, |form| {
                            form.child(
                                field()
                                    .label(t("library.sideload.field.user_agent"))
                                    .description(t("library.sideload.user_agent_hint"))
                                    .child(Input::new(&self.user_agent)),
                            )
                            .child(
                                field().label_indent(false).child(
                                    Switch::new("sideload-fullscreen")
                                        .label(t("library.sideload.fullscreen"))
                                        .checked(self.fullscreen)
                                        .on_change(cx.listener(|this, v: &bool, _, cx| {
                                            this.fullscreen = *v;
                                            cx.notify();
                                        })),
                                ),
                            )
                        })
                        .child(
                            field().label_indent(false).child(
                                h_flex()
                                    .w_full()
                                    .gap_3()
                                    .child(
                                        div().flex_1().min_w_0().child(
                                            v_form().child(
                                                field()
                                                    .label(t("library.sideload.field.developer"))
                                                    .child(Input::new(&self.developer)),
                                            ),
                                        ),
                                    )
                                    .child(
                                        div().flex_1().min_w_0().child(
                                            v_form().child(
                                                field()
                                                    .label(t("library.sideload.field.release_date"))
                                                    .child(Input::new(&self.release_date)),
                                            ),
                                        ),
                                    ),
                            ),
                        )
                        .child(
                            field()
                                .label(t("library.sideload.field.genres"))
                                .description(t("library.sideload.genres_hint"))
                                .child(Input::new(&self.genres)),
                        )
                        .child(field().label(irori_ui::game_mode::title()).child(
                            super::mode_boxes(&self.modes, |this: &mut Self| &mut this.modes, cx),
                        ))
                        .child(
                            field()
                                .label(t("library.sideload.field.description"))
                                .description(t("library.sideload.description_hint"))
                                .child(Textarea::new(&self.description)),
                        ),
                )
                .into_any_element(),
        };

        v_flex()
            .gap_3()
            .child(self.render_details_tabs(cx))
            .child(body)
            .into_any_element()
    }

    #[cfg(test)]
    pub(crate) fn toggle_more_for_test(&mut self, open: bool, cx: &mut Context<Self>) {
        self.more = open;
        cx.notify();
    }

    #[cfg(test)]
    pub(crate) fn set_details_tab_for_test(&mut self, tab: DetailsTab, cx: &mut Context<Self>) {
        self.details_tab = tab;
        cx.notify();
    }

    #[cfg(test)]
    pub(crate) fn find_online_flash_for_test(&self) -> Option<u64> {
        self.find_online_flash
    }

    #[cfg(test)]
    pub(crate) fn picker_for_test(&self) -> Option<Entity<ArtPicker>> {
        self.picker.clone()
    }

    /// Status banner or executable selector shown below the Executable row
    /// while or after running a Windows installer.
    fn render_installer_state(&self, cx: &mut Context<Self>) -> Option<gpui_kit::AnyElement> {
        let muted = cx.theme().muted_foreground;
        match &self.installer {
            InstallerState::Idle | InstallerState::Starting => None,
            InstallerState::Running { .. } => Some(
                v_flex()
                    .gap_2()
                    .child(
                        Alert::info(
                            "sideload-installer-running",
                            t("library.sideload.installer_running.body"),
                        )
                        .title(t("library.sideload.installer_running.title")),
                    )
                    .child(
                        h_flex()
                            .justify_end()
                            .gap_2()
                            .child(
                                Button::new("sideload-installer-stop")
                                    .small()
                                    .ghost()
                                    .label(t("library.sideload.installer_running.stop"))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.stop_waiting_installer(cx);
                                    })),
                            )
                            .child(
                                Button::new("sideload-installer-scan-now")
                                    .small()
                                    .outline()
                                    .label(t("library.sideload.installer_running.scan_now"))
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.scan_installer_now(window, cx);
                                    })),
                            ),
                    )
                    .into_any_element(),
            ),
            InstallerState::Picked {
                run,
                picked,
                alternatives,
            } => {
                let program = candidate_display(&picked.executable, &run.prefix);
                let has_more = alternatives.len() > 1;
                Some(
                    v_flex()
                        .gap_2()
                        .child(
                            Alert::info(
                                "sideload-installer-picked",
                                t_args(
                                    "library.sideload.installer_picked.body",
                                    &[("program", &program)],
                                ),
                            )
                            .title(t("library.sideload.installer_picked.title")),
                        )
                        .when(has_more, |d| {
                            d.child(
                                h_flex().justify_end().child(
                                    Button::new("sideload-installer-change")
                                        .small()
                                        .ghost()
                                        .label(t("library.sideload.installer_picked.change"))
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.reopen_candidate_chooser(cx);
                                        })),
                                ),
                            )
                        })
                        .into_any_element(),
                )
            }
            InstallerState::Choose { run, candidates } => {
                let rows = candidates.iter().enumerate().map(|(ix, c)| {
                    let name = c
                        .executable
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_else(|| c.executable.display().to_string());
                    let detail = candidate_display(&c.executable, &run.prefix);
                    ListItem::new(ElementId::Name(
                        format!("sideload-installer-candidate-{ix}").into(),
                    ))
                    .py_2()
                    .accessibility_label(name.clone())
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.choose_installer_candidate(ix, window, cx);
                    }))
                    .child(
                        v_flex()
                            .w_full()
                            .min_w_0()
                            .child(div().text_sm().font_medium().truncate().child(name))
                            .child(div().text_xs().text_color(muted).truncate().child(detail)),
                    )
                });
                Some(
                    v_flex()
                        .id("sideload-installer-choose")
                        .gap_2()
                        .child(
                            Alert::info(
                                "sideload-installer-choose-alert",
                                t("library.sideload.installer_choose.body"),
                            )
                            .title(t("library.sideload.installer_choose.title")),
                        )
                        .child(
                            v_flex()
                                .id("sideload-installer-candidates")
                                .max_h_48()
                                .overflow_y_scroll()
                                .gap_1()
                                .children(rows),
                        )
                        .into_any_element(),
                )
            }
            InstallerState::Manual { run } => {
                let prefix_str = run.prefix.to_string_lossy().into_owned();
                Some(
                    v_flex()
                        .gap_2()
                        .child(
                            Alert::info(
                                "sideload-installer-started",
                                t_args(
                                    "library.sideload.installer_started.body",
                                    &[("prefix", &prefix_str)],
                                ),
                            )
                            .title(t("library.sideload.installer_started.title")),
                        )
                        .child(
                            h_flex().justify_end().child(
                                Button::new("sideload-installer-scan-again")
                                    .small()
                                    .outline()
                                    .label(t("library.sideload.installer_scan_again"))
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.scan_installer_now(window, cx);
                                    })),
                            ),
                        )
                        .into_any_element(),
                )
            }
        }
    }
}

/// Human-friendly path of an installed executable, relative to the prefix's
/// `drive_c` when inside it.
fn candidate_display(executable: &Path, prefix: &Path) -> String {
    for base in [
        prefix.join("pfx/drive_c"),
        prefix.join("drive_c"),
        prefix.to_path_buf(),
    ] {
        if let Ok(rel) = executable.strip_prefix(&base) {
            return rel.display().to_string();
        }
    }
    executable.display().to_string()
}

impl Render for SideloadDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(picker) = &self.picker {
            return v_flex().id("sideload-art").child(picker.clone());
        }
        let rom_system = self.rom_system(cx).filter(|_| self.editing.is_none());
        let hints = self.render_hints(rom_system, cx);
        let browser = self.platform == SideloadPlatform::Browser;
        let windows = self.platform == SideloadPlatform::Windows;
        let installer_busy = self.installer.is_busy();
        let installer_panel = windows.then(|| self.render_installer_state(cx)).flatten();
        let platform = self.platform;
        let wine = self.wine.clone().filter(|_| windows);
        let more = self.more;
        let details = more.then(|| self.render_details(window, cx));
        let theme = cx.theme();
        let primary = theme.primary;
        let active_fg = active_accent(cx);
        let fg = theme.foreground;
        let muted = theme.muted_foreground;
        let rail_bg = theme.secondary.opacity(0.45);
        let hover_bg = theme.secondary.opacity(0.85);
        let border = theme.border;
        let radius = theme.radius;

        let platform_selector = h_flex()
            .id("sideload-platform")
            .p_0p5()
            .gap_1()
            .rounded(radius)
            .bg(rail_bg)
            .border_1()
            .border_color(border)
            .children(PLATFORMS.iter().enumerate().map(|(ix, &p)| {
                let selected = platform == p;
                h_flex()
                    .id(SharedString::from(format!("platform-{ix}")))
                    .flex_1()
                    .min_w_0()
                    .justify_center()
                    .items_center()
                    .gap_1p5()
                    .px_2p5()
                    .py_1()
                    .rounded_md()
                    .border_1()
                    .cursor_pointer()
                    .keyboard_focusable(cx)
                    .text_xs()
                    .when(selected, |el| {
                        el.bg(primary.opacity(0.18))
                            .border_color(primary)
                            .text_color(active_fg)
                            .font_semibold()
                    })
                    .when(!selected, |el| {
                        el.border_color(transparent_black())
                            .text_color(muted)
                            .hover(move |s| s.bg(hover_bg).text_color(fg))
                    })
                    .child(Icon::new(platform_icon(p)).xsmall().flex_shrink_0())
                    .child(div().min_w_0().truncate().child(platform_label(p)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if this.platform != p {
                            this.platform = p;
                            this.error = None;
                            cx.notify();
                        }
                    }))
            }));

        let title_empty = text(&self.title, cx).is_empty();
        let fetching_art = self.fetching_art;

        v_flex()
            .id("sideload-form")
            .gap_4()
            .children(hints)
            .child(
                v_form()
                    .child(
                        field().label(t("library.sideload.field.title")).child(
                            h_flex()
                                .w_full()
                                .gap_2()
                                .child(div().flex_1().min_w_0().child(Input::new(&self.title)))
                                .child(
                                    Button::new("sideload-fetch-art")
                                        .debug_selector(|| "sideload-fetch-art".into())
                                        .outline()
                                        .icon(IconName::ImageDown)
                                        .label(t("library.sideload.fetch_art"))
                                        .tooltip(t("library.sideload.fetch_art_tooltip"))
                                        .loading(fetching_art)
                                        .disabled(fetching_art || title_empty)
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.fetch_art(window, cx)
                                        })),
                                ),
                        ),
                    )
                    .child(
                        field()
                            .label(t("library.sideload.field.platform"))
                            .when(browser, |f| f.description(t("library.sideload.web_hint")))
                            .child(platform_selector),
                    )
                    .when(!browser, |form| {
                        form.child(
                            field()
                                .label(t("library.sideload.field.executable"))
                                .description(match platform {
                                    SideloadPlatform::Windows => {
                                        t("library.sideload.executable_hint.windows")
                                    }
                                    _ => t("library.sideload.executable_hint.native"),
                                })
                                .child(
                                    self.path_row(
                                        "browse-executable",
                                        |this| &this.executable,
                                        false,
                                        cx,
                                    )
                                    .flex_wrap()
                                    .when(windows, |row| {
                                        row.child(
                                            Button::new("run-installer")
                                                .outline()
                                                .icon(IconName::PackageOpen)
                                                .label(t("library.sideload.run_installer"))
                                                .tooltip(t(
                                                    "library.sideload.run_installer_tooltip",
                                                ))
                                                .loading(installer_busy)
                                                .disabled(installer_busy)
                                                .on_click(cx.listener(|this, _, window, cx| {
                                                    this.run_installer(window, cx)
                                                })),
                                        )
                                    }),
                                ),
                        )
                        .when_some(installer_panel, |form, panel| {
                            form.child(field().label_indent(false).child(panel))
                        })
                    })
                    .when_some(wine, |form, wine| {
                        form.child(field().label_indent(false).child(wine))
                    })
                    .when(browser, |form| {
                        form.child(
                            field()
                                .label(t("library.sideload.field.address"))
                                .child(Input::new(&self.url)),
                        )
                    }),
            )
            .child(
                h_flex().child(
                    Button::new("sideload-more")
                        .ghost()
                        .small()
                        .icon(if more {
                            IconName::ChevronDown
                        } else {
                            IconName::ChevronRight
                        })
                        .label(if more {
                            t("library.sideload.fewer_details")
                        } else {
                            t("library.sideload.more_details")
                        })
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.more = !this.more;
                            this.find_online_flash = None;
                            cx.notify();
                        })),
                ),
            )
            .children(details)
    }
}

/// The dialog's footer: the error, Cancel and Add (or Save), outside the
/// scrolling form so they never move out of sight.
pub struct SideloadFooter {
    dialog: Entity<SideloadDialog>,
    _observe: Subscription,
}

impl SideloadFooter {
    fn new(dialog: Entity<SideloadDialog>, cx: &mut Context<Self>) -> Self {
        let observe = cx.observe(&dialog, |_, _, cx| cx.notify());
        Self {
            dialog,
            _observe: observe,
        }
    }
}

impl Render for SideloadFooter {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let state = self.dialog.read(cx);
        if state.picker.is_some() {
            return v_flex().id("sideload-footer");
        }
        let adding = state.editing.is_none();
        let saving = state.saving;
        let error = state.error.clone();
        let dialog = self.dialog.clone();
        v_flex()
            .id("sideload-footer")
            .w_full()
            .gap_3()
            .when_some(error, |this, e| {
                this.child(Alert::error("sideload-error", e))
            })
            .child(
                h_flex()
                    .w_full()
                    .justify_end()
                    .flex_wrap()
                    .gap_2()
                    .child(
                        Button::new("sideload-cancel")
                            .ghost()
                            .label(t("library.common.cancel"))
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                    )
                    .child(
                        Button::new("sideload-save")
                            .primary()
                            .label(if adding {
                                t("library.sideload.add")
                            } else {
                                t("library.common.save")
                            })
                            .loading(saving)
                            .disabled(saving)
                            .on_click(move |_, window, cx| {
                                dialog.update(cx, |d, cx| d.save(window, cx));
                            }),
                    ),
            )
    }
}

/// Where a new app called `title` gets its prefix by default.
fn new_app_prefix(title: &str, cx: &App) -> PathBuf {
    let game = irori_core::GameInfo {
        title: title.to_string(),
        ..Default::default()
    };
    irori_runtime::default_prefix(&backend(cx).settings.app(), &game)
}

/// Error line for an installer that could not be started.
fn installer_error(error: &anyhow::Error) -> SharedString {
    t_args(
        "library.sideload.error.installer",
        &[("error", &format!("{error:#}"))],
    )
}

fn open(editing: Option<String>, prefill: Option<SideloadApp>, window: &mut Window, cx: &mut App) {
    let view = cx.new(|cx| {
        cx.on_release(|this: &mut SideloadDialog, cx| this.discard_provisional(cx))
            .detach();
        SideloadDialog::with_app(editing, prefill, window, cx)
    });
    show(view, window, cx);
}

fn show(view: Entity<SideloadDialog>, window: &mut Window, cx: &mut App) {
    let footer = cx.new(|cx| SideloadFooter::new(view.clone(), cx));
    let width = dialog_width(window, 40.);
    let focus = Cell::new(true);
    window.open_dialog(cx, move |dialog, window, cx| {
        if focus.replace(false) {
            let input = view.read(cx).title.clone();
            input.focus_handle(cx).focus(window, cx);
        }
        let title = view.read(cx).dialog_title(cx);
        dialog
            .title(title)
            .w(width)
            .child(view.clone())
            .footer(footer.clone())
    });
}

/// Open the "Add game" dialog for an app that is not from a store.
pub fn open_add_game_dialog(window: &mut Window, cx: &mut App) {
    open(None, None, window, cx);
}

/// Open the "Add game" dialog filled in for a file dropped on the library:
/// the game it starts when it is a program, desktop entry, shortcut or app
/// bundle, else just the file (a ROM, which the dialog offers to the ROM
/// library).
pub(crate) fn open_add_dropped_file_dialog(file: PathBuf, window: &mut Window, cx: &mut App) {
    let found = irori_ui::backend(cx).spawn_blocking({
        let file = file.clone();
        move || irori_sideload::scan::game_of_file(&file)
    });
    let task = irori_ui::run(cx, async move { Ok(found.await?) });
    window
        .spawn(cx, async move |cx| {
            let found = task.await.ok().flatten();
            let _ = cx.update(|window, cx| {
                open(None, Some(dropped_app(&file, found)), window, cx);
            });
        })
        .detach();
}

/// The new app a dropped file fills the form with.
fn dropped_app(file: &Path, found: Option<irori_sideload::scan::FoundGame>) -> SideloadApp {
    match found {
        Some(game) => SideloadApp {
            cover_art: game
                .icon
                .as_ref()
                .filter(|p| super::is_picture(p))
                .map(|p| p.display().to_string()),
            title: game.title,
            executable: Some(game.executable),
            platform: game.platform,
            install_folder: game.install_folder,
            launcher_args: game.args,
            ..Default::default()
        },
        None => SideloadApp {
            title: title_from_executable(file).unwrap_or_default(),
            executable: Some(file.to_path_buf()),
            ..Default::default()
        },
    }
}

/// Open the "Add game" dialog filled in from a ROM library entry, to play
/// its file as a PC game instead (a Windows or native program the scan took
/// for a ROM). Saving moves what the user keeps about the entry (favourite,
/// categories, playtime, title and art, status, rating, notes) to the new
/// app and takes the entry out of the ROM library; the file stays.
pub fn open_play_as_pc_game_dialog(id: &GameId, window: &mut Window, cx: &mut App) {
    let Some(game) = backend(cx).game(id) else {
        return;
    };
    let prefill = rom_as_app(&game);
    let rom = id.clone();
    let view = cx.new(|cx| {
        cx.on_release(|this: &mut SideloadDialog, cx| this.discard_provisional(cx))
            .detach();
        let mut dialog = SideloadDialog::with_app(None, Some(prefill), window, cx);
        dialog.replaces = Some(rom);
        dialog
    });
    show(view, window, cx);
}

/// The app a ROM entry becomes: its file as the program, Windows for a
/// Windows program and this system's own otherwise, its title and art.
fn rom_as_app(game: &GameInfo) -> SideloadApp {
    let file = game
        .install
        .as_ref()
        .map(|i| i.executable.trim())
        .filter(|e| !e.is_empty())
        .map(PathBuf::from)
        .or_else(|| game.install_path());
    let windows = file.as_deref().is_some_and(|f| {
        f.extension().is_some_and(|e| {
            ["exe", "bat", "cmd", "msi", "lnk"]
                .iter()
                .any(|w| e.eq_ignore_ascii_case(w))
        })
    });
    let platform = match irori_core::HostPlatform::current() {
        _ if windows => SideloadPlatform::Windows,
        irori_core::HostPlatform::Mac => SideloadPlatform::Mac,
        irori_core::HostPlatform::Windows => SideloadPlatform::Windows,
        irori_core::HostPlatform::Linux => SideloadPlatform::Linux,
    };
    SideloadApp {
        title: game.title.clone(),
        executable: file,
        platform,
        cover_art: game.art_cover.clone(),
        background_art: game.art_background.clone(),
        ..Default::default()
    }
}

/// Open the edit dialog of a manually added app.
pub(crate) fn open_edit_app_dialog(app_name: String, window: &mut Window, cx: &mut App) {
    open(Some(app_name), None, window, cx);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_modes() {
        assert_eq!(save_mode(None, None), SaveMode::Add);
        assert_eq!(save_mode(None, Some("p")), SaveMode::Finish("p".into()));
        assert_eq!(save_mode(Some("e"), Some("p")), SaveMode::Edit("e".into()));
    }

    #[test]
    fn a_dropped_file_fills_in_the_form() {
        use irori_sideload::scan::{FoundGame, FoundKind};
        let rom = dropped_app(Path::new("/roms/Super_Mario.sfc"), None);
        assert_eq!(rom.title, "Super Mario");
        assert_eq!(
            rom.executable.as_deref(),
            Some(Path::new("/roms/Super_Mario.sfc"))
        );
        let entry = dropped_app(
            Path::new("/apps/tux.desktop"),
            Some(FoundGame {
                title: "SuperTux".into(),
                executable: "/usr/games/supertux2".into(),
                platform: SideloadPlatform::Linux,
                args: "--fullscreen".into(),
                install_folder: None,
                icon: Some("/icons/tux.png".into()),
                kind: FoundKind::DesktopEntry,
                others: Vec::new(),
            }),
        );
        assert_eq!(entry.title, "SuperTux");
        assert_eq!(
            entry.executable.as_deref(),
            Some(Path::new("/usr/games/supertux2"))
        );
        assert_eq!(entry.platform, SideloadPlatform::Linux);
        assert_eq!(entry.launcher_args, "--fullscreen");
        assert_eq!(entry.cover_art.as_deref(), Some("/icons/tux.png"));
    }

    #[test]
    fn a_rom_entry_becomes_an_added_program() {
        let rom = |file: &str| GameInfo {
            runner: Some(Runner::Emulation),
            app_name: "abc".into(),
            title: "Doom".into(),
            art_cover: Some("https://example.com/doom.png".into()),
            is_installed: true,
            install: Some(irori_core::InstalledInfo {
                install_path: "/roms/dos".into(),
                executable: file.into(),
                ..Default::default()
            }),
            ..Default::default()
        };
        let app = rom_as_app(&rom("/roms/dos/DOOM.EXE"));
        assert_eq!(app.title, "Doom");
        assert_eq!(app.executable, Some(PathBuf::from("/roms/dos/DOOM.EXE")));
        assert_eq!(app.platform, SideloadPlatform::Windows);
        assert_eq!(
            app.cover_art.as_deref(),
            Some("https://example.com/doom.png")
        );
        let native = rom_as_app(&rom("/roms/linux/game.x86_64"));
        assert_eq!(
            native.platform,
            match irori_core::HostPlatform::current() {
                irori_core::HostPlatform::Mac => SideloadPlatform::Mac,
                irori_core::HostPlatform::Windows => SideloadPlatform::Windows,
                irori_core::HostPlatform::Linux => SideloadPlatform::Linux,
            }
        );
    }

    #[test]
    fn suggests_titles() {
        assert_eq!(
            title_from_executable(Path::new("/games/Hollow_Knight.exe")).as_deref(),
            Some("Hollow Knight")
        );
        assert_eq!(
            title_from_executable(Path::new("/opt/celeste/Celeste.bin.x86_64")).as_deref(),
            Some("Celeste bin")
        );
        assert_eq!(
            title_from_executable(Path::new(
                "/home/edward/Downloads/Void Reaver [FitGirl Repack]/setup.exe"
            ))
            .as_deref(),
            Some("Void Reaver")
        );
        assert_eq!(title_from_executable(Path::new("/")), None);
    }

    #[test]
    fn double_flash_curve_peaks_twice_and_settles_at_zero() {
        assert!(double_flash_strength(0.0) < 0.01);
        assert!((double_flash_strength(0.25) - 1.0).abs() < 1e-5);
        assert!(double_flash_strength(0.5) < 0.01);
        assert!((double_flash_strength(0.75) - 1.0).abs() < 1e-5);
        assert!(double_flash_strength(1.0) < 0.01);
    }
}
