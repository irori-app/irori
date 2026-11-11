//! A mouse wheel or touchpad in Big Picture: travel turned into the same
//! focus moves the controller makes, so the view scrolls by the spring that
//! follows the focus. Nothing scrolls past an end (the focus stops there),
//! the layout never jumps between two offsets, and turning the wheel back
//! answers at once instead of first unwinding travel spent at an end.
//!
//! Which way the travel goes per screen: rows and grids (Shelves, Wall, the
//! Library) move by row vertically and along a row horizontally; the
//! single-shelf layouts (Spotlight, Cinema, Carousel) move along their
//! strip either way, as their shelf switcher changes the shelf; Compact's
//! list runs vertically, so vertical travel goes along it and horizontal
//! changes the shelf (its arrows are turned the same way). A game's page and
//! the quick menu only take vertical travel, through their rows.

use super::home::{Dir, HomeLayout};

/// Pixels of wheel or touchpad travel that move the focus by one item.
pub const WHEEL_STEP: f32 = 60.0;
/// The most moves one event makes, so a hard flick doesn't fly through a
/// whole shelf; travel beyond it is dropped.
pub const MAX_MOVES: usize = 8;

/// The screen the wheel turns over.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WheelView {
    Home(HomeLayout),
    /// The Library's grid, or the in-game overlay's screenshots.
    Grid,
    /// A game's page, the quick menu or a tab of the in-game overlay:
    /// rows, vertical travel only.
    Rows,
}

impl WheelView {
    /// The direction travel toward the top (`vertical`) or left goes when
    /// `back`, else toward the bottom or right.
    pub fn direction(self, vertical: bool, back: bool) -> Dir {
        let along_strip = matches!(
            self,
            WheelView::Home(HomeLayout::Spotlight | HomeLayout::Cinema | HomeLayout::Carousel)
        );
        match (vertical && !along_strip, back) {
            (true, true) => Dir::Up,
            (true, false) => Dir::Down,
            (false, true) => Dir::Left,
            (false, false) => Dir::Right,
        }
    }
}

/// Travel not yet turned into a move.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct WheelTravel {
    travel: f32,
    vertical: bool,
    view: Option<WheelView>,
}

impl WheelTravel {
    /// Turn one event's travel (`dx`, `dy` in pixels, positive toward the
    /// top and left as GPUI reports it) into moves on `view`. The larger
    /// axis counts; switching axis, direction or screen starts afresh.
    pub fn feed(&mut self, view: WheelView, dx: f32, dy: f32) -> Vec<Dir> {
        let vertical = view == WheelView::Rows || dy.abs() >= dx.abs();
        let amount = if vertical { dy } else { dx };
        if amount == 0.0 || !amount.is_finite() {
            return Vec::new();
        }
        if vertical != self.vertical || self.travel * amount < 0.0 || self.view != Some(view) {
            self.travel = 0.0;
        }
        self.view = Some(view);
        self.vertical = vertical;
        self.travel += amount;
        let whole = (self.travel.abs() / WHEEL_STEP).floor();
        if whole < 1.0 {
            return Vec::new();
        }
        let back = self.travel > 0.0;
        let moves = (whole as usize).min(MAX_MOVES);
        self.travel = if moves < whole as usize {
            0.0
        } else {
            self.travel - WHEEL_STEP.copysign(self.travel) * whole
        };
        vec![view.direction(vertical, back); moves]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grid::FocusGrid;
    use crate::layout::home::{grid_move, wall_step};
    use crate::layout::{GridStep, Metrics, end_scroll, grid_step, list_scroll, strip_start};

    const DOWN: f32 = -20.0;
    const UP: f32 = 20.0;

    /// Shelves of these lengths: a short one, a long one, a middling one.
    const LENS: [usize; 3] = [3, 30, 12];

    #[test]
    fn small_steps_add_up_and_reversing_answers_at_once() {
        let mut w = WheelTravel::default();
        let view = WheelView::Home(HomeLayout::Shelves);
        assert!(w.feed(view, 0.0, DOWN).is_empty());
        assert!(w.feed(view, 0.0, DOWN).is_empty());
        assert_eq!(w.feed(view, 0.0, DOWN), vec![Dir::Down]);
        // 40 px of downward travel left over must not swallow the turn back.
        assert!(w.feed(view, 0.0, DOWN).is_empty());
        assert!(w.feed(view, 0.0, DOWN).is_empty());
        assert!(w.feed(view, 0.0, UP * 3.0).iter().all(|d| *d == Dir::Up));
        assert_eq!(w.feed(view, 0.0, UP * 3.0), vec![Dir::Up]);
        // A sideways stroke starts afresh too.
        assert!(w.feed(view, -40.0, 0.0).is_empty());
        assert_eq!(w.feed(view, -40.0, 5.0), vec![Dir::Right]);
        // Nothing, or nonsense, moves nothing.
        assert!(w.feed(view, 0.0, 0.0).is_empty());
        assert!(w.feed(view, f32::NAN, f32::NAN).is_empty());
    }

    #[test]
    fn travel_on_another_screen_starts_afresh() {
        let mut w = WheelTravel::default();
        assert!(w.feed(WheelView::Grid, 0.0, -50.0).is_empty());
        assert!(
            w.feed(WheelView::Rows, 0.0, -50.0).is_empty(),
            "the page opened"
        );
        assert_eq!(w.feed(WheelView::Rows, 0.0, -10.0), vec![Dir::Down]);
    }

    #[test]
    fn a_flick_moves_a_few_covers_not_the_whole_shelf() {
        let mut w = WheelTravel::default();
        let moves = w.feed(WheelView::Grid, 0.0, -10_000.0);
        assert_eq!(moves, vec![Dir::Down; MAX_MOVES]);
        assert!(
            w.feed(WheelView::Grid, 0.0, -1.0).is_empty(),
            "rest dropped"
        );
        assert_eq!(
            w.feed(WheelView::Grid, 0.0, -2.5 * WHEEL_STEP),
            vec![Dir::Down; 2]
        );
    }

    #[test]
    fn pages_and_menus_take_vertical_travel_only() {
        let mut w = WheelTravel::default();
        assert!(w.feed(WheelView::Rows, -500.0, 0.0).is_empty());
        assert_eq!(w.feed(WheelView::Rows, -500.0, -60.0), vec![Dir::Down]);
    }

    #[test]
    fn each_layout_turns_the_wheel_its_own_way() {
        use HomeLayout::*;
        for (layout, vertical) in [
            (Shelves, Dir::Down),
            (Wall, Dir::Down),
            (Compact, Dir::Down),
            (Spotlight, Dir::Right),
            (Cinema, Dir::Right),
            (Carousel, Dir::Right),
        ] {
            let view = WheelView::Home(layout);
            assert_eq!(view.direction(true, false), vertical, "{layout:?}");
            assert_eq!(view.direction(false, true), Dir::Left, "{layout:?}");
        }
        assert_eq!(WheelView::Grid.direction(true, true), Dir::Up);
    }

    /// Where each Home layout's spring heads for a focus: the shelf strip's
    /// first cover, the wall's or the compact list's scroll, the carousel's
    /// focused cover, and the shelf for the layouts that stack shelves.
    fn offset(layout: HomeLayout, m: &Metrics, (row, col): (usize, usize)) -> (f32, f32) {
        let shown = 5;
        match layout {
            HomeLayout::Shelves => (
                strip_start(row, LENS.len(), 2) as f32,
                strip_start(col, LENS[row], shown) as f32,
            ),
            HomeLayout::Spotlight | HomeLayout::Cinema => {
                (row as f32, strip_start(col, LENS[row], shown) as f32)
            }
            HomeLayout::Carousel => (row as f32, col as f32),
            HomeLayout::Compact => {
                let row_h = crate::layout::home::LIST_ROW;
                let scroll = list_scroll(col, None, row_h, 0.0, 20.0);
                (
                    row as f32,
                    end_scroll(scroll, LENS[row] as f32 * row_h, 20.0),
                )
            }
            HomeLayout::Wall => {
                let wall = m.wall();
                let (tops, total) = wall.sections(&LENS);
                (wall.scroll(&tops, total, (row, col), 30.0), 0.0)
            }
        }
    }

    /// Most an offset may be: the end of the content, never past it.
    fn limit(layout: HomeLayout, m: &Metrics, row: usize) -> (f32, f32) {
        let shown = 5;
        match layout {
            HomeLayout::Shelves => (
                (LENS.len() - 2) as f32,
                LENS[row].saturating_sub(shown) as f32,
            ),
            HomeLayout::Spotlight | HomeLayout::Cinema => (
                (LENS.len() - 1) as f32,
                LENS[row].saturating_sub(shown) as f32,
            ),
            HomeLayout::Carousel => ((LENS.len() - 1) as f32, (LENS[row] - 1) as f32),
            HomeLayout::Compact => (
                (LENS.len() - 1) as f32,
                (LENS[row] as f32 * crate::layout::home::LIST_ROW - 20.0).max(0.0),
            ),
            HomeLayout::Wall => {
                let wall = m.wall();
                let (_, total) = wall.sections(&LENS);
                ((total - 30.0).max(0.0), 0.0)
            }
        }
    }

    /// Feed `events` of travel on `layout`, moving the focus as Home does,
    /// and check every offset its spring is sent to stays inside the
    /// content and moves at most one step's worth per move.
    fn scroll(
        layout: HomeLayout,
        grid: &mut FocusGrid,
        events: &[(f32, f32)],
    ) -> Vec<(usize, usize)> {
        let m = Metrics::new(80.0, 45.0);
        let cols = m.wall().cols;
        let mut w = WheelTravel::default();
        let mut seen = Vec::new();
        for (dx, dy) in events {
            for dir in w.feed(WheelView::Home(layout), *dx, *dy) {
                let before = grid.focused(&LENS).unwrap();
                if layout == HomeLayout::Wall {
                    if let Some((r, c)) = wall_step(&LENS, cols, before, dir) {
                        grid.set(r, c, &LENS);
                    }
                } else {
                    grid.apply(grid_move(layout, dir), &LENS);
                }
                let after = grid.focused(&LENS).unwrap();
                let (a, b) = (offset(layout, &m, before), offset(layout, &m, after));
                let (max_a, max_b) = limit(layout, &m, after.0);
                assert!(
                    (0.0..=max_a + 0.01).contains(&b.0) && (0.0..=max_b + 0.01).contains(&b.1),
                    "{layout:?}: {b:?} past the end ({max_a}, {max_b})"
                );
                if before.0 == after.0 && layout != HomeLayout::Wall {
                    let step = if layout == HomeLayout::Compact {
                        crate::layout::home::LIST_ROW
                    } else {
                        1.0
                    };
                    assert!(
                        (b.1 - a.1).abs() <= step + 0.01,
                        "{layout:?}: jumped from {a:?} to {b:?}"
                    );
                }
                seen.push(after);
            }
        }
        seen
    }

    #[test]
    fn the_wheel_scrolls_every_home_layout_within_its_ends() {
        let down = vec![(0.0, DOWN); 400];
        let up = vec![(0.0, UP); 400];
        let right = vec![(DOWN, 0.0); 400];
        for layout in HomeLayout::ALL {
            let mut grid = FocusGrid::default();
            grid.set(1, 0, &LENS);
            let forward = scroll(layout, &mut grid, &down);
            assert!(!forward.is_empty(), "{layout:?} moves");
            let end = grid.focused(&LENS).unwrap();
            // Turning further at the end moves nothing and stores nothing:
            // one step back answers at once.
            assert!(scroll(layout, &mut grid, &down).iter().all(|f| *f == end));
            let back = scroll(layout, &mut grid, &up[..3]);
            assert_eq!(back.len(), 1, "{layout:?}: turning back answers at once");
            assert_ne!(back[0], end, "{layout:?}");
            scroll(layout, &mut grid, &up);
            scroll(layout, &mut grid, &right);
        }
    }

    #[test]
    fn the_wheel_runs_through_the_library_grid_and_stops_at_its_ends() {
        let (len, cols, shown): (usize, usize, usize) = (23, 5, 2);
        let rows = len.div_ceil(cols);
        let mut w = WheelTravel::default();
        let mut ix = 0;
        let mut starts = Vec::new();
        for (dx, dy) in [(0.0, DOWN); 200].into_iter().chain([(0.0, UP); 200]) {
            for dir in w.feed(WheelView::Grid, dx, dy) {
                let step = match dir {
                    Dir::Up => GridStep::Up,
                    Dir::Down => GridStep::Down,
                    Dir::Left => GridStep::Left,
                    Dir::Right => GridStep::Right,
                };
                if let Some(next) = grid_step(ix, len, cols, step) {
                    ix = next;
                }
                let start = strip_start(ix / cols, rows, shown);
                assert!(start <= rows - shown);
                if let Some(last) = starts.last() {
                    assert!(start.abs_diff(*last) <= 1, "no jumps");
                }
                starts.push(start);
            }
        }
        assert_eq!(starts.iter().max(), Some(&(rows - shown)));
        assert_eq!(starts.last(), Some(&0));
    }
}
