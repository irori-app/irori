//! Saved library views: the filters, grouping and sort of the library under
//! a name ("Playing now", "Backlog"), one click away in the toolbar, the
//! sidebar and the command palette.
//!
//! A few useful views are added the first time, and from then on they are
//! the user's like any other: renamed, reordered or deleted, never brought
//! back. Built-in views keep their translated name until renamed. Views are
//! kept in `state/library-views.json`; the search text is never part of one.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use gpui_kit::{App, AppContext as _, Context, Entity, Global, SharedString, Task};
use irori_core::settings::{CoverShape, FocusCoverSize, LibraryLayout, ListRowSize};
use irori_core::{Paths, json_store};
use irori_services::{EffectiveStatus, LibrarySort};
use irori_ui::i18n::t;
use serde::{Deserialize, Serialize};

use crate::filter::{Grouping, InstallFilter, ViewFilter};
use crate::filters_panel::active_filters;

/// The views Irori adds the first time, kept apart so their names follow
/// the interface language until the user renames them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum BuiltinView {
    /// Games being played, most recently played first.
    PlayingNow,
    /// Games planned, paused or never played, in status sections.
    Backlog,
    /// Installed games not finished or dropped, most recently played first.
    ReadyToPlay,
    /// Newest in the library first.
    RecentlyAdded,
}

impl BuiltinView {
    /// Built-in views added for a fresh library.
    pub const SEEDED: [BuiltinView; 1] = [BuiltinView::RecentlyAdded];

    /// Built-in views no longer seeded; untouched copies are trimmed once on
    /// upgrade while renamed or edited copies stay as user views.
    pub fn is_retired(self) -> bool {
        matches!(
            self,
            BuiltinView::PlayingNow | BuiltinView::Backlog | BuiltinView::ReadyToPlay
        )
    }

    pub fn name(self) -> SharedString {
        t(match self {
            BuiltinView::PlayingNow => "library.views.builtin.playing_now",
            BuiltinView::Backlog => "library.views.builtin.backlog",
            BuiltinView::ReadyToPlay => "library.views.builtin.ready_to_play",
            BuiltinView::RecentlyAdded => "library.views.builtin.recently_added",
        })
    }

    /// The criteria the view starts with.
    pub fn filter(self) -> ViewFilter {
        let statuses = |s: &[EffectiveStatus]| s.iter().copied().collect::<BTreeSet<_>>();
        match self {
            BuiltinView::PlayingNow => ViewFilter {
                statuses: statuses(&[EffectiveStatus::Playing]),
                sort: LibrarySort::LastPlayed,
                ..Default::default()
            },
            BuiltinView::Backlog => ViewFilter {
                statuses: statuses(&[
                    EffectiveStatus::PlanToPlay,
                    EffectiveStatus::OnHold,
                    EffectiveStatus::NotPlayed,
                ]),
                group: Grouping::Status,
                sort: LibrarySort::DateAdded,
                ..Default::default()
            },
            BuiltinView::ReadyToPlay => ViewFilter {
                install: InstallFilter::Installed,
                statuses: statuses(&[
                    EffectiveStatus::Playing,
                    EffectiveStatus::OnHold,
                    EffectiveStatus::PlanToPlay,
                    EffectiveStatus::Played,
                    EffectiveStatus::NotPlayed,
                ]),
                sort: LibrarySort::LastPlayed,
                ..Default::default()
            },
            BuiltinView::RecentlyAdded => ViewFilter {
                sort: LibrarySort::DateAdded,
                ..Default::default()
            },
        }
    }
}

/// One saved view.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedView {
    /// Stable within the file; never reused.
    pub id: u64,
    /// The user's name. `None` for a built-in view not renamed yet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub builtin: Option<BuiltinView>,
    pub filter: ViewFilter,
    /// Layout pinned by this view (Grid, List or Focus), or `None` to keep
    /// whichever layout the library currently uses.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layout: Option<LibraryLayout>,
}

impl SavedView {
    /// The name shown.
    pub fn label(&self) -> SharedString {
        match (&self.name, self.builtin) {
            (Some(name), _) => name.clone().into(),
            (None, Some(builtin)) => builtin.name(),
            (None, None) => SharedString::default(),
        }
    }

    /// Whether `filter` shows what this view shows (the search text aside).
    pub fn shows(&self, filter: &ViewFilter) -> bool {
        self.filter
            == ViewFilter {
                query: String::new(),
                ..filter.clone()
            }
    }

    /// Whether `filter` and `layout` match this view (the search text aside).
    /// Views without a pinned layout match any layout.
    pub fn matches(&self, filter: &ViewFilter, layout: LibraryLayout) -> bool {
        self.shows(filter) && self.layout.is_none_or(|pinned| pinned == layout)
    }
}

/// Why a view name cannot be used.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NameError {
    Empty,
    Exists,
}

impl NameError {
    pub fn message(self) -> SharedString {
        match self {
            NameError::Empty => t("library.views.name_empty"),
            NameError::Exists => t("library.views.name_exists"),
        }
    }
}

/// One tab in the library's top strip: a built-in preset or a dynamic
/// collection (smart category). `View(u64)` is kept deserializable so legacy
/// `library-views.json` files continue to load cleanly for the command palette.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", content = "id", rename_all = "camelCase")]
pub enum TabKey {
    All,
    Installed,
    Favorites,
    RecentlyAdded,
    View(u64),
    Collection(String),
}

impl TabKey {
    /// `All` and `Installed` can be reordered, never hidden or deleted.
    pub fn is_locked(&self) -> bool {
        matches!(self, TabKey::All | TabKey::Installed)
    }

    /// Canonical JSON map key for per-tab visualization preferences.
    pub fn storage_key(&self) -> String {
        match self {
            TabKey::All => "all".into(),
            TabKey::Installed => "installed".into(),
            TabKey::Favorites => "favorites".into(),
            TabKey::RecentlyAdded => "recentlyAdded".into(),
            TabKey::View(id) => format!("view:{id}"),
            TabKey::Collection(id) => format!("collection:{id}"),
        }
    }
}

/// Visualization preferences remembered independently for one library tab:
/// layout, cover shape, size, and sort/group order.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct TabViewPrefs {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layout: Option<LibraryLayout>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cover_shape: Option<CoverShape>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tile_size: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub list_row_size: Option<ListRowSize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub focus_cover_size: Option<FocusCoverSize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sort: Option<LibrarySort>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub descending: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<Grouping>,
}

impl TabViewPrefs {
    pub fn is_empty(&self) -> bool {
        self.layout.is_none()
            && self.cover_shape.is_none()
            && self.tile_size.is_none()
            && self.list_row_size.is_none()
            && self.focus_cover_size.is_none()
            && self.sort.is_none()
            && self.descending.is_none()
            && self.group.is_none()
    }
}

/// Smart category IDs in the user's canonical category order (pinned first,
/// then by name).
pub fn smart_collection_ids(user: &irori_services::UserData) -> Vec<String> {
    user.categories_in_order()
        .into_iter()
        .filter_map(|(cat, _)| match cat {
            irori_services::CategoryRef::Smart(id) => Some(id),
            irori_services::CategoryRef::Manual(_) => None,
        })
        .collect()
}

/// Saved library views (for the command palette) plus the tab strip's order,
/// visibility, active tab and per-tab visualization settings.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct SavedViews {
    pub views: Vec<SavedView>,
    /// The built-in views were added once; deleting them keeps them gone.
    pub seeded: bool,
    /// Retired built-in views (Playing now, Backlog, Ready to play) were
    /// trimmed once when untouched.
    pub trimmed: bool,
    next_id: u64,
    /// User-defined tab order across presets and dynamic collections.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tab_order: Vec<TabKey>,
    /// Tabs hidden from the top strip (`All` and `Installed` are always shown).
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub hidden_tabs: BTreeSet<TabKey>,
    /// Per-tab visualization settings keyed by [`TabKey::storage_key`].
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub tab_prefs: BTreeMap<String, TabViewPrefs>,
    /// Last active tab, restored on startup so `All` and `Recently added`
    /// remain distinct even when sort was customized.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_tab: Option<TabKey>,
}

impl SavedViews {
    /// Add the built-in views the first time.
    pub fn seed(&mut self) {
        if self.seeded {
            return;
        }
        self.seeded = true;
        self.trimmed = true;
        for builtin in BuiltinView::SEEDED {
            let id = self.take_id();
            self.views.push(SavedView {
                id,
                name: None,
                builtin: Some(builtin),
                filter: builtin.filter(),
                layout: None,
            });
        }
    }

    /// One-time migration: remove retired built-in views (`PlayingNow`,
    /// `Backlog`, `ReadyToPlay`) that were never renamed (`name.is_none()`)
    /// and never edited (`filter == builtin.filter()`). Returns `true` when
    /// the migration ran so the caller can persist `trimmed: true`.
    pub fn trim_retired_builtins(&mut self) -> bool {
        if self.trimmed {
            return false;
        }
        self.trimmed = true;
        self.views.retain(|view| {
            !matches!(
                view.builtin,
                Some(b)
                    if b.is_retired()
                        && view.name.is_none()
                        && view.filter == b.filter()
                        && view.layout.is_none()
            )
        });
        true
    }

    fn take_id(&mut self) -> u64 {
        let used = self.views.iter().map(|v| v.id).max().unwrap_or(0);
        self.next_id = self.next_id.max(used) + 1;
        self.next_id
    }

    pub fn get(&self, id: u64) -> Option<&SavedView> {
        self.views.iter().find(|v| v.id == id)
    }

    /// The first view matching `filter` and `layout`, preferring views that
    /// explicitly pin `layout` over layout-agnostic views.
    pub fn active(&self, filter: &ViewFilter, layout: LibraryLayout) -> Option<&SavedView> {
        self.views
            .iter()
            .find(|v| v.layout == Some(layout) && v.shows(filter))
            .or_else(|| {
                self.views
                    .iter()
                    .find(|v| v.layout.is_none() && v.shows(filter))
            })
    }

    /// Check a name, ignoring case, against the other views (`renaming` is
    /// the view being renamed).
    pub fn validate_name(&self, raw: &str, renaming: Option<u64>) -> Result<String, NameError> {
        let name = raw.trim();
        if name.is_empty() {
            return Err(NameError::Empty);
        }
        let lower = name.to_lowercase();
        if self
            .views
            .iter()
            .any(|v| Some(v.id) != renaming && v.label().to_lowercase() == lower)
        {
            return Err(NameError::Exists);
        }
        Ok(name.to_string())
    }

    /// Save `filter` (without its search text) and optional pinned `layout` as
    /// a new view at the end.
    pub fn add(
        &mut self,
        name: &str,
        filter: &ViewFilter,
        layout: Option<LibraryLayout>,
    ) -> Result<u64, NameError> {
        let name = self.validate_name(name, None)?;
        let id = self.take_id();
        self.views.push(SavedView {
            id,
            name: Some(name),
            builtin: None,
            filter: ViewFilter {
                query: String::new(),
                ..filter.clone()
            },
            layout,
        });
        Ok(id)
    }

    /// Replace a view's criteria with `filter` and its pinned layout with `layout`.
    pub fn update_filter(
        &mut self,
        id: u64,
        filter: &ViewFilter,
        layout: Option<LibraryLayout>,
    ) -> bool {
        let Some(view) = self.views.iter_mut().find(|v| v.id == id) else {
            return false;
        };
        view.filter = ViewFilter {
            query: String::new(),
            ..filter.clone()
        };
        view.layout = layout;
        true
    }

    pub fn rename(&mut self, id: u64, name: &str) -> Result<(), NameError> {
        let name = self.validate_name(name, Some(id))?;
        if let Some(view) = self.views.iter_mut().find(|v| v.id == id) {
            view.name = Some(name);
        }
        Ok(())
    }

    /// Move a view up (`-1`) or down (`1`) the list; false at either end.
    pub fn shift(&mut self, id: u64, by: isize) -> bool {
        let Some(from) = self.views.iter().position(|v| v.id == id) else {
            return false;
        };
        let to = from as isize + by;
        if to < 0 || to as usize >= self.views.len() {
            return false;
        }
        let view = self.views.remove(from);
        self.views.insert(to as usize, view);
        true
    }

    /// Remove a view, returning it and where it was, for an undo.
    pub fn remove(&mut self, id: u64) -> Option<(usize, SavedView)> {
        let at = self.views.iter().position(|v| v.id == id)?;
        let key = TabKey::View(id);
        self.tab_order.retain(|k| k != &key);
        self.hidden_tabs.remove(&key);
        Some((at, self.views.remove(at)))
    }

    /// Put a removed view back where it was.
    pub fn restore(&mut self, at: usize, view: SavedView) {
        if self.get(view.id).is_none() {
            self.views.insert(at.min(self.views.len()), view);
        }
    }

    /// Whether `key` is hidden from the top tab bar. Locked tabs (`All` and
    /// `Installed`) are never hidden.
    pub fn is_tab_hidden(&self, key: &TabKey) -> bool {
        !key.is_locked() && self.hidden_tabs.contains(key)
    }

    /// Show or hide `key` in the top tab bar. Returns `false` if `key` is
    /// locked or already in that state.
    pub fn set_tab_hidden(&mut self, key: &TabKey, hidden: bool) -> bool {
        if key.is_locked() {
            return false;
        }
        if hidden {
            self.hidden_tabs.insert(key.clone())
        } else {
            self.hidden_tabs.remove(key)
        }
    }

    /// Resolve every tab in the top strip (presets and dynamic collections)
    /// into a deterministic list, preserving saved order, placing new
    /// collections right after `Favorites`, and dropping stale or legacy
    /// saved-view entries.
    pub fn resolved_tabs(&self, smart_ids: &[String]) -> Vec<TabKey> {
        let mut valid: BTreeSet<TabKey> = [
            TabKey::All,
            TabKey::Installed,
            TabKey::Favorites,
            TabKey::RecentlyAdded,
        ]
        .into_iter()
        .collect();
        for id in smart_ids {
            valid.insert(TabKey::Collection(id.clone()));
        }

        let mut out = Vec::with_capacity(valid.len());
        let mut seen = BTreeSet::new();
        for key in &self.tab_order {
            if valid.contains(key) && seen.insert(key.clone()) {
                out.push(key.clone());
            }
        }
        for preset in [
            TabKey::All,
            TabKey::Installed,
            TabKey::Favorites,
            TabKey::RecentlyAdded,
        ] {
            if seen.insert(preset.clone()) {
                out.push(preset);
            }
        }
        for id in smart_ids {
            let key = TabKey::Collection(id.clone());
            if seen.insert(key.clone()) {
                if let Some(fav_ix) = out.iter().position(|t| *t == TabKey::Favorites) {
                    let mut insert_at = fav_ix + 1;
                    while matches!(out.get(insert_at), Some(TabKey::Collection(_))) {
                        insert_at += 1;
                    }
                    out.insert(insert_at, key);
                } else {
                    out.push(key);
                }
            }
        }
        out
    }

    /// Saved visualization preferences for `key`, if any were customized.
    pub fn prefs_for(&self, key: &TabKey) -> Option<&TabViewPrefs> {
        self.tab_prefs.get(&key.storage_key())
    }

    /// Update the per-tab visualization preferences for `key`, pruning empty
    /// entries so untouched tabs stay omitted from disk.
    pub fn update_prefs(&mut self, key: &TabKey, f: impl FnOnce(&mut TabViewPrefs)) {
        let storage_key = key.storage_key();
        let entry = self.tab_prefs.entry(storage_key.clone()).or_default();
        f(entry);
        if entry.is_empty() {
            self.tab_prefs.remove(&storage_key);
        }
    }

    /// Remember which tab is currently active (or `None` for custom filter
    /// combinations).
    pub fn set_active_tab(&mut self, key: Option<TabKey>) {
        self.active_tab = key;
    }

    /// Remove and return the saved visualization preferences for `key` (used
    /// when deleting a collection with Undo support).
    pub fn take_tab_prefs(&mut self, key: &TabKey) -> Option<TabViewPrefs> {
        self.tab_prefs.remove(&key.storage_key())
    }

    /// Restore previously removed per-tab visualization preferences for `key`.
    pub fn restore_tab_prefs(&mut self, key: &TabKey, prefs: TabViewPrefs) {
        if !prefs.is_empty() {
            self.tab_prefs.insert(key.storage_key(), prefs);
        }
    }

    /// Tabs currently visible in the top strip, in resolved order.
    pub fn visible_tabs(&self, smart_ids: &[String]) -> Vec<TabKey> {
        self.resolved_tabs(smart_ids)
            .into_iter()
            .filter(|k| !self.is_tab_hidden(k))
            .collect()
    }

    /// Move `from` to `to`'s slot in the unified tab order.
    pub fn move_tab(&mut self, from: &TabKey, to: &TabKey, smart_ids: &[String]) -> bool {
        if from == to {
            return false;
        }
        let mut order = self.resolved_tabs(smart_ids);
        let Some(from_ix) = order.iter().position(|k| k == from) else {
            return false;
        };
        let Some(to_ix) = order.iter().position(|k| k == to) else {
            return false;
        };
        let moved = order.remove(from_ix);
        order.insert(to_ix, moved);
        self.tab_order = order;
        true
    }

    /// Shift `key` by `by` (`-1` earlier, `+1` later) in the unified tab order.
    pub fn shift_tab(&mut self, key: &TabKey, by: isize, smart_ids: &[String]) -> bool {
        let mut order = self.resolved_tabs(smart_ids);
        let Some(from) = order.iter().position(|k| k == key) else {
            return false;
        };
        let to = from as isize + by;
        if to < 0 || to as usize >= order.len() {
            return false;
        }
        let moved = order.remove(from);
        order.insert(to as usize, moved);
        self.tab_order = order;
        true
    }

    /// Shift `key` across currently visible tabs (`-1` left, `+1` right) so
    /// tab-bar actions move past hidden tabs in a single step.
    pub fn shift_visible_tab(&mut self, key: &TabKey, by: isize, smart_ids: &[String]) -> bool {
        let visible = self.visible_tabs(smart_ids);
        let Some(vis_from) = visible.iter().position(|k| k == key) else {
            return false;
        };
        let vis_to = vis_from as isize + by;
        if vis_to < 0 || vis_to as usize >= visible.len() {
            return false;
        }
        let target = visible[vis_to as usize].clone();
        self.move_tab(key, &target, smart_ids)
    }

    /// Ensure `key` is visible (not hidden) in the tab bar.
    pub fn ensure_tab_visible(&mut self, key: &TabKey) {
        self.hidden_tabs.remove(key);
    }

    /// Drop any saved order, visibility or per-tab visualization state for a
    /// deleted tab.
    pub fn forget_tab(&mut self, key: &TabKey) {
        self.tab_order.retain(|k| k != key);
        self.hidden_tabs.remove(key);
        self.tab_prefs.remove(&key.storage_key());
        if self.active_tab.as_ref() == Some(key) {
            self.active_tab = None;
        }
    }
}

fn state_file() -> PathBuf {
    Paths::get().state_dir().join("library-views.json")
}

/// The saved views as an observable model, persisted off the UI thread.
pub struct ViewsModel {
    views: SavedViews,
    _save: Option<Task<()>>,
}

impl ViewsModel {
    pub fn views(&self) -> &SavedViews {
        &self.views
    }

    /// Change the views; saves and notifies when something changed.
    pub fn update<R>(&mut self, f: impl FnOnce(&mut SavedViews) -> R, cx: &mut Context<Self>) -> R {
        let before = self.views.clone();
        let out = f(&mut self.views);
        if before != self.views {
            if !cfg!(test) {
                let snapshot = self.views.clone();
                self._save = Some(cx.background_spawn(async move {
                    if let Err(e) = json_store::write(&state_file(), &snapshot) {
                        tracing::warn!("saving library views: {e:#}");
                    }
                }));
            }
            cx.notify();
        }
        out
    }
}

struct ViewsHandle(Entity<ViewsModel>);
impl Global for ViewsHandle {}

/// The saved views, loaded (and the built-in ones added) on first use.
pub fn saved_views(cx: &mut App) -> Entity<ViewsModel> {
    if let Some(handle) = cx.try_global::<ViewsHandle>() {
        return handle.0.clone();
    }
    // A small file, read once like the library's own view criteria.
    let mut views: SavedViews = if cfg!(test) {
        SavedViews::default()
    } else {
        json_store::read_or_default(&state_file())
    };
    let fresh = !views.seeded;
    views.seed();
    let trimmed = views.trim_retired_builtins();
    let model = cx.new(|cx| {
        let mut model = ViewsModel { views, _save: None };
        if (fresh || trimmed) && !cfg!(test) {
            let snapshot = model.views.clone();
            model._save = Some(cx.background_spawn(async move {
                if let Err(e) = json_store::write(&state_file(), &snapshot) {
                    tracing::warn!("saving library views: {e:#}");
                }
            }));
        }
        model
    });
    cx.set_global(ViewsHandle(model.clone()));
    model
}

/// Every view as (id, name), in order, for menus and the palette.
pub fn list(cx: &mut App) -> Vec<(u64, SharedString)> {
    let model = saved_views(cx);
    model
        .read(cx)
        .views()
        .views
        .iter()
        .map(|v| (v.id, v.label()))
        .collect()
}

/// The view the library shows now, if its criteria and layout match one.
pub fn active_view(cx: &mut App) -> Option<u64> {
    let filters = active_filters(cx)?;
    let model = saved_views(cx);
    let filter = filters.read(cx).filter().clone();
    let layout = crate::view::remove::active_library(cx)
        .map(|v| v.read(cx).current_layout())
        .unwrap_or_else(|| crate::library_layout(cx));
    model.read(cx).views().active(&filter, layout).map(|v| v.id)
}

/// Show a saved view in the library, keeping the search text. Also switches
/// the library's layout when the view pins one. Without a library view yet,
/// the criteria and layout are stored for the one about to open.
pub fn apply_view(id: u64, cx: &mut App) {
    let model = saved_views(cx);
    let Some((filter, layout)) = model
        .read(cx)
        .views()
        .get(id)
        .map(|v| (v.filter.clone(), v.layout))
    else {
        return;
    };
    match active_filters(cx) {
        Some(filters) => filters.update(cx, |m, cx| {
            m.update(
                move |f| {
                    *f = ViewFilter {
                        query: std::mem::take(&mut f.query),
                        ..filter
                    }
                },
                cx,
            )
        }),
        None => {
            if let Err(e) = crate::filter::save(&filter) {
                tracing::warn!("saving library view: {e:#}");
            }
        }
    }
    if let Some(layout) = layout {
        match crate::view::remove::active_library(cx) {
            Some(view) => view.update(cx, |v, cx| v.set_layout(layout, cx)),
            None => crate::set_library_layout(layout, cx),
        }
    }
    irori_ui::navigate(irori_ui::Route::Library, cx);
}

#[cfg(test)]
mod tests {
    use super::*;
    use irori_core::Runner;
    use irori_services::StoreFilter;

    #[test]
    fn seeds_built_in_views_once() {
        let mut views = SavedViews::default();
        views.seed();
        assert_eq!(views.views.len(), BuiltinView::SEEDED.len());
        assert_eq!(views.views[0].builtin, Some(BuiltinView::RecentlyAdded));
        assert!(views.trimmed);
        let first = views.views[0].id;
        views.remove(first);
        views.seed();
        assert_eq!(views.views.len(), BuiltinView::SEEDED.len() - 1);
        // Ids are never reused, even after a delete.
        let id = views.add("Mine", &ViewFilter::default(), None).unwrap();
        assert!(views.views.iter().filter(|v| v.id == id).count() == 1);
        assert_ne!(id, first);
    }

    #[test]
    fn saves_criteria_without_the_search_and_finds_the_active_view() {
        let mut views = SavedViews::default();
        let filter = ViewFilter {
            query: "hades".into(),
            store: StoreFilter::Store(Runner::Gog),
            sort: LibrarySort::Playtime,
            group: Grouping::Status,
            ..Default::default()
        };
        let id = views
            .add("  GOG by playtime ", &filter, Some(LibraryLayout::List))
            .unwrap();
        let saved = views.get(id).unwrap();
        assert_eq!(saved.name.as_deref(), Some("GOG by playtime"));
        assert_eq!(saved.layout, Some(LibraryLayout::List));
        assert!(saved.filter.query.is_empty());
        // Typing a search keeps the view active when the layout matches; another
        // layout or sort does not.
        assert_eq!(
            views.active(&filter, LibraryLayout::List).map(|v| v.id),
            Some(id)
        );
        assert!(views.active(&filter, LibraryLayout::Grid).is_none());
        let other = ViewFilter {
            sort: LibrarySort::Title,
            ..filter.clone()
        };
        assert!(views.active(&other, LibraryLayout::List).is_none());
        assert!(views.update_filter(id, &other, Some(LibraryLayout::Grid)));
        assert_eq!(
            views.active(&other, LibraryLayout::Grid).map(|v| v.id),
            Some(id)
        );
        assert!(views.active(&other, LibraryLayout::List).is_none());
    }

    #[test]
    fn names_are_unique_ignoring_case() {
        let mut views = SavedViews::default();
        views.seed();
        let a = views.add("Couch", &ViewFilter::default(), None).unwrap();
        assert_eq!(
            views.add("couch ", &ViewFilter::default(), None),
            Err(NameError::Exists)
        );
        assert_eq!(
            views.add("   ", &ViewFilter::default(), None),
            Err(NameError::Empty)
        );
        // Built-in names count too, in the interface language.
        let builtin = BuiltinView::RecentlyAdded.name().to_string();
        assert_eq!(
            views.add(&builtin, &ViewFilter::default(), None),
            Err(NameError::Exists)
        );
        // Renaming to its own name in another case is fine.
        assert!(views.rename(a, "COUCH").is_ok());
        assert_eq!(views.get(a).unwrap().label(), "COUCH");
    }

    #[test]
    fn reorders_removes_and_restores() {
        let mut views = SavedViews::default();
        let a = views.add("A", &ViewFilter::default(), None).unwrap();
        let b = views.add("B", &ViewFilter::default(), None).unwrap();
        let c = views.add("C", &ViewFilter::default(), None).unwrap();
        let order = |v: &SavedViews| v.views.iter().map(|v| v.id).collect::<Vec<_>>();
        assert!(views.shift(c, -1));
        assert_eq!(order(&views), [a, c, b]);
        assert!(!views.shift(a, -1));
        assert!(!views.shift(b, 1));
        let (at, removed) = views.remove(c).unwrap();
        assert_eq!(order(&views), [a, b]);
        views.restore(at, removed);
        assert_eq!(order(&views), [a, c, b]);
    }

    #[test]
    fn round_trips_through_json() {
        let mut views = SavedViews::default();
        views.seed();
        views
            .add("Mine", &ViewFilter::default(), Some(LibraryLayout::List))
            .unwrap();
        let text = serde_json::to_string(&views).unwrap();
        let back: SavedViews = serde_json::from_str(&text).unwrap();
        assert_eq!(back, views);
        assert!(text.contains("\"builtin\":\"recentlyAdded\""));
        assert!(text.contains("\"layout\":\"list\""));
        // Builtin view omits layout when None.
        assert_eq!(back.views[0].layout, None);
    }

    #[test]
    fn trims_untouched_retired_builtins_once() {
        // An older file with no `trimmed` key still deserializes all 4 builtins.
        let old = SavedViews {
            views: vec![
                SavedView {
                    id: 1,
                    name: None,
                    builtin: Some(BuiltinView::PlayingNow),
                    filter: BuiltinView::PlayingNow.filter(),
                    layout: None,
                },
                SavedView {
                    id: 2,
                    name: Some("My Backlog".into()),
                    builtin: Some(BuiltinView::Backlog),
                    filter: BuiltinView::Backlog.filter(),
                    layout: None,
                },
                SavedView {
                    id: 3,
                    name: None,
                    builtin: Some(BuiltinView::ReadyToPlay),
                    filter: ViewFilter {
                        favourites_only: true,
                        ..BuiltinView::ReadyToPlay.filter()
                    },
                    layout: None,
                },
                SavedView {
                    id: 4,
                    name: None,
                    builtin: Some(BuiltinView::RecentlyAdded),
                    filter: BuiltinView::RecentlyAdded.filter(),
                    layout: None,
                },
                SavedView {
                    id: 5,
                    name: Some("Custom".into()),
                    builtin: None,
                    filter: ViewFilter::default(),
                    layout: None,
                },
            ],
            seeded: true,
            trimmed: false,
            next_id: 5,
            tab_order: Vec::new(),
            hidden_tabs: BTreeSet::new(),
            tab_prefs: BTreeMap::new(),
            active_tab: None,
        };
        let mut raw = serde_json::to_value(&old).unwrap();
        raw.as_object_mut().unwrap().remove("trimmed");
        let mut views: SavedViews = serde_json::from_value(raw).unwrap();
        assert!(!views.trimmed);

        assert!(views.trim_retired_builtins());
        assert!(views.trimmed);
        // Untouched PlayingNow is gone; renamed Backlog, edited ReadyToPlay,
        // RecentlyAdded and Custom all stay.
        let ids: Vec<u64> = views.views.iter().map(|v| v.id).collect();
        assert_eq!(ids, vec![2, 3, 4, 5]);

        // Never runs a second time, even if an untouched retired builtin reappears.
        views.views.push(SavedView {
            id: 6,
            name: None,
            builtin: Some(BuiltinView::Backlog),
            filter: BuiltinView::Backlog.filter(),
            layout: None,
        });
        assert!(!views.trim_retired_builtins());
        assert!(views.get(6).is_some());
    }

    #[test]
    fn trims_legacy_four_builtin_file_without_trimmed_or_newer_filter_keys() {
        let legacy = r#"{
            "views": [
                {"id": 1, "builtin": "playingNow", "filter": {"statuses": ["playing"], "sort": "LastPlayed"}},
                {"id": 2, "builtin": "backlog", "filter": {"statuses": ["planToPlay", "onHold", "notPlayed"], "sort": "DateAdded", "group": "status"}},
                {"id": 3, "builtin": "readyToPlay", "filter": {"install": "installed", "statuses": ["playing", "planToPlay", "played", "onHold", "notPlayed"], "sort": "LastPlayed"}},
                {"id": 4, "builtin": "recentlyAdded", "filter": {"sort": "DateAdded"}}
            ],
            "seeded": true,
            "nextId": 4
        }"#;
        let mut views: SavedViews = serde_json::from_str(legacy).unwrap();
        assert!(!views.trimmed);
        assert!(views.trim_retired_builtins());
        assert!(views.trimmed);
        assert_eq!(views.views.len(), 1);
        assert_eq!(views.views[0].id, 4);
        assert_eq!(views.views[0].builtin, Some(BuiltinView::RecentlyAdded));
    }

    #[test]
    fn keeps_facets_a_newer_version_saved() {
        let text = r#"{"views": [{"id": 1, "name": "Couch", "filter": {
            "modes": ["local"], "players": {"min": 2}}}], "seeded": true, "nextId": 1}"#;
        let mut views: SavedViews = serde_json::from_str(text).unwrap();
        let view = views.get(1).unwrap().clone();
        assert_eq!(view.filter.modes, [irori_core::GameMode::Local].into());
        // Applying the view and saving it again keeps what this version
        // can't use, and the view still reads as the one shown.
        assert!(view.shows(&view.filter));
        views.rename(1, "Couch co-op").unwrap();
        let written = serde_json::to_value(&views).unwrap();
        assert_eq!(
            written["views"][0]["filter"]["players"],
            serde_json::json!({"min": 2})
        );
    }

    #[test]
    fn resolved_tabs_reorders_protects_locked_and_hides_optional_tabs() {
        let mut views = SavedViews::default();
        views.seed();
        // Legacy saved views stay out of the top tab strip (command palette only).
        let _custom_id = views.add("Co-op", &ViewFilter::default(), None).unwrap();
        let smart = vec!["s-souls".to_string(), "s-indie".to_string()];

        // Default resolution puts collections right after Favorites, followed by Recently added.
        assert_eq!(
            views.resolved_tabs(&smart),
            vec![
                TabKey::All,
                TabKey::Installed,
                TabKey::Favorites,
                TabKey::Collection("s-souls".into()),
                TabKey::Collection("s-indie".into()),
                TabKey::RecentlyAdded,
            ]
        );

        // Locked tabs (All, Installed) cannot be hidden, even if forced into hidden_tabs.
        assert!(!views.set_tab_hidden(&TabKey::All, true));
        assert!(!views.set_tab_hidden(&TabKey::Installed, true));
        views.hidden_tabs.insert(TabKey::All);
        assert!(!views.is_tab_hidden(&TabKey::All));
        assert!(views.visible_tabs(&smart).contains(&TabKey::All));

        // Favorites and Recently added can be hidden and shown again.
        assert!(views.set_tab_hidden(&TabKey::Favorites, true));
        assert!(views.set_tab_hidden(&TabKey::RecentlyAdded, true));
        let vis = views.visible_tabs(&smart);
        assert!(!vis.contains(&TabKey::Favorites));
        assert!(!vis.contains(&TabKey::RecentlyAdded));
        assert!(vis.contains(&TabKey::Collection("s-souls".into())));

        // Reordering works across presets (including locked ones) and collections.
        assert!(views.move_tab(&TabKey::Installed, &TabKey::All, &smart));
        assert_eq!(views.resolved_tabs(&smart)[0], TabKey::Installed);
        assert_eq!(views.resolved_tabs(&smart)[1], TabKey::All);
        assert!(views.shift_tab(&TabKey::Collection("s-souls".into()), -1, &smart));
        // Moving across visible tabs skips hidden tabs in one step.
        assert!(views.shift_visible_tab(&TabKey::Collection("s-souls".into()), -1, &smart));
        let vis_after = views.visible_tabs(&smart);
        assert_eq!(vis_after[1], TabKey::Collection("s-souls".into()));
        assert_eq!(vis_after[2], TabKey::All);
        // `Favorites` (hidden) now sits between `All` and `s-indie`; shifting
        // `s-indie` left by 1 skips over hidden `Favorites` past `All` in one step.
        assert!(views.shift_visible_tab(&TabKey::Collection("s-indie".into()), -1, &smart));
        let vis_after_skip = views.visible_tabs(&smart);
        assert_eq!(vis_after_skip[2], TabKey::Collection("s-indie".into()));
        assert_eq!(vis_after_skip[3], TabKey::All);
        // JSON round-trip preserves tab_order and hidden_tabs and drops orphan tabs.
        let json = serde_json::to_string(&views).unwrap();
        let back: SavedViews = serde_json::from_str(&json).unwrap();
        assert_eq!(back, views);
        let fewer_smart = vec!["s-souls".to_string()];
        assert!(
            !back
                .resolved_tabs(&fewer_smart)
                .contains(&TabKey::Collection("s-indie".into()))
        );
    }

    #[test]
    fn tab_prefs_persist_independently_and_round_trip() {
        let mut views = SavedViews::default();
        let souls = TabKey::Collection("s-souls".into());
        views.update_prefs(&souls, |p| {
            p.layout = Some(LibraryLayout::Grid);
            p.cover_shape = Some(CoverShape::Banner);
            p.tile_size = Some(13.5);
            p.sort = Some(LibrarySort::Score);
            p.descending = Some(false);
            p.group = Some(Grouping::None);
        });
        views.update_prefs(&TabKey::Installed, |p| {
            p.layout = Some(LibraryLayout::List);
            p.list_row_size = Some(ListRowSize::Large);
            p.sort = Some(LibrarySort::LastPlayed);
            p.group = Some(Grouping::Status);
        });
        views.set_active_tab(Some(souls.clone()));

        let json = serde_json::to_string(&views).unwrap();
        let back: SavedViews = serde_json::from_str(&json).unwrap();
        assert_eq!(back, views);
        assert_eq!(
            back.prefs_for(&souls).and_then(|p| p.cover_shape),
            Some(CoverShape::Banner)
        );
        assert_eq!(
            back.prefs_for(&TabKey::Installed)
                .and_then(|p| p.list_row_size),
            Some(ListRowSize::Large)
        );
        assert!(back.prefs_for(&TabKey::All).is_none());

        // Deleting a collection drops and can restore its per-tab prefs.
        let saved = views.take_tab_prefs(&souls).expect("prefs existed");
        views.forget_tab(&souls);
        assert!(views.prefs_for(&souls).is_none());
        assert_eq!(views.active_tab, None);
        views.restore_tab_prefs(&souls, saved);
        assert_eq!(
            views.prefs_for(&souls).and_then(|p| p.cover_shape),
            Some(CoverShape::Banner)
        );
    }
}
