//! In-window navigation with back/forward history. Features request
//! navigation through [`navigate`]; the shell renders the current route.

use gpui_kit::{App, AppContext as _, Context, Entity, Global, SharedString};
use irori_core::GameId;

/// A destination in the main window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Route {
    /// The library, optionally scoped to a named view (e.g. a category).
    Library,
    Game(GameId),
    Downloads,
    /// What can be updated: games, components and Irori.
    Updates,
    /// Settings, opened on a section id (e.g. "general", "wine").
    Settings(SharedString),
    /// Settings of one game, opened on a section id.
    GameSettings(GameId, SharedString),
    WineManager,
    /// Emulators, their versions and cores, and each system's default.
    Emulators,
    Accounts,
    Stores,
    /// Play time per week, store and game, and streaks.
    Stats,
    /// A year's play told as full-window stories; `None` opens the year
    /// the review picks (see the stats page).
    YearInReview(Option<i32>),
    /// Gamepad-first fullscreen mode.
    BigPicture,
    /// First-run welcome and setup.
    Onboarding,
}

impl Route {
    /// Top-level section this route belongs to, for sidebar highlighting.
    pub fn section(&self) -> &'static str {
        match self {
            Route::Library | Route::Game(_) | Route::GameSettings(..) => "library",
            Route::Downloads => "downloads",
            Route::Updates => "updates",
            Route::Settings(_) => "settings",
            Route::WineManager => "wine",
            Route::Emulators => "emulators",
            Route::Accounts => "accounts",
            Route::Stores => "stores",
            Route::Stats | Route::YearInReview(_) => "stats",
            Route::BigPicture => "big-picture",
            Route::Onboarding => "onboarding",
        }
    }
}

/// Back/forward history around the current route, kept apart from the
/// entity so its rules can be tested without a window.
#[derive(Debug)]
struct History {
    current: Route,
    back: Vec<Route>,
    forward: Vec<Route>,
}

/// How many routes "Back" remembers.
const HISTORY_LIMIT: usize = 64;

impl History {
    fn new(current: Route) -> Self {
        Self {
            current,
            back: Vec::new(),
            forward: Vec::new(),
        }
    }

    /// Moves to `route`; false when nothing changed.
    fn go(&mut self, route: Route) -> bool {
        if route == self.current {
            return false;
        }
        // Going to the route just left is going back: a page's own "Back"
        // button (a game's settings returning to the game) must not stack the
        // two pages, or the next Back lands on the page just left again.
        if self.back.last() == Some(&route) {
            return self.back();
        }
        let prev = std::mem::replace(&mut self.current, route);
        self.back.push(prev);
        if self.back.len() > HISTORY_LIMIT {
            self.back.remove(0);
        }
        self.forward.clear();
        true
    }

    fn back(&mut self) -> bool {
        let Some(prev) = self.back.pop() else {
            return false;
        };
        let cur = std::mem::replace(&mut self.current, prev);
        self.forward.push(cur);
        true
    }

    fn forward(&mut self) -> bool {
        let Some(next) = self.forward.pop() else {
            return false;
        };
        let cur = std::mem::replace(&mut self.current, next);
        self.back.push(cur);
        true
    }
}

pub struct Navigator {
    history: History,
}

impl Global for NavigatorHandle {}

#[derive(Clone)]
pub struct NavigatorHandle(pub Entity<Navigator>);

impl Navigator {
    pub fn current(&self) -> &Route {
        &self.history.current
    }

    pub fn can_go_back(&self) -> bool {
        !self.history.back.is_empty()
    }

    pub fn can_go_forward(&self) -> bool {
        !self.history.forward.is_empty()
    }

    /// The route `back` returns to.
    pub fn previous(&self) -> Option<&Route> {
        self.history.back.last()
    }

    /// Goes to `route`. Going to the route "Back" would return to is the
    /// same as going back, so pages that return with a button of their own
    /// keep the history the user expects.
    pub fn go(&mut self, route: Route, cx: &mut Context<Self>) {
        if self.history.go(route) {
            cx.notify();
        }
    }

    /// Replace the current route without adding history.
    pub fn replace(&mut self, route: Route, cx: &mut Context<Self>) {
        self.history.current = route;
        cx.notify();
    }

    pub fn back(&mut self, cx: &mut Context<Self>) {
        if self.history.back() {
            cx.notify();
        }
    }

    pub fn forward(&mut self, cx: &mut Context<Self>) {
        if self.history.forward() {
            cx.notify();
        }
    }
}

pub(crate) fn install(cx: &mut App) {
    let nav = cx.new(|_| Navigator {
        history: History::new(Route::Library),
    });
    cx.set_global(NavigatorHandle(nav));
}

/// The navigator entity.
pub fn navigator(cx: &App) -> Entity<Navigator> {
    cx.global::<NavigatorHandle>().0.clone()
}

/// Navigate to a route.
pub fn navigate(route: Route, cx: &mut App) {
    navigator(cx).update(cx, |n, cx| n.go(route, cx));
}

pub fn go_back(cx: &mut App) {
    navigator(cx).update(cx, |n, cx| n.back(cx));
}

pub fn go_forward(cx: &mut App) {
    navigator(cx).update(cx, |n, cx| n.forward(cx));
}

#[cfg(test)]
mod tests {
    use super::*;
    use irori_core::Runner;

    fn game() -> Route {
        Route::Game(GameId::new(Runner::Gog, "1"))
    }

    #[test]
    fn returning_from_game_settings_keeps_library_behind_the_game() {
        let mut h = History::new(Route::Library);
        h.go(game());
        h.go(Route::GameSettings(
            GameId::new(Runner::Gog, "1"),
            "general".into(),
        ));
        // The settings page's own "Back" button navigates to the game.
        h.go(game());
        assert_eq!(h.current, game());
        assert!(h.back());
        assert_eq!(h.current, Route::Library);
        assert!(!h.back());
    }

    #[test]
    fn going_somewhere_new_clears_forward() {
        let mut h = History::new(Route::Library);
        h.go(Route::Downloads);
        h.back();
        assert_eq!(h.forward, vec![Route::Downloads]);
        h.go(Route::Stats);
        assert!(h.forward.is_empty());
        assert_eq!(h.back, vec![Route::Library]);
    }

    #[test]
    fn same_route_changes_nothing_and_history_is_bounded() {
        let mut h = History::new(Route::Library);
        assert!(!h.go(Route::Library));
        for i in 0..100 {
            h.go(Route::Settings(format!("s{i}").into()));
        }
        assert_eq!(h.back.len(), HISTORY_LIMIT);
    }
}
