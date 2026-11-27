//! The library screen view.

mod actions;
mod bulk;
pub(crate) mod details;
mod empty;
mod frame;
mod look;
mod menus;
mod panel;
mod quick;
pub(crate) mod remove;
mod render_grid;
mod render_list;
mod sort;
mod tabs_manager;
mod toolbar;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Instant;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::popover::Popover;
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::slider::{SliderEvent, SliderState, SliderValue};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{
    ActiveTheme as _, Icon, Selectable, Sizable as _, StyledExt as _, VirtualListScrollHandle,
    h_flex, v_flex, v_virtual_list,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, EventEmitter, ExternalPaths, FocusHandle,
    Focusable, Hsla, InteractiveElement as _, IntoElement, KeyDownEvent, Keystroke,
    ParentElement as _, Pixels, Render, RenderOnce, SharedString, Size, Styled as _, Subscription,
    Window, canvas, div, px, rems,
};
use irori_core::settings::{
    CoverShape, Density, FocusCoverSize, LibraryBackground, LibraryLayout, ListRowSize, UiPrefs,
};
use irori_core::{GameId, GameInfo, GameStatus, Runner, StatusKind};
use irori_ui::components::{ArtBackdrop, EmptyState};
use irori_ui::i18n::{t, t_args};
use irori_ui::{backend, models};

use self::frame::{Frame, Item, item_rem};
use crate::availability::Availability;
use crate::commands::GameState;
use crate::dialogs;
use crate::filter::{self, FilteredGames, Grouping, ViewFilter};
use crate::filters_panel::{FilterModel, FiltersPanel, set_active_filters};
use crate::grid::{self, Move};
use crate::layouts;
use crate::top::TopCache;
use crate::{
    ActivateSelected, EndSelection, ExtendDown, ExtendFirst, ExtendLast, ExtendLeft,
    ExtendPageDown, ExtendPageUp, ExtendRight, ExtendUp, FocusSearch, GRID_CONTEXT,
    LIBRARY_CONTEXT, LibraryEvent, NextLibraryView, OpenSelected, PickFocused, RemovePicked,
    SelectAllGames, SelectDown, SelectFirst, SelectLast, SelectLeft, SelectPageDown, SelectPageUp,
    SelectRight, SelectUp, ShowFocus, ShowGrid, ShowList, ToggleDrawer, ToggleModeGroups,
    ToggleStatusGroups,
};
use irori_ui::art::LruImageCache;

/// Width of a favourite tile in the top row, in rem.
const FAVOURITE_REM: f32 = 8.0;
/// Decoded images kept by the library's image cache until the first layout
/// sizes it to the viewport (see [`grid::cached_images`]).
const IMAGE_CACHE_CAPACITY: usize = 96;

/// The library screen. Create one per window with [`LibraryView::new`] and
/// render it for `Route::Library`. Emits [`LibraryEvent`]s for actions owned
/// by the game feature.
pub struct LibraryView {
    focus_handle: FocusHandle,
    grid_focus: FocusHandle,
    search: Entity<InputState>,
    filters: Entity<FilterModel>,
    filters_panel: Entity<FiltersPanel>,
    pub(super) filters_open: bool,
    pub(super) look_open: bool,
    pub(super) sort_open: bool,
    pub(super) sort_more_open: Option<bool>,
    pub(super) tabs_manager_open: bool,
    pub(super) greeting_open: bool,
    greeting_input: Option<(Entity<InputState>, Subscription)>,
    tile_slider: Entity<SliderState>,
    scroll: VirtualListScrollHandle,
    images: Entity<LruImageCache>,
    games: FilteredGames,
    top: TopCache,
    /// What the empty library shows (see `empty.rs`), made once.
    found_sources: Option<gpui_kit::AnyView>,
    /// Active top-strip tab whose per-tab visualization settings are live, or
    /// `None` while custom Filters-popover criteria are active.
    pub(super) active_tab: Option<crate::views::TabKey>,
    layout: LibraryLayout,
    /// The view the user chose (`libraryLayout` or the active tab's saved
    /// layout); `layout` differs from it while `libraryAutoLayout` switched
    /// with the window's width.
    chosen_layout: LibraryLayout,
    /// Whether the window was wide enough for Focus at the last frame,
    /// while `libraryAutoLayout` is on.
    auto_wide: Option<bool>,
    pub(super) cover_shape: CoverShape,
    tile_rem: f32,
    pub(super) list_row_size: ListRowSize,
    pub(super) focus_cover_size: FocusCoverSize,
    /// Whether `tile_slider` needs syncing with `tile_rem` on the next frame
    /// (set when switching tabs without a `&mut Window`).
    pub(super) tile_slider_dirty: bool,
    /// Suppresses recursive filter-observer sync while `apply_tab` updates the
    /// filter model.
    pub(super) suppress_filter_sync: bool,
    density: Density,
    /// The interface preferences, for the background behind the covers.
    ui_prefs: UiPrefs,
    /// Measured size of the scroll region (zero until the first paint).
    viewport: Size<Pixels>,
    /// Cached game counts for the view tabs.
    chip_counts: crate::chips::ChipCountCache,
    /// Last interface language applied to the search placeholder.
    search_placeholder_lang: Option<String>,
    /// Capacity last given to `images`.
    image_capacity: usize,
    frame: Frame,
    /// Grouping of the last frame: a change reorders every game, so the list
    /// starts again from the top.
    grouping: Grouping,
    /// The selected game: the keyboard's place in the grid or list, and the
    /// game the drawer or the Focus view's page shows.
    focused: Option<GameId>,
    /// Where `focused` was in the shown list, so Focus can stay near it when
    /// a filter hides it.
    focused_ix: Option<usize>,
    /// The grid's drawer and the views of selected games.
    panels: panel::Panels,
    /// Scroll the selected game into view once the items are rebuilt (the
    /// view changed).
    scroll_pending: bool,
    hovered: Option<GameId>,
    menu_open: Option<GameId>,
    /// Games picked for bulk actions.
    selection: crate::selection::Selection,
    /// What the bulk bar offers for the picks, worked out once per change.
    bulk: bulk::BulkCache,
    /// The toolbar held over the list, measured.
    toolbar: toolbar::ListToolbar,
    /// The stores the library has games of, once per library revision.
    stores_with_games: filter::PerRevision<HashSet<Runner>>,
    /// Total installed games in the library, once per library revision.
    installed_count: filter::PerRevision<usize>,
    launching: HashSet<GameId>,
    /// Games whose next launch skips the store's update check ("Play now").
    skip_version_check: HashSet<GameId>,
    availability: Availability,
    /// Cached and queued ProtonDB / Steam Deck badges for visible games.
    compat: crate::compat_badges::CompatQueue,
    /// In-flight compatibility lookup tasks; cleared when turned off or dropped.
    compat_tasks: HashMap<GameId, gpui_kit::Task<()>>,
    /// Last observed offline state for compatibility badge lookups.
    compat_was_offline: bool,
    /// Whether a game has desktop/app-menu shortcuts, with when it was
    /// checked. Checked when a game is hovered or focused, so its menu can
    /// offer the one shortcut command that applies.
    shortcuts: HashMap<GameId, (bool, Instant)>,
    shortcut_checks: HashSet<GameId>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<LibraryEvent> for LibraryView {}

impl Focusable for LibraryView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl LibraryView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        crate::init(cx);
        let m = models(cx).clone();
        let ui = m.settings.read(cx).app.ui.clone();

        let search =
            cx.new(|cx| InputState::new(window, cx).placeholder(t("library.search.placeholder")));
        let filters = cx.new(|_| FilterModel::new(filter::load()));
        set_active_filters(&filters, cx);
        remove::set_active_library(cx.entity().downgrade(), cx);
        // After the saved criteria, so a change asked for before the view
        // existed (a game page's mode chip) wins over them.
        crate::filters_panel::apply_pending(&filters, cx);
        let filters_panel = cx.new(|cx| FiltersPanel::new(filters.clone(), cx));
        let art = irori_ui::art::art_store(cx);
        let saved_views = crate::views::saved_views(cx);
        let (active_tab, initial_prefs) = {
            let saved = saved_views.read(cx).views();
            let user = &m.library.read(cx).user;
            let smart_ids = crate::views::smart_collection_ids(user);
            let initial_filter = filters.read(cx).filter();
            let tab =
                crate::chips::active_tab_for(initial_filter, saved.active_tab.as_ref(), &smart_ids);
            let prefs = tab
                .as_ref()
                .and_then(|k| saved.prefs_for(k))
                .cloned()
                .unwrap_or_default();
            (tab, prefs)
        };
        let initial_layout = initial_prefs.layout.unwrap_or(ui.library_layout);
        let cover_shape = initial_prefs.cover_shape.unwrap_or(ui.cover_shape);
        let tile_rem = initial_prefs
            .tile_size
            .unwrap_or(ui.tile_size)
            .clamp(grid::MIN_TILE_REM, grid::MAX_TILE_REM);
        let list_row_size = initial_prefs
            .list_row_size
            .unwrap_or(ui.library_list_row_size);
        let focus_cover_size = initial_prefs
            .focus_cover_size
            .unwrap_or(ui.library_focus_cover_size);
        let tile_slider = cx.new(|_| {
            SliderState::new()
                .min(grid::MIN_TILE_REM)
                .max(grid::MAX_TILE_REM)
                .step(0.5)
                .default_value(tile_rem)
        });

        let subscriptions = vec![
            cx.observe(&m.library, |this: &mut Self, library, cx| {
                // A smart category deleted here or on another computer
                // leaves the filter, and the saved view state, at once.
                let user = library.read(cx).user.clone();
                this.filters
                    .update(cx, |m, cx| m.update(|f| f.forget_missing_smart(&user), cx));
                cx.notify();
            }),
            cx.observe(&m.statuses, |_, _, cx| cx.notify()),
            // A reordered queue moves the places queued covers show.
            cx.observe(&m.downloads, |_, _, cx| cx.notify()),
            cx.observe(&m.accounts, |_, _, cx| cx.notify()),
            cx.observe(&art, |_, _, cx| cx.notify()),
            cx.observe(&filters, |this, filters, cx| {
                this.sync_tab_from_filter(filters, cx);
                cx.notify();
            }),
            cx.observe(&saved_views, |_, _, cx| cx.notify()),
            cx.observe_in(&m.settings, window, |this, settings, window, cx| {
                let ui = settings.read(cx).app.ui.clone();
                let prev = this.ui_prefs.clone();
                let tab_prefs = this
                    .active_tab
                    .as_ref()
                    .and_then(|k| {
                        crate::views::saved_views(cx)
                            .read(cx)
                            .views()
                            .prefs_for(k)
                            .cloned()
                    })
                    .unwrap_or_default();
                if prev.library_layout != ui.library_layout && tab_prefs.layout.is_none() {
                    // Switched from elsewhere (Settings → Appearance).
                    this.chosen_layout = ui.library_layout;
                    this.layout = ui.library_layout;
                    this.scroll_to_focused();
                }
                if prev.cover_shape != ui.cover_shape && tab_prefs.cover_shape.is_none() {
                    this.cover_shape = ui.cover_shape;
                }
                if prev.library_list_row_size != ui.library_list_row_size
                    && tab_prefs.list_row_size.is_none()
                {
                    this.list_row_size = ui.library_list_row_size;
                }
                if prev.library_focus_cover_size != ui.library_focus_cover_size
                    && tab_prefs.focus_cover_size.is_none()
                {
                    this.focus_cover_size = ui.library_focus_cover_size;
                }
                if !ui.library_auto_layout {
                    this.auto_wide = None;
                }
                this.density = ui.density;
                this.ui_prefs = ui.clone();
                if !this.compat_active() {
                    this.cancel_compat();
                }
                let size = ui.tile_size.clamp(grid::MIN_TILE_REM, grid::MAX_TILE_REM);
                if (prev.tile_size - ui.tile_size).abs() > f32::EPSILON
                    && tab_prefs.tile_size.is_none()
                    && (size - this.tile_rem).abs() > f32::EPSILON
                {
                    this.tile_rem = size;
                    this.tile_slider
                        .update(cx, |s, cx| s.set_value(size, window, cx));
                }
                cx.notify();
            }),
            cx.subscribe_in(
                &search,
                window,
                |this, state, event, window, cx| match event {
                    InputEvent::Change => {
                        let query = state.read(cx).value().to_string();
                        this.filters
                            .update(cx, |m, cx| m.update(|f| f.query = query, cx));
                    }
                    InputEvent::PressEnter { .. } => this.enter_results(window, cx),
                    _ => {}
                },
            ),
            cx.subscribe(
                &tile_slider,
                |this, _, event: &SliderEvent, cx| match event {
                    SliderEvent::Change(v) => {
                        this.tile_rem = slider_value(v);
                        cx.notify();
                    }
                    SliderEvent::Release(v) => {
                        let size = slider_value(v);
                        this.set_tile_size(size, cx);
                    }
                },
            ),
        ];

        Self {
            focus_handle: cx.focus_handle(),
            grid_focus: cx.focus_handle().tab_stop(true),
            search,
            filters,
            filters_panel,
            filters_open: false,
            look_open: false,
            sort_open: false,
            sort_more_open: None,
            tabs_manager_open: false,
            greeting_open: false,
            greeting_input: None,
            tile_slider,
            scroll: VirtualListScrollHandle::new(),
            images: LruImageCache::new(IMAGE_CACHE_CAPACITY, cx),
            games: FilteredGames::default(),
            top: TopCache::default(),
            found_sources: None,
            active_tab,
            layout: initial_layout,
            chosen_layout: initial_layout,
            auto_wide: None,
            cover_shape,
            tile_rem,
            list_row_size,
            focus_cover_size,
            tile_slider_dirty: false,
            suppress_filter_sync: false,
            density: ui.density,
            ui_prefs: ui.clone(),
            viewport: Size::default(),
            chip_counts: Default::default(),
            search_placeholder_lang: None,
            image_capacity: IMAGE_CACHE_CAPACITY,
            frame: Frame::default(),
            grouping: Grouping::None,
            focused: None,
            focused_ix: None,
            panels: panel::Panels::new(ui.library_drawer_width),
            scroll_pending: false,
            toolbar: Default::default(),
            hovered: None,
            menu_open: None,
            selection: crate::selection::Selection::default(),
            launching: HashSet::new(),
            bulk: Default::default(),
            stores_with_games: Default::default(),
            installed_count: Default::default(),
            skip_version_check: HashSet::new(),
            availability: Availability::default(),
            compat: crate::compat_badges::CompatQueue::default(),
            compat_tasks: HashMap::new(),
            compat_was_offline: false,
            shortcuts: HashMap::new(),
            shortcut_checks: HashSet::new(),
            _subscriptions: subscriptions,
        }
    }

    /// Current view criteria.
    pub fn filter(&self, cx: &App) -> ViewFilter {
        self.filters.read(cx).filter().clone()
    }

    /// Layout currently displayed by this library view.
    pub(crate) fn current_layout(&self) -> LibraryLayout {
        self.layout
    }

    /// Change the view criteria programmatically (e.g. "show updates" from a
    /// notification).
    pub fn update_filter(&mut self, f: impl FnOnce(&mut ViewFilter), cx: &mut Context<Self>) {
        self.filters.update(cx, |m, cx| m.update(f, cx));
    }

    /// Games currently shown, in display order.
    pub fn visible_games(&self) -> Arc<[Arc<GameInfo>]> {
        self.frame.games.clone()
    }

    /// Play statuses of the shown sections, in order; empty when not
    /// grouped by play status.
    #[cfg(test)]
    pub(crate) fn status_groups(&self) -> Vec<irori_services::EffectiveStatus> {
        self.frame
            .groups
            .iter()
            .filter_map(|g| match g.kind {
                crate::filter::GroupKind::Status(s) => Some(s),
                _ => None,
            })
            .collect()
    }

    /// The game with keyboard focus in the grid or list.
    pub fn focused_game(&self) -> Option<&GameId> {
        self.focused.as_ref()
    }

    /// Picked games in display order, and whether selection mode is on.
    #[cfg(test)]
    pub(crate) fn selection_state(&self) -> (Vec<String>, bool) {
        let picked = self.selection.games(&self.frame.games);
        (
            picked.iter().map(|g| g.title.clone()).collect(),
            self.selection.is_active(),
        )
    }

    #[cfg(test)]
    pub(crate) fn click_for_test(
        &mut self,
        id: &GameId,
        modifiers: gpui_kit::Modifiers,
        count: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.click_game(id, modifiers, count, window, cx)
    }

    /// Scroll `y` down from the top, as the wheel would.
    #[cfg(test)]
    pub(crate) fn scroll_for_test(&mut self, y: Pixels, cx: &mut Context<Self>) {
        use gpui_kit::point;
        self.scroll.set_offset(point(px(0.), -y));
        cx.notify();
    }

    /// Forget the selected game, as a fresh library has none.
    #[cfg(test)]
    pub(crate) fn clear_focus_for_test(&mut self, cx: &mut Context<Self>) {
        self.focused = None;
        self.focused_ix = None;
        cx.notify();
    }

    /// Switch views without remembering it (tests share one settings file).
    #[cfg(test)]
    pub(crate) fn show_layout_for_test(&mut self, layout: LibraryLayout, cx: &mut Context<Self>) {
        self.layout = layout;
        self.scroll_to_focused();
        cx.notify();
    }

    /// Whether the view tabs strip is shown.
    #[cfg(test)]
    pub(crate) fn view_tabs_shown(&self) -> bool {
        self.ui_prefs.library_quick_controls
    }

    /// Cached counts for the preset and saved-view tabs.
    #[cfg(test)]
    pub(crate) fn chip_counts(&self) -> &crate::chips::ChipCountMap {
        &self.frame.chip_counts
    }
    /// Turn the header's quick controls on or off without saving it (tests
    /// share one settings file).
    #[cfg(test)]
    pub(crate) fn set_quick_controls_for_test(&mut self, on: bool, cx: &mut Context<Self>) {
        self.ui_prefs.library_quick_controls = on;
        cx.notify();
    }

    /// Turn opening the drawer on a click on or off without saving it
    /// (tests share one settings file).
    #[cfg(test)]
    pub(crate) fn set_open_details_for_test(&mut self, on: bool, cx: &mut Context<Self>) {
        self.ui_prefs.library_open_details_on_select = on;
        cx.notify();
    }

    /// Follow the window's width with the view, or stop, without saving it
    /// (tests share one settings file).
    #[cfg(test)]
    pub(crate) fn set_auto_layout_for_test(&mut self, on: bool, cx: &mut Context<Self>) {
        self.ui_prefs.library_auto_layout = on;
        self.auto_wide = None;
        cx.notify();
    }

    /// The view shown, whether the drawer is open, and its width in rem.
    #[cfg(test)]
    pub(crate) fn view_state(&self, window: &Window) -> (LibraryLayout, bool, f32) {
        (
            self.layout,
            self.panels.drawer_open,
            self.drawer_rem(window),
        )
    }

    /// The list's scroll offset (down from the top), the content height its
    /// scrollbar knows, the height of every item laid end to end, and the
    /// height of the list's viewport.
    #[cfg(test)]
    pub(crate) fn scroll_state(&self) -> (Pixels, Pixels, Pixels, Pixels) {
        use gpui_kit::component::scroll::ScrollbarHandle as _;
        let items = self
            .frame
            .sizes
            .iter()
            .fold(px(0.), |sum, size| sum + size.height);
        (
            -self.scroll.offset().y,
            self.scroll.content_size().height,
            items,
            self.scroll.viewport_bounds().size.height,
        )
    }

    #[cfg(test)]
    pub(crate) fn grid_focus_handle(&self) -> FocusHandle {
        self.grid_focus.clone()
    }

    #[cfg(test)]
    pub(crate) fn search_focus_handle(&self, cx: &App) -> FocusHandle {
        self.search.focus_handle(cx)
    }

    #[cfg(test)]
    pub(crate) fn current_cover_shape(&self) -> CoverShape {
        self.cover_shape
    }

    #[cfg(test)]
    pub(crate) fn current_list_row_size(&self) -> ListRowSize {
        self.list_row_size
    }

    #[cfg(test)]
    pub(crate) fn current_tile_rem(&self) -> f32 {
        self.tile_rem
    }

    #[cfg(test)]
    pub(crate) fn set_sort_for_test(
        &mut self,
        sort: irori_services::LibrarySort,
        cx: &mut Context<Self>,
    ) {
        self.filters
            .update(cx, |m, cx| m.update(|f| f.sort = sort, cx));
    }

    #[cfg(test)]
    pub(crate) fn close_sort_popover_for_test(&mut self, cx: &mut Context<Self>) {
        self.sort_open = false;
        self.sort_more_open = None;
        cx.notify();
    }

    #[cfg(test)]
    pub(crate) fn set_layout_for_test(&mut self, layout: LibraryLayout, cx: &mut Context<Self>) {
        self.layout = layout;
        cx.notify();
    }

    #[cfg(test)]
    pub(crate) fn is_greeting_open(&self) -> bool {
        self.greeting_open
    }

    #[cfg(test)]
    pub(crate) fn greeting_input_focused_for_test(&self, window: &Window, cx: &App) -> bool {
        self.greeting_input
            .as_ref()
            .is_some_and(|(input, _)| input.focus_handle(cx).is_focused(window))
    }

    #[cfg(test)]
    pub(crate) fn greeting_state_for_test(&self) -> (bool, Option<String>) {
        (
            self.ui_prefs.library_greeting,
            self.ui_prefs.greeting_name.clone(),
        )
    }

    /// Move keyboard focus to the search field.
    pub fn focus_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.search.update(cx, |s, cx| s.focus(window, cx));
    }

    /// Whether the active layout currently displays compatibility badges.
    pub(super) fn compat_active(&self) -> bool {
        match self.layout {
            LibraryLayout::Grid => self.ui_prefs.cover_compat_badges,
            LibraryLayout::List => self.ui_prefs.library_list_columns.compat,
            LibraryLayout::Focus => false,
        }
    }

    /// Queue a background ProtonDB / Steam Deck lookup for `game` if not
    /// already cached or queued.
    pub(super) fn request_compat(
        &mut self,
        game: &Arc<GameInfo>,
        allow_installed: bool,
        cx: &mut Context<Self>,
    ) {
        let offline = backend(cx).working_offline();
        if self.compat_was_offline && !offline {
            self.compat.clear_offline_skips();
        }
        self.compat_was_offline = offline;
        if self.compat.request(game, allow_installed) {
            self.pump_compat(cx);
        }
    }

    /// Start up to `compat_badges::MAX_CONCURRENT` compatibility lookups off
    /// the UI thread, checking the disk cache first before fetching ProtonDB
    /// and Steam Deck compatibility.
    fn pump_compat(&mut self, cx: &mut Context<Self>) {
        enum LookupOutcome {
            Done(Option<irori_extras::wiki::SteamCompat>),
            OfflineMiss,
        }
        let (generation, batch) = self.compat.take_next();
        for game in batch {
            let id = game.id();
            let b = backend(cx);
            let game_for_io = game.clone();
            let io_task = irori_ui::state::run_infallible(cx, async move {
                if let Some(cached) = b.cached_compat(&game_for_io).await {
                    return LookupOutcome::Done(cached);
                }
                if cfg!(test) {
                    return LookupOutcome::Done(None);
                }
                if b.working_offline() {
                    return LookupOutcome::OfflineMiss;
                }
                LookupOutcome::Done(b.fetch_compat(&game_for_io).await)
            });
            let id_for_ui = id.clone();
            let ui_task = cx.spawn(async move |this, cx| {
                let outcome = io_task.await.unwrap_or(LookupOutcome::Done(None));
                this.update(cx, |this, cx| {
                    this.compat_tasks.remove(&id_for_ui);
                    let updated = match outcome {
                        LookupOutcome::Done(result) => {
                            this.compat.finish(generation, &id_for_ui, result)
                        }
                        LookupOutcome::OfflineMiss => {
                            this.compat.finish_offline(generation, &id_for_ui)
                        }
                    };
                    if updated {
                        this.pump_compat(cx);
                        cx.notify();
                    }
                })
                .ok();
            });
            self.compat_tasks.insert(id, ui_task);
        }
    }

    /// Cancel all queued and in-flight compatibility badge lookups.
    pub(super) fn cancel_compat(&mut self) {
        self.compat.cancel_all();
        self.compat_tasks.clear();
    }

    /// A file or folder dropped on the library: the Add game dialog filled in
    /// from the file, or the games found in the folder. Only the first of
    /// several dropped paths is added.
    fn on_paths_dropped(
        &mut self,
        paths: &ExternalPaths,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(path) = paths.paths().first().cloned() else {
            return;
        };
        let bundle = cfg!(target_os = "macos")
            && path
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("app"));
        if path.is_dir() && !bundle {
            crate::dialogs::folder::open_add_dropped_folder_dialog(path, window, cx);
        } else {
            crate::dialogs::sideload::open_add_dropped_file_dialog(path, window, cx);
        }
    }

    /// Type to search: a printable key the grid or list doesn't use itself
    /// moves to the search field and adds its character to the query.
    fn type_to_search(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(text) = typed_search_text(&event.keystroke) else {
            return;
        };
        cx.stop_propagation();
        self.search.update(cx, |s, cx| {
            // Puts the caret at the end, where the character goes.
            let query = s.value();
            s.set_value(query, window, cx);
            s.focus(window, cx);
            s.insert(text, window, cx);
        });
    }

    fn open_greeting_popover(
        &mut self,
        current: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let initial = current.to_owned();
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(initial)
                .placeholder(t("library.greeting.name_placeholder"))
        });
        input.update(cx, |s, cx| s.focus(window, cx));
        let sub = cx.subscribe_in(&input, window, |this, _, event, _, cx| match event {
            InputEvent::Change => cx.notify(),
            InputEvent::PressEnter { .. } => this.save_greeting_name(cx),
            _ => {}
        });
        self.greeting_input = Some((input, sub));
        self.greeting_open = true;
        cx.notify();
    }

    fn save_greeting_name(&mut self, cx: &mut Context<Self>) {
        let raw = self
            .greeting_input
            .as_ref()
            .map(|(input, _)| input.read(cx).value().trim().to_string())
            .unwrap_or_default();
        self.greeting_open = false;
        self.greeting_input = None;
        if raw.is_empty() {
            self.ui_prefs.library_greeting = false;
            self.ui_prefs.greeting_name = None;
            persist_ui(cx, |u| {
                u.library_greeting = false;
                u.greeting_name = None;
            });
        } else {
            self.ui_prefs.greeting_name = Some(raw.clone());
            self.ui_prefs.library_greeting = true;
            persist_ui(cx, move |u| {
                u.greeting_name = Some(raw);
                u.library_greeting = true;
            });
        }
        cx.notify();
    }

    fn hide_greeting(&mut self, cx: &mut Context<Self>) {
        self.greeting_open = false;
        self.greeting_input = None;
        self.ui_prefs.library_greeting = false;
        persist_ui(cx, |u| u.library_greeting = false);
        cx.notify();
    }

    // ---- rendering --------------------------------------------------------------

    fn render_header(&mut self, _window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (total, refreshing) = {
            let lib = models(cx).library.read(cx);
            (lib.snapshot.len(), lib.refreshing)
        };
        let installed = self.frame.installed_count;
        let shown = self.frame.games.len();
        let filter = self.filters.read(cx).filter().clone();
        let (shown_str, count_str, installed_str) =
            (shown.to_string(), total.to_string(), installed.to_string());
        // Installing from GOG offline installers needs the GOG library.
        let gog_signed_in = models(cx)
            .accounts
            .read(cx)
            .users
            .get(&Runner::Gog)
            .is_some_and(Option::is_some);
        let greeting_on = self.ui_prefs.library_greeting;
        let description = match (filter.is_narrowed(), total) {
            (_, 0) => t("library.header.no_games"),
            (true, 1) => t_args(
                "library.header.shown_of_total_one",
                &[("shown", &shown_str), ("total", &count_str)],
            ),
            (true, _) => t_args(
                "library.header.shown_of_total_other",
                &[("shown", &shown_str), ("total", &count_str)],
            ),
            (false, 1) if installed > 0 => t_args(
                "library.header.count_installed_one",
                &[("installed", &installed_str)],
            ),
            (false, 1) => t("library.common.games_one"),
            (false, _) if installed > 0 => t_args(
                "library.header.count_installed_other",
                &[("count", &count_str), ("installed", &installed_str)],
            ),
            (false, _) => t_args("library.common.games_other", &[("count", &count_str)]),
        };

        let title_el: AnyElement = if greeting_on {
            let pod = crate::greeting::current_part_of_day();
            let resolved = crate::greeting::resolve_name(self.ui_prefs.greeting_name.as_deref());
            let (before, trigger_label, after, initial_name, underlined) = match resolved {
                Some(name) => {
                    let (before, after) = pod.split_greeting();
                    (before, SharedString::from(name.clone()), after, name, true)
                }
                None => (
                    SharedString::default(),
                    pod.plain_greeting(),
                    SharedString::default(),
                    String::new(),
                    false,
                ),
            };
            let theme = cx.theme();
            let primary = theme.primary;
            let muted = theme.muted_foreground;
            let view = cx.entity().downgrade();
            let open_view = view.clone();
            let save_view = view.clone();
            let cancel_view = view.clone();
            let hide_view = view.clone();
            let input_entity = self.greeting_input.as_ref().map(|(input, _)| input.clone());
            let input_focus = input_entity.as_ref().map(|i| i.focus_handle(cx));
            let popover = Popover::new("library-greeting-popover")
                .open(self.greeting_open)
                .when_some(input_focus, |p, handle| p.track_focus(&handle))
                .on_open_change(move |open, window, cx| {
                    let initial = initial_name.clone();
                    open_view
                        .update(cx, |this, cx| {
                            if *open {
                                this.open_greeting_popover(&initial, window, cx);
                            } else if this.greeting_open {
                                this.greeting_open = false;
                                this.greeting_input = None;
                                cx.notify();
                            }
                        })
                        .ok();
                })
                .trigger(GreetingTrigger {
                    label: trigger_label,
                    primary,
                    underlined,
                    selected: self.greeting_open,
                })
                .content(move |_, _, _| {
                    let save_view = save_view.clone();
                    let cancel_view = cancel_view.clone();
                    let hide_view = hide_view.clone();
                    let input = input_entity.clone();
                    v_flex()
                        .w(rems(19.))
                        .gap_2p5()
                        .child(
                            div()
                                .text_xs()
                                .font_semibold()
                                .text_color(muted)
                                .child(t("library.greeting.name_label")),
                        )
                        .when_some(input, |el, input| el.child(Input::new(&input).small()))
                        .child(
                            h_flex()
                                .w_full()
                                .justify_between()
                                .gap_2()
                                .child(
                                    Button::new("greeting-hide")
                                        .small()
                                        .ghost()
                                        .label(t("library.greeting.hide"))
                                        .on_click(move |_, _, cx| {
                                            hide_view
                                                .update(cx, |this, cx| this.hide_greeting(cx))
                                                .ok();
                                        }),
                                )
                                .child(
                                    h_flex()
                                        .gap_1p5()
                                        .child(
                                            Button::new("greeting-cancel")
                                                .small()
                                                .ghost()
                                                .label(t("library.common.cancel"))
                                                .on_click(move |_, _, cx| {
                                                    cancel_view
                                                        .update(cx, |this, cx| {
                                                            this.greeting_open = false;
                                                            this.greeting_input = None;
                                                            cx.notify();
                                                        })
                                                        .ok();
                                                }),
                                        )
                                        .child(
                                            Button::new("greeting-save")
                                                .small()
                                                .primary()
                                                .label(t("library.common.save"))
                                                .on_click(move |_, _, cx| {
                                                    save_view
                                                        .update(cx, |this, cx| {
                                                            this.save_greeting_name(cx)
                                                        })
                                                        .ok();
                                                }),
                                        ),
                                ),
                        )
                });
            h_flex()
                .items_baseline()
                .text_3xl()
                .font_extrabold()
                .min_w_0()
                .overflow_hidden()
                .when(!before.is_empty(), |el| {
                    el.child(div().flex_shrink_0().child(before))
                })
                .child(div().min_w_0().child(popover))
                .when(!after.is_empty(), |el| {
                    el.child(div().flex_shrink_0().child(after))
                })
                .into_any_element()
        } else {
            div()
                .text_3xl()
                .font_extrabold()
                .truncate()
                .child(t("library.header.title"))
                .into_any_element()
        };

        let actions = h_flex()
            .gap_2()
            .flex_shrink_0()
            .child(
                Button::new("library-refresh")
                    .outline()
                    .icon(IconName::RefreshCw)
                    .tooltip(if refreshing {
                        t("library.header.refreshing")
                    } else {
                        t("library.header.refresh")
                    })
                    .loading(refreshing)
                    .on_click(|_, _, cx| irori_ui::models::refresh_library(false, cx)),
            )
            .child({
                let view = cx.entity().downgrade();
                Button::new("library-add-game")
                    .icon(IconName::Plus)
                    .label(t("library.header.add_game"))
                    .dropdown_caret(true)
                    .dropdown_menu(move |mut menu, _, _| {
                        let view = view.clone();
                        menu = menu
                            .item(
                                PopupMenuItem::new(t("library.header.add_game_item"))
                                    .icon(Icon::new(IconName::Plus))
                                    .on_click(|_, window, cx| {
                                        dialogs::sideload::open_add_game_dialog(window, cx)
                                    }),
                            )
                            .item(
                                PopupMenuItem::new(t("library.sideload.scan_folder"))
                                    .icon(Icon::new(IconName::FolderPlus))
                                    .on_click(|_, window, cx| {
                                        dialogs::folder::open_add_folder_dialog(window, cx)
                                    }),
                            );
                        if gog_signed_in {
                            menu = menu.item(
                                PopupMenuItem::new(t("library.header.import_installers"))
                                    .icon(Icon::new(IconName::FolderInput))
                                    .on_click(move |_, _, cx| {
                                        view.update(cx, |_, cx| {
                                            cx.emit(LibraryEvent::RequestImportInstallers)
                                        })
                                        .ok();
                                    }),
                            );
                        }
                        menu.separator().item(
                            PopupMenuItem::new(t("library.duplicates.open"))
                                .icon(Icon::new(IconName::Copy))
                                .on_click(|_, window, cx| {
                                    dialogs::duplicates::open_duplicates_dialog(window, cx)
                                }),
                        )
                    })
                    .into_any_element()
            });

        let header = h_flex()
            .w_full()
            .gap_4()
            .justify_between()
            .child(
                v_flex().gap_1().min_w_0().child(title_el).child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .truncate()
                        .child(description),
                ),
            )
            .child(actions);

        v_flex()
            .gap_4()
            .px(rems(grid::EDGE_REM))
            .pt_5()
            .pb_3()
            .child(header)
            .children(self.render_steam_hint(cx))
            .when(!self.frame.toolbar_in_list, |el| {
                el.child(self.render_toolbar_block(&filter, cx))
            })
            .child(irori_ui::tour::mark(
                irori_ui::tour::anchors::LIBRARY_HEADER,
            ))
    }

    /// A quiet suggestion, while it would help, to read the whole Steam
    /// library from the Steam client (only installed Steam games show
    /// without a Steam account).
    fn render_steam_hint(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !irori_ui::hooks::steam_library_hint_shown(cx) {
            return None;
        }
        let theme = cx.theme();
        Some(
            h_flex()
                .id("library-steam-hint")
                .gap_3()
                .px_3()
                .py_2()
                .rounded(theme.radius)
                .border_1()
                .border_color(theme.border)
                .bg(theme.group_box)
                .child(
                    Icon::new(IconName::Info)
                        .small()
                        .text_color(theme.muted_foreground)
                        .flex_shrink_0(),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_sm()
                        .child(t("library.steam_hint.text")),
                )
                .child(
                    Button::new("library-steam-hint-read")
                        .small()
                        .outline()
                        .label(t("library.steam_hint.action"))
                        .on_click(|_, window, cx| {
                            irori_ui::hooks::open_steam_library_hint(window, cx)
                        }),
                )
                .child(
                    Button::new("library-steam-hint-dismiss")
                        .small()
                        .ghost()
                        .icon(IconName::X)
                        .tooltip(t("library.steam_hint.dismiss"))
                        .on_click(cx.listener(|_, _, _, cx| {
                            irori_ui::hooks::dismiss_steam_library_hint(cx);
                            cx.notify();
                        })),
                )
                .into_any_element(),
        )
    }

    fn render_body(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let (total, refreshing) = {
            let lib = models(cx).library.read(cx);
            (lib.snapshot.len(), lib.refreshing)
        };
        if total == 0 {
            if refreshing {
                return v_flex()
                    .size_full()
                    .items_center()
                    .justify_center()
                    .gap_3()
                    .child(Spinner::new().large())
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child(t("library.body.loading")),
                    )
                    .into_any_element();
            }
            return self.render_empty(window, cx);
        }

        if self.frame.games.is_empty() {
            let filter = self.filters.read(cx).filter().clone();
            let trimmed_query = filter.query.trim();
            let (icon, title, description) = if !trimmed_query.is_empty() {
                (
                    IconName::SearchX,
                    t("library.no_match.search_title"),
                    t_args(
                        "library.no_match.search_description",
                        &[("query", trimmed_query)],
                    ),
                )
            } else if filter.favourites_only && filter.active_count() == 1 {
                (
                    IconName::Star,
                    t("library.no_match.favourites_title"),
                    t("library.no_match.favourites_description"),
                )
            } else {
                (
                    IconName::SearchX,
                    t("library.no_match.title"),
                    t_args(
                        "library.no_match.description",
                        &[(
                            "filters",
                            &filter
                                .describe(&models(cx).library.read(cx).user)
                                .join(" · "),
                        )],
                    ),
                )
            };
            return EmptyState::new(icon, title)
                .description(description)
                .action(
                    Button::new("clear-filters")
                        .icon(IconName::X)
                        .label(t("library.no_match.clear"))
                        .on_click(
                            cx.listener(|this, _, window, cx| this.clear_filters(window, cx)),
                        ),
                )
                .into_any_element();
        }

        let view = cx.entity().downgrade();
        let measure = canvas(
            move |bounds, _, cx| {
                let size = bounds.size;
                view.update(cx, |this, cx| {
                    if (this.viewport.width - size.width).abs() > px(0.5)
                        || (this.viewport.height - size.height).abs() > px(0.5)
                    {
                        this.viewport = size;
                        cx.notify();
                    }
                })
                .ok();
            },
            |_, _, _, _| {},
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full();

        let list = div()
            .size_full()
            .on_scroll_wheel(cx.listener(|this, _, _, cx| {
                if this.filters_open
                    || this.look_open
                    || this.sort_open
                    || this.tabs_manager_open
                    || this.greeting_open
                {
                    if this.filters_open {
                        this.filters_open = false;
                        this.filters_panel
                            .update(cx, |panel, _| panel.reset_more_open());
                    }
                    this.look_open = false;
                    if this.sort_open {
                        this.sort_open = false;
                        this.sort_more_open = None;
                    }
                    this.tabs_manager_open = false;
                    this.greeting_open = false;
                    this.greeting_input = None;
                    cx.notify();
                }
            }))
            .child(
                v_virtual_list(
                    cx.entity(),
                    "library-items",
                    self.frame.sizes.clone(),
                    |this, range, window, cx| this.render_items(range, window, cx),
                )
                .track_scroll(&self.scroll)
                .size_full(),
            );

        let layout = self.layout;
        let list_toolbar = self.frame.toolbar_in_list.then(|| {
            let filter = self.filters.read(cx).filter().clone();
            self.render_list_toolbar(&filter, window, cx)
        });
        let content =
            div()
                .image_cache(self.images.clone())
                .id("library-content")
                .key_context(GRID_CONTEXT)
                .track_focus(&self.grid_focus)
                .on_key_down(cx.listener(Self::type_to_search))
                .on_action(
                    cx.listener(|this, _: &SelectLeft, w, cx| this.move_focus(Move::Left, w, cx)),
                )
                .on_action(
                    cx.listener(|this, _: &SelectRight, w, cx| this.move_focus(Move::Right, w, cx)),
                )
                .on_action(
                    cx.listener(|this, _: &SelectUp, w, cx| this.move_focus(Move::Up, w, cx)),
                )
                .on_action(
                    cx.listener(|this, _: &SelectDown, w, cx| this.move_focus(Move::Down, w, cx)),
                )
                .on_action(
                    cx.listener(|this, _: &SelectFirst, w, cx| this.move_focus(Move::First, w, cx)),
                )
                .on_action(
                    cx.listener(|this, _: &SelectLast, w, cx| this.move_focus(Move::Last, w, cx)),
                )
                .on_action(cx.listener(|this, _: &SelectPageUp, w, cx| {
                    this.move_focus(Move::PageUp(1), w, cx)
                }))
                .on_action(cx.listener(|this, _: &SelectPageDown, w, cx| {
                    this.move_focus(Move::PageDown(1), w, cx)
                }))
                .on_action(cx.listener(|this, _: &ExtendLeft, w, cx| {
                    this.step_focus(Move::Left, true, w, cx)
                }))
                .on_action(cx.listener(|this, _: &ExtendRight, w, cx| {
                    this.step_focus(Move::Right, true, w, cx)
                }))
                .on_action(
                    cx.listener(|this, _: &ExtendUp, w, cx| this.step_focus(Move::Up, true, w, cx)),
                )
                .on_action(cx.listener(|this, _: &ExtendDown, w, cx| {
                    this.step_focus(Move::Down, true, w, cx)
                }))
                .on_action(cx.listener(|this, _: &ExtendFirst, w, cx| {
                    this.step_focus(Move::First, true, w, cx)
                }))
                .on_action(cx.listener(|this, _: &ExtendLast, w, cx| {
                    this.step_focus(Move::Last, true, w, cx)
                }))
                .on_action(cx.listener(|this, _: &ExtendPageUp, w, cx| {
                    this.step_focus(Move::PageUp(1), true, w, cx)
                }))
                .on_action(cx.listener(|this, _: &ExtendPageDown, w, cx| {
                    this.step_focus(Move::PageDown(1), true, w, cx)
                }))
                .on_action(cx.listener(|this, _: &PickFocused, w, cx| this.pick_focused(w, cx)))
                .on_action(cx.listener(|this, _: &OpenSelected, w, cx| this.open_focused(w, cx)))
                .on_action(cx.listener(|this, _: &ToggleDrawer, w, cx| this.toggle_drawer(w, cx)))
                .on_action(cx.listener(|this, _: &SelectAllGames, _, cx| this.select_all(cx)))
                .on_action(cx.listener(|this, _: &RemovePicked, w, cx| this.remove_picked(w, cx)))
                .on_action(cx.listener(|this, _: &EndSelection, _, cx| this.end_selection(cx)))
                .on_action(
                    cx.listener(|this, _: &ActivateSelected, w, cx| this.activate_focused(w, cx)),
                )
                .relative()
                .h_full()
                .min_h_0()
                .map(|el| match layout {
                    LibraryLayout::Focus => el.w(rems(self.frame.focus_rem)).flex_shrink_0(),
                    _ => el.flex_1().min_w_0(),
                })
                .child(list)
                .vertical_scrollbar(&self.scroll)
                .children(list_toolbar)
                .child(irori_ui::tour::mark(irori_ui::tour::anchors::LIBRARY_GAMES))
                .when(layout == LibraryLayout::Focus, |el| {
                    el.child(self.render_focus_handle(cx))
                });
        let drawer = (layout == LibraryLayout::Grid && self.panels.drawer_open)
            .then(|| self.render_drawer(window, cx));
        let focus_page = (layout == LibraryLayout::Focus).then(|| self.render_focus_page(cx));
        h_flex()
            .id("library-body")
            .relative()
            .flex_1()
            .min_h_0()
            .w_full()
            .items_start()
            .on_drag_move(cx.listener(Self::on_drawer_drag))
            .on_drag_move(cx.listener(Self::on_focus_drag))
            .child(measure)
            .child(content)
            .children(focus_page)
            .children(drawer)
            .into_any_element()
    }

    /// The background the user chose behind the covers: the focused game's
    /// art, else the last played game's, or their picture.
    fn render_backdrop(&self, cx: &mut Context<Self>) -> Option<impl IntoElement + use<>> {
        let prefs = &self.ui_prefs;
        let game = match prefs.library_background {
            LibraryBackground::None => return None,
            LibraryBackground::GameArt => {
                let b = backend(cx);
                self.focused
                    .as_ref()
                    .and_then(|id| b.game(id))
                    .or_else(|| b.recent_games(false).into_iter().next())
                    // Nothing focused or played yet: the first game shown.
                    .or_else(|| self.frame.games.first().cloned())
            }
            LibraryBackground::Picture => None,
        };
        let url = irori_ui::components::library_backdrop_url(prefs, game.as_deref(), cx)?;
        Some(irori_ui::components::library_backdrop(prefs, Some(url)).image_cache(&self.images))
    }
}

impl Render for LibraryView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.prepare_frame(window, cx);
        self.sync_panel(window, cx);
        let bulk_bar = self.render_bulk_bar(cx);
        // Focus sits over the selected game's art, covers and page alike; the
        // other layouts show the background the user chose.
        let focus_backdrop = (self.layout == LibraryLayout::Focus)
            .then(|| {
                let id = self.panels.shown_game().or(self.focused.as_ref())?;
                let art = self.game(id, cx)?.hero_art().map(str::to_owned);
                Some(ArtBackdrop::new(art).image_cache(&self.images))
            })
            .flatten();
        let backdrop = if self.layout == LibraryLayout::Focus {
            None
        } else {
            self.render_backdrop(cx)
        };
        v_flex()
            .id("library")
            .relative()
            .children(backdrop)
            .key_context(LIBRARY_CONTEXT)
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(|this, _: &ShowGrid, w, cx| {
                this.on_show_layout(LibraryLayout::Grid, w, cx)
            }))
            .on_action(cx.listener(|this, _: &ShowList, w, cx| {
                this.on_show_layout(LibraryLayout::List, w, cx)
            }))
            .on_action(cx.listener(|this, _: &ShowFocus, w, cx| {
                this.on_show_layout(LibraryLayout::Focus, w, cx)
            }))
            .on_action(cx.listener(|this, _: &NextLibraryView, w, cx| {
                this.on_show_layout(layouts::next_layout(this.layout), w, cx)
            }))
            .on_action(
                cx.listener(|this, _: &FocusSearch, window, cx| this.focus_search(window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &ToggleStatusGroups, _, cx| this.toggle_status_groups(cx)),
            )
            .on_action(cx.listener(|this, _: &ToggleModeGroups, _, cx| this.toggle_mode_groups(cx)))
            .on_drop(cx.listener(Self::on_paths_dropped))
            .size_full()
            .min_h_0()
            .text_color(cx.theme().foreground)
            .children(focus_backdrop)
            .child(self.render_header(window, cx))
            .child(self.render_body(window, cx))
            .children(bulk_bar)
            .child(
                // Shows where a dragged file lands; it takes no clicks.
                div()
                    .id("library-drop-target")
                    .absolute()
                    .inset_0()
                    .drag_over::<ExternalPaths>(|style, _, _, cx| style.bg(cx.theme().drop_target)),
            )
    }
}

/// The id, icon and name of a view's button in the header's switch.
fn layout_button(layout: LibraryLayout) -> (&'static str, IconName, SharedString) {
    match layout {
        LibraryLayout::Grid => (
            "layout-grid",
            IconName::LayoutGrid,
            t("library.toolbar.grid"),
        ),
        LibraryLayout::List => ("layout-list", IconName::List, t("library.toolbar.list")),
        LibraryLayout::Focus => (
            "layout-focus",
            IconName::LayoutPanelLeft,
            t("library.toolbar.focus"),
        ),
    }
}

fn slider_value(v: &SliderValue) -> f32 {
    v.start().clamp(grid::MIN_TILE_REM, grid::MAX_TILE_REM)
}

/// Whether covers carry store badges: when they are on, and not while the
/// library lists one store, where every badge would say the same, unless
/// the user wants them there too.
fn store_badges_shown(ui: &UiPrefs, one_store: bool) -> bool {
    ui.show_store_badges && (!one_store || ui.store_badges_one_store)
}

/// Persist a UI preference off the UI thread.
fn persist_ui(cx: &App, f: impl FnOnce(&mut irori_core::settings::UiPrefs) + Send + 'static) {
    let b = backend(cx);
    irori_ui::state::run_infallible(cx, async move { b.settings.update_app(|s| f(&mut s.ui)) })
        .detach();
}

/// Label of a running operation: "Installing 42%".
fn status_text(status: &GameStatus) -> String {
    let label = crate::i18n::status_label(status.kind);
    match status.progress.and_then(|p| p.percent) {
        Some(pc) if status.kind != StatusKind::Playing => t_args(
            "library.status.progress",
            &[("status", &label), ("percent", &format!("{pc:.0}"))],
        )
        .to_string(),
        _ => label,
    }
}

/// Secondary line under a game's title.
fn caption(game: &GameInfo, state: &GameState, status: Option<&GameStatus>) -> String {
    let mut parts: Vec<String> = Vec::new();
    let text = |key: &str| t(key).to_string();
    if state.hidden {
        parts.push(text("library.caption.hidden"));
    }
    match status {
        Some(s) if s.kind.is_busy() => parts.push(status_text(s)),
        _ if state.unavailable => parts.push(text("library.caption.files_missing")),
        _ if state.sideload && !state.installed && !state.browser => {
            parts.push(text("library.caption.program_missing"))
        }
        // Installed games stand out by their full-colour cover; the caption
        // only names what needs attention.
        _ if state.installed => {
            if state.has_update && !state.sideload {
                parts.push(text("library.caption.update_available"));
            }
        }
        _ => match &game.third_party_managed_app {
            Some(app) => {
                parts.push(t_args("library.caption.needs_launcher", &[("app", app)]).to_string())
            }
            None if !game.installable && !state.sideload => {
                parts.push(text("library.caption.not_installable"))
            }
            None => {}
        },
    }
    parts.join(" · ")
}

/// The text a key press types into the search field when the grid or list
/// has focus: a printable character without Ctrl, Cmd or Fn (shortcuts),
/// and not whitespace, which starts nothing.
pub(crate) fn typed_search_text(keystroke: &Keystroke) -> Option<String> {
    let m = &keystroke.modifiers;
    if m.control || m.platform || m.function {
        return None;
    }
    let text = keystroke.key_char.as_deref()?;
    (!text.is_empty() && text.chars().all(|c| !c.is_control() && !c.is_whitespace()))
        .then(|| text.to_string())
}

/// Clickable name (or plain greeting) trigger for the library greeting popover.
#[derive(IntoElement)]
struct GreetingTrigger {
    label: SharedString,
    primary: Hsla,
    underlined: bool,
    selected: bool,
}

impl Selectable for GreetingTrigger {
    fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    fn is_selected(&self) -> bool {
        self.selected
    }
}

impl RenderOnce for GreetingTrigger {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let primary = self.primary;
        div()
            .id("library-greeting-name")
            .debug_selector(|| "library-greeting-name".into())
            .min_w_0()
            .truncate()
            .cursor_pointer()
            .when(self.underlined, |el| {
                el.border_b_2().border_color(if self.selected {
                    primary
                } else {
                    primary.opacity(0.55)
                })
            })
            .when(self.selected, |el| el.text_color(primary))
            .hover(move |s| s.text_color(primary).border_color(primary))
            .child(self.label)
    }
}

#[cfg(test)]
mod type_to_search_tests {
    use super::*;

    #[test]
    fn store_badges_leave_a_one_store_library_unless_asked() {
        let mut ui = UiPrefs::default();
        assert!(store_badges_shown(&ui, false));
        assert!(!store_badges_shown(&ui, true));
        ui.store_badges_one_store = true;
        assert!(store_badges_shown(&ui, true));
        ui.show_store_badges = false;
        assert!(!store_badges_shown(&ui, false));
    }

    #[test]
    fn only_plain_printable_keys_start_a_search() {
        let key = |s: &str| typed_search_text(&Keystroke::parse(s).unwrap().with_simulated_ime());
        assert_eq!(key("h").as_deref(), Some("h"));
        assert_eq!(key("shift-h").as_deref(), Some("H"));
        assert_eq!(key("ctrl-h"), None);
        assert_eq!(key("cmd-h"), None);
        assert_eq!(key("space"), None);
        assert_eq!(key("enter"), None);
        assert_eq!(key("left"), None);
        let accented = Keystroke {
            key: "e".into(),
            key_char: Some("é".into()),
            modifiers: Default::default(),
        };
        assert_eq!(typed_search_text(&accented).as_deref(), Some("é"));
    }
}
