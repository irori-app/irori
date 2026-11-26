//! The library's view criteria as an observable model, and the Filters
//! popover that edits them. A search field at its top finds any filter
//! value by name ([`crate::value_search`]), with how many games carry it.

use std::cell::Cell;
use std::collections::HashSet;
use std::rc::Rc;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, Sizable as _, StyledExt as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, Bounds, Context, Entity, Global, Hsla,
    InteractiveElement as _, IntoElement, ParentElement as _, Pixels, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, Subscription, Task, WeakEntity, Window, div,
    rems, transparent_black,
};
use irori_core::{GameMode, InstallPlatform, Runner};
use irori_services::{EffectiveStatus, StoreFilter};
use irori_ui::i18n::{t, t_args};
use irori_ui::style::{KeyboardFocusable as _, TEXT_CONTRAST, legible};
use irori_ui::{models, play_status};

use crate::filter::{
    self, AccountFilter, CategoryFilter, Company, HiddenMode, InstallFilter, ReleaseWindow,
    ViewFilter,
};
use crate::value_search::{self, Section, ValueCache};

/// Owner of the library's [`ViewFilter`]. Persists every change except the
/// search query, off the UI thread.
pub struct FilterModel {
    filter: ViewFilter,
    _save: Option<Task<()>>,
}

impl FilterModel {
    pub fn new(filter: ViewFilter) -> Self {
        Self {
            filter,
            _save: None,
        }
    }

    pub fn filter(&self) -> &ViewFilter {
        &self.filter
    }

    /// Change the criteria; notifies observers once, and only on a change.
    pub fn update(&mut self, f: impl FnOnce(&mut ViewFilter), cx: &mut Context<Self>) {
        let before = self.filter.clone();
        f(&mut self.filter);
        if before == self.filter {
            return;
        }
        let persisted_changed = ViewFilter {
            query: String::new(),
            ..before
        } != ViewFilter {
            query: String::new(),
            ..self.filter.clone()
        };
        if persisted_changed && !cfg!(test) {
            let snapshot = self.filter.clone();
            self._save = Some(cx.background_spawn(async move {
                if let Err(e) = filter::save(&snapshot) {
                    tracing::warn!("saving library view: {e:#}");
                }
            }));
        }
        cx.notify();
    }
}

struct ActiveFilters(WeakEntity<FilterModel>);
impl Global for ActiveFilters {}

/// Make `model` the one dialogs update (renaming or deleting the selected
/// category). Set by the library view.
pub fn set_active_filters(model: &Entity<FilterModel>, cx: &mut App) {
    cx.set_global(ActiveFilters(model.downgrade()));
}

/// The library's filter model, while a library view exists.
pub fn active_filters(cx: &App) -> Option<Entity<FilterModel>> {
    cx.try_global::<ActiveFilters>()?.0.upgrade()
}

/// A change of the criteria asked for while no library view existed.
type Change = Box<dyn FnOnce(&mut ViewFilter)>;

/// Changes waiting for the next library view, in the order asked.
#[derive(Default)]
struct PendingChanges(Vec<Change>);
impl Global for PendingChanges {}

/// Change the library's criteria: the open view's at once, else those of the
/// next view, which applies the change over its saved criteria as it is
/// created (a game page opened before the library, from a link or at
/// start, asks this). Either way the change is saved like any other.
pub fn change_filters(change: impl FnOnce(&mut ViewFilter) + 'static, cx: &mut App) {
    match active_filters(cx) {
        Some(model) => model.update(cx, |m, cx| m.update(change, cx)),
        None => cx
            .default_global::<PendingChanges>()
            .0
            .push(Box::new(change)),
    }
}

/// Apply the changes waiting for a library view to its new `model`.
pub(crate) fn apply_pending(model: &Entity<FilterModel>, cx: &mut App) {
    if !cx.has_global::<PendingChanges>() {
        return;
    }
    let changes = std::mem::take(&mut cx.global_mut::<PendingChanges>().0);
    for change in changes {
        model.update(cx, |m, cx| m.update(change, cx));
    }
}

/// Content of the Filters popover. Re-renders when the model changes.
pub struct FiltersPanel {
    model: Entity<FilterModel>,
    /// The stores the library has games of.
    stores_with_games: crate::filter::PerRevision<HashSet<Runner>>,
    /// The systems the library has games of.
    systems: crate::filter::PerRevision<Vec<String>>,
    /// The genres the library's games have, as (key, name).
    genres: crate::filter::PerRevision<Vec<(String, String)>>,
    /// The commonest store tags, as (key, name).
    tags: crate::filter::PerRevision<Vec<(String, String)>>,
    /// The commonest developers and publishers, as (key, name).
    developers: crate::filter::PerRevision<Vec<(String, String)>>,
    publishers: crate::filter::PerRevision<Vec<(String, String)>>,
    /// Whether the library holds any browser / web-app games.
    has_browser_games: crate::filter::PerRevision<bool>,
    /// Bottom edge of the Filters trigger button in window coordinates,
    /// updated during trigger prepaint so the panel sizes to the room below
    /// the button on the very first frame without measuring itself.
    anchor_bottom: Rc<Cell<Option<Pixels>>>,
    /// The field that finds a filter value by name, made at the first draw
    /// (it needs the window).
    search: Option<Entity<InputState>>,
    /// Every filter value with its count, for the search field.
    values: ValueCache,
    /// User override for the collapsible "More filters" section (`None`
    /// auto-expands when any secondary filter is active and collapses
    /// otherwise; latches open once auto-expanded until collapsed, reset, or
    /// the popover closes).
    more_open: Option<bool>,
    /// Secondary filter count observed on the previous render, so activating a
    /// new secondary filter (e.g. via value search or a saved view) always
    /// reveals "More filters".
    last_more_active: usize,
    _observe: Vec<Subscription>,
}

/// Vertical chrome around the scrollable filter body (`library-filters-sections`
/// or search results): popover offset (0.25rem), popover padding and borders
/// (~1.6rem), search field (~1.75rem), panel gaps (1.5rem), footer (~2.3rem),
/// popup window margin (0.5rem), and a small safety margin.
const PANEL_CHROME_REM: f32 = 8.5;

/// Maximum height of the scrollable filter list so the popover fits cleanly
/// below `anchor_bottom` without triggering upward viewport clamping. Falls
/// back to a usable height only when there is too little room below the button.
fn filters_body_height(
    visible: Bounds<Pixels>,
    anchor_bottom: Option<Pixels>,
    rem: Pixels,
) -> Pixels {
    let chrome = rem * PANEL_CHROME_REM;
    let most = (visible.size.height - chrome).max(rem * 4.);
    let min_comfortable = (rem * 10.).min(most);
    let Some(bottom) = anchor_bottom else {
        return (visible.size.height * 0.5 - chrome).clamp(rem * 4., most);
    };
    let room_below = visible.bottom() - bottom - chrome;
    if room_below >= min_comfortable {
        room_below.min(most)
    } else {
        (visible.size.height * 0.5 - chrome)
            .max(min_comfortable)
            .min(most)
    }
}

impl FiltersPanel {
    pub fn new(model: Entity<FilterModel>, cx: &mut Context<Self>) -> Self {
        let m = models(cx).clone();
        Self {
            stores_with_games: Default::default(),
            systems: Default::default(),
            genres: Default::default(),
            tags: Default::default(),
            developers: Default::default(),
            publishers: Default::default(),
            has_browser_games: Default::default(),
            anchor_bottom: Rc::new(Cell::new(None)),
            search: None,
            values: ValueCache::default(),
            more_open: None,
            last_more_active: 0,
            _observe: vec![
                cx.observe(&model, |_, _, cx| cx.notify()),
                cx.observe(&m.library, |_, _, cx| cx.notify()),
                cx.observe(&m.accounts, |_, _, cx| cx.notify()),
                cx.observe(&m.settings, |_, _, cx| cx.notify()),
            ],
            model,
        }
    }

    /// Shared cell where the Filters trigger button records its bottom edge
    /// in window coordinates.
    pub(crate) fn anchor_bottom(&self) -> Rc<Cell<Option<Pixels>>> {
        self.anchor_bottom.clone()
    }

    /// Clear any manual "More filters" latch when the popover closes so the
    /// next open collapses when no secondary filters are active.
    pub(crate) fn reset_more_open(&mut self) {
        self.more_open = None;
        self.last_more_active = 0;
    }

    fn set(&self, f: impl FnOnce(&mut ViewFilter) + 'static, cx: &mut Context<Self>) {
        self.model.update(cx, |m, cx| m.update(f, cx));
    }

    /// The value search field, made on first use.
    fn search_field(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Entity<InputState> {
        if let Some(search) = &self.search {
            return search.clone();
        }
        let search =
            cx.new(|cx| InputState::new(window, cx).placeholder(t("library.filters.find_value")));
        self._observe
            .push(cx.subscribe(&search, |_, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    cx.notify();
                }
            }));
        self.search = Some(search.clone());
        search
    }

    /// Type `text` into the value search field.
    #[cfg(test)]
    pub(crate) fn search_for(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        let search = self.search_field(window, cx);
        let text = text.to_owned();
        search.update(cx, |s, cx| s.set_value(text, window, cx));
    }

    /// The values the last search found, as (name, count); `None` when
    /// nothing is searched or nothing was drawn yet.
    #[cfg(test)]
    pub(crate) fn found_values(&self, cx: &App) -> Option<Vec<(String, usize)>> {
        let query = self.search.as_ref()?.read(cx).value().trim().to_owned();
        if query.is_empty() {
            return None;
        }
        let all = self.values.cached()?;
        Some(
            value_search::find(&all, &query)
                .iter()
                .map(|c| (c.label.clone(), c.count))
                .collect(),
        )
    }
}

/// The popover section a found value belongs to.
fn section_label(section: Section) -> SharedString {
    match section {
        Section::Category => t("library.filters.section.categories"),
        Section::System => t("library.filters.section.system"),
        Section::Status => t("library.filters.section.play_status"),
        Section::Mode => irori_ui::game_mode::title(),
        Section::Genre => t("library.filters.section.genre"),
        Section::Tag => t("library.filters.section.steam_tags"),
        Section::Developer => t("library.filters.section.developer"),
        Section::Publisher => t("library.filters.section.publisher"),
    }
}

impl FiltersPanel {
    /// The values whose names hold `query`, each ticked as its section
    /// would tick it, with its section and how many games carry it.
    fn render_values(
        &mut self,
        f: &ViewFilter,
        query: &str,
        max_h: Pixels,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (snapshot, user, index) = {
            let lib = models(cx).library.read(cx);
            (
                lib.snapshot.clone(),
                lib.user.clone(),
                lib.categories.clone(),
            )
        };
        let all = self
            .values
            .get(&snapshot, &user, &index, irori_ui::i18n::language(), || {
                let categories: Vec<_> = irori_ui::categories::entries(&user, &index)
                    .into_iter()
                    .map(|e| (e.category, e.name, e.count))
                    .collect();
                let system = |id: &str| {
                    irori_engines::systems::get(id).map_or(id.to_owned(), |s| s.short.clone())
                };
                let status = |s: EffectiveStatus| play_status::label(s).to_string();
                let mode = |m: GameMode| irori_ui::game_mode::label(m).to_string();
                let labels = value_search::Labels {
                    system: &system,
                    status: &status,
                    mode: &mode,
                };
                value_search::candidates(&snapshot, &user, &categories, &labels)
            });
        let hits = value_search::find(&all, query);
        let muted = cx.theme().muted_foreground;
        let more = hits.len().saturating_sub(value_search::MAX_SHOWN);
        let rows: Vec<AnyElement> = hits
            .iter()
            .take(value_search::MAX_SHOWN)
            .enumerate()
            .map(|(ix, hit)| {
                let value = hit.value.clone();
                let games = match hit.count {
                    1 => t("library.filters.value_games_one"),
                    n => t_args(
                        "library.filters.value_games_other",
                        &[("count", &n.to_string())],
                    ),
                };
                h_flex()
                    .w_full()
                    .gap_2()
                    .child(
                        div().flex_1().min_w_0().child(
                            Checkbox::new(SharedString::from(format!("filter-value-{ix}")))
                                .label(hit.label.clone())
                                .checked(value.is_picked(f))
                                .on_change(cx.listener(move |this, _: &bool, _, cx| {
                                    let value = value.clone();
                                    this.set(move |f| value.toggle(f), cx)
                                })),
                        ),
                    )
                    .child(
                        h_flex()
                            .flex_none()
                            .gap_2()
                            .text_xs()
                            .text_color(muted)
                            .child(section_label(hit.value.section()))
                            .child(games),
                    )
                    .into_any_element()
            })
            .collect();
        v_flex()
            .w_full()
            .max_h(max_h)
            .child(
                v_flex().flex_1().overflow_hidden().child(
                    div().flex_1().overflow_hidden().child(
                        v_flex()
                            .id("library-filter-values")
                            .size_full()
                            .overflow_y_scrollbar()
                            .pr_3()
                            .gap_2()
                            .when(rows.is_empty(), |el| {
                                el.child(div().text_sm().text_color(muted).child(t_args(
                                    "library.filters.no_values",
                                    &[("query", query)],
                                )))
                            })
                            .children(rows)
                            .when(more > 0, |el| {
                                el.child(div().text_xs().text_color(muted).child(match more {
                                    1 => t("library.filters.more_values_one"),
                                    n => t_args(
                                        "library.filters.more_values_other",
                                        &[("count", &n.to_string())],
                                    ),
                                }))
                            }),
                    ),
                ),
            )
            .into_any_element()
    }
}

impl FiltersPanel {
    /// Category checkboxes, both kinds (smart ones marked): games in any
    /// checked category are shown. Each category's "leave out" button
    /// hides its games instead, whatever else picks them.
    fn render_categories(
        &self,
        f: &ViewFilter,
        categories: &[irori_ui::categories::CategoryEntry],
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        let mut boxes: Vec<AnyElement> = Vec::with_capacity(categories.len() + 1);
        if !categories.is_empty() {
            let entry = CategoryFilter::Uncategorized;
            boxes.push(
                Checkbox::new("filter-category-none")
                    .label(entry.label())
                    .checked(f.categories.contains(&entry))
                    .on_change(cx.listener(move |this, _: &bool, _, cx| {
                        let entry = entry.clone();
                        this.set(move |f| f.toggle_category(entry), cx)
                    }))
                    .into_any_element(),
            );
        }
        for (ix, entry) in categories.iter().enumerate() {
            let category = entry.category.clone();
            let excluded = f.excludes_category(&category);
            let check = Checkbox::new(SharedString::from(format!("filter-category-{ix}")))
                .label(entry.name.clone())
                .checked(f.shows_category(&category))
                .on_change(cx.listener(move |this, _: &bool, _, cx| {
                    let category = category.clone();
                    this.set(move |f| f.toggle(&category), cx)
                }));
            let category = entry.category.clone();
            let danger = cx.theme().danger;
            let leave_out = Button::new(SharedString::from(format!("filter-category-out-{ix}")))
                .xsmall()
                .ghost()
                .icon(IconName::Ban)
                .when(excluded, |b| b.text_color(danger))
                .tooltip(if excluded {
                    t("library.filters.categories.stop_leaving_out")
                } else {
                    t("library.filters.categories.leave_out")
                })
                .on_click(cx.listener(move |this, _, _, cx| {
                    let category = category.clone();
                    this.set(move |f| f.toggle_excluded(&category), cx)
                }));
            boxes.push(
                h_flex()
                    .gap_1p5()
                    .min_w_0()
                    .child(leave_out)
                    .child(
                        div()
                            .min_w_0()
                            .when(excluded, |el| el.line_through())
                            .child(check),
                    )
                    .when(entry.is_smart(), |el| {
                        el.child(
                            Icon::new(irori_ui::categories::icon_for(
                                &entry.category,
                                entry.icon.as_deref(),
                            ))
                            .xsmall()
                            .text_color(muted),
                        )
                    })
                    .into_any_element(),
            );
        }
        v_flex()
            .gap_2()
            .child(
                h_flex()
                    .justify_between()
                    .child(section(t("library.filters.section.categories"), cx))
                    .child(
                        h_flex()
                            .gap_1()
                            .when(f.has_categories() || f.has_excluded_categories(), |el| {
                                el.child(
                                    Button::new("filter-categories-clear")
                                        .xsmall()
                                        .ghost()
                                        .label(t("library.filters.categories.show_all"))
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.set(|f| f.clear_categories(), cx)
                                        })),
                                )
                            })
                            .child(
                                Button::new("filter-categories-manage")
                                    .xsmall()
                                    .ghost()
                                    .icon(IconName::Settings2)
                                    .label(t("library.filters.categories.manage"))
                                    .on_click(|_, window, cx| {
                                        crate::open_categories_dialog(None, window, cx)
                                    }),
                            ),
                    ),
            )
            .when(categories.is_empty(), |el| {
                el.child(
                    div()
                        .text_xs()
                        .text_color(muted)
                        .child(t("library.filters.categories.empty")),
                )
            })
            .child(
                v_flex()
                    .id("filter-categories")
                    .gap_2()
                    .max_h_40()
                    .overflow_y_scroll()
                    .children(boxes),
            )
    }
}

fn section(title: SharedString, cx: &App) -> impl IntoElement {
    div()
        .text_xs()
        .font_semibold()
        .text_color(cx.theme().muted_foreground)
        .child(title)
}

/// Readable primary accent colour on the Filters popover surface.
fn active_accent(cx: &App) -> Hsla {
    let theme = cx.theme();
    legible(theme.primary, &[theme.popover], TEXT_CONTRAST, cx)
}

/// Accent-highlighted toggle chip that caps its width to the popover so long
/// metadata labels (e.g. developer and publisher names) truncate cleanly.
fn filter_chip(
    id: impl Into<SharedString>,
    icon: Option<IconName>,
    label: impl Into<SharedString>,
    selected: bool,
    on_click: impl Fn(&mut Window, &mut App) + 'static,
    cx: &App,
) -> AnyElement {
    let theme = cx.theme();
    let primary = theme.primary;
    let active_fg = active_accent(cx);
    let fg = theme.foreground;
    let muted = theme.muted_foreground;
    let border = theme.border;
    let idle_bg = theme.secondary.opacity(0.35);
    let hover_bg = theme.secondary.opacity(0.8);

    let id: SharedString = id.into();
    let debug_id = id.to_string();
    h_flex()
        .id(id)
        .debug_selector(move || debug_id.clone())
        .max_w_full()
        .min_w_0()
        .overflow_hidden()
        .items_center()
        .gap_1()
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
            el.bg(idle_bg)
                .border_color(border)
                .text_color(muted)
                .hover(move |s| s.bg(hover_bg).text_color(fg))
        })
        .when_some(icon, |el, glyph| {
            el.child(Icon::new(glyph).xsmall().flex_shrink_0())
        })
        .child(div().max_w_full().min_w_0().truncate().child(label.into()))
        .on_click(move |_, window, cx| on_click(window, cx))
        .into_any_element()
}

type SegmentClick = Box<dyn Fn(&mut Window, &mut App)>;

struct FilterSegment {
    id: SharedString,
    icon: Option<IconName>,
    label: SharedString,
    selected: bool,
    on_click: SegmentClick,
}

impl FilterSegment {
    fn new(
        id: impl Into<SharedString>,
        icon: Option<IconName>,
        label: impl Into<SharedString>,
        selected: bool,
        on_click: impl Fn(&mut Window, &mut App) + 'static,
    ) -> Self {
        Self {
            id: id.into(),
            icon,
            label: label.into(),
            selected,
            on_click: Box::new(on_click),
        }
    }
}

/// Single-choice segmented bar with unmistakable accent styling on the active
/// segment.
fn segmented_filter_bar(segments: impl IntoIterator<Item = FilterSegment>, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let primary = theme.primary;
    let active_fg = active_accent(cx);
    let fg = theme.foreground;
    let muted = theme.muted_foreground;
    let rail_bg = theme.secondary.opacity(0.45);
    let hover_bg = theme.secondary.opacity(0.85);
    let border = theme.border;
    let radius = theme.radius;

    h_flex()
        .w_full()
        .p_0p5()
        .gap_1()
        .rounded(radius)
        .bg(rail_bg)
        .border_1()
        .border_color(border)
        .children(segments.into_iter().map(move |seg| {
            let on_click = seg.on_click;
            let debug_id = seg.id.to_string();
            h_flex()
                .id(seg.id)
                .debug_selector(move || debug_id.clone())
                .flex_auto()
                .min_w_0()
                .justify_center()
                .items_center()
                .gap_1()
                .px_1p5()
                .py_1()
                .rounded_md()
                .border_1()
                .cursor_pointer()
                .keyboard_focusable(cx)
                .text_xs()
                .when(seg.selected, |el| {
                    el.bg(primary.opacity(0.18))
                        .border_color(primary)
                        .text_color(active_fg)
                        .font_semibold()
                })
                .when(!seg.selected, |el| {
                    el.border_color(transparent_black())
                        .text_color(muted)
                        .hover(move |s| s.bg(hover_bg).text_color(fg))
                })
                .when_some(seg.icon, |el, glyph| {
                    el.child(Icon::new(glyph).xsmall().flex_shrink_0())
                })
                .child(div().min_w_0().truncate().child(seg.label))
                .on_click(move |_, window, cx| on_click(window, cx))
        }))
        .into_any_element()
}

/// Whether `library` contains at least one browser / web-app game.
pub(crate) fn library_has_browser_games(library: &irori_services::LibrarySnapshot) -> bool {
    library.games.iter().any(|g| {
        g.browser_url.is_some()
            || g.installed_platform() == Some(InstallPlatform::Browser)
            || g.platforms.contains(&InstallPlatform::Browser)
    })
}

/// Number of active filters housed inside the collapsible "More filters" group.
pub(crate) fn more_filters_active(f: &ViewFilter) -> usize {
    [
        f.favourites_only,
        f.updates_only,
        f.offline_only,
        f.hide_unavailable,
        f.hide_third_party,
        f.has_categories() || f.has_excluded_categories(),
        !f.modes.is_empty(),
        f.controller.is_some(),
        f.released != ReleaseWindow::Any,
        f.max_age.is_some(),
        f.hidden != HiddenMode::Hide,
        !f.developers.is_empty(),
        !f.publishers.is_empty(),
    ]
    .into_iter()
    .filter(|b| *b)
    .count()
}

impl FiltersPanel {
    fn offered_stores(&mut self, cx: &mut Context<Self>) -> Vec<Runner> {
        let m = models(cx).clone();
        let snapshot = m.library.read(cx).snapshot.clone();
        let shown = m.settings.read(cx).app.ui.stores;
        let accounts = m.accounts.read(cx);
        let with_games = self
            .stores_with_games
            .get(&snapshot, filter::stores_with_games);
        filter::offered_stores(with_games, &|r| accounts.is_logged_in(r), &shown)
    }

    /// Store chips, shown when the library offers at least two stores.
    fn render_stores(
        &self,
        f: &ViewFilter,
        stores: &[Runner],
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if stores.len() < 2 {
            return None;
        }
        let current = match f.store {
            StoreFilter::Store(r) if stores.contains(&r) => Some(r),
            _ => None,
        };
        let model = self.model.clone();
        let options = std::iter::once(None).chain(stores.iter().copied().map(Some));
        Some(
            v_flex()
                .debug_selector(|| "library-filters-store".into())
                .w_full()
                .min_w_0()
                .gap_2()
                .child(section(t("library.filters.section.store"), cx))
                .child(
                    h_flex()
                        .w_full()
                        .min_w_0()
                        .flex_wrap()
                        .gap_1p5()
                        .children(options.map(|option| {
                            let model = model.clone();
                            let (id, label): (SharedString, SharedString) = match option {
                                Some(r) => {
                                    (format!("store-{}", r.id()).into(), r.store_name().into())
                                }
                                None => ("store-all".into(), t("library.toolbar.all_stores")),
                            };
                            filter_chip(
                                id,
                                None,
                                label,
                                option == current,
                                move |_, cx| {
                                    let store = match option {
                                        Some(r) => StoreFilter::Store(r),
                                        None => StoreFilter::All,
                                    };
                                    model.update(cx, |m, cx| {
                                        m.update(
                                            move |f| {
                                                if f.store != store {
                                                    f.account = None;
                                                }
                                                f.store = store;
                                            },
                                            cx,
                                        )
                                    });
                                },
                                cx,
                            )
                        })),
                )
                .into_any_element(),
        )
    }

    /// Account switcher for the shown store, when it has two or more signed-in accounts.
    fn render_accounts(
        &self,
        f: &ViewFilter,
        stores: &[Runner],
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let runner = f.shown_store(stores)?;
        let accounts: Vec<(String, String)> = models(cx)
            .accounts
            .read(cx)
            .accounts(runner)
            .iter()
            .filter(|a| a.signed_in)
            .map(|a| (a.account_id.clone(), a.display_name.clone()))
            .collect();
        if accounts.len() < 2 {
            return None;
        }
        let current = f
            .account
            .as_ref()
            .filter(|a| a.runner == runner)
            .map(|a| a.account_id.clone());
        let model = self.model.clone();
        Some(
            v_flex()
                .w_full()
                .min_w_0()
                .gap_2()
                .child(section(t("library.filters.section.account"), cx))
                .child(
                    h_flex().w_full().min_w_0().flex_wrap().gap_1p5().children(
                        std::iter::once(None)
                            .chain(accounts.iter().map(|(id, _)| Some(id.clone())))
                            .enumerate()
                            .map(|(ix, option)| {
                                let model = model.clone();
                                let label: SharedString = match &option {
                                    Some(id) => accounts
                                        .iter()
                                        .find(|(a, _)| a == id)
                                        .map(|(_, name)| name.clone())
                                        .unwrap_or_default()
                                        .into(),
                                    None => t("library.toolbar.all_accounts"),
                                };
                                let selected = option == current;
                                filter_chip(
                                    SharedString::from(format!("filter-account-{ix}")),
                                    None,
                                    label,
                                    selected,
                                    move |_, cx| {
                                        let account = option
                                            .clone()
                                            .map(|account_id| AccountFilter { runner, account_id });
                                        model.update(cx, |m, cx| {
                                            m.update(move |f| f.account = account, cx)
                                        });
                                    },
                                    cx,
                                )
                            }),
                    ),
                )
                .into_any_element(),
        )
    }

    /// The system chips, for libraries with emulated games.
    fn render_systems(&mut self, f: &ViewFilter, cx: &mut Context<Self>) -> Option<AnyElement> {
        let snapshot = models(cx).library.read(cx).snapshot.clone();
        let present = self
            .systems
            .get(&snapshot, crate::filter::present_systems)
            .clone();
        if present.is_empty() {
            return None;
        }
        let model = self.model.clone();
        Some(
            v_flex()
                .w_full()
                .min_w_0()
                .gap_2()
                .child(section(t("library.filters.section.system"), cx))
                .child(h_flex().w_full().min_w_0().flex_wrap().gap_1p5().children(
                    present.into_iter().map(|id| {
                        let model = model.clone();
                        let label = irori_engines::systems::get(&id)
                            .map_or(id.clone(), |s| s.short.clone());
                        let selected = f.systems.contains(&id);
                        filter_chip(
                            SharedString::from(format!("system-{id}")),
                            None,
                            label,
                            selected,
                            move |_, cx| {
                                let id = id.clone();
                                model.update(cx, |m, cx| {
                                    m.update(
                                        move |f| {
                                            if !f.systems.remove(&id) {
                                                f.systems.insert(id);
                                            }
                                        },
                                        cx,
                                    )
                                });
                            },
                            cx,
                        )
                    }),
                ))
                .into_any_element(),
        )
    }
}

impl FiltersPanel {
    /// One chip per play status; several may be picked.
    fn render_statuses(&self, f: &ViewFilter, cx: &mut Context<Self>) -> AnyElement {
        let model = self.model.clone();
        v_flex()
            .w_full()
            .min_w_0()
            .gap_2()
            .child(section(t("library.filters.section.play_status"), cx))
            .child(h_flex().w_full().min_w_0().flex_wrap().gap_1p5().children(
                EffectiveStatus::ALL.into_iter().map(|status| {
                    let model = model.clone();
                    filter_chip(
                        SharedString::from(format!("play-status-{status:?}")),
                        Some(play_status::icon(status)),
                        play_status::label(status),
                        f.statuses.contains(&status),
                        move |_, cx| {
                            model.update(cx, |m, cx| {
                                m.update(
                                    move |f| {
                                        if !f.statuses.remove(&status) {
                                            f.statuses.insert(status);
                                        }
                                    },
                                    cx,
                                )
                            });
                        },
                        cx,
                    )
                }),
            ))
            .into_any_element()
    }

    /// Controller support: any or full; a second click shows every game.
    fn render_controller(&self, f: &ViewFilter, cx: &mut Context<Self>) -> AnyElement {
        use irori_core::ControllerSupport;
        let model = self.model.clone();
        v_flex()
            .w_full()
            .min_w_0()
            .gap_2()
            .child(section(t("library.filters.section.controller"), cx))
            .child(
                h_flex().w_full().min_w_0().flex_wrap().gap_1p5().children(
                    [ControllerSupport::Partial, ControllerSupport::Full]
                        .into_iter()
                        .map(|level| {
                            let model = model.clone();
                            filter_chip(
                                SharedString::from(format!("controller-{level:?}")),
                                Some(IconName::Gamepad2),
                                crate::filter::controller_label(level),
                                f.controller == Some(level),
                                move |_, cx| {
                                    model.update(cx, |m, cx| {
                                        m.update(
                                            move |f| {
                                                f.controller =
                                                    (f.controller != Some(level)).then_some(level)
                                            },
                                            cx,
                                        )
                                    });
                                },
                                cx,
                            )
                        }),
                ),
            )
            .into_any_element()
    }

    /// Game modes: games with any of the chosen ones are shown.
    fn render_modes(&self, f: &ViewFilter, cx: &mut Context<Self>) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let model = self.model.clone();
        v_flex()
            .w_full()
            .min_w_0()
            .gap_2()
            .child(section(irori_ui::game_mode::title(), cx))
            .child(h_flex().w_full().min_w_0().flex_wrap().gap_1p5().children(
                GameMode::ALL.into_iter().map(|mode| {
                    let model = model.clone();
                    filter_chip(
                        SharedString::from(format!("game-mode-{}", mode.id())),
                        Some(irori_ui::game_mode::icon(mode)),
                        irori_ui::game_mode::label(mode),
                        f.modes.contains(&mode),
                        move |_, cx| {
                            model.update(cx, |m, cx| {
                                m.update(
                                    move |f| {
                                        if !f.modes.remove(&mode) {
                                            f.modes.insert(mode);
                                        }
                                    },
                                    cx,
                                )
                            });
                        },
                        cx,
                    )
                }),
            ))
            .child(
                div()
                    .text_xs()
                    .text_color(muted)
                    .child(t("library.filters.modes.hint")),
            )
            .into_any_element()
    }
}

impl FiltersPanel {
    /// One chip per genre the library has; several may be picked. `None`
    /// when no game has a genre.
    fn render_genres(&mut self, f: &ViewFilter, cx: &mut Context<Self>) -> Option<AnyElement> {
        let snapshot = models(cx).library.read(cx).snapshot.clone();
        let genres = self
            .genres
            .get(&snapshot, crate::filter::present_genres)
            .clone();
        if genres.is_empty() {
            return None;
        }
        let picked: std::collections::HashSet<String> =
            f.genres.iter().map(|g| filter::genre_key(g)).collect();
        let model = self.model.clone();
        Some(
            v_flex()
                .w_full()
                .min_w_0()
                .gap_2()
                .child(section(t("library.filters.section.genre"), cx))
                .child(h_flex().w_full().min_w_0().flex_wrap().gap_1p5().children(
                    genres.into_iter().map(|(key, name)| {
                        let model = model.clone();
                        let selected = picked.contains(&key);
                        filter_chip(
                            SharedString::from(format!("genre-{key}")),
                            None,
                            name.clone(),
                            selected,
                            move |_, cx| {
                                let (key, name) = (key.clone(), name.clone());
                                model.update(cx, |m, cx| {
                                    m.update(
                                        move |f| {
                                            let before = f.genres.len();
                                            f.genres.retain(|g| filter::genre_key(g) != key);
                                            if f.genres.len() == before {
                                                f.genres.insert(name);
                                            }
                                        },
                                        cx,
                                    )
                                });
                            },
                            cx,
                        )
                    }),
                ))
                .into_any_element(),
        )
    }

    /// One chip per common Steam tag, and any picked ones; several may be
    /// picked. `None` when no game has a store tag.
    fn render_tags(&mut self, f: &ViewFilter, cx: &mut Context<Self>) -> Option<AnyElement> {
        use irori_services::search::tag_key;
        let snapshot = models(cx).library.read(cx).snapshot.clone();
        let mut tags = self
            .tags
            .get(&snapshot, crate::filter::present_tags)
            .clone();
        for picked in &f.tags {
            let key = tag_key(picked);
            if !tags.iter().any(|(k, _)| *k == key) {
                tags.push((key, picked.clone()));
            }
        }
        if tags.is_empty() {
            return None;
        }
        let picked: std::collections::HashSet<String> = f.tags.iter().map(|t| tag_key(t)).collect();
        let model = self.model.clone();
        Some(
            v_flex()
                .w_full()
                .min_w_0()
                .gap_2()
                .child(section(t("library.filters.section.steam_tags"), cx))
                .child(
                    v_flex().w_full().min_w_0().max_h(rems(10.)).child(
                        v_flex().flex_1().overflow_hidden().child(
                            div().flex_1().overflow_hidden().child(
                                v_flex()
                                    .id("filter-tags-scroll")
                                    .size_full()
                                    .overflow_y_scrollbar()
                                    .pr_2()
                                    .child(
                                        h_flex().w_full().min_w_0().flex_wrap().gap_1p5().children(
                                            tags.into_iter().map(|(key, name)| {
                                                let model = model.clone();
                                                let selected = picked.contains(&key);
                                                filter_chip(
                                                    SharedString::from(format!("tag-{key}")),
                                                    None,
                                                    name.clone(),
                                                    selected,
                                                    move |_, cx| {
                                                        let (key, name) =
                                                            (key.clone(), name.clone());
                                                        model.update(cx, |m, cx| {
                                                            m.update(
                                                                move |f| {
                                                                    let before = f.tags.len();
                                                                    f.tags.retain(|t| {
                                                                        tag_key(t) != key
                                                                    });
                                                                    if f.tags.len() == before {
                                                                        f.tags.insert(name);
                                                                    }
                                                                },
                                                                cx,
                                                            )
                                                        });
                                                    },
                                                    cx,
                                                )
                                            }),
                                        ),
                                    ),
                            ),
                        ),
                    ),
                )
                .into_any_element(),
        )
    }

    /// One chip per common developer or publisher, and any picked ones;
    /// several may be picked. `None` when no store names one.
    fn render_companies(
        &mut self,
        f: &ViewFilter,
        company: Company,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let snapshot = models(cx).library.read(cx).snapshot.clone();
        let (cache, picked, title, id) = match company {
            Company::Developer => (
                &mut self.developers,
                &f.developers,
                "library.filters.section.developer",
                "developer",
            ),
            Company::Publisher => (
                &mut self.publishers,
                &f.publishers,
                "library.filters.section.publisher",
                "publisher",
            ),
        };
        let mut offered = cache
            .get(&snapshot, |l| filter::present_companies(l, company))
            .clone();
        for name in picked {
            let key = filter::company_key(name);
            if !offered.iter().any(|(k, _)| *k == key) {
                offered.push((key, name.clone()));
            }
        }
        if offered.is_empty() {
            return None;
        }
        let picked: std::collections::HashSet<String> =
            picked.iter().map(|c| filter::company_key(c)).collect();
        let model = self.model.clone();
        Some(
            v_flex()
                .w_full()
                .min_w_0()
                .gap_2()
                .child(section(t(title), cx))
                .child(h_flex().w_full().min_w_0().flex_wrap().gap_1p5().children(
                    offered.into_iter().map(|(key, name)| {
                        let model = model.clone();
                        filter_chip(
                            SharedString::from(format!("{id}-{key}")),
                            None,
                            name.clone(),
                            picked.contains(&key),
                            move |_, cx| {
                                let (key, name) = (key.clone(), name.clone());
                                model.update(cx, |m, cx| {
                                    m.update(
                                        move |f| {
                                            let set = match company {
                                                Company::Developer => &mut f.developers,
                                                Company::Publisher => &mut f.publishers,
                                            };
                                            filter::toggle_named(
                                                set,
                                                &key,
                                                name,
                                                filter::company_key,
                                            )
                                        },
                                        cx,
                                    )
                                });
                            },
                            cx,
                        )
                    }),
                ))
                .into_any_element(),
        )
    }

    /// Hide / show / only hidden games, behind the PIN when one is set, and
    /// the PIN itself: set it, lock again, or change it.
    fn render_hidden(
        &self,
        f: &ViewFilter,
        hidden: [(HiddenMode, SharedString); 3],
        cx: &mut Context<Self>,
    ) -> AnyElement {
        use irori_ui::hidden_lock;
        let locked = hidden_lock::locked(cx);
        let pin_set = hidden_lock::pin_set(cx);
        // While locked the filter can't be showing hidden games, whatever a
        // saved view says.
        let shown = if locked { HiddenMode::Hide } else { f.hidden };
        let model = self.model.clone();
        v_flex()
            .w_full()
            .min_w_0()
            .gap_2()
            .child(
                h_flex()
                    .gap_2()
                    .justify_between()
                    .child(section(t("library.filters.section.hidden"), cx))
                    .child(
                        h_flex()
                            .gap_1()
                            .when(pin_set && !locked, |row| {
                                row.child(
                                    Button::new("hidden-lock")
                                        .ghost()
                                        .xsmall()
                                        .icon(IconName::Lock)
                                        .label(t("library.filters.hidden.lock"))
                                        .on_click(|_, _, cx| hidden_lock::lock(cx)),
                                )
                            })
                            .when(!locked, |row| {
                                row.child(
                                    Button::new("hidden-pin")
                                        .ghost()
                                        .xsmall()
                                        .label(if pin_set {
                                            t("library.filters.hidden.change_pin")
                                        } else {
                                            t("library.filters.hidden.set_pin")
                                        })
                                        .on_click(|_, window, cx| {
                                            hidden_lock::open_pin_dialog(window, cx)
                                        }),
                                )
                            }),
                    ),
            )
            .child(segmented_filter_bar(
                hidden.into_iter().enumerate().map(|(ix, (v, label))| {
                    let model = model.clone();
                    FilterSegment::new(
                        SharedString::from(format!("hidden-{ix}")),
                        (locked && v != HiddenMode::Hide).then_some(IconName::Lock),
                        label,
                        shown == v,
                        move |window, cx| {
                            if v == HiddenMode::Hide || !hidden_lock::locked(cx) {
                                model.update(cx, |m, cx| m.update(move |f| f.hidden = v, cx));
                                return;
                            }
                            let model = model.clone();
                            hidden_lock::open_unlock_dialog(
                                std::rc::Rc::new(move |_, cx| {
                                    model.update(cx, |m, cx| m.update(|f| f.hidden = v, cx))
                                }),
                                window,
                                cx,
                            );
                        },
                    )
                }),
                cx,
            ))
            .into_any_element()
    }

    /// The age limit: "Any" or the youngest age every rating must allow,
    /// and, with a limit, whether games no board rated are shown.
    fn render_age(&self, f: &ViewFilter, cx: &mut Context<Self>) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let limits = std::iter::once(None).chain(filter::AGE_LIMITS.into_iter().map(Some));
        let model = self.model.clone();
        v_flex()
            .w_full()
            .min_w_0()
            .gap_2()
            .child(section(t("library.filters.section.age"), cx))
            .child(segmented_filter_bar(
                limits.enumerate().map(|(ix, limit)| {
                    let model = model.clone();
                    let label: SharedString = match limit {
                        None => t("library.filters.age.any"),
                        Some(age) => age.to_string().into(),
                    };
                    FilterSegment::new(
                        SharedString::from(format!("age-{ix}")),
                        None,
                        label,
                        f.max_age == limit,
                        move |_, cx| {
                            model.update(cx, |m, cx| m.update(move |f| f.max_age = limit, cx));
                        },
                    )
                }),
                cx,
            ))
            .when(f.max_age.is_some(), |el| {
                el.child(
                    Checkbox::new("filter-age-unrated")
                        .label(t("library.filters.age.include_unrated"))
                        .checked(f.include_unrated)
                        .on_change(cx.listener(|this, v: &bool, _, cx| {
                            let v = *v;
                            this.set(move |f| f.include_unrated = v, cx)
                        })),
                )
            })
            .child(
                div()
                    .text_xs()
                    .text_color(muted)
                    .child(t("library.filters.age.hint")),
            )
            .into_any_element()
    }
}

impl Render for FiltersPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let f = self.model.read(cx).filter().clone();
        let offered_stores = self.offered_stores(cx);
        let stores = self.render_stores(&f, &offered_stores, cx);
        let accounts = self.render_accounts(&f, &offered_stores, cx);
        let systems = self.render_systems(&f, cx);
        let genres = self.render_genres(&f, cx);
        let tags = self.render_tags(&f, cx);
        let developers = self.render_companies(&f, Company::Developer, cx);
        let publishers = self.render_companies(&f, Company::Publisher, cx);
        let (categories, has_browser) = {
            let lib = models(cx).library.read(cx);
            let cats = irori_ui::categories::entries(&lib.user, &lib.categories);
            let snapshot = lib.snapshot.clone();
            let has_browser = *self
                .has_browser_games
                .get(&snapshot, library_has_browser_games);
            (cats, has_browser)
        };
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let fg = theme.foreground;
        let border = theme.border;
        let secondary = theme.secondary;
        let primary = theme.primary;
        let active_fg = active_accent(cx);
        let radius = theme.radius;
        let model = self.model.clone();

        let mut platforms: Vec<(Option<InstallPlatform>, SharedString)> = vec![
            (None, t("library.filters.platform.any")),
            (
                Some(InstallPlatform::Windows),
                InstallPlatform::Windows.label().into(),
            ),
            (
                Some(InstallPlatform::Linux),
                InstallPlatform::Linux.label().into(),
            ),
            (
                Some(InstallPlatform::Mac),
                InstallPlatform::Mac.label().into(),
            ),
        ];
        if has_browser || f.platform == Some(InstallPlatform::Browser) {
            platforms.push((
                Some(InstallPlatform::Browser),
                t("library.platform.browser"),
            ));
        }
        let installs = [
            (InstallFilter::Any, t("library.filters.install.all")),
            (
                InstallFilter::Installed,
                t("library.filters.install.installed"),
            ),
            (
                InstallFilter::NotInstalled,
                t("library.filters.install.not_installed"),
            ),
        ];
        let released = [
            ReleaseWindow::Any,
            ReleaseWindow::ThisYear,
            ReleaseWindow::LastFiveYears,
            ReleaseWindow::Older,
        ];
        let hidden = [
            (HiddenMode::Hide, t("library.filters.hidden.hide")),
            (HiddenMode::Show, t("library.filters.hidden.show")),
            (HiddenMode::Only, t("library.filters.hidden.only")),
        ];

        let more_active = more_filters_active(&f);
        if more_active > self.last_more_active || (more_active > 0 && self.more_open.is_none()) {
            self.more_open = Some(true);
        }
        self.last_more_active = more_active;
        let more_expanded = self.more_open.unwrap_or(false);
        let more_label = if more_active > 0 {
            t_args(
                "library.filters.more_filters_active",
                &[("count", &more_active.to_string())],
            )
        } else {
            t("library.filters.more_filters")
        };

        // The popover opens under its button and clamps upward if it overflows
        // the window: size the scrollable body from the button's measured
        // bottom edge so it fits below the trigger on the very first frame.
        let visible = irori_ui::style::visible_bounds(window);
        let rem = window.rem_size();
        let max_h = filters_body_height(visible, self.anchor_bottom.get(), rem);

        let more_body = more_expanded.then(|| {
            let released_model = model.clone();
            v_flex()
                .debug_selector(|| "library-filters-more-body".into())
                .w_full()
                .min_w_0()
                .gap_4()
                .p_2p5()
                .rounded(radius)
                .bg(secondary.opacity(0.28))
                .border_1()
                .border_color(border.opacity(0.65))
                .child(
                    v_flex()
                        .w_full()
                        .min_w_0()
                        .gap_2()
                        .child(section(t("library.filters.section.show_only"), cx))
                        .child(
                            Checkbox::new("filter-favourites")
                                .label(t("library.filters.favourites"))
                                .checked(f.favourites_only)
                                .on_change(cx.listener(|this, v: &bool, _, cx| {
                                    let v = *v;
                                    this.set(move |f| f.favourites_only = v, cx)
                                })),
                        )
                        .child(
                            Checkbox::new("filter-updates")
                                .label(t("library.filters.updates"))
                                .checked(f.updates_only)
                                .on_change(cx.listener(|this, v: &bool, _, cx| {
                                    let v = *v;
                                    this.set(move |f| f.updates_only = v, cx)
                                })),
                        )
                        .child(
                            Checkbox::new("filter-offline")
                                .label(t("library.filters.offline"))
                                .checked(f.offline_only)
                                .on_change(cx.listener(|this, v: &bool, _, cx| {
                                    let v = *v;
                                    this.set(move |f| f.offline_only = v, cx)
                                })),
                        )
                        .child(
                            Checkbox::new("filter-unavailable")
                                .label(t("library.filters.hide_missing"))
                                .checked(f.hide_unavailable)
                                .on_change(cx.listener(|this, v: &bool, _, cx| {
                                    let v = *v;
                                    this.set(move |f| f.hide_unavailable = v, cx)
                                })),
                        )
                        .child(
                            Checkbox::new("filter-third-party")
                                .label(t("library.filters.hide_third_party"))
                                .checked(f.hide_third_party)
                                .on_change(cx.listener(|this, v: &bool, _, cx| {
                                    let v = *v;
                                    this.set(move |f| f.hide_third_party = v, cx)
                                })),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(muted)
                                .child(t("library.filters.third_party_hint")),
                        ),
                )
                .child(self.render_categories(&f, &categories, cx))
                .child(self.render_modes(&f, cx))
                .child(self.render_controller(&f, cx))
                .child(
                    v_flex()
                        .w_full()
                        .min_w_0()
                        .gap_2()
                        .child(section(t("library.filters.section.released"), cx))
                        .child(segmented_filter_bar(
                            released.iter().enumerate().map(|(ix, &w)| {
                                let m = released_model.clone();
                                FilterSegment::new(
                                    SharedString::from(format!("released-{ix}")),
                                    None,
                                    w.label(),
                                    f.released == w,
                                    move |_, cx| {
                                        m.update(cx, |m, cx| m.update(move |f| f.released = w, cx));
                                    },
                                )
                            }),
                            cx,
                        )),
                )
                .child(self.render_age(&f, cx))
                .child(self.render_hidden(&f, hidden, cx))
                .when_some(developers, |el, d| el.child(d))
                .when_some(publishers, |el, p| el.child(p))
        });

        let install_model = model.clone();
        let platform_model = model.clone();
        let sections = v_flex().w_full().min_w_0().max_h(max_h).child(
            v_flex().flex_1().overflow_hidden().child(
                div().flex_1().overflow_hidden().child(
                    v_flex()
                        .id("library-filters-sections")
                        .size_full()
                        .overflow_y_scrollbar()
                        .pr_3()
                        .gap_4()
                        .when_some(stores, |el, stores| el.child(stores))
                        .when_some(accounts, |el, accounts| el.child(accounts))
                        .when_some(systems, |el, systems| el.child(systems))
                        .child(
                            v_flex()
                                .w_full()
                                .min_w_0()
                                .gap_2()
                                .child(section(t("library.filters.section.install"), cx))
                                .child(segmented_filter_bar(
                                    installs.into_iter().enumerate().map(|(ix, (v, label))| {
                                        let m = install_model.clone();
                                        FilterSegment::new(
                                            SharedString::from(format!("install-{ix}")),
                                            None,
                                            label,
                                            f.install == v,
                                            move |_, cx| {
                                                m.update(cx, |m, cx| {
                                                    m.update(move |f| f.install = v, cx)
                                                });
                                            },
                                        )
                                    }),
                                    cx,
                                )),
                        )
                        .child(
                            v_flex()
                                .w_full()
                                .min_w_0()
                                .gap_2()
                                .child(section(t("library.filters.section.platform"), cx))
                                .child(h_flex().w_full().min_w_0().flex_wrap().gap_1p5().children(
                                    platforms.into_iter().enumerate().map(|(ix, (p, label))| {
                                        let m = platform_model.clone();
                                        filter_chip(
                                            SharedString::from(format!("platform-{ix}")),
                                            None,
                                            label,
                                            f.platform == p,
                                            move |_, cx| {
                                                m.update(cx, |m, cx| {
                                                    m.update(move |f| f.platform = p, cx)
                                                });
                                            },
                                            cx,
                                        )
                                    }),
                                )),
                        )
                        .child(self.render_statuses(&f, cx))
                        .when_some(genres, |el, genres| el.child(genres))
                        .when_some(tags, |el, tags| el.child(tags))
                        .child(
                            v_flex()
                                .w_full()
                                .min_w_0()
                                .gap_2()
                                .child(
                                    h_flex()
                                        .id("filter-more-toggle")
                                        .debug_selector(|| "library-filters-more-toggle".into())
                                        .w_full()
                                        .items_center()
                                        .justify_between()
                                        .px_2p5()
                                        .py_1p5()
                                        .rounded(radius)
                                        .border_1()
                                        .cursor_pointer()
                                        .keyboard_focusable(cx)
                                        .when(more_active > 0, |el| {
                                            el.bg(primary.opacity(0.12))
                                                .border_color(primary.opacity(0.5))
                                                .text_color(active_fg)
                                                .font_semibold()
                                        })
                                        .when(more_active == 0, |el| {
                                            el.bg(secondary.opacity(0.35))
                                                .border_color(border)
                                                .text_color(muted)
                                                .font_medium()
                                                .hover(move |s| {
                                                    s.bg(secondary.opacity(0.75)).text_color(fg)
                                                })
                                        })
                                        .child(
                                            div().text_xs().min_w_0().truncate().child(more_label),
                                        )
                                        .child(
                                            Icon::new(if more_expanded {
                                                IconName::ChevronDown
                                            } else {
                                                IconName::ChevronRight
                                            })
                                            .xsmall()
                                            .flex_shrink_0(),
                                        )
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.more_open = Some(!more_expanded);
                                            cx.notify();
                                        })),
                                )
                                .children(more_body),
                        ),
                ),
            ),
        );

        let search = self.search_field(window, cx);
        let query = search.read(cx).value().trim().to_owned();
        let body = if query.is_empty() {
            sections.into_any_element()
        } else {
            self.render_values(&f, &query, max_h, cx)
        };

        v_flex()
            .id("library-filters-panel")
            .debug_selector(|| "library-filters-panel".into())
            .relative()
            .w_96()
            .gap_3()
            .child(
                Input::new(&search)
                    .small()
                    .cleanable(true)
                    .prefix(Icon::new(IconName::Search).small()),
            )
            .child(body)
            .child(
                h_flex()
                    .justify_between()
                    .pt_2()
                    .border_t_1()
                    .border_color(cx.theme().border)
                    .child(div().text_xs().text_color(muted).min_w_0().child(
                        match f.active_count() {
                            0 => t("library.filters.active_none"),
                            1 => t("library.filters.active_one"),
                            n => {
                                t_args("library.filters.active_other", &[("count", &n.to_string())])
                            }
                        },
                    ))
                    .child(
                        Button::new("filter-reset")
                            .small()
                            .ghost()
                            .label(t("library.filters.reset"))
                            .disabled(f.active_count() == 0)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.reset_more_open();
                                this.set(ViewFilter::reset_filters, cx)
                            })),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::{point, px, size};

    #[test]
    fn more_filters_counts_only_secondary_facets() {
        let mut f = ViewFilter::default();
        assert_eq!(more_filters_active(&f), 0);
        // Primary facets (including Tags, Genres, Install) do not trigger More filters.
        f.install = InstallFilter::Installed;
        f.genres.insert("RPG".into());
        f.tags.insert("Roguelike".into());
        assert_eq!(more_filters_active(&f), 0);

        // Secondary facets (including Show only and Categories) each count once.
        f.favourites_only = true;
        f.categories.insert(CategoryFilter::Named("Backlog".into()));
        f.developers.insert("Supergiant Games".into());
        f.released = ReleaseWindow::ThisYear;
        assert_eq!(more_filters_active(&f), 4);

        f.publishers.insert("Devolver".into());
        f.max_age = Some(12);
        f.hidden = HiddenMode::Show;
        f.modes.insert(GameMode::Coop);
        f.controller = Some(irori_core::ControllerSupport::Full);
        f.offline_only = true;
        assert_eq!(more_filters_active(&f), 10);
    }

    #[test]
    fn detects_browser_games_only_when_present() {
        let pc = crate::filter::tests::game(Runner::Steam, "1", "Hades", true);
        let snap = crate::filter::tests::snapshot(vec![pc.clone()]);
        assert!(!library_has_browser_games(&snap));

        let mut web =
            (*crate::filter::tests::game(Runner::Sideload, "2", "GeForce NOW", true)).clone();
        web.browser_url = Some("https://play.geforcenow.com".into());
        let snap_web = crate::filter::tests::snapshot(vec![pc, std::sync::Arc::new(web)]);
        assert!(library_has_browser_games(&snap_web));
    }

    #[test]
    fn filters_fit_under_their_button_or_fall_back_near_bottom() {
        let rem = px(16.);
        let chrome = rem * PANEL_CHROME_REM;
        let visible = Bounds::new(point(px(0.), px(0.)), size(px(1000.), px(800.)));
        // Room under a button near the top: fills the remaining space below it.
        assert_eq!(
            filters_body_height(visible, Some(px(100.)), rem),
            px(800.) - px(100.) - chrome
        );
        // Near the foot: too little room below, so it falls back to half the
        // window (minus chrome) and lets the positioner shift upward.
        assert_eq!(
            filters_body_height(visible, Some(px(700.)), rem),
            px(400.) - chrome
        );
        // Never taller than the window, even before the trigger is measured.
        let short = Bounds::new(point(px(0.), px(0.)), size(px(1000.), px(200.)));
        assert!(filters_body_height(short, None, rem) <= px(200.));
    }

    #[test]
    fn filters_fit_1080p_below_trigger_including_top_sections() {
        let rem = px(16.);
        let chrome = rem * PANEL_CHROME_REM;
        // Header toolbar (140px), Continue playing (440px), Favourites (560px).
        for anchor_bottom in [px(140.), px(440.), px(560.)] {
            for height in [px(1080.), px(1032.)] {
                let visible = Bounds::new(point(px(0.), px(0.)), size(px(1920.), height));
                let max_h = filters_body_height(visible, Some(anchor_bottom), rem);
                assert!(
                    max_h >= rem * 10.,
                    "unexpectedly cramped for anchor={anchor_bottom:?} height={height:?}: {max_h:?}"
                );
                assert!(
                    anchor_bottom + max_h + chrome <= visible.bottom(),
                    "overflows {height:?} (would push popover over trigger): anchor={anchor_bottom:?} max_h={max_h:?}"
                );
            }
        }
    }

    #[test]
    fn persisted_part_ignores_query() {
        let a = ViewFilter {
            query: "a".into(),
            ..Default::default()
        };
        let b = ViewFilter {
            query: "b".into(),
            ..Default::default()
        };
        assert_eq!(
            ViewFilter {
                query: String::new(),
                ..a
            },
            ViewFilter {
                query: String::new(),
                ..b
            }
        );
    }
}
