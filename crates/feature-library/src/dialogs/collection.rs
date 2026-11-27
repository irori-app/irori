//! Steam-style hybrid collection dialog ("New collection" / "Edit collection").
//!
//! Creates or edits a [`SmartCategory`] combining optional dynamic tag/genre
//! rules (`Match::Any` across `Rule::Tags` and `Rule::Genre`) with manual
//! game inclusions (`included_games`) and exclusions (`excluded_games`), plus
//! a curated colour and Lucide icon auto-suggested from picked tags.

use std::cell::Cell;
use std::collections::{BTreeSet, HashSet};

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, Sizable as _, StyledExt as _, WindowExt as _, h_flex,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, Focusable as _, InteractiveElement as _,
    IntoElement, ParentElement as _, Render, SharedString, StatefulInteractiveElement as _,
    Styled as _, Subscription, Window, div, px, rems,
};
use irori_services::search::tag_key;
use irori_services::smart_categories::{
    IndexKey, count_matches, format_hex_colour, matches_rules_only,
};
use irori_services::{Condition, Match, Rule, SmartCategory};
use irori_ui::components::{ColorWheel, ColorWheelLabels, ColorWheelPick, ColorWheelReset};
use irori_ui::i18n::{t, t_args};
use irori_ui::style::{KeyboardFocusable as _, TEXT_CONTRAST, legible};
use irori_ui::{backend, models};

use super::categories::{NameError, validate_name};
use super::collection_look::{
    IconPickerPopover, collection_quick_swatches, initial_wheel_hex, render_colour_picker,
    render_icon_picker,
};
use super::{dialog_width, text};
use crate::views::{self, TabKey};

/// Popular tag and genre suggestions offered alongside tags found in the user's library.
pub const POPULAR_TAGS: &[&str] = &[
    "Souls-like",
    "Metroidvania",
    "Roguelike",
    "Roguelite",
    "Anime",
    "JRPG",
    "CRPG",
    "ARPG",
    "Action",
    "Adventure",
    "RPG",
    "Indie",
    "Strategy",
    "Simulation",
    "Horror",
    "Survival",
    "Survival Horror",
    "Psychological Horror",
    "Open World",
    "Sandbox",
    "Platformer",
    "2D Platformer",
    "3D Platformer",
    "Puzzle",
    "FPS",
    "Third-Person Shooter",
    "Shooter",
    "Looter Shooter",
    "Tactical",
    "Turn-Based",
    "Deckbuilder",
    "Bullet Hell",
    "Hack and Slash",
    "Beat 'em up",
    "Stealth",
    "RTS",
    "4X",
    "Grand Strategy",
    "City Builder",
    "Tower Defense",
    "Management",
    "Farming Sim",
    "Life Sim",
    "Cozy",
    "Relaxing",
    "Visual Novel",
    "Point & Click",
    "Narrative",
    "Story Rich",
    "Choices Matter",
    "Mystery",
    "Detective",
    "Cyberpunk",
    "Sci-Fi",
    "Fantasy",
    "Dark Fantasy",
    "Post-Apocalyptic",
    "Medieval",
    "Pixel Graphics",
    "Retro",
    "Isometric",
    "Top-Down",
    "Co-op",
    "Local Co-op",
    "Online Co-op",
    "Multiplayer",
    "PvP",
    "Singleplayer",
    "MMO",
    "Racing",
    "Sports",
    "Fighting",
    "Rhythm",
    "VR",
    "Arcade",
    "Casual",
    "Free to Play",
    "Crafting",
    "Base Building",
    "Exploration",
];

/// Max search results shown at once in the manual game picker.
const MAX_GAME_RESULTS: usize = 60;

/// Whether `category` is a simple `Match::Any` tag/genre (or manual-only)
/// collection that can be edited safely in [`CollectionDialog`] without losing
/// other rule types or `Match::All` semantics.
pub(crate) fn is_simple_tag_collection(category: &SmartCategory) -> bool {
    !category.is_unreadable()
        && category.matching == Match::Any
        && category
            .rules
            .iter()
            .all(|c| matches!(c.rule, Some(Rule::Tags { .. } | Rule::Genre { .. })))
}

/// Resolve a game key to its display title across primary entries and
/// secondary copies of combined multi-store entries.
pub(crate) fn excluded_game_title(snapshot: &irori_services::LibrarySnapshot, key: &str) -> String {
    snapshot
        .games
        .iter()
        .find(|g| {
            g.id().key() == key
                || snapshot
                    .copies_of(&g.id())
                    .iter()
                    .any(|copy| copy.key() == key)
        })
        .map(|g| g.title.clone())
        .unwrap_or_else(|| key.to_owned())
}

/// All copy keys of the library entry headed by `game` (itself alone when uncombined).
fn entry_copy_keys(
    snapshot: &irori_services::LibrarySnapshot,
    game: &irori_core::GameInfo,
) -> Vec<String> {
    let id = game.id();
    let copies = snapshot.copies_of(&id);
    if copies.is_empty() {
        vec![id.key()]
    } else {
        copies.iter().map(irori_core::GameId::key).collect()
    }
}

/// Extract every tag and genre currently matched by `category`, deduplicated by [`tag_key`].
pub(crate) fn extract_terms(category: &SmartCategory) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for cond in &category.rules {
        let items: &[String] = match &cond.rule {
            Some(Rule::Tags { tags }) => tags,
            Some(Rule::Genre { genres }) => genres,
            _ => continue,
        };
        for raw in items {
            let trimmed = raw.trim();
            let key = tag_key(trimmed);
            if !key.is_empty() && seen.insert(key) {
                out.push(trimmed.to_owned());
            }
        }
    }
    out
}

/// Build a [`SmartCategory`] from `base`, replacing its name, colour, icon,
/// tag/genre rules, manually included games, and manually excluded games while
/// preserving any unknown fields and `leave_out` rows.
pub(crate) fn build_collection_category(
    base: &SmartCategory,
    name: String,
    colour: Option<String>,
    icon: Option<String>,
    terms: &[String],
    included_games: &BTreeSet<String>,
    excluded_games: &BTreeSet<String>,
) -> SmartCategory {
    let mut next = base.clone();
    next.name = name;
    next.matching = Match::Any;
    next.look.colour = colour;
    next.look.icon = icon.filter(|slug| slug != "sparkles");
    next.included_games = included_games.clone();
    next.excluded_games = excluded_games.clone();
    let cleaned: Vec<String> = {
        let mut seen = HashSet::new();
        let mut list = Vec::new();
        for term in terms {
            let trimmed = term.trim();
            let key = tag_key(trimmed);
            if !key.is_empty() && seen.insert(key) {
                list.push(trimmed.to_owned());
            }
        }
        list
    };
    next.manual_only = cleaned.is_empty();
    next.rules = if cleaned.is_empty() {
        Vec::new()
    } else {
        vec![
            Condition::new(Rule::Tags {
                tags: cleaned.clone(),
            }),
            Condition::new(Rule::Genre { genres: cleaned }),
        ]
    };
    next
}

/// Combine library tags, library genres, and [`POPULAR_TAGS`] into a deduplicated,
/// alphabetically sorted list of `(key, display_name)`.
pub(crate) fn available_terms(library: &irori_services::LibrarySnapshot) -> Vec<(String, String)> {
    let mut map: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    for (k, name) in crate::filter::present_genres(library) {
        let key = tag_key(&name);
        if !key.is_empty() {
            map.entry(key).or_insert(name);
        } else if !k.is_empty() {
            map.entry(k).or_insert(name);
        }
    }
    for tag in library.games.iter().flat_map(|g| g.store_tags.iter()) {
        let trimmed = tag.trim();
        let key = tag_key(trimmed);
        if !key.is_empty() {
            map.entry(key).or_insert_with(|| trimmed.to_owned());
        }
    }
    for &popular in POPULAR_TAGS {
        let key = tag_key(popular);
        map.entry(key).or_insert_with(|| popular.to_owned());
    }
    let mut out: Vec<(String, String)> = map.into_iter().collect();
    out.sort_by_cached_key(|(k, name)| (name.to_lowercase(), k.clone()));
    out
}

/// Delete smart category `id`, remove its tab metadata, and push an Undo notification.
pub(crate) fn delete_collection_with_undo(id: String, window: &mut Window, cx: &mut App) {
    let saved = models(cx)
        .library
        .read(cx)
        .user
        .smart_categories
        .get(&id)
        .cloned();
    let Some(removed) = saved else {
        return;
    };
    let name = removed.name.clone();
    let b = backend(cx);
    let delete_id = id.clone();
    irori_ui::state::run_infallible(cx, async move {
        b.userdata.delete_smart_category(&delete_id);
    })
    .detach();
    let tab_key = TabKey::Collection(id.clone());
    let saved_prefs = views::saved_views(cx).update(cx, |m, cx| {
        m.update(
            |v| {
                let prefs = v.take_tab_prefs(&tab_key);
                v.forget_tab(&tab_key);
                prefs
            },
            cx,
        )
    });
    if let Some(filters) = crate::filters_panel::active_filters(cx) {
        let forget_id = id.clone();
        filters.update(cx, |m, cx| m.update(|f| f.forget_smart(&forget_id), cx));
    }

    let restore_backend = backend(cx);
    let restore_id = id.clone();
    let restore_cat = removed.clone();
    window.push_notification(
        Notification::info(t_args("library.collection.deleted", &[("name", &name)]))
            .action(move |_, _, cx| {
                let note = cx.entity().downgrade();
                let b = restore_backend.clone();
                let id = restore_id.clone();
                let cat = restore_cat.clone();
                let prefs = saved_prefs.clone();
                Button::new("undo-delete-collection")
                    .label(t("library.bulk.undo"))
                    .small()
                    .on_click(move |_, window, cx| {
                        let b = b.clone();
                        let id_copy = id.clone();
                        let cat_copy = cat.clone();
                        irori_ui::state::run_infallible(cx, async move {
                            b.userdata.save_smart_category(Some(&id_copy), cat_copy);
                        })
                        .detach();
                        let key = TabKey::Collection(id.clone());
                        let prefs = prefs.clone();
                        views::saved_views(cx).update(cx, |m, cx| {
                            m.update(
                                |v| {
                                    v.ensure_tab_visible(&key);
                                    if let Some(prefs) = prefs {
                                        v.restore_tab_prefs(&key, prefs);
                                    }
                                },
                                cx,
                            )
                        });
                        let _ = note.update(cx, |n, cx| n.dismiss(window, cx));
                    })
            })
            .autohide(true),
        cx,
    );
}

/// The hybrid Steam-style collection editor dialog.
pub struct CollectionDialog {
    id: Option<String>,
    base: SmartCategory,
    name: Entity<InputState>,
    tag_input: Entity<InputState>,
    game_search: Entity<InputState>,
    colour: Option<String>,
    color_wheel: Entity<ColorWheel>,
    color_popover_open: bool,
    icon: Option<String>,
    icon_popover: Entity<IconPickerPopover>,
    icon_popover_open: bool,
    icon_customized: bool,
    picked: Vec<String>,
    included_games: BTreeSet<String>,
    excluded_games: BTreeSet<String>,
    suggestions: Vec<(String, String)>,
    preview: Option<(SmartCategory, IndexKey, usize)>,
    _subscriptions: Vec<Subscription>,
}

impl CollectionDialog {
    pub fn new(
        id: Option<String>,
        base: SmartCategory,
        initial_games: Vec<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let picked = extract_terms(&base);
        let colour = base.look.colour.clone();
        let icon_customized = base.look.icon.is_some();
        let icon =
            base.look.icon.clone().or_else(|| {
                irori_ui::categories::suggest_icon_for_terms(&picked).map(str::to_owned)
            });
        let mut included_games = base.included_games.clone();
        let mut excluded_games = base.excluded_games.clone();
        for key in initial_games {
            if !key.is_empty() {
                excluded_games.remove(&key);
                included_games.insert(key);
            }
        }
        let name = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(t("library.collection.name_placeholder"))
                .default_value(base.name.clone())
        });
        let tag_input = cx.new(|cx| {
            InputState::new(window, cx).placeholder(t("library.collection.tag_placeholder"))
        });
        let game_search = cx.new(|cx| {
            InputState::new(window, cx).placeholder(t("library.collection.game_search_placeholder"))
        });
        let initial_hex = initial_wheel_hex(colour.as_deref(), cx);
        let fallback_hex = initial_wheel_hex(None, cx);
        let quick_swatches = collection_quick_swatches(cx);
        let weak_for_wheel = cx.entity().downgrade();
        let color_wheel = cx.new(|cx| {
            ColorWheel::new(
                &initial_hex,
                &fallback_hex,
                ColorWheelReset::Clear {
                    has_value: colour.is_some(),
                },
                None,
                ColorWheelLabels::standard(
                    t("library.categories.colour"),
                    t("ui.category.colour.none"),
                ),
                quick_swatches,
                move |pick, _released, _window, cx| {
                    let _ = weak_for_wheel.update(cx, |this, cx| {
                        match pick {
                            ColorWheelPick::Hex(hex) => {
                                this.colour = format_hex_colour(&hex);
                            }
                            ColorWheelPick::Named(name) => {
                                this.colour = Some(name);
                                this.color_popover_open = false;
                            }
                            ColorWheelPick::Cleared => {
                                this.colour = None;
                                this.color_popover_open = false;
                            }
                        }
                        this.sync_icon_popover(cx);
                        cx.notify();
                    });
                },
                window,
                cx,
            )
        });
        let weak_for_icon = cx.entity().downgrade();
        let icon_popover = cx.new(|cx| {
            IconPickerPopover::new(
                icon.as_deref(),
                colour.clone(),
                move |slug, _window, cx| {
                    let _ = weak_for_icon.update(cx, |this, cx| {
                        this.set_icon(slug, cx);
                        this.icon_popover_open = false;
                    });
                },
                window,
                cx,
            )
        });
        let suggestions = {
            let lib = models(cx).library.read(cx);
            available_terms(&lib.snapshot)
        };
        let library = models(cx).library.clone();
        let subscriptions = vec![
            cx.observe(&library, |this, library, cx| {
                this.suggestions = available_terms(&library.read(cx).snapshot);
                cx.notify();
            }),
            cx.subscribe_in(&name, window, |this, _, event, window, cx| match event {
                InputEvent::Change => cx.notify(),
                InputEvent::PressEnter { .. } => this.save(window, cx),
                _ => {}
            }),
            cx.subscribe_in(
                &tag_input,
                window,
                |this, _, event, window, cx| match event {
                    InputEvent::Change => cx.notify(),
                    InputEvent::PressEnter { .. } => this.add_typed_tag(window, cx),
                    _ => {}
                },
            ),
            cx.subscribe_in(&game_search, window, |_, _, event, _, cx| {
                if matches!(event, InputEvent::Change) {
                    cx.notify();
                }
            }),
        ];
        Self {
            id,
            base,
            name,
            tag_input,
            game_search,
            colour,
            color_wheel,
            color_popover_open: false,
            icon,
            icon_popover,
            icon_popover_open: false,
            icon_customized,
            picked,
            included_games,
            excluded_games,
            suggestions,
            preview: None,
            _subscriptions: subscriptions,
        }
    }

    fn sync_icon_popover(&self, cx: &mut Context<Self>) {
        let icon = self.icon.clone();
        let colour = self.colour.clone();
        self.icon_popover
            .update(cx, |p, cx| p.sync(icon.as_deref(), colour, cx));
    }

    fn set_icon(&mut self, slug: &str, cx: &mut Context<Self>) {
        self.icon = (slug != "sparkles").then(|| slug.to_owned());
        self.icon_customized = true;
        self.sync_icon_popover(cx);
        cx.notify();
    }

    fn set_colour(&mut self, colour: Option<String>, cx: &mut Context<Self>) {
        self.colour = colour;
        self.color_popover_open = false;
        self.sync_icon_popover(cx);
        cx.notify();
    }

    fn on_colour_popover_toggle(
        &mut self,
        open: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.color_popover_open = open;
        if open {
            let hex = initial_wheel_hex(self.colour.as_deref(), cx);
            let fallback = initial_wheel_hex(None, cx);
            let swatches = collection_quick_swatches(cx);
            let has_value = self.colour.is_some();
            self.color_wheel.update(cx, |w, cx| {
                w.set_quick_swatches(swatches, cx);
                w.rebind(
                    t("library.categories.colour"),
                    &hex,
                    &fallback,
                    ColorWheelReset::Clear { has_value },
                    None,
                    window,
                    cx,
                );
            });
        }
        cx.notify();
    }

    fn on_icon_popover_toggle(&mut self, open: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.icon_popover_open = open;
        if open {
            let icon = self.icon.clone();
            let colour = self.colour.clone();
            self.icon_popover.update(cx, |p, cx| {
                p.clear_search(window, cx);
                p.sync(icon.as_deref(), colour, cx);
            });
        }
        cx.notify();
    }

    fn sync_auto_icon(&mut self) {
        if !self.icon_customized {
            self.icon =
                irori_ui::categories::suggest_icon_for_terms(&self.picked).map(str::to_owned);
        }
    }

    fn toggle_term(&mut self, label: String, cx: &mut Context<Self>) {
        let key = tag_key(&label);
        if key.is_empty() {
            return;
        }
        let before = self.picked.len();
        self.picked.retain(|t| tag_key(t) != key);
        if self.picked.len() == before {
            self.picked.push(label.trim().to_owned());
        }
        self.sync_auto_icon();
        cx.notify();
    }

    fn add_typed_tag(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let raw = text(&self.tag_input, cx);
        if raw.is_empty() {
            return;
        }
        for part in raw.split(',') {
            let trimmed = part.trim();
            let key = tag_key(trimmed);
            if !key.is_empty() && !self.picked.iter().any(|t| tag_key(t) == key) {
                self.picked.push(trimmed.to_owned());
            }
        }
        self.tag_input
            .update(cx, |s, cx| s.set_value(String::new(), window, cx));
        self.sync_auto_icon();
        cx.notify();
    }

    fn add_manual_game(
        &mut self,
        primary_key: String,
        copy_keys: &[String],
        cx: &mut Context<Self>,
    ) {
        for key in copy_keys {
            self.excluded_games.remove(key);
        }
        self.included_games.insert(primary_key);
        cx.notify();
    }

    fn remove_manual_game(&mut self, copy_keys: &[String], cx: &mut Context<Self>) {
        let mut changed = false;
        for key in copy_keys {
            changed |= self.included_games.remove(key);
        }
        if changed {
            cx.notify();
        }
    }

    fn exclude_game(&mut self, primary_key: String, copy_keys: &[String], cx: &mut Context<Self>) {
        for key in copy_keys {
            self.included_games.remove(key);
        }
        self.excluded_games.insert(primary_key);
        cx.notify();
    }

    fn restore_excluded(&mut self, copy_keys: &[String], cx: &mut Context<Self>) {
        let mut changed = false;
        for key in copy_keys {
            changed |= self.excluded_games.remove(key);
        }
        if changed {
            cx.notify();
        }
    }

    fn draft(&self, cx: &App) -> SmartCategory {
        build_collection_category(
            &self.base,
            text(&self.name, cx),
            self.colour.clone(),
            self.icon.clone(),
            &self.picked,
            &self.included_games,
            &self.excluded_games,
        )
    }

    fn name_result(&self, cx: &App) -> Result<String, NameError> {
        let user = &models(cx).library.read(cx).user;
        let existing: Vec<String> = user
            .categories_in_order()
            .into_iter()
            .map(|(_, n)| n)
            .collect();
        let renaming = self.id.as_ref().map(|_| self.base.name.as_str());
        match validate_name(&text(&self.name, cx), &existing, renaming) {
            Err(NameError::Unchanged) => Ok(self.base.name.clone()),
            other => other,
        }
    }

    fn count(&mut self, draft: &SmartCategory, cx: &App) -> usize {
        let inputs = backend(cx).category_inputs();
        let lib = models(cx).library.read(cx);
        let key = IndexKey::new(&lib.snapshot, &lib.user, &inputs);
        if let Some((d, k, n)) = &self.preview
            && d == draft
            && *k == key
        {
            return *n;
        }
        let n = count_matches(
            draft,
            &lib.snapshot,
            &lib.user,
            &inputs,
            chrono::Local::now().date_naive(),
            &chrono::Local,
        );
        self.preview = Some((draft.clone(), key, n));
        n
    }

    fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Ok(name) = self.name_result(cx) else {
            return;
        };
        let mut category = self.draft(cx);
        category.name = name.clone();
        let is_new = self.id.is_none();
        let target_id = self
            .id
            .clone()
            .unwrap_or_else(irori_services::smart_categories::new_id);
        let save_id = target_id.clone();
        let b = backend(cx);
        irori_ui::state::run_infallible(cx, async move {
            b.userdata.save_smart_category(Some(&save_id), category);
        })
        .detach();
        if is_new {
            let tab = TabKey::Collection(target_id);
            views::saved_views(cx).update(cx, |m, cx| m.update(|v| v.ensure_tab_visible(&tab), cx));
        }
        window.close_dialog(cx);
        window.push_notification(
            Notification::success(t_args("library.smart.saved", &[("name", &name)])),
            cx,
        );
    }

    fn render_colour_picker(&self, cx: &mut Context<Self>) -> AnyElement {
        render_colour_picker(
            self.colour.as_deref(),
            self.color_popover_open,
            self.color_wheel.clone(),
            |this, _, cx| this.set_colour(None, cx),
            |this, c, _, cx| this.set_colour(Some(c.to_owned()), cx),
            |this, open, window, cx| this.on_colour_popover_toggle(open, window, cx),
            cx,
        )
    }

    fn render_icon_picker(&self, cx: &mut Context<Self>) -> AnyElement {
        render_icon_picker(
            self.colour.as_deref(),
            self.icon.as_deref(),
            self.icon_popover_open,
            self.icon_popover.clone(),
            |this, slug, _, cx| this.set_icon(slug, cx),
            |this, open, window, cx| this.on_icon_popover_toggle(open, window, cx),
            cx,
        )
    }

    fn render_manual_games_column(
        &self,
        draft: &SmartCategory,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let query = text(&self.game_search, cx);
        let query_lower = query.to_lowercase();
        let theme = cx.theme();
        let primary = theme.primary;
        let active_fg = legible(primary, &[theme.background], TEXT_CONTRAST, cx);
        let fg = theme.foreground;
        let muted = theme.muted_foreground;
        let border = theme.border;
        let picker_bg = theme.secondary.opacity(0.24);
        let row_bg = theme.secondary.opacity(0.28);
        let radius = theme.radius;

        let lib = models(cx).library.read(cx);
        let inputs = backend(cx).category_inputs();
        let today = chrono::Local::now().date_naive();

        let body_rows: Vec<AnyElement> = if !query_lower.is_empty() {
            let mut matches: Vec<_> = lib
                .snapshot
                .games
                .iter()
                .filter(|g| g.title.to_lowercase().contains(&query_lower))
                .collect();
            matches.sort_by_cached_key(|g| {
                let lower = g.title.to_lowercase();
                (!lower.starts_with(&query_lower), lower)
            });
            matches.truncate(MAX_GAME_RESULTS);

            if matches.is_empty() {
                vec![
                    div()
                        .w_full()
                        .py_6()
                        .text_xs()
                        .text_color(muted)
                        .flex()
                        .justify_center()
                        .child(t_args(
                            "library.filters.no_values",
                            &[("query", query.as_str())],
                        ))
                        .into_any_element(),
                ]
            } else {
                matches
                    .into_iter()
                    .enumerate()
                    .map(|(ix, game)| {
                        let primary_key = game.id().key();
                        let copy_keys = entry_copy_keys(&lib.snapshot, game);
                        let is_excluded = copy_keys.iter().any(|k| self.excluded_games.contains(k));
                        let is_included = copy_keys.iter().any(|k| self.included_games.contains(k));
                        let by_rules = !draft.rules.is_empty()
                            && matches_rules_only(
                                draft,
                                &lib.snapshot,
                                &lib.user,
                                &inputs,
                                &game.id(),
                                today,
                                &chrono::Local,
                            );
                        let action: AnyElement = if is_excluded {
                            let restore_keys = copy_keys.clone();
                            Button::new(SharedString::from(format!("collection-game-act-{ix}")))
                                .xsmall()
                                .ghost()
                                .icon(IconName::Undo2)
                                .label(t("library.collection.restore"))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.restore_excluded(&restore_keys, cx);
                                }))
                                .into_any_element()
                        } else if is_included {
                            let rem_keys = copy_keys.clone();
                            Button::new(SharedString::from(format!("collection-game-act-{ix}")))
                                .xsmall()
                                .outline()
                                .icon(IconName::Check)
                                .label(t("library.collection.added_game"))
                                .text_color(active_fg)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.remove_manual_game(&rem_keys, cx);
                                }))
                                .into_any_element()
                        } else if by_rules {
                            let excl_key = primary_key.clone();
                            let excl_keys = copy_keys.clone();
                            h_flex()
                                .items_center()
                                .gap_1()
                                .flex_shrink_0()
                                .child(
                                    div()
                                        .px_1p5()
                                        .py_0p5()
                                        .rounded_sm()
                                        .bg(primary.opacity(0.14))
                                        .text_xs()
                                        .text_color(active_fg)
                                        .child(t("library.collection.by_tag")),
                                )
                                .child(
                                    Button::new(SharedString::from(format!(
                                        "collection-game-excl-{ix}"
                                    )))
                                    .xsmall()
                                    .ghost()
                                    .icon(IconName::X)
                                    .tooltip(t("library.collection.exclude_game"))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.exclude_game(excl_key.clone(), &excl_keys, cx);
                                    })),
                                )
                                .into_any_element()
                        } else {
                            let add_key = primary_key.clone();
                            let add_keys = copy_keys.clone();
                            Button::new(SharedString::from(format!("collection-game-act-{ix}")))
                                .xsmall()
                                .outline()
                                .icon(IconName::Plus)
                                .label(t("library.collection.add_game"))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.add_manual_game(add_key.clone(), &add_keys, cx);
                                }))
                                .into_any_element()
                        };

                        h_flex()
                            .w_full()
                            .items_center()
                            .justify_between()
                            .gap_2()
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .bg(row_bg)
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .text_xs()
                                    .text_color(fg)
                                    .truncate()
                                    .child(game.title.clone()),
                            )
                            .child(action)
                            .into_any_element()
                    })
                    .collect()
            }
        } else {
            let mut rows: Vec<AnyElement> = Vec::new();
            let mut included_items: Vec<(String, String)> = self
                .included_games
                .iter()
                .map(|key| (key.clone(), excluded_game_title(&lib.snapshot, key)))
                .collect();
            included_items.sort_by_cached_key(|(_, title)| title.to_lowercase());

            for (ix, (key, title)) in included_items.into_iter().enumerate() {
                let rem_keys = vec![key];
                rows.push(
                    h_flex()
                        .w_full()
                        .items_center()
                        .justify_between()
                        .gap_2()
                        .px_2()
                        .py_1()
                        .rounded_md()
                        .bg(row_bg)
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_xs()
                                .text_color(fg)
                                .truncate()
                                .child(title),
                        )
                        .child(
                            Button::new(SharedString::from(format!("collection-included-{ix}")))
                                .xsmall()
                                .ghost()
                                .icon(IconName::X)
                                .tooltip(t("library.collection.remove_game"))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.remove_manual_game(&rem_keys, cx);
                                })),
                        )
                        .into_any_element(),
                );
            }

            if !self.excluded_games.is_empty() {
                if !rows.is_empty() {
                    rows.push(
                        div()
                            .w_full()
                            .h(px(1.))
                            .my_1()
                            .bg(border)
                            .into_any_element(),
                    );
                }
                let mut excluded_items: Vec<(String, String)> = self
                    .excluded_games
                    .iter()
                    .map(|key| (key.clone(), excluded_game_title(&lib.snapshot, key)))
                    .collect();
                excluded_items.sort_by_cached_key(|(_, title)| title.to_lowercase());
                let excluded_heading = match excluded_items.len() {
                    1 => t("library.collection.excluded_one"),
                    n => t_args(
                        "library.collection.excluded_other",
                        &[("count", &n.to_string())],
                    ),
                };
                rows.push(
                    div()
                        .px_1()
                        .text_xs()
                        .font_semibold()
                        .text_color(muted)
                        .child(excluded_heading)
                        .into_any_element(),
                );
                for (ix, (key, title)) in excluded_items.into_iter().enumerate() {
                    let restore_keys = vec![key];
                    rows.push(
                        h_flex()
                            .w_full()
                            .items_center()
                            .justify_between()
                            .gap_2()
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .bg(row_bg)
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .text_xs()
                                    .text_color(muted)
                                    .truncate()
                                    .child(title),
                            )
                            .child(
                                Button::new(SharedString::from(format!("collection-restore-{ix}")))
                                    .xsmall()
                                    .ghost()
                                    .icon(IconName::Undo2)
                                    .label(t("library.collection.restore"))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.restore_excluded(&restore_keys, cx);
                                    })),
                            )
                            .into_any_element(),
                    );
                }
            }

            if rows.is_empty() {
                rows.push(
                    div()
                        .w_full()
                        .p_3()
                        .text_xs()
                        .text_color(muted)
                        .child(t("library.collection.manual_empty"))
                        .into_any_element(),
                );
            }
            rows
        };

        let manual_count = self.included_games.len();
        v_flex()
            .flex_1()
            .min_w_0()
            .gap_1p5()
            .child(
                h_flex()
                    .w_full()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .child(
                        div()
                            .text_xs()
                            .font_semibold()
                            .text_color(muted)
                            .child(t("library.collection.manual_heading")),
                    )
                    .when(manual_count > 0, |el| {
                        el.child(
                            div()
                                .text_xs()
                                .font_medium()
                                .text_color(active_fg)
                                .child(manual_count.to_string()),
                        )
                    }),
            )
            .child(
                Input::new(&self.game_search)
                    .small()
                    .cleanable(true)
                    .prefix(Icon::new(IconName::Search).xsmall()),
            )
            .child(
                div()
                    .debug_selector(|| "collection-games-viewport".into())
                    .w_full()
                    .rounded(radius)
                    .bg(picker_bg)
                    .border_1()
                    .border_color(border)
                    .overflow_hidden()
                    .child(
                        v_flex()
                            .id("collection-games-picker")
                            .debug_selector(|| "collection-games-picker".into())
                            .w_full()
                            .h(rems(15.5))
                            .overflow_y_scrollbar()
                            .p_2()
                            .pr_3()
                            .gap_1()
                            .children(body_rows),
                    ),
            )
            .into_any_element()
    }
}

impl Render for CollectionDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let draft = self.draft(cx);
        let count = self.count(&draft, cx);
        let name = self.name_result(cx);
        let can_save = name.is_ok();
        let query = text(&self.tag_input, cx);
        let query_key = tag_key(&query);
        let query_lower = query.to_lowercase();
        let picked_keys: HashSet<String> = self.picked.iter().map(|p| tag_key(p)).collect();

        let theme = cx.theme();
        let primary = theme.primary;
        let active_fg = legible(primary, &[theme.background], TEXT_CONTRAST, cx);
        let fg = theme.foreground;
        let muted = theme.muted_foreground;
        let border = theme.border;
        let idle_bg = theme.secondary.opacity(0.35);
        let hover_bg = theme.secondary.opacity(0.8);
        let picker_bg = theme.secondary.opacity(0.24);
        let radius = theme.radius;
        let danger = theme.danger;

        let colour_row = self.render_colour_picker(cx);
        let icon_picker = self.render_icon_picker(cx);
        let manual_column = self.render_manual_games_column(&draft, cx);
        let has_picked = !self.picked.is_empty();
        let picked_chips: Vec<AnyElement> = self
            .picked
            .iter()
            .enumerate()
            .map(|(ix, label)| {
                let remove_label = label.clone();
                h_flex()
                    .id(SharedString::from(format!("collection-picked-{ix}")))
                    .items_center()
                    .gap_1()
                    .px_2()
                    .py_0p5()
                    .rounded_md()
                    .bg(primary.opacity(0.18))
                    .border_1()
                    .border_color(primary)
                    .text_xs()
                    .font_semibold()
                    .text_color(active_fg)
                    .cursor_pointer()
                    .child(label.clone())
                    .child(Icon::new(IconName::X).xsmall())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.toggle_term(remove_label.clone(), cx);
                    }))
                    .into_any_element()
            })
            .collect();

        let suggestion_chips: Vec<AnyElement> = self
            .suggestions
            .iter()
            .filter(|(k, label)| {
                query_key.is_empty()
                    || k.contains(&query_key)
                    || label.to_lowercase().contains(&query_lower)
            })
            .enumerate()
            .map(|(ix, (key, label))| {
                let selected = picked_keys.contains(key);
                let click_label = label.clone();
                let slug = key.clone();
                h_flex()
                    .id(SharedString::from(format!("collection-tag-{ix}")))
                    .debug_selector(move || format!("collection-tag-{slug}"))
                    .max_w_full()
                    .min_w_0()
                    .items_center()
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
                    .child(div().min_w_0().truncate().child(label.clone()))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.toggle_term(click_label.clone(), cx);
                    }))
                    .into_any_element()
            })
            .collect();

        let editing_id = self.id.clone();
        let count_label = match count {
            0 => t("library.collection.zero_games"),
            1 => t("library.smart.matches_one"),
            n => t_args("library.smart.matches_other", &[("count", &n.to_string())]),
        };

        let tags_column = v_flex()
            .flex_1()
            .min_w_0()
            .gap_1p5()
            .child(
                div()
                    .text_xs()
                    .font_semibold()
                    .text_color(muted)
                    .child(t("library.collection.tags_heading")),
            )
            .child(
                h_flex()
                    .w_full()
                    .gap_1p5()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(Input::new(&self.tag_input).small().cleanable(true)),
                    )
                    .child(
                        Button::new("collection-add-tag")
                            .small()
                            .outline()
                            .icon(IconName::Plus)
                            .label(t("library.collection.add_tag"))
                            .disabled(query.is_empty())
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.add_typed_tag(window, cx);
                            })),
                    ),
            )
            .when(has_picked, |el| {
                el.child(h_flex().w_full().flex_wrap().gap_1().children(picked_chips))
            })
            .child(
                div()
                    .debug_selector(|| "collection-tag-viewport".into())
                    .w_full()
                    .rounded(radius)
                    .bg(picker_bg)
                    .border_1()
                    .border_color(border)
                    .overflow_hidden()
                    .child(
                        v_flex()
                            .id("collection-tag-picker")
                            .debug_selector(|| "collection-tag-picker".into())
                            .w_full()
                            .h(rems(15.5))
                            .overflow_y_scrollbar()
                            .p_2()
                            .pr_3()
                            .child(
                                h_flex()
                                    .w_full()
                                    .flex_wrap()
                                    .gap_1p5()
                                    .children(suggestion_chips),
                            ),
                    ),
            );

        v_flex()
            .id("collection-dialog")
            .debug_selector(|| "collection-dialog".into())
            .w_full()
            .gap_3()
            .child(v_flex().gap_1().child(Input::new(&self.name)).when(
                name == Err(NameError::Exists),
                |el| {
                    el.child(
                        div()
                            .text_xs()
                            .text_color(danger)
                            .child(NameError::Exists.message()),
                    )
                },
            ))
            .child(colour_row)
            .child(icon_picker)
            .child(
                h_flex()
                    .w_full()
                    .gap_3()
                    .items_start()
                    .child(tags_column)
                    .child(manual_column),
            )
            .child(
                h_flex()
                    .w_full()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .pt_1()
                    .border_t_1()
                    .border_color(border)
                    .child(
                        h_flex()
                            .items_center()
                            .gap_2()
                            .child(div().text_sm().text_color(muted).child(count_label))
                            .when_some(editing_id, |el, id| {
                                el.child(
                                    Button::new("collection-advanced-rules")
                                        .xsmall()
                                        .ghost()
                                        .icon(IconName::Settings2)
                                        .label(t("library.categories.edit_rules"))
                                        .on_click(move |_, window, cx| {
                                            window.close_dialog(cx);
                                            super::smart::open_smart_category_editor(
                                                Some(id.clone()),
                                                None,
                                                window,
                                                cx,
                                            );
                                        }),
                                )
                            }),
                    )
                    .child(
                        h_flex()
                            .justify_end()
                            .gap_2()
                            .child(
                                Button::new("collection-cancel")
                                    .ghost()
                                    .label(t("library.common.cancel"))
                                    .on_click(|_, window, cx| window.close_dialog(cx)),
                            )
                            .child(
                                Button::new("collection-save")
                                    .primary()
                                    .label(if self.id.is_some() {
                                        t("library.common.save")
                                    } else {
                                        t("library.collection.create")
                                    })
                                    .disabled(!can_save)
                                    .on_click(
                                        cx.listener(|this, _, window, cx| this.save(window, cx)),
                                    ),
                            ),
                    ),
            )
    }
}

/// Open the hybrid collection creator (`editing == None`) or editor (`Some(id)`).
///
/// Smart categories with non-tag/genre rules or `Match::All` automatically open
/// the full rule editor so saving never strips advanced rules.
pub fn open_collection_dialog(editing: Option<String>, window: &mut Window, cx: &mut App) {
    open_collection_dialog_with_games(editing, Vec::new(), window, cx);
}

/// Open the hybrid collection dialog with `initial_games` pre-added to the
/// collection's manual game list (used by "+ New collection…" from a game's
/// context menu or the bulk selection bar).
pub fn open_collection_dialog_with_games(
    editing: Option<String>,
    initial_games: Vec<String>,
    window: &mut Window,
    cx: &mut App,
) {
    let base = match &editing {
        Some(id) => {
            let Some(saved) = models(cx)
                .library
                .read(cx)
                .user
                .smart_categories
                .get(id)
                .cloned()
            else {
                return;
            };
            if saved.is_unreadable() {
                return;
            }
            if !is_simple_tag_collection(&saved) {
                super::smart::open_smart_category_editor(Some(id.clone()), None, window, cx);
                return;
            }
            saved
        }
        None => SmartCategory::default(),
    };
    let title = match &editing {
        Some(_) => t_args("library.collection.edit_title", &[("name", &base.name)]),
        None => t("library.collection.new_title"),
    };
    let view = cx.new(|cx| CollectionDialog::new(editing, base, initial_games, window, cx));
    let width = dialog_width(window, 44.);
    let focus = Cell::new(true);
    window.open_dialog(cx, move |dialog, window, cx| {
        if focus.replace(false) {
            view.read(cx).name.focus_handle(cx).focus(window, cx);
        }
        dialog.title(title.clone()).w(width).child(view.clone())
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_and_extracts_hybrid_collection_preserving_inclusions_exclusions_and_icon() {
        let mut base = SmartCategory::default();
        base.excluded_games.insert("steam:570".into());
        let mut included = BTreeSet::new();
        included.insert("gog:42".into());
        let built = build_collection_category(
            &base,
            "Souls & Action".into(),
            Some("red".into()),
            Some("flame".into()),
            &[
                "Souls-like".into(),
                "souls-like".into(),
                " Action ".into(),
                "".into(),
            ],
            &included,
            &base.excluded_games,
        );
        assert!(is_simple_tag_collection(&built));
        assert!(!built.manual_only);
        assert_eq!(built.name, "Souls & Action");
        assert_eq!(built.matching, Match::Any);
        assert_eq!(built.look.colour.as_deref(), Some("red"));
        assert_eq!(built.look.icon.as_deref(), Some("flame"));
        assert_eq!(built.included_games, included);
        assert_eq!(built.excluded_games, base.excluded_games);
        assert_eq!(built.rules.len(), 2);
        assert_eq!(
            built.rules[0].rule,
            Some(Rule::Tags {
                tags: vec!["Souls-like".into(), "Action".into()]
            })
        );
        assert_eq!(
            built.rules[1].rule,
            Some(Rule::Genre {
                genres: vec!["Souls-like".into(), "Action".into()]
            })
        );
        assert_eq!(extract_terms(&built), vec!["Souls-like", "Action"]);

        // Manual-only collection (0 tags) marks `manual_only = true` and normalizes default icon.
        let manual = build_collection_category(
            &SmartCategory::default(),
            "Weekend Backlog".into(),
            None,
            Some("sparkles".into()),
            &[],
            &included,
            &BTreeSet::new(),
        );
        assert!(is_simple_tag_collection(&manual));
        assert!(manual.manual_only);
        assert!(manual.rules.is_empty());
        assert_eq!(manual.look.icon, None);
        assert_eq!(manual.included_games, included);
    }

    #[test]
    fn non_tag_or_match_all_smart_categories_are_not_simple_tag_collections() {
        let with_installed = SmartCategory::new(
            "Installed RPGs",
            Match::Any,
            vec![
                Rule::Tags {
                    tags: vec!["RPG".into()],
                },
                Rule::Installed { installed: true },
            ],
            vec![],
        );
        assert!(!is_simple_tag_collection(&with_installed));

        let match_all_tags = SmartCategory::new(
            "Action AND RPG",
            Match::All,
            vec![
                Rule::Tags {
                    tags: vec!["Action".into()],
                },
                Rule::Genre {
                    genres: vec!["RPG".into()],
                },
            ],
            vec![],
        );
        assert!(!is_simple_tag_collection(&match_all_tags));
    }

    #[test]
    fn excluded_game_title_resolves_primary_and_secondary_copies() {
        let steam = crate::filter::tests::game(irori_core::Runner::Steam, "100", "Hades", true);
        let epic = crate::filter::tests::game(irori_core::Runner::Legendary, "200", "Hades", false);
        let user = irori_services::UserData::default();
        let snapshot = irori_services::LibrarySnapshot::new(
            1,
            irori_services::copies::group(vec![steam, epic], &user, true),
            std::collections::HashMap::new(),
        );
        assert_eq!(snapshot.games.len(), 1);
        assert_eq!(excluded_game_title(&snapshot, "steam:100"), "Hades");
        assert_eq!(excluded_game_title(&snapshot, "legendary:200"), "Hades");
        assert_eq!(excluded_game_title(&snapshot, "gog:999"), "gog:999");
    }

    #[test]
    fn popular_tags_include_souls_like_and_have_65_plus_unique_keys() {
        let mut keys = HashSet::new();
        for &tag in POPULAR_TAGS {
            let key = tag_key(tag);
            assert!(!key.is_empty(), "empty key for {tag}");
            assert!(keys.insert(key), "duplicate tag_key for {tag}");
        }
        assert!(
            keys.len() >= 65,
            "expected at least 65 curated gamer tags, got {}",
            keys.len()
        );
        assert!(
            keys.contains(&tag_key("Souls-like")),
            "POPULAR_TAGS must include Souls-like"
        );
    }
}
