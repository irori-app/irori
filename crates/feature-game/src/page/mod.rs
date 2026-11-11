//! The game page (`Route::Game`), over the game's art blurred across the
//! whole page: the title and a details line, then three columns: the cover
//! with the primary action, tools, rating and quick facts; the screenshots,
//! tabs, play history and related games; and the details table. Narrower
//! pages fold the details under the play history, then stack everything. A
//! slim bar keeps the primary action in reach once it scrolls away.
//!
//! The library shows the same page without leaving the library
//! ([`GamePage::embedded`]): whole beside its Focus view's covers, or as a
//! summary in the grid's drawer (`drawer`).

mod achievements;
mod age_rating;
mod compat;
mod details;
mod drawer;
mod extras;
mod game_modes;
mod hero;
mod info;
mod journal;
mod menu;
mod overlay;
mod related;
mod sandbox;
mod screenshots;
mod timeline;

use std::cell::Cell;
use std::collections::HashSet;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::Button;
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::select::SelectEvent;
use gpui_kit::component::{ActiveTheme as _, StyledExt as _, WindowExt as _, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, AppContext as _, Context, Div, Entity, EventEmitter, FocusHandle, Focusable, Hsla,
    InteractiveElement as _, IntoElement, ParentElement, Pixels, Render, ScrollHandle,
    SharedString, StatefulInteractiveElement as _, Styled, Subscription, Window, canvas, div, px,
};
use irori_core::{Achievement, ExtraInfo, GameId, GameInfo, LaunchOption};
use irori_services::LaunchParams;
use irori_ui::art::{LruImageCache, art_store};
use irori_ui::components::{ArtBackdrop, BackdropInk, EmptyState};
use irori_ui::hooks::GamePanel;
use irori_ui::i18n::t;
use irori_ui::{Route, backend, models, navigate};

use crate::choice::{Choice, ChoiceSelect, choice_select, set_choices};
use crate::hooks::{self, CompatibilityInfo};
use crate::load::Load;
use crate::state::{ActionInputs, PrimaryAction};
use crate::{
    OpenGameLog, OpenGameSettingsAction, RunPrimaryAction, ToggleFavourite, TroubleshootLaunch, ops,
};

pub use achievements::AchievementFilter;
pub use menu::game_menu;
use overlay::EosOverlayPanel;

/// Key context of the game page; bindings are registered by [`crate::init`].
pub const GAME_PAGE_CONTEXT: &str = "GamePage";

/// Events the shell may react to.
#[derive(Debug, Clone, PartialEq)]
pub enum GamePageEvent {
    /// The game's (possibly overridden) title became known or changed;
    /// use it for the window title or breadcrumb.
    TitleChanged(SharedString),
    /// "Edit title and art…" was chosen: open the metadata editor for the
    /// game (`irori_library::open_edit_game_dialog`).
    RequestEdit(GameId),
    /// "Categories…" was chosen: open category assignment for the page's
    /// game ([`GamePage::game_id`]) through `irori_library`'s categories dialog.
    RequestCategories,
    /// A game-mode chip was chosen: show the library's games of that mode
    /// (`irori_library::show_game_mode`).
    ShowGameMode(irori_core::GameMode),
}

/// A small uppercase label over the page ("SCREENSHOTS", "DETAILS"), in
/// `color`: the backdrop's or a panel's muted ink.
pub(crate) fn caption(text: impl Into<SharedString>, color: Hsla) -> Div {
    let text: SharedString = text.into();
    div()
        .text_xs()
        .font_bold()
        .text_color(color)
        .child(SharedString::from(text.to_uppercase()))
}

/// The heading of a section of the page ("Screenshots", "Details"), in
/// `color`: the backdrop's or a panel's text ink.
pub(crate) fn section_title(text: impl Into<SharedString>, color: Hsla) -> Div {
    div()
        .text_lg()
        .font_semibold()
        .text_color(color)
        .child(text.into())
}

/// Section of the info area.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InfoTab {
    About,
    Achievements,
    Compatibility,
    Requirements,
    Changelog,
    Extras,
    Mods,
}

/// Game page for one game. Owns the page's async data; live state (status,
/// queue, library entry) comes from the shared models.
pub struct GamePage {
    id: GameId,
    /// Where the library shows the page, when it is not on its own route.
    panel: Option<GamePanel>,
    focus: FocusHandle,
    game: Option<Arc<GameInfo>>,
    extra: Load<ExtraInfo>,
    achievements: Load<Arc<Vec<Achievement>>>,
    /// Offline installers and goodies, fetched when the Extras tab opens.
    extras: Load<Arc<Vec<irori_core::ExtraFile>>>,
    achievement_filter: AchievementFilter,
    achievement_view: achievements::RowsCache,
    /// Hidden achievements the user chose to see.
    achievement_reveal: achievements::Reveal,
    launch_options: Vec<LaunchOption>,
    launch_select: Entity<ChoiceSelect>,
    selected_option: Option<LaunchOption>,
    compat: Load<CompatibilityInfo>,
    /// EOS overlay controls (Epic Windows installs with a store handle).
    overlay: Option<Entity<EosOverlayPanel>>,
    /// The Mods tab, made when first opened.
    mods: Option<Entity<crate::mods::ModsPanel>>,
    files_available: bool,
    shortcuts_exist: Option<bool>,
    tab: InfoTab,
    launch_pending: bool,
    /// Commands in flight, keyed by command name (for loading states).
    pending: HashSet<&'static str>,
    image_cache: Entity<LruImageCache>,
    was_installed: bool,
    was_playing: bool,
    scroll: ScrollHandle,
    /// Where the primary action ends in the scrolled content, measured each
    /// frame; past it the sticky bar shows.
    header_height: Rc<Cell<Pixels>>,
    /// Width of the page, measured each frame; layouts that adapt to the
    /// window (header, screenshot grid) read it.
    page_width: Rc<Cell<Pixels>>,
    /// Whether the sticky bar showed at the last check.
    sticky: bool,
    /// The whole changelog is shown, not just its latest entries.
    changelog_expanded: bool,
    /// Index of the screenshot open in the viewer.
    viewer: Option<usize>,
    viewer_focus: FocusHandle,
    /// The viewer's filmstrip, scrolled to keep the open screenshot in view.
    viewer_strip: ScrollHandle,
    /// The play history.
    timeline: timeline::TimelineView,
    related: related::RelatedCache,
    /// Text colours over the backdrop, as of the last render.
    ink: BackdropInk,
    /// The game's sandbox setting, read when settings change.
    sandbox: irori_core::SandboxSettings,
    /// The sandbox card shows what the game can and can't reach.
    sandbox_open: bool,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<GamePageEvent> for GamePage {}

impl Focusable for GamePage {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl GamePage {
    /// Create the page for `id` and start loading store details,
    /// achievements, launch options and compatibility data.
    pub fn new(id: GameId, window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self::build(id, None, window, cx)
    }

    /// The page for `id` inside the library, shaped as `panel`. It leaves
    /// keyboard focus where it is, and in [`GamePanel::Focus`] draws no
    /// backdrop: the library draws one behind its covers and the page.
    pub fn embedded(
        id: GameId,
        panel: GamePanel,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::build(id, Some(panel), window, cx)
    }

    fn build(
        id: GameId,
        panel: Option<GamePanel>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let m = models(cx).clone();
        let game = m
            .library
            .read(cx)
            .snapshot
            .get(&id)
            .cloned()
            .or_else(|| backend(cx).game(&id));
        let launch_select = choice_select(Vec::new(), None, window, cx);
        let subscriptions = vec![
            cx.observe_in(&m.library, window, |this, _, window, cx| {
                this.on_library_changed(window, cx)
            }),
            cx.observe_in(&m.statuses, window, |this, _, window, cx| {
                this.on_status_changed(window, cx)
            }),
            cx.observe(&m.downloads, |_, _, cx| cx.notify()),
            cx.observe_in(&m.sessions, window, |this, _, window, cx| {
                if this.played_last(cx) {
                    this.load_timeline(window, cx);
                }
            }),
            cx.observe(&m.accounts, |_, _, cx| cx.notify()),
            cx.observe(&m.settings, |this, _, cx| {
                this.sandbox = sandbox::configured(&this.id, cx);
                cx.notify()
            }),
            cx.observe(&art_store(cx), |_, _, cx| cx.notify()),
            cx.subscribe_in(
                &launch_select,
                window,
                |this, _, e: &SelectEvent<Vec<Choice>>, _, cx| {
                    let SelectEvent::Confirm(Some(v)) = e else {
                        return;
                    };
                    this.choose_launch_option(v.as_ref(), cx);
                },
            ),
        ];
        let focus = cx.focus_handle();
        if panel.is_none() {
            window.focus(&focus, cx);
        }
        let was_installed = game.as_ref().is_some_and(|g| g.is_installed);
        let mut this = Self {
            id,
            panel,
            focus,
            game,
            extra: Load::Idle,
            achievements: Load::Idle,
            extras: Load::Idle,
            achievement_filter: AchievementFilter::All,
            achievement_view: achievements::RowsCache::default(),
            achievement_reveal: achievements::Reveal::default(),
            launch_options: Vec::new(),
            launch_select,
            selected_option: None,
            compat: Load::Idle,
            overlay: None,
            mods: None,
            files_available: true,
            shortcuts_exist: None,
            tab: InfoTab::About,
            launch_pending: false,
            pending: HashSet::new(),
            image_cache: LruImageCache::new(48, cx),
            was_installed,
            was_playing: false,
            scroll: ScrollHandle::new(),
            header_height: Rc::new(Cell::new(px(0.))),
            page_width: Rc::new(Cell::new(px(0.))),
            sticky: false,
            changelog_expanded: false,
            viewer: None,
            viewer_focus: cx.focus_handle(),
            viewer_strip: ScrollHandle::new(),
            timeline: timeline::TimelineView::default(),
            related: related::RelatedCache::default(),
            ink: BackdropInk::of(None, cx),
            sandbox: Default::default(),
            sandbox_open: false,
            _subscriptions: subscriptions,
        };
        this.sandbox = sandbox::configured(&this.id, cx);
        this.was_playing = models(cx).statuses.read(cx).is_playing(&this.id);
        this.load_all(window, cx);
        this
    }

    /// Width of the page in rem, or `None` before the first layout.
    pub(super) fn page_rem(&self, window: &Window) -> Option<f32> {
        let width = self.page_width.get();
        (width > px(0.)).then(|| width / window.rem_size())
    }

    /// Whether the header has scrolled far enough for the sticky bar.
    fn sticky_wanted(&self) -> bool {
        let header_height = self.header_height.get();
        header_height > px(0.) && -self.scroll.offset().y > header_height - px(48.)
    }

    /// The game shown.
    pub fn game_id(&self) -> &GameId {
        &self.id
    }

    /// Current title (override applied), if the game is known.
    pub fn title(&self) -> Option<SharedString> {
        self.game.as_ref().map(|g| g.title.clone().into())
    }

    /// The launch option Play uses.
    pub fn selected_launch_option(&self) -> Option<&LaunchOption> {
        self.selected_option.as_ref()
    }

    fn load_all(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(game) = self.game.clone() else {
            return;
        };
        cx.emit(GamePageEvent::TitleChanged(game.title.clone().into()));
        self.load_extra(window, cx);
        self.load_achievements(window, cx);
        self.load_compat(window, cx);
        self.load_install_state(window, cx);
        self.load_timeline(window, cx);
    }

    /// Data that depends on the game being installed.
    fn load_install_state(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(game) = self.game.clone() else {
            return;
        };
        self.overlay = overlay::applies(&game, cx).then(|| {
            let game = game.clone();
            cx.new(|cx| EosOverlayPanel::new(game, window, cx))
        });
        if !game.is_installed {
            self.launch_options.clear();
            self.selected_option = None;
            self.files_available = true;
            self.shortcuts_exist = None;
            return;
        }
        let b = backend(cx);
        let id = self.id.clone();
        let browser = game.browser_url.is_some();
        let task = irori_ui::run(cx, {
            let game = game.clone();
            async move {
                let store = b
                    .store(id.runner)
                    .ok_or_else(|| crate::i18n::store_unavailable(id.runner))?;
                let options = store.launch_options(&id.app_name).await.unwrap_or_default();
                let available = browser || store.is_available(&id.app_name).await;
                let shortcuts = irori_extras::shortcuts::exists(&game).await;
                Ok((options, available, shortcuts))
            }
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok((options, available, shortcuts)) = task.await else {
                return;
            };
            let _ = this.update_in(cx, |this, window, cx| {
                this.files_available = available;
                this.shortcuts_exist = Some(shortcuts);
                this.set_launch_options(options, window, cx);
                cx.notify();
            });
        })
        .detach();
    }

    fn set_launch_options(
        &mut self,
        options: Vec<LaunchOption>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let last = backend(cx).settings.game(&self.id).last_used_launch_option;
        self.selected_option = last
            .filter(|l| options.contains(l))
            .or_else(|| options.first().cloned());
        let items = options
            .iter()
            .enumerate()
            .map(|(ix, o)| Choice::new(ix.to_string(), o.label()))
            .collect();
        let selected = self
            .selected_option
            .as_ref()
            .and_then(|s| options.iter().position(|o| o == s))
            .map(|ix| ix.to_string());
        set_choices(&self.launch_select, items, selected.as_deref(), window, cx);
        self.launch_options = options;
    }

    fn choose_launch_option(&mut self, value: &str, cx: &mut Context<Self>) {
        let Some(option) = value
            .parse::<usize>()
            .ok()
            .and_then(|ix| self.launch_options.get(ix).cloned())
        else {
            return;
        };
        if let Ok(v) = serde_json::to_value(&option) {
            backend(cx)
                .settings
                .set_game_value(&self.id, "lastUsedLaunchOption", Some(v));
        }
        self.selected_option = Some(option);
        cx.notify();
    }

    pub(super) fn load_extra(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // An added game's page has only what the user wrote, unless IGDB
        // may fill it in.
        if self.id.runner == irori_core::Runner::Sideload && !backend(cx).settings.app().igdb {
            return;
        }
        self.extra = Load::Loading;
        let b = backend(cx);
        let id = self.id.clone();
        let task = irori_ui::run(cx, async move {
            if b.store(id.runner).is_none() {
                return Err(crate::i18n::store_unavailable(id.runner));
            }
            b.extra_info(&id).await
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                this.extra = Load::from_result(result);
                cx.notify();
            });
        })
        .detach();
    }

    fn load_achievements(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let b = backend(cx);
        // Added games have achievements only in their files.
        if self.id.runner == irori_core::Runner::Sideload && !b.settings.app().file_achievements {
            return;
        }
        if !matches!(self.achievements, Load::Ready(_)) {
            self.achievements = Load::Loading;
        }
        let id = self.id.clone();
        let language = b.settings.app().ui_language.clone();
        let task = irori_ui::run(cx, async move {
            if b.store(id.runner).is_none() {
                return Err(crate::i18n::store_unavailable(id.runner));
            }
            b.achievements(&id, &language).await
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                this.achievements = Load::from_result(result);
                if this.tab == InfoTab::Achievements && !this.has_achievements() {
                    this.tab = InfoTab::About;
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Read the compatibility data again (after the HowLongToBeat match
    /// changed).
    pub(super) fn reload_compat(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.compat = Load::Idle;
        self.load_compat(window, cx);
        cx.notify();
    }

    fn load_compat(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(game) = self.game.clone() else {
            return;
        };
        // PC compatibility data (ProtonDB, Steam Deck) says nothing about
        // emulated games, and a title match would be another game.
        if matches!(
            game.runner(),
            irori_core::Runner::Sideload | irori_core::Runner::Emulation
        ) {
            return;
        }
        let Some(task) = hooks::fetch_compatibility(game, cx) else {
            return;
        };
        self.compat = Load::Loading;
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                this.compat = Load::from_result(result);
                cx.notify();
            });
        })
        .detach();
    }

    fn on_library_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let next = models(cx)
            .library
            .read(cx)
            .snapshot
            .get(&self.id)
            .cloned()
            .or_else(|| backend(cx).game(&self.id));
        let first = self.game.is_none() && next.is_some();
        let title_changed = match (&self.game, &next) {
            (Some(a), Some(b)) => a.title != b.title,
            _ => false,
        };
        self.game = next;
        if first {
            self.load_all(window, cx);
        } else if title_changed && let Some(t) = self.title() {
            cx.emit(GamePageEvent::TitleChanged(t));
        }
        let installed = self.game.as_ref().is_some_and(|g| g.is_installed);
        if installed != self.was_installed {
            self.was_installed = installed;
            self.load_install_state(window, cx);
        }
        cx.notify();
    }

    fn on_status_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let playing = models(cx).statuses.read(cx).is_playing(&self.id);
        if self.was_playing && !playing {
            // Unlocks happen while playing; refresh after the session.
            self.load_achievements(window, cx);
        }
        self.was_playing = playing;
        cx.notify();
    }

    pub(crate) fn has_achievements(&self) -> bool {
        self.achievements.ready().is_some_and(|a| !a.is_empty())
    }

    /// Primary action as currently shown.
    pub fn primary_action(&self, cx: &App) -> Option<PrimaryAction> {
        let game = self.game.as_ref()?;
        let m = models(cx);
        let status = m.statuses.read(cx).kind(&self.id);
        let queue = &m.downloads.read(cx).queue;
        let has_update = m.library.read(cx).has_update(&self.id);
        let mut inputs = ActionInputs::from_game(game, status, queue, has_update);
        inputs.files_available = self.files_available;
        inputs.launch_pending = self.launch_pending;
        Some(PrimaryAction::from_inputs(&inputs))
    }

    /// Run the primary action (the same command as the main button).
    pub fn run_primary(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(action) = self.primary_action(cx) else {
            return;
        };
        match action {
            PrimaryAction::Install => crate::open_install_sheet(self.id.clone(), window, cx),
            PrimaryAction::Play => self.play(self.selected_option.clone(), window, cx),
            PrimaryAction::Update => ops::queue_update(&self.id, cx),
            PrimaryAction::Stop => self.run_pending("stop", ops::stop(&self.id, window, cx), cx),
            PrimaryAction::Resume => ops::resume_downloads(cx),
            PrimaryAction::Queued => ops::cancel_download(&self.id, true, cx),
            PrimaryAction::Missing => ops::change_install_location(&self.id, window, cx),
            PrimaryAction::Downloading { .. }
            | PrimaryAction::Launching
            | PrimaryAction::Busy(_)
            | PrimaryAction::NotInstallable
            | PrimaryAction::Unsupported => {}
        }
    }

    pub(crate) fn play(
        &mut self,
        option: Option<LaunchOption>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.play_with(
            LaunchParams {
                option,
                ..Default::default()
            },
            window,
            cx,
        );
    }

    /// Launch with one-off choices (verbose logs, no update check).
    pub(crate) fn play_with(
        &mut self,
        params: LaunchParams,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.launch_pending {
            return;
        }
        let online = models(cx).accounts.read(cx).online;
        let offline_capable = self.game.as_ref().is_some_and(|g| g.can_run_offline);
        if !online && !offline_capable {
            window.push_notification(
                gpui_kit::component::notification::Notification::warning(t(
                    "game.page.offline_warning",
                )),
                cx,
            );
        }
        self.launch_pending = true;
        cx.notify();
        let task = ops::play_with(&self.id, params, window, cx);
        cx.spawn(async move |this, cx| {
            task.await;
            let _ = this.update(cx, |this, cx| {
                this.launch_pending = false;
                cx.notify();
            });
        })
        .detach();
    }

    /// Track a command for its loading state.
    pub(crate) fn run_pending(
        &mut self,
        key: &'static str,
        task: gpui_kit::Task<bool>,
        cx: &mut Context<Self>,
    ) {
        self.pending.insert(key);
        cx.notify();
        cx.spawn(async move |this, cx| {
            task.await;
            let _ = this.update(cx, |this, cx| {
                this.pending.remove(key);
                cx.notify();
            });
        })
        .detach();
    }

    fn toggle_favourite(&mut self, cx: &mut Context<Self>) {
        let fav = models(cx).library.read(cx).is_favourite(&self.id);
        ops::set_favourite(&self.id, !fav, cx);
    }

    fn on_run_primary(
        &mut self,
        _: &RunPrimaryAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.run_primary(window, cx);
    }

    fn on_toggle_favourite(&mut self, _: &ToggleFavourite, _: &mut Window, cx: &mut Context<Self>) {
        self.toggle_favourite(cx);
    }

    fn on_open_log(&mut self, _: &OpenGameLog, window: &mut Window, cx: &mut Context<Self>) {
        if self.id.runner.manages_installs() {
            crate::open_log_sheet(self.id.clone(), window, cx);
        }
    }

    fn on_troubleshoot(
        &mut self,
        _: &TroubleshootLaunch,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.id.runner.manages_installs() {
            crate::open_troubleshooter(self.id.clone(), window, cx);
        }
    }

    fn on_open_settings(
        &mut self,
        _: &OpenGameSettingsAction,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(section) = ops::settings_section(&self.id) {
            ops::open_settings(&self.id, section, cx);
        }
    }
}

impl Render for GamePage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let base = div()
            .id("game-page")
            .key_context(GAME_PAGE_CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::on_run_primary))
            .on_action(cx.listener(Self::on_toggle_favourite))
            .on_action(cx.listener(Self::on_open_log))
            .on_action(cx.listener(Self::on_open_settings))
            .on_action(cx.listener(Self::on_troubleshoot))
            .size_full()
            .when(self.panel.is_none(), |d| d.bg(cx.theme().background))
            .text_color(cx.theme().foreground);
        let Some(game) = self.game.clone() else {
            return base.child(
                EmptyState::new(IconName::Gamepad2, t("game.page.not_found_title"))
                    .description(t("game.page.not_found_description"))
                    .action(
                        Button::new("back-to-library")
                            .label(t("game.page.back_to_library"))
                            .on_click(|_, _, cx| navigate(Route::Library, cx)),
                    ),
            );
        };
        let art = game.hero_art().map(str::to_owned);
        if self.panel == Some(GamePanel::Drawer) {
            self.ink = BackdropInk::of(None, cx);
            return base.child(self.render_drawer(&game, window, cx));
        }
        self.ink = BackdropInk::of(art.as_deref(), cx);
        let backdrop =
            (self.panel.is_none()).then(|| ArtBackdrop::new(art).image_cache(&self.image_cache));
        let header = self.render_header(&game, cx);
        let body = self.render_body(&game, window, cx);
        self.sticky = self.sticky_wanted();
        let sticky = self.sticky.then(|| self.render_sticky_bar(&game, cx));
        let viewer = self.render_viewer(window, cx);
        let page_width = self.page_width.clone();
        base.relative()
            .children(backdrop)
            .child(
                div()
                    .id("game-page-scroll")
                    .size_full()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    // The sticky bar depends on how far the page scrolled. Redraw only
                    // when it should appear or go, once the scroll has been applied.
                    .on_scroll_wheel(cx.listener(|_, _, window, cx| {
                        cx.defer_in(window, |this, _, cx| {
                            if this.sticky_wanted() != this.sticky {
                                cx.notify();
                            }
                        });
                    }))
                    .child(
                        v_flex()
                            .w_full()
                            .child(
                                div().relative().w_full().child(header).child(
                                    canvas(
                                        move |bounds, window, _| {
                                            // Layouts read the width while rendering, so
                                            // render again once it changed.
                                            if page_width.replace(bounds.size.width)
                                                != bounds.size.width
                                            {
                                                window.refresh();
                                            }
                                        },
                                        |_, _, _, _| {},
                                    )
                                    .absolute()
                                    .top_0()
                                    .left_0()
                                    .size_full(),
                                ),
                            )
                            .child(body),
                    ),
            )
            .vertical_scrollbar(&self.scroll)
            .children(sticky)
            .children(viewer)
    }
}
