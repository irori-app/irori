//! The library's view criteria as an observable model, and the Filters
//! popover that edits them. A search field at its top finds any filter
//! value by name ([`crate::value_search`]), with how many games carry it.

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonGroup, ButtonVariants as _};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, Selectable as _, Sizable as _, StyledExt as _,
    h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, Bounds, Context, Entity, Global, InteractiveElement as _,
    IntoElement, ParentElement as _, Pixels, Render, SharedString, StatefulInteractiveElement as _,
    Styled as _, Subscription, Task, WeakEntity, Window, canvas, div, px,
};
use irori_core::{GameMode, InstallPlatform};
use irori_services::EffectiveStatus;
use irori_ui::i18n::{t, t_args};
use irori_ui::{models, play_status};

use crate::filter::{
    self, CategoryFilter, Company, HiddenMode, InstallFilter, ReleaseWindow, ViewFilter,
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
    /// The systems the library has games of.
    systems: crate::filter::PerRevision<Vec<String>>,
    /// The genres the library's games have, as (key, name).
    genres: crate::filter::PerRevision<Vec<(String, String)>>,
    /// The commonest store tags, as (key, name).
    tags: crate::filter::PerRevision<Vec<(String, String)>>,
    /// The commonest developers and publishers, as (key, name).
    developers: crate::filter::PerRevision<Vec<(String, String)>>,
    publishers: crate::filter::PerRevision<Vec<(String, String)>>,
    /// Where the panel was last drawn, for the room under it.
    top: Option<Pixels>,
    /// The field that finds a filter value by name, made at the first draw
    /// (it needs the window).
    search: Option<Entity<InputState>>,
    /// Every filter value with its count, for the search field.
    values: ValueCache,
    _observe: Vec<Subscription>,
}

/// Room the filter sections may take: from `top` (the panel's measured top,
/// unknown before its first paint) to the foot of the `visible` window, less
/// the panel's footer and margins, but never under half the window, where
/// the popover is moved up to fit instead.
fn filters_height(visible: Bounds<Pixels>, top: Option<Pixels>, rem: Pixels) -> Pixels {
    // The search field deduction (2.5rem), gaps, footer with reset button,
    // popover padding, and a safety margin below the panel.
    let reserved = rem * 6.;
    let most = (visible.size.height - reserved).max(rem * 8.);
    let top = top.unwrap_or(visible.origin.y + rem * 10.);
    let least = (visible.size.height * 0.5 - reserved).max(rem * 4.);
    (visible.bottom() - top - reserved).clamp(least, most)
}

impl FiltersPanel {
    pub fn new(model: Entity<FilterModel>, cx: &mut Context<Self>) -> Self {
        let library = models(cx).library.clone();
        Self {
            systems: Default::default(),
            genres: Default::default(),
            tags: Default::default(),
            developers: Default::default(),
            publishers: Default::default(),
            top: None,
            search: None,
            values: ValueCache::default(),
            _observe: vec![
                cx.observe(&model, |_, _, cx| cx.notify()),
                cx.observe(&library, |_, _, cx| cx.notify()),
            ],
            model,
        }
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
            .id("library-filter-values")
            .max_h(max_h)
            .overflow_y_scrollbar()
            .pr_3()
            .gap_2()
            .when(rows.is_empty(), |el| {
                el.child(
                    div()
                        .text_sm()
                        .text_color(muted)
                        .child(t_args("library.filters.no_values", &[("query", query)])),
                )
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
            })
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
            let leave_out = Button::new(SharedString::from(format!("filter-category-out-{ix}")))
                .xsmall()
                .ghost()
                .icon(IconName::Ban)
                .selected(excluded)
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
                            Icon::new(irori_ui::categories::icon(&entry.category))
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
                    .child(section(t("library.filters.section.categories")))
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

fn section(title: SharedString) -> impl IntoElement {
    div().text_xs().font_medium().child(title)
}

impl FiltersPanel {
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
        Some(
            v_flex()
                .gap_2()
                .child(section(t("library.filters.section.system")))
                .child(
                    h_flex()
                        .flex_wrap()
                        .gap_1()
                        .children(present.into_iter().map(|id| {
                            let label = irori_engines::systems::get(&id)
                                .map_or(id.clone(), |s| s.short.clone());
                            let selected = f.systems.contains(&id);
                            Button::new(SharedString::from(format!("system-{id}")))
                                .xsmall()
                                .outline()
                                .label(label)
                                .selected(selected)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    let id = id.clone();
                                    this.set(
                                        move |f| {
                                            if !f.systems.remove(&id) {
                                                f.systems.insert(id);
                                            }
                                        },
                                        cx,
                                    )
                                }))
                        })),
                )
                .into_any_element(),
        )
    }
}

impl FiltersPanel {
    /// One chip per play status; several may be picked.
    fn render_statuses(&self, f: &ViewFilter, cx: &mut Context<Self>) -> AnyElement {
        v_flex()
            .gap_2()
            .child(section(t("library.filters.section.play_status")))
            .child(
                h_flex()
                    .flex_wrap()
                    .gap_1()
                    .children(EffectiveStatus::ALL.into_iter().map(|status| {
                        Button::new(SharedString::from(format!("play-status-{status:?}")))
                            .xsmall()
                            .outline()
                            .icon(Icon::new(play_status::icon(status)))
                            .label(play_status::label(status))
                            .selected(f.statuses.contains(&status))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.set(
                                    move |f| {
                                        if !f.statuses.remove(&status) {
                                            f.statuses.insert(status);
                                        }
                                    },
                                    cx,
                                )
                            }))
                    })),
            )
            .into_any_element()
    }

    /// Controller support: any or full; a second click shows every game.
    fn render_controller(&self, f: &ViewFilter, cx: &mut Context<Self>) -> AnyElement {
        use irori_core::ControllerSupport;
        v_flex()
            .gap_2()
            .child(section(t("library.filters.section.controller")))
            .child(
                h_flex().flex_wrap().gap_1().children(
                    [ControllerSupport::Partial, ControllerSupport::Full]
                        .into_iter()
                        .map(|level| {
                            Button::new(SharedString::from(format!("controller-{level:?}")))
                                .xsmall()
                                .outline()
                                .icon(Icon::new(IconName::Gamepad2))
                                .label(crate::filter::controller_label(level))
                                .selected(f.controller == Some(level))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.set(
                                        move |f| {
                                            f.controller =
                                                (f.controller != Some(level)).then_some(level)
                                        },
                                        cx,
                                    )
                                }))
                        }),
                ),
            )
            .into_any_element()
    }

    /// Game modes: games with any of the chosen ones are shown.
    fn render_modes(&self, f: &ViewFilter, cx: &mut Context<Self>) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        v_flex()
            .gap_2()
            .child(section(irori_ui::game_mode::title()))
            .child(
                h_flex()
                    .flex_wrap()
                    .gap_1()
                    .children(GameMode::ALL.into_iter().map(|mode| {
                        Button::new(SharedString::from(format!("game-mode-{}", mode.id())))
                            .xsmall()
                            .outline()
                            .icon(Icon::new(irori_ui::game_mode::icon(mode)))
                            .label(irori_ui::game_mode::label(mode))
                            .selected(f.modes.contains(&mode))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.set(
                                    move |f| {
                                        if !f.modes.remove(&mode) {
                                            f.modes.insert(mode);
                                        }
                                    },
                                    cx,
                                )
                            }))
                    })),
            )
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
        Some(
            v_flex()
                .gap_2()
                .child(section(t("library.filters.section.genre")))
                .child(
                    h_flex()
                        .flex_wrap()
                        .gap_1()
                        .children(genres.into_iter().map(|(key, name)| {
                            let selected = picked.contains(&key);
                            Button::new(SharedString::from(format!("genre-{key}")))
                                .xsmall()
                                .outline()
                                .label(name.clone())
                                .selected(selected)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    let (key, name) = (key.clone(), name.clone());
                                    this.set(
                                        move |f| {
                                            let before = f.genres.len();
                                            f.genres.retain(|g| filter::genre_key(g) != key);
                                            if f.genres.len() == before {
                                                f.genres.insert(name);
                                            }
                                        },
                                        cx,
                                    )
                                }))
                        })),
                )
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
        Some(
            v_flex()
                .gap_2()
                .child(section(t("library.filters.section.steam_tags")))
                .child(h_flex().flex_wrap().gap_1().children(tags.into_iter().map(
                    |(key, name)| {
                        let selected = picked.contains(&key);
                        Button::new(SharedString::from(format!("tag-{key}")))
                            .xsmall()
                            .outline()
                            .label(name.clone())
                            .selected(selected)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                let (key, name) = (key.clone(), name.clone());
                                this.set(
                                    move |f| {
                                        let before = f.tags.len();
                                        f.tags.retain(|t| tag_key(t) != key);
                                        if f.tags.len() == before {
                                            f.tags.insert(name);
                                        }
                                    },
                                    cx,
                                )
                            }))
                    },
                )))
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
        Some(
            v_flex()
                .gap_2()
                .child(section(t(title)))
                .child(
                    h_flex()
                        .flex_wrap()
                        .gap_1()
                        .children(offered.into_iter().map(|(key, name)| {
                            Button::new(SharedString::from(format!("{id}-{key}")))
                                .xsmall()
                                .outline()
                                .label(name.clone())
                                .selected(picked.contains(&key))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    let (key, name) = (key.clone(), name.clone());
                                    this.set(
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
                                }))
                        })),
                )
                .into_any_element(),
        )
    }

    /// Hide / show / only hidden games, behind the PIN when one is set, and
    /// the PIN itself: set it, lock again, or change it.
    fn render_hidden(
        &self,
        f: &ViewFilter,
        hidden: [(HiddenMode, SharedString); 3],
        values: [HiddenMode; 3],
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
            .gap_2()
            .child(
                h_flex()
                    .gap_2()
                    .justify_between()
                    .child(section(t("library.filters.section.hidden")))
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
            .child(
                ButtonGroup::new("filter-hidden")
                    .small()
                    .outline()
                    .children(hidden.into_iter().enumerate().map(|(ix, (v, label))| {
                        Button::new(SharedString::from(format!("hidden-{ix}")))
                            .label(label)
                            .when(locked && v != HiddenMode::Hide, |b| b.icon(IconName::Lock))
                            .selected(shown == v)
                    }))
                    .on_click(cx.listener(move |this, clicked: &Vec<usize>, window, cx| {
                        let Some(&ix) = clicked.first() else {
                            return;
                        };
                        let v = values[ix];
                        if v == HiddenMode::Hide || !hidden_lock::locked(cx) {
                            this.set(move |f| f.hidden = v, cx);
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
                    })),
            )
            .into_any_element()
    }

    /// The age limit: "Any" or the youngest age every rating must allow,
    /// and, with a limit, whether games no board rated are shown.
    fn render_age(&self, f: &ViewFilter, cx: &mut Context<Self>) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let limits: Vec<Option<u8>> = std::iter::once(None)
            .chain(filter::AGE_LIMITS.into_iter().map(Some))
            .collect();
        let values = limits.clone();
        v_flex()
            .gap_2()
            .child(section(t("library.filters.section.age")))
            .child(
                ButtonGroup::new("filter-age")
                    .small()
                    .outline()
                    .children(limits.into_iter().enumerate().map(|(ix, limit)| {
                        let label: SharedString = match limit {
                            None => t("library.filters.age.any"),
                            Some(age) => age.to_string().into(),
                        };
                        Button::new(SharedString::from(format!("age-{ix}")))
                            .label(label)
                            .selected(f.max_age == limit)
                    }))
                    .on_click(cx.listener(move |this, clicked: &Vec<usize>, _, cx| {
                        if let Some(&ix) = clicked.first() {
                            let limit = values[ix];
                            this.set(move |f| f.max_age = limit, cx);
                        }
                    })),
            )
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
        let systems = self.render_systems(&f, cx);
        let genres = self.render_genres(&f, cx);
        let tags = self.render_tags(&f, cx);
        let developers = self.render_companies(&f, Company::Developer, cx);
        let publishers = self.render_companies(&f, Company::Publisher, cx);
        let categories = {
            let lib = models(cx).library.read(cx);
            irori_ui::categories::entries(&lib.user, &lib.categories)
        };
        let muted = cx.theme().muted_foreground;

        let platforms: [(Option<InstallPlatform>, SharedString); 5] = [
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
            (
                Some(InstallPlatform::Browser),
                t("library.platform.browser"),
            ),
        ];
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
        let platform_values = platforms.clone().map(|(p, _)| p);
        let install_values = installs.clone().map(|(v, _)| v);
        let hidden_values = hidden.clone().map(|(v, _)| v);

        // The popover opens under its button and is only moved, never
        // shrunk, to stay in the window: the sections scroll within the room
        // below the button (or half the window, where it is moved up).
        let visible = irori_ui::style::visible_bounds(window);
        let rem = window.rem_size();
        // Less the search field above the sections.
        let max_h = (filters_height(visible, self.top, rem) - rem * 2.5).max(rem * 4.);
        let view = cx.entity().downgrade();
        let measure = canvas(
            move |bounds, _, cx| {
                view.update(cx, |this, cx| {
                    let moved = this
                        .top
                        .is_none_or(|top| (top - bounds.origin.y).abs() > px(0.5));
                    if moved {
                        this.top = Some(bounds.origin.y);
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

        let sections = v_flex()
            .id("library-filters-sections")
            .max_h(max_h)
            .overflow_y_scrollbar()
            .pr_3()
            .gap_4()
            .when_some(systems, |el, systems| el.child(systems))
            .child(
                v_flex()
                    .gap_2()
                    .child(section(t("library.filters.section.platform")))
                    .child(
                        ButtonGroup::new("filter-platform")
                            .small()
                            .outline()
                            .children(platforms.into_iter().enumerate().map(|(ix, (p, label))| {
                                Button::new(SharedString::from(format!("platform-{ix}")))
                                    .label(label)
                                    .selected(f.platform == p)
                            }))
                            .on_click(cx.listener(move |this, clicked: &Vec<usize>, _, cx| {
                                if let Some(&ix) = clicked.first() {
                                    let p = platform_values[ix];
                                    this.set(move |f| f.platform = p, cx);
                                }
                            })),
                    ),
            )
            .child(
                v_flex()
                    .gap_2()
                    .child(section(t("library.filters.section.install")))
                    .child(
                        ButtonGroup::new("filter-install")
                            .small()
                            .outline()
                            .children(installs.into_iter().enumerate().map(|(ix, (v, label))| {
                                Button::new(SharedString::from(format!("install-{ix}")))
                                    .label(label)
                                    .selected(f.install == v)
                            }))
                            .on_click(cx.listener(move |this, clicked: &Vec<usize>, _, cx| {
                                if let Some(&ix) = clicked.first() {
                                    let v = install_values[ix];
                                    this.set(move |f| f.install = v, cx);
                                }
                            })),
                    ),
            )
            .child(
                v_flex()
                    .gap_2()
                    .child(section(t("library.filters.section.released")))
                    .child(
                        ButtonGroup::new("filter-released")
                            .small()
                            .outline()
                            .children(released.iter().enumerate().map(|(ix, w)| {
                                Button::new(SharedString::from(format!("released-{ix}")))
                                    .label(w.label())
                                    .selected(f.released == *w)
                            }))
                            .on_click(cx.listener(move |this, clicked: &Vec<usize>, _, cx| {
                                if let Some(&ix) = clicked.first() {
                                    let w = released[ix];
                                    this.set(move |f| f.released = w, cx);
                                }
                            })),
                    ),
            )
            .child(self.render_hidden(&f, hidden, hidden_values, cx))
            .child(self.render_modes(&f, cx))
            .child(self.render_controller(&f, cx))
            .when_some(genres, |el, genres| el.child(genres))
            .when_some(tags, |el, tags| el.child(tags))
            .when_some(developers, |el, d| el.child(d))
            .when_some(publishers, |el, p| el.child(p))
            .child(self.render_age(&f, cx))
            .child(self.render_statuses(&f, cx))
            .child(self.render_categories(&f, &categories, cx))
            .child(
                v_flex()
                    .gap_2()
                    .child(section(t("library.filters.section.show_only")))
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
            .relative()
            .w_96()
            .gap_3()
            .child(measure)
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
                                this.set(ViewFilter::reset_filters, cx)
                            })),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::{point, size};

    #[test]
    fn filters_fit_under_their_button_or_in_half_the_window() {
        let rem = px(16.);
        let visible = Bounds::new(point(px(0.), px(0.)), size(px(1000.), px(800.)));
        // Room under a button near the top: the rest of the window.
        assert_eq!(
            filters_height(visible, Some(px(100.)), rem),
            px(800. - 100. - 96.)
        );
        // Near the foot: half the window, and the popover moves up.
        assert_eq!(filters_height(visible, Some(px(700.)), rem), px(400. - 96.));
        // Never taller than the window, even before it was measured.
        let short = Bounds::new(point(px(0.), px(0.)), size(px(1000.), px(200.)));
        assert!(filters_height(short, None, rem) <= px(200.));
    }

    #[test]
    fn filters_fit_1080p_at_100_percent_scaling() {
        let rem = px(16.);
        let top = px(140.);
        for height in [px(1080.), px(1032.)] {
            let visible = Bounds::new(point(px(0.), px(0.)), size(px(1920.), height));
            let max_h = (filters_height(visible, Some(top), rem) - rem * 2.5).max(rem * 4.);
            assert!(max_h > rem * 20., "unexpectedly cramped: {max_h:?}");
            assert!(
                top + max_h + rem * 8. <= visible.bottom(),
                "overflows {height:?}: top={top:?} max_h={max_h:?}"
            );
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
