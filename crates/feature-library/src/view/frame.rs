//! Frame derivation, virtual-list layout items, and per-game state and
//! availability checks.

use std::collections::HashSet;
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::{App, Context, Pixels, ScrollStrategy, SharedString, Size, Window, px, size};
use irori_core::settings::LibraryLayout;
use irori_core::{GameId, GameInfo, InstallPlatform, Runner};
use irori_services::StoreFilter;
use irori_ui::i18n::t;
use irori_ui::{backend, models};

use super::{CARD_REM, FAVOURITE_REM, LibraryView, toolbar};
use crate::availability;
use crate::commands::GameState;
use crate::filter::{self, GroupKind, HiddenMode, StatusGroup};
use crate::grid::{self, GridMetrics};
use crate::layouts;
use crate::top::TopKind;

/// How long a checked shortcut state is trusted before it is checked again.
const SHORTCUT_TTL: Duration = Duration::from_secs(30);

/// One entry of the virtual list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Item {
    Top,
    /// Room for the toolbar under the top row (see `toolbar.rs`).
    Toolbar,
    Heading,
    /// The heading of `Frame::groups[ix]`.
    GroupHeading(usize),
    ListHeader,
    /// The games at `start..end` of the shown list.
    GridRow {
        start: usize,
        end: usize,
    },
    ListRow(usize),
}

#[derive(Debug, Clone, PartialEq)]
struct FrameKey {
    count: usize,
    top: Option<(TopKind, usize)>,
    groups: Vec<(GroupKind, usize)>,
    layout: LibraryLayout,
    /// Width of the region the covers or rows fill, in rem.
    list_rem: u32,
    metrics: GridMetrics,
    rem: Pixels,
    width: Pixels,
    /// Height of the toolbar's room in the list, in whole pixels; zero
    /// when the toolbar is in the header.
    toolbar: u32,
}

/// Everything derived for the current frame. Rebuilt only when its inputs
/// change (the game list itself is cached in `FilteredGames`).
pub(super) struct Frame {
    pub(super) games: Arc<[Arc<GameInfo>]>,
    /// Sections of `games`; empty when the list is not grouped.
    pub(super) groups: Arc<[StatusGroup]>,
    pub(super) top: Option<(TopKind, Arc<[Arc<GameInfo>]>)>,
    pub(super) stores: Vec<Runner>,
    /// Title of the main section under the top row.
    pub(super) heading: SharedString,
    pub(super) metrics: GridMetrics,
    /// Which of `grid::LIST_COLUMNS` fit the width.
    pub(super) list_columns: [bool; 7],
    /// Width of the Focus view's column of covers, in rem.
    pub(super) focus_rem: f32,
    pub(super) items: Rc<Vec<Item>>,
    pub(super) sizes: Rc<Vec<Size<Pixels>>>,
    key: Option<FrameKey>,
    /// The library lists one store's games, so store badges would all
    /// say the same.
    pub(super) one_store: bool,
    /// The toolbar sits in the list under the top row, not in the header.
    pub(super) toolbar_in_list: bool,
    /// Cached counts for the preset tabs and narrowing saved views.
    pub(super) chip_counts: crate::chips::ChipCountMap,
}

impl Default for Frame {
    fn default() -> Self {
        Self {
            games: Arc::from(Vec::new()),
            groups: Arc::from(Vec::new()),
            top: None,
            stores: Vec::new(),
            heading: SharedString::default(),
            metrics: grid::grid_metrics(60.0, 11.0),
            list_columns: [true; 7],
            focus_rem: layouts::focus_column_rem(2, layouts::FOCUS_TILE_REM),
            items: Rc::default(),
            sizes: Rc::default(),
            one_store: false,
            key: None,
            toolbar_in_list: false,
            chip_counts: Default::default(),
        }
    }
}

impl LibraryView {
    pub(super) fn prepare_frame(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let m = models(cx).clone();
        let (snapshot, user, updates, categories) = {
            let lib = m.library.read(cx);
            (
                lib.snapshot.clone(),
                lib.user.clone(),
                lib.updates.clone(),
                lib.categories.clone(),
            )
        };
        let app = m.settings.read(cx).app.clone();
        let stores = {
            let accounts = m.accounts.read(cx);
            let with_games = self
                .stores_with_games
                .get(&snapshot, filter::stores_with_games);
            filter::offered_stores(with_games, &|r| accounts.is_logged_in(r), &app.ui.stores)
        };
        let filter = self.filters.read(cx).filter().clone();
        self.check_availability(&snapshot, cx);
        let missing = self.availability.missing().clone();

        let lang = irori_ui::i18n::language();
        if self.search_placeholder_lang.as_deref() != Some(lang.as_str()) {
            let placeholder = t("library.search.placeholder");
            self.search
                .update(cx, |s, cx| s.set_placeholder(placeholder, window, cx));
            self.search_placeholder_lang = Some(lang);
        }

        if std::mem::take(&mut self.tile_slider_dirty) {
            let size = self.tile_rem;
            self.tile_slider
                .update(cx, |s, cx| s.set_value(size, window, cx));
        }

        if self.ui_prefs.library_quick_controls {
            let smart_ids = crate::views::smart_collection_ids(&user);
            let inputs = crate::chips::ChipCountInputs {
                smart_ids: &smart_ids,
                stores: &stores,
                library: &snapshot,
                user: &user,
                categories: &categories,
                updates: &updates,
                missing: &missing,
            };
            self.frame.chip_counts = self.chip_counts.get(&inputs).clone();
        }

        let games = self.games.get_in(
            &filter,
            &stores,
            &snapshot,
            &user,
            &categories,
            &updates,
            &missing,
        );
        let groups = self.games.groups();
        if filter.group != self.grouping {
            if self.frame.key.is_some() {
                self.scroll.scroll_to_item(0, ScrollStrategy::Top);
            }
            self.grouping = filter.group;
        }
        // Focus keeps to the covers; its page says the rest.
        let top = if filter.query.trim().is_empty()
            && filter.hidden != HiddenMode::Only
            && self.layout != LibraryLayout::Focus
        {
            self.top.get(
                app.library_top_section,
                app.max_recent_games as usize,
                filter.hidden == HiddenMode::Show && !snapshot.hidden_locked,
                &snapshot,
                &user,
            )
        } else {
            None
        };

        let rem = window.rem_size();
        let width = if self.viewport.width > px(0.) {
            self.viewport.width
        } else {
            irori_ui::style::visible_size(window).width
        };
        let height = if self.viewport.height > px(0.) {
            self.viewport.height
        } else {
            irori_ui::style::visible_size(window).height
        };
        // The measured width is the library's whole body; the drawer and the
        // Focus page take their share of it.
        let body_rem = f32::from(width) / f32::from(rem);
        let focus_tile = layouts::focus_tile_rem(self.focus_cover_size);
        // Following the window's width switches the view for now; the
        // user's own choice stays what is saved.
        if self.ui_prefs.library_auto_layout {
            let wide = layouts::wide_enough_for_focus(body_rem, focus_tile);
            if let Some(layout) = layouts::auto_layout(self.layout, self.auto_wide, wide) {
                self.layout = layout;
                if !self.compat_active() {
                    self.cancel_compat();
                }
                self.scroll_to_focused();
                // The rows above were worked out for the other view.
                cx.notify();
            }
            self.auto_wide = Some(wide);
        }
        let focus_rem = layouts::focus_column_rem(
            layouts::focus_columns_for(self.ui_prefs.library_focus_columns, body_rem, focus_tile),
            focus_tile,
        );
        let list_rem = match self.layout {
            LibraryLayout::Grid if self.panels.drawer_open => {
                body_rem - layouts::drawer_width(self.panels.drawer_rem, body_rem)
            }
            LibraryLayout::Focus => focus_rem,
            _ => body_rem,
        };
        let available = list_rem - 2.0 * grid::EDGE_REM;
        let caption = self.ui_prefs.cover_caption;
        let mut metrics = match self.layout {
            LibraryLayout::Focus => grid::focus_metrics(available, focus_tile, caption),
            _ => grid::grid_metrics_shaped(
                available,
                self.tile_rem,
                self.density,
                caption,
                self.cover_shape,
            ),
        };
        metrics.list_row_rem = grid::list_row_rem(self.list_row_size, self.density);
        // The top row isn't filtered, so the controls that filter sit under
        // it; without one (or with nothing to show) they stay in the header.
        let toolbar_in_list = top.is_some() && !games.is_empty();
        let toolbar_height = self.toolbar_height(rem);

        let key = FrameKey {
            count: games.len(),
            top: top.as_ref().map(|(k, g)| (*k, g.len())),
            groups: groups
                .iter()
                .map(|g| (g.kind.clone(), g.range.len()))
                .collect(),
            layout: self.layout,
            list_rem: (list_rem * 16.0).round() as u32,
            metrics,
            rem,
            width,
            toolbar: if toolbar_in_list {
                f32::from(toolbar_height).round() as u32
            } else {
                0
            },
        };
        if self.frame.key.as_ref() != Some(&key) {
            let mut items = Vec::new();
            if top.is_some() {
                items.push(Item::Top);
                if toolbar_in_list {
                    items.push(Item::Toolbar);
                }
                // Grouped lists head each section instead.
                if groups.is_empty() {
                    items.push(Item::Heading);
                }
            }
            if self.layout == LibraryLayout::List {
                items.push(Item::ListHeader);
            }
            for (ix, range) in section_ranges(&groups, games.len()).into_iter().enumerate() {
                if !groups.is_empty() {
                    items.push(Item::GroupHeading(ix));
                }
                match self.layout {
                    LibraryLayout::Grid | LibraryLayout::Focus => {
                        items.extend(range.clone().step_by(metrics.columns.max(1)).map(|start| {
                            Item::GridRow {
                                start,
                                end: (start + metrics.columns).min(range.end),
                            }
                        }))
                    }
                    LibraryLayout::List => items.extend(range.map(Item::ListRow)),
                }
            }
            let top_kind = top.as_ref().map(|(k, _)| *k);
            let sizes = items
                .iter()
                .map(|item| match item {
                    Item::Toolbar => size(width, toolbar_height),
                    _ => size(width, rem * item_rem(*item, top_kind, &metrics)),
                })
                .collect();
            self.frame.items = Rc::new(items);
            self.frame.sizes = Rc::new(sizes);
            self.frame.key = Some(key);
        }
        // Picks a filter or search now hides are dropped: bulk actions only
        // ever touch games on screen.
        if !Arc::ptr_eq(&self.frame.games, &games) {
            self.selection.keep_shown(&games);
        }
        self.frame.games = games;
        self.frame.groups = groups;
        self.frame.top = top;
        self.frame.toolbar_in_list = toolbar_in_list;
        self.frame.stores = stores;
        self.frame.one_store = matches!(filter.store, StoreFilter::Store(_));
        self.frame.heading = if filter.has_categories() {
            filter.category_heading(&user).into()
        } else if filter.favourites_only {
            t("library.heading.favourites")
        } else {
            t("library.heading.all")
        };
        self.frame.metrics = metrics;
        self.frame.list_columns = grid::list_columns_for(
            available,
            self.ui_prefs.library_list_columns,
            self.list_row_size,
        );
        self.frame.focus_rem = focus_rem;
        self.follow_selection();
        if std::mem::take(&mut self.scroll_pending) {
            // A new view keeps the selected game in view, else it opens at
            // its top: the old view's offset means nothing in its rows.
            match self.focused_ix.and_then(|ix| self.item_of_game(ix)) {
                Some(item) => self.scroll.scroll_to_item(item, ScrollStrategy::Center),
                None => self.scroll.scroll_to_item(0, ScrollStrategy::Top),
            }
        }

        // Continue-playing cards show art and a logo each.
        let top_images = self
            .frame
            .top
            .as_ref()
            .map_or(0, |(_, games)| 2 * games.len());
        let (per_row, row_rem) = match self.layout {
            LibraryLayout::Grid | LibraryLayout::Focus => (metrics.columns, metrics.row_rem),
            LibraryLayout::List => (1, metrics.list_row_rem),
        };
        let capacity = grid::cached_images(
            per_row,
            row_rem,
            f32::from(height) / f32::from(rem),
            top_images,
        );
        if capacity != self.image_capacity {
            self.image_capacity = capacity;
            self.images
                .update(cx, |images, cx| images.set_capacity(capacity, cx));
        }
    }

    pub(super) fn columns(&self) -> usize {
        match self.layout {
            LibraryLayout::Grid | LibraryLayout::Focus => self.frame.metrics.columns,
            LibraryLayout::List => 1,
        }
    }

    /// Keep the selection on a shown game after the list changed: Focus
    /// always shows one, the drawer closes when its game went.
    fn follow_selection(&mut self) {
        let len = self.frame.games.len();
        let found = self.focused.as_ref().and_then(|id| self.games.position(id));
        let always = self.layout == LibraryLayout::Focus;
        match layouts::keep_selection(found, self.focused_ix, len, always) {
            Some(ix) => {
                self.focused_ix = Some(ix);
                if found != Some(ix) {
                    self.focused = Some(self.frame.games[ix].id());
                }
            }
            // The grid's keyboard place is kept for when the game shows again;
            // its drawer closes.
            None => self.panels.drawer_open = false,
        }
    }

    /// Virtual-list index of the row holding the game at `game_ix`.
    pub(super) fn item_of_game(&self, game_ix: usize) -> Option<usize> {
        self.frame.items.iter().position(|item| match item {
            Item::GridRow { start, end } => (*start..*end).contains(&game_ix),
            Item::ListRow(i) => *i == game_ix,
            _ => false,
        })
    }

    /// Ranges of the shown list that start a new row.
    pub(super) fn sections(&self) -> Vec<Range<usize>> {
        section_ranges(&self.frame.groups, self.frame.games.len())
    }

    // ---- state and commands ---------------------------------------------

    pub(super) fn state_for(&self, game: &GameInfo, in_recent: bool, cx: &App) -> GameState {
        let m = models(cx);
        let lib = m.library.read(cx);
        let id = game.id();
        GameState {
            installed: game.is_installed,
            installable: game.installable,
            sideload: id.runner == Runner::Sideload,
            browser: game.browser_url.is_some()
                || game.installed_platform() == Some(InstallPlatform::Browser),
            external: !id.runner.manages_installs(),
            client: id.runner == Runner::Steam,
            status: m.statuses.read(cx).kind(&id),
            has_update: lib.has_update(&id),
            favourite: lib.is_favourite(&id),
            hidden: lib.is_hidden(&id),
            has_install_path: game.install_path().is_some(),
            in_recent,
            launching: self.launching.contains(&id),
            shortcut: self.shortcuts.get(&id).map(|(exists, _)| *exists),
            unavailable: game.is_installed && self.availability.missing().contains(&id),
            rom: id.runner == Runner::Emulation,
        }
    }

    /// Start a background availability check when the library changed.
    fn check_availability(
        &mut self,
        snapshot: &irori_services::LibrarySnapshot,
        cx: &mut Context<Self>,
    ) {
        let revision = snapshot.revision;
        if !self.availability.start(revision) {
            return;
        }
        let ids = availability::candidates(snapshot);
        let b = backend(cx);
        let task = irori_ui::state::run_infallible(cx, async move {
            let mut missing = HashSet::new();
            for id in ids {
                if let Some(store) = b.store(id.runner)
                    && !store.is_available(&id.app_name).await
                {
                    missing.insert(id);
                }
            }
            missing
        });
        cx.spawn(async move |this, cx| {
            let missing = task.await.unwrap_or_default();
            this.update(cx, |this, cx| {
                if this.availability.finish(revision, missing) {
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// Check whether a game has shortcuts, unless a recent answer is known.
    pub(super) fn check_shortcut(&mut self, id: &GameId, cx: &mut Context<Self>) {
        if self
            .shortcuts
            .get(id)
            .is_some_and(|(_, at)| at.elapsed() < SHORTCUT_TTL)
        {
            return;
        }
        let Some(game) = self.game(id, cx).filter(|g| g.is_installed) else {
            return;
        };
        if !self.shortcut_checks.insert(id.clone()) {
            return;
        }
        let task = irori_ui::state::run_infallible(cx, async move {
            irori_extras::shortcuts::exists(&game).await
        });
        let id = id.clone();
        cx.spawn(async move |this, cx| {
            let exists = task.await;
            this.update(cx, |this, _| {
                this.shortcut_checks.remove(&id);
                if let Some(exists) = exists {
                    this.shortcuts.insert(id, (exists, Instant::now()));
                }
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn game(&self, id: &GameId, cx: &App) -> Option<Arc<GameInfo>> {
        models(cx).library.read(cx).snapshot.get(id).cloned()
    }
}

/// The sections of a list of `len` games, or the whole list when it is not
/// grouped.
fn section_ranges(groups: &[StatusGroup], len: usize) -> Vec<Range<usize>> {
    if groups.is_empty() {
        let whole = 0..len;
        vec![whole]
    } else {
        groups.iter().map(|g| g.range.clone()).collect()
    }
}

/// Height of a virtual-list item, in rem.
pub(super) fn item_rem(item: Item, top: Option<TopKind>, metrics: &GridMetrics) -> f32 {
    match item {
        Item::Top => {
            // Favourites carry their title below the cover; continue-playing
            // cards carry it on the art.
            let (art, caption) = match top {
                Some(TopKind::Favourites) => (FAVOURITE_REM * grid::COVER_RATIO, grid::CAPTION_REM),
                _ => (CARD_REM * 9.0 / 16.0, 0.0),
            };
            grid::HEADING_REM + grid::ROW_INSET_REM + art + caption + grid::GAP_REM
        }
        Item::Heading | Item::GroupHeading(_) => grid::HEADING_REM,
        Item::Toolbar => toolbar::TOOLBAR_REM,
        Item::ListHeader => 2.25,
        Item::GridRow { .. } => metrics.row_rem,
        Item::ListRow(_) => metrics.list_row_rem,
    }
}
