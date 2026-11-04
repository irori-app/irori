//! The Big Picture screen: a fullscreen, controller-first launcher with two
//! tabs (Home's shelves and the Library grid), a page per game, the quick
//! menu and a bar of controller hints.
//!
//! Keyboard focus stays on the view's root; covers, chips and rows are a
//! virtual focus (`aria-activedescendant` style) driven by the gamepad
//! actions, which the keyboard reaches through the `BigPicture` key context.
//! Only the Library's search field takes real focus, and it sits outside that
//! context so typed letters stay letters.
//!
//! Rendering lives in one module per region: `backdrop` (the theme's
//! background, or the focused game's art, behind everything), `chrome` (top
//! bar and hints), `home` with a module per other Home layout (`spotlight`,
//! `cinema`, `wall`, `carousel`, `compact`), `library`, `page` (a game's
//! page) and `log` (its log panel), `overlay` (quick menu and
//! confirmations), `editor` (the theme editor), `tile` (covers) and
//! `commands` (what the backend runs). `look` holds the theme and layout in
//! use and the colours they give.

mod backdrop;
mod carousel;
mod chrome;
mod cinema;
mod commands;
mod compact;
mod editor;
mod home;
mod library;
mod log;
mod look;
mod overlay;
mod page;
mod spotlight;
mod tile;
mod wall;

#[cfg(test)]
mod tests;

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use chrono::Timelike as _;
use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::component::{ActiveTheme as _, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, AppContext as _, Context, Entity, FocusHandle, Focusable, InteractiveElement as _,
    IntoElement, ParentElement as _, Render, Role, ScrollWheelEvent, SharedString,
    StatefulInteractiveElement as _, Styled as _, Subscription, Task, Window, div,
};
use irori_core::{GameId, GameInfo, InstallPlatform};
use irori_ui::art::{LruImageCache, art_store};
use irori_ui::i18n::t;
use irori_ui::models;
use look::{Colors, Look};

use crate::gamepad::{self, GamepadInput};
use crate::grid::FocusGrid;
use crate::layout::home::{Dir, grid_move, wall_step};
use crate::layout::{GridStep, HomeLayout, Metrics, grid_step};
use crate::model::{
    Chip, GameAction, GameState, LibraryFilter, PageRow, Row, StatusFilter, build_rows,
    cover_action, cycle, disabled_reason, install_platforms, library_games, main_action, page_rows,
    show_options,
};
use crate::theme::ShelfKind;

/// Focus must rest this long before the backdrop art follows it, so fast
/// scrolling does not fetch a hero image per cover passed.
const HERO_DELAY: Duration = Duration::from_millis(250);
/// Decoded cover images kept in memory.
const IMAGE_CACHE: usize = 128;
/// Pixels of wheel or touchpad travel that move the focus by one item.
const WHEEL_STEP: f32 = 60.0;

/// Big Picture's top-level tabs, switched with the shoulder buttons.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Home,
    Library,
}

impl Tab {
    pub const ALL: [Tab; 2] = [Tab::Home, Tab::Library];

    pub fn label(self) -> SharedString {
        t(match self {
            Tab::Home => "bigpicture.tab.home",
            Tab::Library => "bigpicture.tab.library",
        })
    }
}

/// The game page over the tabs.
#[derive(Debug, Clone, PartialEq, Eq)]
struct GamePage {
    id: GameId,
    /// Focused row.
    ix: usize,
    /// Platform the Install row installs for, when chosen.
    platform: Option<InstallPlatform>,
}

/// What covers the screen above the tabs and pages.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Overlay {
    Menu {
        ix: usize,
    },
    /// "Uninstall “…”?" with Cancel (0) or Uninstall (1) focused.
    Uninstall {
        id: GameId,
        choice: usize,
    },
    /// "Delete the theme “…”?" over the editor, the same way.
    DeleteTheme {
        id: String,
        choice: usize,
    },
}

/// Where the Library tab's focus is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LibraryFocus {
    Chips(usize),
    Grid,
}

/// The Library tab's state: filter, the games it yields (derived when the
/// library, user data or filter change), focus and search.
struct LibraryTab {
    filter: LibraryFilter,
    games: Vec<Arc<GameInfo>>,
    key: Option<(u64, usize, LibraryFilter)>,
    focus: LibraryFocus,
    ix: usize,
    search: Entity<InputState>,
    searching: bool,
}

/// The Big Picture view (route [`irori_ui::Route::BigPicture`]). Owns its
/// shelves and Library list (derived from the models when they change), the
/// focus, the open page and overlay, and the window's fullscreen state while
/// it lives: it enters fullscreen when created and restores the previous
/// state when released.
pub struct BigPictureView {
    focus: FocusHandle,
    /// The theme and layout in use, the user's themes and the editor.
    look: Look,
    tab: Tab,
    page: Option<GamePage>,
    /// Pages show the game's log in place of its details (Show log).
    show_log: bool,
    log: Option<log::LogPanel>,
    overlay: Option<Overlay>,
    rows: Vec<Row>,
    rows_key: Option<(u64, usize, u32, Vec<ShelfKind>)>,
    grid: FocusGrid,
    library: LibraryTab,
    /// Wheel travel not yet turned into a focus move.
    wheel: f32,
    /// The cover under the mouse (its element id), which shows its button.
    hovered_cover: Option<SharedString>,
    pending: HashSet<GameId>,
    image_cache: Entity<LruImageCache>,
    /// The game whose art is behind everything, the one it replaced (fading
    /// out), and a counter that restarts the crossfade.
    hero: Option<Arc<GameInfo>>,
    hero_prev: Option<Arc<GameInfo>>,
    hero_gen: u64,
    hero_task: Option<Task<()>>,
    _clock_task: Task<()>,
    _editor_subscription: Option<Subscription>,
    _subscriptions: Vec<Subscription>,
}

/// The current time in the interface language's usual clock format.
fn clock_text() -> SharedString {
    let now = chrono::Local::now();
    crate::text::clock(now.hour(), now.minute())
}

impl BigPictureView {
    /// Build the view, enter fullscreen (remembering the previous state) and
    /// take keyboard focus.
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        let m = models(cx).clone();
        let search = cx
            .new(|cx| InputState::new(window, cx).placeholder(t("bigpicture.search.placeholder")));
        let mut subscriptions = vec![
            cx.observe(&m.library, |this, _, cx| {
                this.refresh(cx);
                cx.notify();
            }),
            cx.observe(&m.settings, |this, settings, cx| {
                let ui = settings.read(cx).app.ui.clone();
                irori_ui::theme::set_zoom(ui.big_picture_zoom(), cx);
                this.look.follow_settings(&ui);
                this.refresh(cx);
                cx.notify();
            }),
            cx.observe(&m.statuses, |_, _, cx| cx.notify()),
            cx.observe(&m.downloads, |_, _, cx| cx.notify()),
            cx.observe(&art_store(cx), |_, _, cx| cx.notify()),
            cx.observe_global::<crate::ShowGame>(|this, cx| this.show_requested_game(cx)),
            cx.on_focus_lost(window, |this, window, cx| {
                let naming = this.look.editor.as_ref().is_some_and(|e| e.naming);
                if !this.library.searching && !naming {
                    this.focus.focus(window, cx);
                }
            }),
            cx.subscribe_in(
                &search,
                window,
                |this, _, event: &InputEvent, window, cx| {
                    match event {
                        InputEvent::Change => this.refresh(cx),
                        InputEvent::PressEnter { .. } | InputEvent::Blur => {
                            this.close_search(window, cx)
                        }
                        _ => {}
                    }
                    cx.notify();
                },
            ),
        ];
        if let Some(pad) = gamepad::model(cx) {
            subscriptions.push(cx.observe(&pad, |_, _, cx| cx.notify()));
        }

        // Big Picture's own interface size, for TVs, while it shows.
        irori_ui::theme::set_zoom(m.settings.read(cx).app.ui.big_picture_zoom(), cx);
        cx.on_release(|_, cx| irori_ui::theme::set_zoom(1.0, cx))
            .detach();

        let restore_fullscreen = window.is_fullscreen();
        if !restore_fullscreen {
            window.toggle_fullscreen();
        }
        cx.on_release_in(window, move |_, window, _| {
            if window.is_fullscreen() != restore_fullscreen {
                window.toggle_fullscreen();
            }
        })
        .detach();

        let clock_task = cx.spawn(async move |this, cx| {
            loop {
                let second = chrono::Local::now().second() as u64;
                cx.background_executor()
                    .timer(Duration::from_secs(60 - second.min(59)))
                    .await;
                let updated = this.update(cx, |_, cx| cx.notify());
                if updated.is_err() {
                    break;
                }
            }
        });

        focus.focus(window, cx);
        let look = Look::new(&m.settings.read(cx).app.ui);
        let mut this = Self {
            focus,
            look,
            tab: Tab::Home,
            page: None,
            show_log: false,
            log: None,
            overlay: None,
            rows: Vec::new(),
            rows_key: None,
            grid: FocusGrid::default(),
            library: LibraryTab {
                filter: LibraryFilter::default(),
                games: Vec::new(),
                key: None,
                focus: LibraryFocus::Grid,
                ix: 0,
                search,
                searching: false,
            },
            wheel: 0.0,
            hovered_cover: None,
            pending: HashSet::new(),
            image_cache: LruImageCache::new(IMAGE_CACHE, cx),
            hero: None,
            hero_prev: None,
            hero_gen: 0,
            hero_task: None,
            _clock_task: clock_task,
            _editor_subscription: None,
            _subscriptions: subscriptions,
        };
        this.load_user_themes(window, cx);
        this.refresh(cx);
        this.show_requested_game(cx);
        this.hero = this.focused_game().cloned();
        this
    }

    /// Open the page of the game asked for through [`crate::show_game`],
    /// focusing it on Home when a shelf has it.
    fn show_requested_game(&mut self, cx: &mut Context<Self>) {
        let Some(id) = crate::take_shown_game(cx) else {
            return;
        };
        let found = self.rows.iter().enumerate().find_map(|(row, r)| {
            r.games
                .iter()
                .position(|g| g.id() == id)
                .map(|col| (row, col))
        });
        if let Some((row, col)) = found {
            let lens = self.lens();
            self.grid.set(row, col, &lens);
            self.tab = Tab::Home;
        }
        self.open_page(id, cx);
    }

    /// The focused game: the page's game, else the focused cover of the tab.
    pub fn focused_game(&self) -> Option<&Arc<GameInfo>> {
        if let Some(page) = &self.page {
            return self.find_game(&page.id);
        }
        self.tab_game()
    }

    /// The focused cover of the current tab.
    fn tab_game(&self) -> Option<&Arc<GameInfo>> {
        match self.tab {
            Tab::Home => {
                let (row, col) = self.grid.focused(&self.lens())?;
                self.rows.get(row)?.games.get(col)
            }
            Tab::Library => self.library.games.get(self.library.ix),
        }
    }

    /// A game by id, from the library or (for one hidden meanwhile) the
    /// lists Big Picture holds.
    fn find_game(&self, id: &GameId) -> Option<&Arc<GameInfo>> {
        self.rows
            .iter()
            .flat_map(|r| r.games.iter())
            .chain(self.library.games.iter())
            .find(|g| g.id() == *id)
    }

    fn page_game(&self, cx: &App) -> Option<Arc<GameInfo>> {
        let id = &self.page.as_ref()?.id;
        models(cx)
            .library
            .read(cx)
            .snapshot
            .get(id)
            .cloned()
            .or_else(|| self.find_game(id).cloned())
    }

    /// Whether the game page is open.
    pub fn is_page_open(&self) -> bool {
        self.page.is_some()
    }

    /// Whether the quick menu or a confirmation is open.
    pub fn is_overlay_open(&self) -> bool {
        self.overlay.is_some()
    }

    fn lens(&self) -> Vec<usize> {
        self.rows.iter().map(|r| r.games.len()).collect()
    }

    fn metrics(&self, window: &Window) -> Metrics {
        let rem = window.rem_size();
        let size = window.viewport_size();
        Metrics::styled(
            size.width / rem,
            size.height / rem,
            self.look.theme().covers,
        )
    }

    /// Rebuild the shelves and the Library list when their inputs changed,
    /// keeping focus on the same game when it still exists.
    fn refresh(&mut self, cx: &mut Context<Self>) {
        let m = models(cx);
        let (snapshot, user) = {
            let lib = m.library.read(cx);
            (lib.snapshot.clone(), lib.user.clone())
        };
        let max_recent = m.settings.read(cx).app.max_recent_games;
        let user_ptr = Arc::as_ptr(&user) as usize;

        let shelves = self.look.theme().shelves.clone();
        let key = (snapshot.revision, user_ptr, max_recent, shelves);
        if self.rows_key.as_ref() != Some(&key) {
            let previous = self.grid.focused(&self.lens()).and_then(|(r, c)| {
                Some((self.rows.get(r)?.kind, self.rows.get(r)?.games.get(c)?.id()))
            });
            self.rows = build_rows(&snapshot, &user, max_recent as usize, &key.3);
            self.rows_key = Some(key);
            let lens = self.lens();
            self.grid.clamp(&lens);
            if let Some((kind, id)) = previous
                && let Some(row) = self.rows.iter().position(|r| r.kind == kind)
                && let Some(col) = self.rows[row].games.iter().position(|g| g.id() == id)
            {
                self.grid.set(row, col, &lens);
            }
        }

        let lib = &mut self.library;
        lib.filter.query = lib.search.read(cx).value().trim().to_string();
        if !show_options(&snapshot, &user).contains(&lib.filter.show) {
            lib.filter.show = Default::default();
        }
        let key = (snapshot.revision, user_ptr, lib.filter.clone());
        if lib.key.as_ref() != Some(&key) {
            let previous = lib.games.get(lib.ix).map(|g| g.id());
            lib.games = library_games(&snapshot, &user, &lib.filter);
            lib.key = Some(key);
            lib.ix = previous
                .and_then(|id| lib.games.iter().position(|g| g.id() == id))
                .unwrap_or(0);
        }

        if self.hero.is_none() {
            self.hero = self.focused_game().cloned();
        }
    }

    fn game_state(game: &GameInfo, cx: &App) -> GameState {
        let m = models(cx);
        let id = game.id();
        let lib = m.library.read(cx);
        let statuses = m.statuses.read(cx);
        GameState {
            installed: game.is_installed,
            installable: game.installable,
            playing: statuses.is_playing(&id),
            busy: statuses.kind(&id).filter(|k| k.is_busy()),
            in_queue: m.downloads.read(cx).queue.active.iter().any(|i| i.id == id),
            has_update: lib.has_update(&id),
            favourite: lib.is_favourite(&id),
            hidden: lib.is_hidden(&id),
            external: !id.runner.manages_installs(),
        }
    }

    // ---- input ---------------------------------------------------------

    fn on_input(&mut self, input: GamepadInput, window: &mut Window, cx: &mut Context<Self>) {
        if input == GamepadInput::Guide {
            self.overlay = None;
            crate::exit_big_picture(cx);
            return;
        }
        if let Some(overlay) = self.overlay.clone() {
            self.overlay_input(overlay, input, window, cx);
        } else if self.look.editor.is_some() {
            self.editor_input(input, window, cx);
        } else if input == GamepadInput::Menu {
            self.overlay = Some(Overlay::Menu { ix: 0 });
            cx.notify();
        } else if self.page.is_some() {
            self.page_input(input, window, cx);
        } else if matches!(input, GamepadInput::PrevPage | GamepadInput::NextPage) {
            let ix = Tab::ALL.iter().position(|t| *t == self.tab).unwrap_or(0);
            let len = Tab::ALL.len();
            let next = if input == GamepadInput::NextPage {
                (ix + 1) % len
            } else {
                (ix + len - 1) % len
            };
            self.set_tab(Tab::ALL[next], cx);
        } else {
            match self.tab {
                Tab::Home => self.home_input(input, window, cx),
                Tab::Library => self.library_input(input, window, cx),
            }
        }
    }

    fn set_tab(&mut self, tab: Tab, cx: &mut Context<Self>) {
        if self.tab != tab {
            self.tab = tab;
            self.after_focus_change(cx);
        }
    }

    fn home_input(&mut self, input: GamepadInput, window: &mut Window, cx: &mut Context<Self>) {
        let dir = match input {
            GamepadInput::Up => Dir::Up,
            GamepadInput::Down => Dir::Down,
            GamepadInput::Left => Dir::Left,
            GamepadInput::Right => Dir::Right,
            GamepadInput::Confirm => {
                if let Some(id) = self.tab_game().map(|g| g.id()) {
                    self.open_page(id, cx);
                }
                return;
            }
            GamepadInput::Alt => {
                self.quick_action(window, cx);
                return;
            }
            GamepadInput::Back => {
                crate::exit_big_picture(cx);
                return;
            }
            _ => return,
        };
        let lens = self.lens();
        let layout = self.look.layout();
        let moved = if layout == HomeLayout::Wall {
            let cols = self.metrics(window).wall().cols;
            match self
                .grid
                .focused(&lens)
                .and_then(|focus| wall_step(&lens, cols, focus, dir))
            {
                Some((row, col)) => {
                    self.grid.set(row, col, &lens);
                    true
                }
                None => false,
            }
        } else {
            self.grid.apply(grid_move(layout, dir), &lens)
        };
        if moved {
            self.after_focus_change(cx);
        }
    }

    fn library_input(&mut self, input: GamepadInput, window: &mut Window, cx: &mut Context<Self>) {
        let len = self.library.games.len();
        match self.library.focus {
            LibraryFocus::Chips(chip) => match input {
                GamepadInput::Left => {
                    self.set_library_focus(LibraryFocus::Chips(chip.saturating_sub(1)), cx)
                }
                GamepadInput::Right => self.set_library_focus(
                    LibraryFocus::Chips((chip + 1).min(Chip::ALL.len() - 1)),
                    cx,
                ),
                GamepadInput::Down if len > 0 => self.set_library_focus(LibraryFocus::Grid, cx),
                GamepadInput::Confirm | GamepadInput::Alt => {
                    self.activate_chip(Chip::ALL[chip.min(Chip::ALL.len() - 1)], window, cx)
                }
                GamepadInput::Back if len > 0 => self.set_library_focus(LibraryFocus::Grid, cx),
                GamepadInput::Back => self.set_tab(Tab::Home, cx),
                _ => {}
            },
            LibraryFocus::Grid => {
                let cols = self.metrics(window).grid_cols;
                let step = match input {
                    GamepadInput::Up => GridStep::Up,
                    GamepadInput::Down => GridStep::Down,
                    GamepadInput::Left => GridStep::Left,
                    GamepadInput::Right => GridStep::Right,
                    GamepadInput::Confirm => {
                        if let Some(id) = self.tab_game().map(|g| g.id()) {
                            self.open_page(id, cx);
                        }
                        return;
                    }
                    GamepadInput::Alt => {
                        self.quick_action(window, cx);
                        return;
                    }
                    GamepadInput::Back => {
                        self.set_tab(Tab::Home, cx);
                        return;
                    }
                    _ => return,
                };
                match grid_step(self.library.ix, len, cols, step) {
                    Some(ix) => {
                        self.library.ix = ix;
                        self.after_focus_change(cx);
                    }
                    None if step == GridStep::Up || len == 0 => {
                        self.set_library_focus(LibraryFocus::Chips(0), cx)
                    }
                    None => {}
                }
            }
        }
    }

    fn set_library_focus(&mut self, focus: LibraryFocus, cx: &mut Context<Self>) {
        if self.library.games.is_empty() && focus == LibraryFocus::Grid {
            return;
        }
        self.library.focus = focus;
        cx.notify();
    }

    /// Confirm on a chip: the next value of Show, Sort or Status, or the
    /// search field.
    fn activate_chip(&mut self, chip: Chip, window: &mut Window, cx: &mut Context<Self>) {
        self.library.focus =
            LibraryFocus::Chips(Chip::ALL.iter().position(|c| *c == chip).unwrap_or(0));
        let filter = &mut self.library.filter;
        match chip {
            Chip::Show => {
                let lib = models(cx).library.read(cx);
                let options = show_options(&lib.snapshot, &lib.user);
                filter.show = cycle(&options, filter.show);
            }
            Chip::Sort => filter.sort = cycle(&crate::model::Sort::ALL, filter.sort),
            Chip::Status => filter.status = cycle(&StatusFilter::ALL, filter.status),
            Chip::Search => {
                self.library.searching = true;
                self.library.search.update(cx, |s, cx| s.focus(window, cx));
            }
        }
        // `refresh` keeps the focused game when the new list still has it.
        self.refresh(cx);
        self.after_focus_change(cx);
    }

    /// Leave the search field for the grid (or the chips when nothing
    /// matches).
    fn close_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.library.searching {
            return;
        }
        self.library.searching = false;
        self.focus.focus(window, cx);
        self.library.focus = if self.library.games.is_empty() {
            LibraryFocus::Chips(Chip::ALL.len() - 1)
        } else {
            LibraryFocus::Grid
        };
        cx.notify();
    }

    fn page_input(&mut self, input: GamepadInput, window: &mut Window, cx: &mut Context<Self>) {
        let Some(game) = self.page_game(cx) else {
            self.page = None;
            cx.notify();
            return;
        };
        let state = Self::game_state(&game, cx);
        let rows = page_rows(state, install_platforms(&game).len());
        let Some(page) = &mut self.page else {
            return;
        };
        let ix = page.ix.min(rows.len().saturating_sub(1));
        match input {
            GamepadInput::Up => page.ix = ix.saturating_sub(1),
            GamepadInput::Down => page.ix = (ix + 1).min(rows.len().saturating_sub(1)),
            GamepadInput::Left | GamepadInput::Right => {
                let forward = input == GamepadInput::Right;
                if let Some(row) = rows.get(ix).copied().filter(|r| r.has_value()) {
                    self.change_value(row, &game, forward, cx);
                }
            }
            GamepadInput::Confirm => {
                if let Some(row) = rows.get(ix).copied() {
                    self.activate_row(row, game, window, cx);
                }
            }
            GamepadInput::Alt => {
                let main = main_action(state);
                self.activate_action(main, game, window, cx);
            }
            GamepadInput::Back => self.page = None,
            _ => {}
        }
        self.after_focus_change(cx);
    }

    fn open_page(&mut self, id: GameId, cx: &mut Context<Self>) {
        self.page = Some(GamePage {
            id,
            ix: 0,
            platform: None,
        });
        self.after_focus_change(cx);
    }

    /// Alt on a cover: the focused game's main command without opening its
    /// page (see [`Self::cover_command`]).
    fn quick_action(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(game) = self.tab_game().cloned() {
            self.cover_command(game, window, cx);
        }
    }

    /// Play, or install with defaults, as [`cover_action`] allows; anything
    /// else opens the page.
    fn cover_command(&mut self, game: Arc<GameInfo>, window: &mut Window, cx: &mut Context<Self>) {
        match cover_action(Self::game_state(&game, cx)) {
            Some(action) => self.run_game_action(action, game, None, window, cx),
            None => self.open_page(game.id(), cx),
        }
    }

    fn overlay_input(
        &mut self,
        overlay: Overlay,
        input: GamepadInput,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match overlay {
            Overlay::DeleteTheme { id, choice } => match input {
                GamepadInput::Up
                | GamepadInput::Down
                | GamepadInput::Left
                | GamepadInput::Right => {
                    self.overlay = Some(Overlay::DeleteTheme {
                        id,
                        choice: 1 - choice.min(1),
                    });
                }
                GamepadInput::Confirm | GamepadInput::Alt => {
                    self.overlay = None;
                    if choice == 1 {
                        self.delete_theme(id, window, cx);
                    }
                }
                GamepadInput::Back | GamepadInput::Menu => self.overlay = None,
                _ => {}
            },
            Overlay::Uninstall { id, choice } => match input {
                GamepadInput::Up
                | GamepadInput::Down
                | GamepadInput::Left
                | GamepadInput::Right => {
                    self.overlay = Some(Overlay::Uninstall {
                        id,
                        choice: 1 - choice.min(1),
                    });
                }
                GamepadInput::Confirm | GamepadInput::Alt => {
                    self.overlay = None;
                    if choice == 1 {
                        self.uninstall(id, window, cx);
                    }
                }
                GamepadInput::Back | GamepadInput::Menu => self.overlay = None,
                _ => {}
            },
            Overlay::Menu { ix } => {
                let items = self.menu(cx);
                let last = items.len().saturating_sub(1);
                match input {
                    GamepadInput::Up => {
                        self.overlay = Some(Overlay::Menu {
                            ix: ix.saturating_sub(1),
                        })
                    }
                    GamepadInput::Down => {
                        self.overlay = Some(Overlay::Menu {
                            ix: (ix + 1).min(last),
                        })
                    }
                    GamepadInput::Confirm | GamepadInput::Alt => {
                        if let Some(item) = items.get(ix) {
                            self.activate_menu_item(*item, window, cx);
                        }
                    }
                    GamepadInput::Back | GamepadInput::Menu => self.overlay = None,
                    _ => {}
                }
            }
        }
        cx.notify();
    }

    /// Activate a row of the game page (Enter or a click).
    fn activate_row(
        &mut self,
        row: PageRow,
        game: Arc<GameInfo>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match row {
            PageRow::Action(action) => self.activate_action(action, game, window, cx),
            PageRow::Platform | PageRow::Favourite | PageRow::Log | PageRow::Hidden => {
                self.change_value(row, &game, true, cx)
            }
            PageRow::Settings => self.open_game_settings(game.id(), cx),
            PageRow::Playtime => {}
        }
    }

    fn activate_action(
        &mut self,
        action: GameAction,
        game: Arc<GameInfo>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let state = Self::game_state(&game, cx);
        if let Some(reason) = disabled_reason(action, state) {
            use gpui_kit::component::WindowExt as _;
            use gpui_kit::component::notification::Notification;
            window.push_notification(Notification::warning(reason).title(game.title.clone()), cx);
            return;
        }
        if action == GameAction::Uninstall {
            self.overlay = Some(Overlay::Uninstall {
                id: game.id(),
                choice: 0,
            });
            cx.notify();
            return;
        }
        let platform = self
            .page
            .as_ref()
            .filter(|p| p.id == game.id())
            .and_then(|p| p.platform);
        self.run_game_action(action, game, platform, window, cx);
    }

    /// Left/Right (or Enter) on a row with a value.
    fn change_value(
        &mut self,
        row: PageRow,
        game: &Arc<GameInfo>,
        forward: bool,
        cx: &mut Context<Self>,
    ) {
        let state = Self::game_state(game, cx);
        match row {
            PageRow::Favourite => self.set_favourite(game.id(), !state.favourite, cx),
            PageRow::Hidden => self.set_hidden(game.id(), !state.hidden, cx),
            PageRow::Log => self.show_log = !self.show_log,
            PageRow::Platform => {
                let platforms = install_platforms(game);
                let Some(page) = &mut self.page else {
                    return;
                };
                let current = page
                    .platform
                    .unwrap_or_else(|| crate::model::default_platform(game));
                let ix = platforms.iter().position(|p| *p == current).unwrap_or(0);
                let len = platforms.len().max(1);
                let next = if forward {
                    (ix + 1) % len
                } else {
                    (ix + len - 1) % len
                };
                page.platform = platforms.get(next).copied();
            }
            _ => {}
        }
        cx.notify();
    }

    fn after_focus_change(&mut self, cx: &mut Context<Self>) {
        self.schedule_hero(cx);
        cx.notify();
    }

    /// Follow the focus with the backdrop once it rests (see [`HERO_DELAY`]).
    fn schedule_hero(&mut self, cx: &mut Context<Self>) {
        let target = self.focused_game().map(|g| g.id());
        if self.hero.as_ref().map(|g| g.id()) == target {
            self.hero_task = None;
            return;
        }
        self.hero_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(HERO_DELAY).await;
            let _ = this.update(cx, |this, cx| {
                let next = this.focused_game().cloned();
                this.hero_prev = this.hero.take();
                this.hero = next;
                this.hero_gen += 1;
                cx.notify();
            });
        }));
    }

    /// Wheel or touchpad travel: vertical moves between shelves or grid
    /// rows, horizontal along a shelf.
    fn on_wheel(&mut self, event: &ScrollWheelEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.page.is_some() || self.overlay.is_some() || self.look.editor.is_some() {
            return;
        }
        let delta = event.delta.pixel_delta(window.line_height());
        let (amount, along) = if f32::from(delta.x.abs()) > f32::from(delta.y.abs()) {
            (f32::from(delta.x), true)
        } else {
            (f32::from(delta.y), false)
        };
        self.wheel += amount;
        while self.wheel.abs() >= WHEEL_STEP {
            let back = self.wheel > 0.0;
            self.wheel -= WHEEL_STEP.copysign(self.wheel);
            let input = match (along, back) {
                (true, true) => GamepadInput::Left,
                (true, false) => GamepadInput::Right,
                (false, true) => GamepadInput::Up,
                (false, false) => GamepadInput::Down,
            };
            if self.tab == Tab::Library && self.library.focus != LibraryFocus::Grid {
                break;
            }
            self.on_input(input, window, cx);
        }
    }
}

impl BigPictureView {
    fn render_empty(&self, c: Colors, cx: &mut Context<Self>) -> impl IntoElement {
        use gpui_kit::assets::IconName;
        use gpui_kit::component::Sizable as _;
        use gpui_kit::component::button::Button;
        use irori_ui::components::EmptyState;
        let refreshing = models(cx).library.read(cx).refreshing;
        div().size_full().text_color(c.foreground).child(
            EmptyState::new(
                IconName::Gamepad2,
                if refreshing {
                    t("bigpicture.empty.loading")
                } else {
                    t("bigpicture.empty.title")
                },
            )
            .description(t("bigpicture.empty.description"))
            .action(
                Button::new("bp-empty-exit")
                    .large()
                    .label(t("bigpicture.exit"))
                    .on_click(|_, _, cx| crate::exit_big_picture(cx)),
            ),
        )
    }
}

impl Focusable for BigPictureView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for BigPictureView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let c = Colors::of(cx, self.look.theme());
        let metrics = self.metrics(window);
        let font = self.look.theme().font.family(&cx.theme().mono_font_family);
        let empty = models(cx).library.read(cx).snapshot.games.is_empty();
        if empty || self.page.is_none() {
            self.sync_log(None, cx);
        }
        let content = if empty {
            self.render_empty(c, cx).into_any_element()
        } else if self.page.is_some() {
            self.render_page(c, metrics, window, cx)
        } else {
            let tab = match self.tab {
                Tab::Home => match self.look.layout() {
                    HomeLayout::Shelves => self.render_home(c, metrics, window, cx),
                    HomeLayout::Spotlight => self.render_spotlight(c, metrics, window, cx),
                    HomeLayout::Cinema => self.render_cinema(c, metrics, window, cx),
                    HomeLayout::Wall => self.render_wall(c, metrics, window, cx),
                    HomeLayout::Carousel => self.render_carousel(c, metrics, window, cx),
                    HomeLayout::Compact => self.render_compact(c, metrics, window, cx),
                },
                Tab::Library => self.render_library(c, metrics, window, cx),
            };
            // A tab (or the tab under a closed page) rises in as it appears,
            // and again when Home changes layout; it keeps its id while
            // showing, so this plays once.
            irori_ui::motion::rise_in(
                SharedString::from(format!("bp-tab-{:?}-{}", self.tab, self.look.layout().id())),
                div().size_full().child(tab),
                12.,
            )
        };
        div()
            .image_cache(self.image_cache.clone())
            .id("big-picture-root")
            .size_full()
            .relative()
            .overflow_hidden()
            .bg(c.background)
            .text_color(c.foreground)
            .font_family(font)
            .child(
                div()
                    .id("big-picture")
                    .role(Role::Application)
                    .aria_label(t("bigpicture.title"))
                    .key_context(crate::CONTEXT)
                    .track_focus(&self.focus)
                    .absolute()
                    .inset_0()
                    .size_full()
                    .on_action(cx.listener(|this, _: &gamepad::Up, w, cx| {
                        this.on_input(GamepadInput::Up, w, cx)
                    }))
                    .on_action(cx.listener(|this, _: &gamepad::Down, w, cx| {
                        this.on_input(GamepadInput::Down, w, cx)
                    }))
                    .on_action(cx.listener(|this, _: &gamepad::Left, w, cx| {
                        this.on_input(GamepadInput::Left, w, cx)
                    }))
                    .on_action(cx.listener(|this, _: &gamepad::Right, w, cx| {
                        this.on_input(GamepadInput::Right, w, cx)
                    }))
                    .on_action(cx.listener(|this, _: &gamepad::Confirm, w, cx| {
                        this.on_input(GamepadInput::Confirm, w, cx)
                    }))
                    .on_action(cx.listener(|this, _: &gamepad::Back, w, cx| {
                        this.on_input(GamepadInput::Back, w, cx)
                    }))
                    .on_action(cx.listener(|this, _: &gamepad::Alt, w, cx| {
                        this.on_input(GamepadInput::Alt, w, cx)
                    }))
                    .on_action(cx.listener(|this, _: &gamepad::Menu, w, cx| {
                        this.on_input(GamepadInput::Menu, w, cx)
                    }))
                    .on_action(cx.listener(|this, _: &gamepad::PrevPage, w, cx| {
                        this.on_input(GamepadInput::PrevPage, w, cx)
                    }))
                    .on_action(cx.listener(|this, _: &gamepad::NextPage, w, cx| {
                        this.on_input(GamepadInput::NextPage, w, cx)
                    }))
                    .on_action(cx.listener(|this, _: &gamepad::Guide, w, cx| {
                        this.on_input(GamepadInput::Guide, w, cx)
                    }))
                    .on_scroll_wheel(
                        cx.listener(|this, e: &ScrollWheelEvent, w, cx| this.on_wheel(e, w, cx)),
                    )
                    .child(self.render_backdrop(c, window, cx))
                    .child(
                        v_flex()
                            .absolute()
                            .inset_0()
                            .size_full()
                            .child(self.render_top_bar(c, metrics, cx))
                            .child(div().flex_1().min_h_0().w_full().child(content))
                            .child(self.render_hints(c, metrics, cx)),
                    )
                    .when_some(self.render_editor(c, metrics, cx), |d, editor| {
                        d.child(editor)
                    })
                    .when_some(self.render_overlay(c, metrics, cx), |d, overlay| {
                        d.child(overlay)
                    }),
            )
            .when(self.library.searching, |d| {
                d.child(self.render_search(c, metrics, cx))
            })
    }
}
