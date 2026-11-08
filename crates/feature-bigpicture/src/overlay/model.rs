//! What the in-game overlay shows and where its focus is, without drawing:
//! the tabs and their rows, the question before quitting the game, what is
//! next among a game's achievements and the queue of unlock notices. Pure
//! logic, tested without a window.

use std::collections::{HashSet, VecDeque};

use irori_core::Achievement;

use crate::gamepad::GamepadInput;

/// The overlay's tabs, in the order LB/RB walk them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Tab {
    Home,
    Screenshots,
    Achievements,
    Journal,
}

impl Tab {
    pub const ALL: [Tab; 4] = [Tab::Home, Tab::Screenshots, Tab::Achievements, Tab::Journal];

    fn index(self) -> usize {
        Self::ALL.iter().position(|t| *t == self).unwrap_or(0)
    }

    /// The tab `step` places along, wrapping around.
    pub fn step(self, step: isize) -> Tab {
        let len = Self::ALL.len() as isize;
        Self::ALL[(self.index() as isize + step).rem_euclid(len) as usize]
    }
}

/// The rows of the Home tab, top to bottom.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HomeRow {
    SystemVolume,
    GameVolume,
    /// The sound output: Left and Right switch to the previous and next.
    Output,
    Screenshot,
    BackToIrori,
    QuitGame,
}

impl HomeRow {
    pub const ALL: [HomeRow; 6] = [
        HomeRow::SystemVolume,
        HomeRow::GameVolume,
        HomeRow::Output,
        HomeRow::Screenshot,
        HomeRow::BackToIrori,
        HomeRow::QuitGame,
    ];

    pub fn at(row: usize) -> Option<HomeRow> {
        Self::ALL.get(row).copied()
    }

    /// Whether Left and Right change this row's value.
    pub fn adjusts(self) -> bool {
        matches!(
            self,
            HomeRow::SystemVolume | HomeRow::GameVolume | HomeRow::Output
        )
    }
}

/// The output `step` places from the current one in `outputs` (by
/// [`AudioOutput::current`]), wrapping around; `None` with fewer than two.
pub fn next_output(
    outputs: &[irori_services::audio::AudioOutput],
    step: i32,
) -> Option<&irori_services::audio::AudioOutput> {
    if outputs.len() < 2 {
        return None;
    }
    let len = outputs.len() as i32;
    let now = outputs.iter().position(|o| o.current).unwrap_or(0) as i32;
    outputs.get((now + step).rem_euclid(len) as usize)
}

/// How many screenshots a row of the Screenshots tab holds.
pub const SHOT_COLUMNS: usize = 3;

/// The two answers to "Quit the game?".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Choice {
    Cancel,
    Quit,
}

/// What an input asks the overlay to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Nothing changed.
    None,
    /// Focus, tab or question changed: draw again.
    Moved,
    /// Close the overlay.
    Close,
    /// Run the focused row of `tab`.
    Activate(Tab, usize),
    /// Move a Home tab volume by `step` units (negative is quieter).
    Adjust(HomeRow, i32),
    /// The player confirmed quitting the game.
    Quit,
}

/// The overlay's focus: the tab shown, the focused row of each tab and the
/// question before quitting, if it is asked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Nav {
    tab: Tab,
    focus: [usize; 4],
    confirm: Option<Choice>,
}

impl Default for Nav {
    fn default() -> Self {
        Self {
            tab: Tab::Home,
            focus: [0; 4],
            confirm: None,
        }
    }
}

impl Nav {
    pub fn tab(&self) -> Tab {
        self.tab
    }

    /// The focused row of `tab`, within `rows` rows.
    pub fn focus(&self, tab: Tab, rows: usize) -> usize {
        self.focus[tab.index()].min(rows.saturating_sub(1))
    }

    /// The question before quitting and its focused answer, when asked.
    pub fn confirm(&self) -> Option<Choice> {
        self.confirm
    }

    /// Show `tab` (a click on it).
    pub fn show(&mut self, tab: Tab) {
        self.tab = tab;
        self.confirm = None;
    }

    /// Focus `row` of the shown tab (a click on it).
    pub fn set_focus(&mut self, row: usize) {
        self.focus[self.tab.index()] = row;
    }

    /// Ask before quitting, with Cancel focused.
    pub fn ask_quit(&mut self) {
        self.confirm = Some(Choice::Cancel);
    }

    /// Stop asking.
    pub fn cancel_quit(&mut self) {
        self.confirm = None;
    }

    /// Handle one input. `rows` is how many rows the shown tab has now.
    pub fn input(&mut self, input: GamepadInput, rows: usize) -> Action {
        use GamepadInput::*;
        if input == Guide {
            return Action::Close;
        }
        if let Some(choice) = self.confirm {
            return match input {
                Left | Right | Up | Down => {
                    self.confirm = Some(match choice {
                        Choice::Cancel => Choice::Quit,
                        Choice::Quit => Choice::Cancel,
                    });
                    Action::Moved
                }
                Confirm => {
                    self.confirm = None;
                    match choice {
                        Choice::Cancel => Action::Moved,
                        Choice::Quit => Action::Quit,
                    }
                }
                Back => {
                    self.confirm = None;
                    Action::Moved
                }
                _ => Action::None,
            };
        }
        let ix = self.tab.index();
        let focus = self.focus(self.tab, rows);
        let columns = if self.tab == Tab::Screenshots {
            SHOT_COLUMNS
        } else {
            1
        };
        let move_to = |to: usize, nav: &mut Self| {
            if to == focus || to >= rows {
                Action::None
            } else {
                nav.focus[ix] = to;
                Action::Moved
            }
        };
        match input {
            Back => Action::Close,
            PrevPage | NextPage => {
                self.tab = self.tab.step(if input == PrevPage { -1 } else { 1 });
                Action::Moved
            }
            Up => match focus.checked_sub(columns) {
                Some(to) => move_to(to, self),
                None => Action::None,
            },
            Down => move_to(focus + columns, self),
            Left | Right => {
                let step = if input == Left { -1 } else { 1 };
                if self.tab == Tab::Home {
                    match HomeRow::at(focus) {
                        Some(row) if row.adjusts() => Action::Adjust(row, step),
                        _ => Action::None,
                    }
                } else if columns > 1 {
                    match focus.checked_add_signed(step as isize) {
                        Some(to) => move_to(to, self),
                        None => Action::None,
                    }
                } else {
                    Action::None
                }
            }
            Confirm if rows > 0 => Action::Activate(self.tab, focus),
            _ => Action::None,
        }
    }
}

/// How many achievements of `list` are unlocked, of how many.
pub fn progress(list: &[Achievement]) -> (usize, usize) {
    (list.iter().filter(|a| a.is_unlocked()).count(), list.len())
}

/// The locked achievements to aim for next, at most `count`: those with
/// progress first (furthest along first), then the ones most players have.
/// Hidden ones come last, since their names say nothing yet.
pub fn next_up(list: &[Achievement], count: usize) -> Vec<&Achievement> {
    let mut locked: Vec<&Achievement> = list.iter().filter(|a| !a.is_unlocked()).collect();
    locked.sort_by(|a, b| {
        b.visible
            .cmp(&a.visible)
            .then_with(|| {
                let (pa, pb) = (a.progress.unwrap_or(-1.0), b.progress.unwrap_or(-1.0));
                pb.total_cmp(&pa)
            })
            .then_with(|| b.rarity.total_cmp(&a.rarity))
            .then_with(|| a.name.cmp(&b.name))
    });
    locked.truncate(count);
    locked
}

/// When an achievement was unlocked, in Unix seconds (the earlier mode's).
fn unlocked_at(a: &Achievement) -> Option<i64> {
    [&a.unlocked_at, &a.hardcore_unlocked_at]
        .into_iter()
        .flatten()
        .filter_map(|t| irori_core::util::parse_rfc3339(t))
        .min()
}

/// The achievements of `list` unlocked at `since` or later, oldest first.
pub fn unlocked_since(list: &[Achievement], since: i64) -> Vec<&Achievement> {
    let mut out: Vec<(i64, &Achievement)> = list
        .iter()
        .filter_map(|a| Some((unlocked_at(a)?, a)))
        .filter(|(at, _)| *at >= since)
        .collect();
    out.sort_by_key(|(at, _)| *at);
    out.into_iter().map(|(_, a)| a).collect()
}

/// One unlock notice in the overlay's corner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Toast {
    pub name: String,
    pub description: String,
    /// The achievement's picture; empty for none.
    pub image: String,
}

/// Unlock notices waiting to be shown, one after another. Each unlock is
/// shown once per run of Irori, however often the overlay opens.
#[derive(Debug, Default)]
pub struct Toasts {
    queue: VecDeque<Toast>,
    seen: HashSet<(String, String)>,
}

impl Toasts {
    /// Queue the unlocks of `game` (its key) in `list` made at `since` or
    /// later and not shown yet. Returns how many were queued.
    pub fn offer(&mut self, game: &str, list: &[Achievement], since: i64) -> usize {
        let mut added = 0;
        for a in unlocked_since(list, since) {
            if self.seen.insert((game.to_string(), a.id.clone())) {
                self.queue.push_back(Toast {
                    name: a.name.clone(),
                    description: a.description.clone(),
                    image: a.image_unlocked.clone(),
                });
                added += 1;
            }
        }
        added
    }

    /// The notice shown now.
    pub fn current(&self) -> Option<&Toast> {
        self.queue.front()
    }

    /// How many wait behind the one shown.
    pub fn waiting(&self) -> usize {
        self.queue.len().saturating_sub(1)
    }

    /// Done with the notice shown; returns whether another follows.
    pub fn advance(&mut self) -> bool {
        self.queue.pop_front();
        !self.queue.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use GamepadInput::*;

    #[test]
    fn shoulders_walk_the_tabs_around() {
        let mut nav = Nav::default();
        assert_eq!(nav.tab(), Tab::Home);
        assert_eq!(nav.input(NextPage, 5), Action::Moved);
        assert_eq!(nav.tab(), Tab::Screenshots);
        nav.input(PrevPage, 0);
        nav.input(PrevPage, 5);
        assert_eq!(nav.tab(), Tab::Journal, "wraps around");
        nav.input(NextPage, 0);
        assert_eq!(nav.tab(), Tab::Home);
    }

    #[test]
    fn home_rows_move_activate_and_adjust() {
        let mut nav = Nav::default();
        let rows = HomeRow::ALL.len();
        assert_eq!(nav.input(Up, rows), Action::None, "at the top");
        assert_eq!(
            nav.input(Right, rows),
            Action::Adjust(HomeRow::SystemVolume, 1)
        );
        assert_eq!(nav.input(Down, rows), Action::Moved);
        assert_eq!(
            nav.input(Left, rows),
            Action::Adjust(HomeRow::GameVolume, -1)
        );
        nav.input(Down, rows);
        assert_eq!(nav.input(Right, rows), Action::Adjust(HomeRow::Output, 1));
        nav.input(Down, rows);
        assert_eq!(nav.input(Right, rows), Action::None, "not a value");
        assert_eq!(nav.input(Confirm, rows), Action::Activate(Tab::Home, 3));
        for _ in 0..10 {
            nav.input(Down, rows);
        }
        assert_eq!(nav.focus(Tab::Home, rows), rows - 1, "stops at the bottom");
        assert_eq!(nav.input(Back, rows), Action::Close);
        assert_eq!(nav.input(Guide, rows), Action::Close);
    }

    #[test]
    fn outputs_cycle_from_the_current_one() {
        use irori_services::audio::AudioOutput;
        let out = |id: &str, current: bool| AudioOutput {
            id: id.into(),
            name: id.into(),
            current,
        };
        let outputs = [out("a", false), out("b", true), out("c", false)];
        let id = |o: Option<&AudioOutput>| o.map(|o| o.id.clone());
        assert_eq!(id(next_output(&outputs, 1)), Some("c".into()));
        assert_eq!(id(next_output(&outputs, -1)), Some("a".into()));
        assert_eq!(id(next_output(&outputs, 2)), Some("a".into()), "wraps");
        assert_eq!(next_output(&outputs[..1], 1), None, "nothing to switch to");
    }

    #[test]
    fn quitting_asks_first_with_cancel_focused() {
        let mut nav = Nav::default();
        nav.ask_quit();
        assert_eq!(nav.confirm(), Some(Choice::Cancel));
        assert_eq!(nav.input(NextPage, 5), Action::None, "tabs wait");
        assert_eq!(nav.input(Confirm, 5), Action::Moved, "Cancel");
        assert_eq!(nav.confirm(), None);
        nav.ask_quit();
        assert_eq!(nav.input(Back, 5), Action::Moved);
        assert_eq!(nav.confirm(), None);
        nav.ask_quit();
        assert_eq!(nav.input(Right, 5), Action::Moved);
        assert_eq!(nav.confirm(), Some(Choice::Quit));
        assert_eq!(nav.input(Confirm, 5), Action::Quit);
        assert_eq!(nav.confirm(), None);
        nav.ask_quit();
        assert_eq!(nav.input(Guide, 5), Action::Close, "Guide still closes");
    }

    #[test]
    fn screenshots_move_through_a_grid() {
        let mut nav = Nav::default();
        nav.show(Tab::Screenshots);
        let rows = 7;
        assert_eq!(nav.input(Left, rows), Action::None);
        nav.input(Right, rows);
        nav.input(Down, rows);
        assert_eq!(nav.focus(Tab::Screenshots, rows), 1 + SHOT_COLUMNS);
        assert_eq!(nav.input(Down, rows), Action::None, "no shot below");
        nav.input(Up, rows);
        assert_eq!(nav.focus(Tab::Screenshots, rows), 1);
        assert_eq!(
            nav.input(Confirm, rows),
            Action::Activate(Tab::Screenshots, 1)
        );
        assert_eq!(nav.input(Confirm, 0), Action::None, "nothing to open");
        assert_eq!(nav.focus(Tab::Screenshots, 1), 0, "clamped to what is left");
        assert_eq!(nav.focus(Tab::Home, 5), 0, "each tab keeps its own focus");
    }

    fn achievement(
        id: &str,
        unlocked: Option<&str>,
        progress: Option<f32>,
        rarity: f32,
    ) -> Achievement {
        Achievement {
            id: id.into(),
            name: id.to_uppercase(),
            visible: true,
            unlocked_at: unlocked.map(String::from),
            progress,
            rarity,
            ..Default::default()
        }
    }

    #[test]
    fn next_up_prefers_progress_then_common_ones() {
        let mut hidden = achievement("hidden", None, Some(0.9), 90.0);
        hidden.visible = false;
        let list = vec![
            achievement("done", Some("2026-01-01T00:00:00Z"), None, 50.0),
            achievement("rare", None, None, 2.0),
            achievement("common", None, None, 60.0),
            achievement("half", None, Some(0.5), 1.0),
            achievement("most", None, Some(0.8), 1.0),
            hidden,
        ];
        assert_eq!(progress(&list), (1, 6));
        let ids: Vec<&str> = next_up(&list, 10).iter().map(|a| a.id.as_str()).collect();
        assert_eq!(ids, ["most", "half", "common", "rare", "hidden"]);
        assert_eq!(next_up(&list, 2).len(), 2);
    }

    #[test]
    fn unlock_notices_queue_once_per_unlock() {
        let since = irori_core::util::parse_rfc3339("2026-05-01T12:00:00Z").unwrap();
        let mut hardcore = achievement("hard", None, None, 1.0);
        hardcore.hardcore_unlocked_at = Some("2026-05-01T12:30:00Z".into());
        let list = vec![
            achievement("before", Some("2026-05-01T11:00:00Z"), None, 1.0),
            achievement("second", Some("2026-05-01T12:20:00Z"), None, 1.0),
            achievement("first", Some("2026-05-01T12:10:00Z"), None, 1.0),
            hardcore,
            achievement("locked", None, None, 1.0),
        ];
        let ids: Vec<&str> = unlocked_since(&list, since)
            .iter()
            .map(|a| a.id.as_str())
            .collect();
        assert_eq!(ids, ["first", "second", "hard"]);

        let mut toasts = Toasts::default();
        assert_eq!(toasts.current(), None);
        assert_eq!(toasts.offer("gog:1", &list, since), 3);
        assert_eq!(toasts.offer("gog:1", &list, since), 0, "shown already");
        assert_eq!(toasts.offer("epic:1", &list, since), 3, "another game");
        assert_eq!(toasts.current().map(|t| t.name.as_str()), Some("FIRST"));
        assert_eq!(toasts.waiting(), 5);
        assert!(toasts.advance());
        assert_eq!(toasts.current().map(|t| t.name.as_str()), Some("SECOND"));
        for _ in 0..4 {
            toasts.advance();
        }
        assert!(!toasts.advance());
        assert_eq!(toasts.current(), None);
    }
}
