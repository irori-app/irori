//! Saved library views: the filters, grouping and sort of the library under
//! a name ("Playing now", "Backlog"), one click away in the toolbar, the
//! sidebar and the command palette.
//!
//! A few useful views are added the first time, and from then on they are
//! the user's like any other: renamed, reordered or deleted, never brought
//! back. Built-in views keep their translated name until renamed. Views are
//! kept in `state/library-views.json`; the search text is never part of one.

use std::collections::BTreeSet;
use std::path::PathBuf;

use gpui_kit::{App, AppContext as _, Context, Entity, Global, SharedString, Task};
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
    pub const ALL: [BuiltinView; 4] = [
        BuiltinView::PlayingNow,
        BuiltinView::Backlog,
        BuiltinView::ReadyToPlay,
        BuiltinView::RecentlyAdded,
    ];

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

/// Every saved view, in the user's order.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct SavedViews {
    pub views: Vec<SavedView>,
    /// The built-in views were added once; deleting them keeps them gone.
    pub seeded: bool,
    next_id: u64,
}

impl SavedViews {
    /// Add the built-in views the first time.
    pub fn seed(&mut self) {
        if self.seeded {
            return;
        }
        self.seeded = true;
        for builtin in BuiltinView::ALL {
            let id = self.take_id();
            self.views.push(SavedView {
                id,
                name: None,
                builtin: Some(builtin),
                filter: builtin.filter(),
            });
        }
    }

    fn take_id(&mut self) -> u64 {
        let used = self.views.iter().map(|v| v.id).max().unwrap_or(0);
        self.next_id = self.next_id.max(used) + 1;
        self.next_id
    }

    pub fn get(&self, id: u64) -> Option<&SavedView> {
        self.views.iter().find(|v| v.id == id)
    }

    /// The first view showing exactly `filter`.
    pub fn active(&self, filter: &ViewFilter) -> Option<&SavedView> {
        self.views.iter().find(|v| v.shows(filter))
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

    /// Save `filter` (without its search text) as a new view at the end.
    pub fn add(&mut self, name: &str, filter: &ViewFilter) -> Result<u64, NameError> {
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
        });
        Ok(id)
    }

    /// Replace a view's criteria with `filter`.
    pub fn update_filter(&mut self, id: u64, filter: &ViewFilter) -> bool {
        let Some(view) = self.views.iter_mut().find(|v| v.id == id) else {
            return false;
        };
        view.filter = ViewFilter {
            query: String::new(),
            ..filter.clone()
        };
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
        Some((at, self.views.remove(at)))
    }

    /// Put a removed view back where it was.
    pub fn restore(&mut self, at: usize, view: SavedView) {
        if self.get(view.id).is_none() {
            self.views.insert(at.min(self.views.len()), view);
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
    let model = cx.new(|cx| {
        let mut model = ViewsModel { views, _save: None };
        if fresh && !cfg!(test) {
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

/// The view the library shows now, if its criteria match one.
pub fn active_view(cx: &mut App) -> Option<u64> {
    let filters = active_filters(cx)?;
    let model = saved_views(cx);
    let filter = filters.read(cx).filter().clone();
    model.read(cx).views().active(&filter).map(|v| v.id)
}

/// Show a saved view in the library, keeping the search text. Without a
/// library view yet, the criteria are stored for the one about to open.
pub fn apply_view(id: u64, cx: &mut App) {
    let model = saved_views(cx);
    let Some(filter) = model.read(cx).views().get(id).map(|v| v.filter.clone()) else {
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
        assert_eq!(views.views.len(), BuiltinView::ALL.len());
        let first = views.views[0].id;
        views.remove(first);
        views.seed();
        assert_eq!(views.views.len(), BuiltinView::ALL.len() - 1);
        // Ids are never reused, even after a delete.
        let id = views.add("Mine", &ViewFilter::default()).unwrap();
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
        let id = views.add("  GOG by playtime ", &filter).unwrap();
        let saved = views.get(id).unwrap();
        assert_eq!(saved.name.as_deref(), Some("GOG by playtime"));
        assert!(saved.filter.query.is_empty());
        // Typing a search keeps the view active; changing the sort does not.
        assert_eq!(views.active(&filter).map(|v| v.id), Some(id));
        let other = ViewFilter {
            sort: LibrarySort::Title,
            ..filter.clone()
        };
        assert!(views.active(&other).is_none());
        assert!(views.update_filter(id, &other));
        assert_eq!(views.active(&other).map(|v| v.id), Some(id));
    }

    #[test]
    fn names_are_unique_ignoring_case() {
        let mut views = SavedViews::default();
        views.seed();
        let a = views.add("Couch", &ViewFilter::default()).unwrap();
        assert_eq!(
            views.add("couch ", &ViewFilter::default()),
            Err(NameError::Exists)
        );
        assert_eq!(
            views.add("   ", &ViewFilter::default()),
            Err(NameError::Empty)
        );
        // Built-in names count too, in the interface language.
        let builtin = BuiltinView::Backlog.name().to_string();
        assert_eq!(
            views.add(&builtin, &ViewFilter::default()),
            Err(NameError::Exists)
        );
        // Renaming to its own name in another case is fine.
        assert!(views.rename(a, "COUCH").is_ok());
        assert_eq!(views.get(a).unwrap().label(), "COUCH");
    }

    #[test]
    fn reorders_removes_and_restores() {
        let mut views = SavedViews::default();
        let a = views.add("A", &ViewFilter::default()).unwrap();
        let b = views.add("B", &ViewFilter::default()).unwrap();
        let c = views.add("C", &ViewFilter::default()).unwrap();
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
        views.add("Mine", &ViewFilter::default()).unwrap();
        let text = serde_json::to_string(&views).unwrap();
        let back: SavedViews = serde_json::from_str(&text).unwrap();
        assert_eq!(back, views);
        assert!(text.contains("\"builtin\":\"playingNow\""));
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
}
