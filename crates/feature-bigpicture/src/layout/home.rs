//! Home's layouts: which there are, how the controller moves through each,
//! and their geometry in rem. Every layout keeps the same focus (a shelf and
//! a game on it, see [`crate::grid::FocusGrid`]), so switching layouts keeps
//! the focused game.

use gpui_kit::SharedString;
use irori_ui::i18n::t;

use super::{FOCUS_SCALE, Metrics, SHELF_HEADING, TILE_GAP, shape_scale};
use crate::grid::GridMove;

/// How Home arranges its games. Each has its own renderer in `view::home`
/// and its sibling modules.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum HomeLayout {
    /// The focused game's title above shelves of covers.
    #[default]
    Shelves,
    /// One game large, with its shelf in a strip below.
    Spotlight,
    /// The focused game's art in a wide, letterboxed band over a filmstrip.
    Cinema,
    /// Every shelf as a dense wall of covers.
    Wall,
    /// A shelf on a turning wheel, the focused cover in front.
    Carousel,
    /// A list of titles beside the focused cover, for small screens such as
    /// the Steam Deck's.
    Compact,
}

impl HomeLayout {
    pub const ALL: [HomeLayout; 6] = [
        HomeLayout::Shelves,
        HomeLayout::Spotlight,
        HomeLayout::Cinema,
        HomeLayout::Wall,
        HomeLayout::Carousel,
        HomeLayout::Compact,
    ];

    /// The id in settings and theme files.
    pub fn id(self) -> &'static str {
        match self {
            HomeLayout::Shelves => "shelves",
            HomeLayout::Spotlight => "spotlight",
            HomeLayout::Cinema => "cinema",
            HomeLayout::Wall => "wall",
            HomeLayout::Carousel => "carousel",
            HomeLayout::Compact => "compact",
        }
    }

    pub fn from_id(id: &str) -> Option<HomeLayout> {
        Self::ALL.into_iter().find(|l| l.id() == id)
    }

    /// The layout a setting names; Shelves for anything unknown.
    pub fn from_setting(id: &str) -> HomeLayout {
        Self::from_id(id).unwrap_or_default()
    }

    pub fn label(self) -> SharedString {
        t(match self {
            HomeLayout::Shelves => "bigpicture.layout.shelves",
            HomeLayout::Spotlight => "bigpicture.layout.spotlight",
            HomeLayout::Cinema => "bigpicture.layout.cinema",
            HomeLayout::Wall => "bigpicture.layout.wall",
            HomeLayout::Carousel => "bigpicture.layout.carousel",
            HomeLayout::Compact => "bigpicture.layout.compact",
        })
    }
}

/// A direction pressed on Home.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dir {
    Up,
    Down,
    Left,
    Right,
}

/// The focus move a direction makes in `layout`, for every layout but the
/// wall (see [`wall_step`]). The compact layout lists a shelf from top to
/// bottom, so Up and Down run along it and Left and Right change shelves.
pub fn grid_move(layout: HomeLayout, dir: Dir) -> GridMove {
    let vertical = layout == HomeLayout::Compact;
    match (dir, vertical) {
        (Dir::Up, false) | (Dir::Left, true) => GridMove::Up,
        (Dir::Down, false) | (Dir::Right, true) => GridMove::Down,
        (Dir::Left, false) | (Dir::Up, true) => GridMove::Left,
        (Dir::Right, false) | (Dir::Down, true) => GridMove::Right,
    }
}

// ---- spotlight -------------------------------------------------------------

/// The spotlight layout: a hero with the focused game's cover large, and a
/// strip of the shelf's covers under it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Spotlight {
    /// Width of a strip cover at rest.
    pub tile: f32,
    /// Height of the strip, heading included.
    pub strip: f32,
    /// Height of the hero above the strip.
    pub hero: f32,
    /// The large cover's size.
    pub feature_w: f32,
    pub feature_h: f32,
}

impl Metrics {
    pub fn spotlight(&self) -> Spotlight {
        let area = self.area();
        let limit = shape_scale(self.ratio);
        let tile = ((area * 0.3 - SHELF_HEADING - TILE_GAP) / (self.ratio * FOCUS_SCALE))
            .min(7.5 * limit * self.size)
            .max(3.5 * limit);
        let strip = SHELF_HEADING + tile * self.ratio * FOCUS_SCALE + TILE_GAP;
        let hero = area - strip;
        let mut feature_h = hero * 0.82;
        let mut feature_w = feature_h / self.ratio;
        let max_w = self.width * 0.38;
        if feature_w > max_w {
            feature_w = max_w;
            feature_h = feature_w * self.ratio;
        }
        Spotlight {
            tile,
            strip,
            hero,
            feature_w,
            feature_h,
        }
    }
}

// ---- cinema ------------------------------------------------------------------

/// Wide-screen cinema's aspect ratio, which the band of art takes.
pub const SCOPE: f32 = 2.39;
/// Stills in the cinema's filmstrip are 16:9.
pub const STILL_RATIO: f32 = 0.5625;

/// The cinema layout: a letterboxed band of art over a filmstrip.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Cinema {
    /// The bar above the band.
    pub bar: f32,
    pub band: f32,
    /// Width of a still at rest.
    pub still: f32,
    /// Height of the filmstrip, heading included.
    pub strip: f32,
}

impl Metrics {
    pub fn cinema(&self) -> Cinema {
        let area = self.area();
        let still = ((area * 0.3 - SHELF_HEADING - TILE_GAP) / (STILL_RATIO * FOCUS_SCALE))
            .min(15.0 * self.size)
            .max(7.0)
            .min(((self.width - self.gutter) / 2.0 - TILE_GAP) / FOCUS_SCALE);
        let strip = SHELF_HEADING + still * STILL_RATIO * FOCUS_SCALE + TILE_GAP;
        let room = area - strip;
        // Bars of at least half a rem keep the band reading as a frame.
        let band = (self.width / SCOPE).min(room - 1.0).max(0.0);
        Cinema {
            bar: ((room - band) / 2.0).max(0.0),
            band,
            still,
            strip,
        }
    }
}

// ---- wall --------------------------------------------------------------------

/// Scale of the focused cover on the wall: small, as neighbours are close.
pub const WALL_FOCUS_SCALE: f32 = 1.08;
/// Space between covers on the wall, in rem.
pub const WALL_GAP: f32 = 0.75;

/// The wall: every shelf as a section of rows of small covers.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Wall {
    pub cols: usize,
    /// Cover width at rest.
    pub tile: f32,
    pub row: f32,
    pub heading: f32,
}

impl Metrics {
    pub fn wall(&self) -> Wall {
        let target = 6.75 * shape_scale(self.ratio) * self.size;
        let room = self.width - 2.0 * self.gutter + WALL_GAP;
        let cols = ((room / (target * WALL_FOCUS_SCALE + WALL_GAP)).floor() as usize).max(3);
        let tile = (room / cols as f32 - WALL_GAP) / WALL_FOCUS_SCALE;
        Wall {
            cols,
            tile,
            row: tile * self.ratio * WALL_FOCUS_SCALE + WALL_GAP,
            heading: SHELF_HEADING,
        }
    }
}

impl Wall {
    /// Rows a section of `len` covers takes.
    pub fn rows(&self, len: usize) -> usize {
        len.div_ceil(self.cols.max(1))
    }

    /// The top of each section (its heading) and the height of them all.
    pub fn sections(&self, lens: &[usize]) -> (Vec<f32>, f32) {
        let mut tops = Vec::with_capacity(lens.len());
        let mut y = 0.0;
        for len in lens {
            tops.push(y);
            if *len > 0 {
                y += self.heading + self.rows(*len) as f32 * self.row;
            }
        }
        (tops, y)
    }

    /// The top of the row holding cover `col` of section `section`.
    pub fn row_top(&self, tops: &[f32], section: usize, col: usize) -> f32 {
        tops.get(section).copied().unwrap_or(0.0)
            + self.heading
            + (col / self.cols.max(1)) as f32 * self.row
    }

    /// How far the wall scrolls to keep the focused row about two fifths of
    /// the way down `view`, without scrolling past either end.
    pub fn scroll(&self, tops: &[f32], total: f32, focus: (usize, usize), view: f32) -> f32 {
        let (section, col) = focus;
        let top = self.row_top(tops, section, col);
        // A section's first row brings its heading into view with it.
        let top = if col < self.cols {
            top - self.heading
        } else {
            top
        };
        (top - view * 0.4).clamp(0.0, (total - view).max(0.0))
    }
}

/// Where `dir` leads from cover `col` of section `row` on a wall `cols`
/// wide, over sections of `lens` covers. Up and Down cross into the
/// neighbouring sections at the same column; Left and Right stay in the
/// section. `None` when there is nowhere to go.
pub fn wall_step(
    lens: &[usize],
    cols: usize,
    (row, col): (usize, usize),
    dir: Dir,
) -> Option<(usize, usize)> {
    let cols = cols.max(1);
    let len = *lens.get(row)?;
    if len == 0 {
        return None;
    }
    let col = col.min(len - 1);
    let column = col % cols;
    match dir {
        Dir::Left => col.checked_sub(1).map(|c| (row, c)),
        Dir::Right => (col + 1 < len).then_some((row, col + 1)),
        Dir::Up if col >= cols => Some((row, col - cols)),
        Dir::Up => {
            let prev = (0..row).rev().find(|r| lens[*r] > 0)?;
            let last_row = (lens[prev] - 1) / cols * cols;
            Some((prev, (last_row + column).min(lens[prev] - 1)))
        }
        Dir::Down if col / cols + 1 < len.div_ceil(cols) => Some((row, (col + cols).min(len - 1))),
        Dir::Down => {
            let next = (row + 1..lens.len()).find(|r| lens[*r] > 0)?;
            Some((next, column.min(lens[next] - 1)))
        }
    }
}

// ---- carousel ----------------------------------------------------------------

/// Covers drawn on each side of the focused one on the wheel.
pub const WHEEL_SIDE: usize = 5;
/// Angle between neighbours on the wheel, in radians.
const WHEEL_STEP: f32 = 0.3;
/// How much the wheel tilts away: 0 is seen edge-on, 1 from above.
const WHEEL_TILT: f32 = 0.32;

/// The carousel: a shelf on a wheel seen from slightly above, turning so
/// the focused cover comes to the front.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Wheel {
    /// The focused cover's width.
    pub tile: f32,
    pub radius: f32,
    /// Height of the title and facts above the wheel.
    pub info: f32,
    /// Height of the wheel's area.
    pub height: f32,
    /// Covers' height over their width.
    pub ratio: f32,
}

/// Where a cover sits on the wheel.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Place {
    /// The cover's centre, from the area's left edge and top.
    pub x: f32,
    pub y: f32,
    pub scale: f32,
    pub opacity: f32,
}

impl Metrics {
    pub fn wheel(&self) -> Wheel {
        let info = if self.compact { 5.0 } else { 9.0 };
        let height = self.area() - info - SHELF_HEADING;
        let limit = shape_scale(self.ratio);
        let tile = (height * 0.78 / self.ratio)
            .min(14.0 * limit * self.size)
            .min(self.width * 0.3)
            .max(4.0);
        Wheel {
            tile,
            radius: self.width * 0.42,
            info,
            height,
            ratio: self.ratio,
        }
    }
}

impl Wheel {
    /// Where the cover `d` places from the focus sits (fractions while the
    /// wheel turns) in an area `width` wide.
    pub fn place(&self, d: f32, width: f32) -> Place {
        let limit = std::f32::consts::FRAC_PI_2;
        let angle = (d * WHEEL_STEP).clamp(-limit, limit);
        let (sin, cos) = angle.sin_cos();
        let scale = 0.5 + 0.5 * cos.powi(2);
        let tile_h = self.tile * self.ratio;
        Place {
            x: width / 2.0 + self.radius * sin,
            // Covers keep their bottoms level as they shrink, and those
            // further round sit higher, on the wheel's far side.
            y: self.height / 2.0 - self.radius * (1.0 - cos) * WHEEL_TILT * 0.5
                + tile_h * (1.0 - scale) * 0.5,
            scale,
            opacity: (1.0 - (d.abs() - (WHEEL_SIDE as f32 - 1.5)).max(0.0) / 1.5).clamp(0.0, 1.0),
        }
    }
}

// ---- compact -------------------------------------------------------------------

/// Height of a row in the compact layout's list, in rem.
pub const LIST_ROW: f32 = 3.5;
/// Height of the compact layout's shelf chips.
pub const SHELF_CHIPS: f32 = 3.25;

/// The compact layout: shelf chips over a list of titles, the focused
/// game's cover beside it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Compact {
    pub list_w: f32,
    pub list_h: f32,
    pub cover_w: f32,
}

impl Metrics {
    pub fn compact_layout(&self) -> Compact {
        let list_h = self.area() - SHELF_CHIPS;
        let list_w = (self.width * 0.44).clamp(18.0, 36.0);
        let side = self.width - list_w - 3.0 * self.gutter;
        let cover_w = (side * 0.48)
            .min((list_h - 1.0) * 0.9 / self.ratio)
            .max(3.0);
        Compact {
            list_w,
            list_h,
            cover_w,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::strip_tiles;
    use crate::theme::{CoverShape, CoverSize, CoverStyle};

    fn screens() -> Vec<Metrics> {
        let mut out = Vec::new();
        for (w, h) in [(1280.0, 800.0), (1920.0, 1080.0), (3840.0, 2160.0)] {
            for scale in [1.0, 1.5, 2.0] {
                for shape in CoverShape::ALL {
                    for size in CoverSize::ALL {
                        let rem = 16.0 * scale;
                        let style = CoverStyle {
                            shape,
                            size,
                            ..Default::default()
                        };
                        out.push(Metrics::styled(w / rem, h / rem, style));
                    }
                }
            }
        }
        out
    }

    #[test]
    fn layouts_round_trip_their_ids() {
        for layout in HomeLayout::ALL {
            assert_eq!(HomeLayout::from_id(layout.id()), Some(layout));
        }
        assert_eq!(HomeLayout::from_setting(""), HomeLayout::Shelves);
        assert_eq!(HomeLayout::from_setting("tilted"), HomeLayout::Shelves);
    }

    #[test]
    fn compact_turns_the_directions() {
        assert_eq!(grid_move(HomeLayout::Shelves, Dir::Down), GridMove::Down);
        assert_eq!(grid_move(HomeLayout::Carousel, Dir::Right), GridMove::Right);
        assert_eq!(grid_move(HomeLayout::Compact, Dir::Down), GridMove::Right);
        assert_eq!(grid_move(HomeLayout::Compact, Dir::Up), GridMove::Left);
        assert_eq!(grid_move(HomeLayout::Compact, Dir::Right), GridMove::Down);
        assert_eq!(grid_move(HomeLayout::Compact, Dir::Left), GridMove::Up);
    }

    #[test]
    fn spotlight_and_cinema_fit_the_screen() {
        for m in screens() {
            let s = m.spotlight();
            assert!((s.hero + s.strip - m.area()).abs() < 0.01);
            assert!(s.feature_h <= s.hero + 0.01, "{m:?}");
            assert!(s.feature_w <= m.width * 0.38 + 0.01);
            assert!(strip_tiles(m.width, m.gutter, s.tile) >= 2, "{m:?}");

            let c = m.cinema();
            let used = 2.0 * c.bar + c.band + c.strip;
            assert!(used <= m.area() + 0.01, "{m:?}: {c:?}");
            assert!(c.band <= m.width / SCOPE + 0.01);
            assert!(strip_tiles(m.width, m.gutter, c.still) >= 2, "{m:?}");
        }
    }

    #[test]
    fn the_wall_fills_its_width() {
        for m in screens() {
            let w = m.wall();
            assert!(w.cols >= 3);
            let used = w.cols as f32 * (w.tile * WALL_FOCUS_SCALE + WALL_GAP) - WALL_GAP;
            assert!((used - (m.width - 2.0 * m.gutter)).abs() < 0.01, "{m:?}");
        }
    }

    #[test]
    fn wall_sections_stack_and_scroll_within_bounds() {
        let w = Wall {
            cols: 4,
            tile: 5.0,
            row: 8.0,
            heading: 2.0,
        };
        let (tops, total) = w.sections(&[10, 0, 3]);
        // 3 rows, then nothing, then 1 row.
        assert_eq!(tops, vec![0.0, 26.0, 26.0]);
        assert_eq!(total, 36.0);
        assert_eq!(w.row_top(&tops, 0, 9), 18.0);
        assert_eq!(w.scroll(&tops, total, (0, 0), 20.0), 0.0);
        assert_eq!(w.scroll(&tops, total, (2, 1), 20.0), 16.0);
        assert_eq!(w.scroll(&tops, total, (0, 5), 20.0), 2.0);
        assert_eq!(w.scroll(&tops, total, (0, 5), 100.0), 0.0);
    }

    #[test]
    fn wall_steps_cross_sections_at_the_same_column() {
        // Section 0: 0 1 2 3 / 4 5 6 7 / 8 9   section 1: empty
        // Section 2: 0 1 2
        let lens = [10, 0, 3];
        assert_eq!(wall_step(&lens, 4, (0, 5), Dir::Up), Some((0, 1)));
        assert_eq!(wall_step(&lens, 4, (0, 1), Dir::Up), None);
        assert_eq!(wall_step(&lens, 4, (0, 6), Dir::Down), Some((0, 9)));
        assert_eq!(wall_step(&lens, 4, (0, 9), Dir::Down), Some((2, 1)));
        assert_eq!(wall_step(&lens, 4, (0, 7), Dir::Down), Some((0, 9)));
        assert_eq!(wall_step(&lens, 4, (2, 2), Dir::Up), Some((0, 9)));
        assert_eq!(wall_step(&lens, 4, (2, 0), Dir::Up), Some((0, 8)));
        assert_eq!(wall_step(&lens, 4, (2, 2), Dir::Down), None);
        assert_eq!(wall_step(&lens, 4, (0, 3), Dir::Right), Some((0, 4)));
        assert_eq!(wall_step(&lens, 4, (2, 0), Dir::Left), None);
        assert_eq!(wall_step(&lens, 4, (2, 2), Dir::Right), None);
        assert_eq!(wall_step(&[], 4, (0, 0), Dir::Down), None);
    }

    #[test]
    fn the_wheel_turns_symmetrically_and_settles_in_front() {
        for m in screens() {
            let wheel = m.wheel();
            let front = wheel.place(0.0, m.width);
            assert!((front.x - m.width / 2.0).abs() < 0.01);
            assert_eq!(front.scale, 1.0);
            assert_eq!(front.opacity, 1.0);
            assert!(wheel.tile * m.ratio <= wheel.height + 0.01 || wheel.tile == 4.0);
            let mut last = front;
            for d in 1..=WHEEL_SIDE {
                let right = wheel.place(d as f32, m.width);
                let left = wheel.place(-(d as f32), m.width);
                assert!((right.x - m.width / 2.0 + left.x - m.width / 2.0).abs() < 0.01);
                assert!(right.x > last.x, "covers run on to the right");
                assert!(right.scale < last.scale, "and shrink");
                assert!(right.opacity <= last.opacity);
                // The cover stays on screen.
                assert!(right.x + wheel.tile * right.scale / 2.0 <= m.width + wheel.tile);
                last = right;
            }
            assert_eq!(last.opacity, 0.0, "the last cover fades out before it pops");
        }
    }

    #[test]
    fn compact_keeps_a_list_and_a_cover() {
        for m in screens() {
            let c = m.compact_layout();
            assert!(c.list_w >= 18.0);
            assert!(
                c.list_w + c.cover_w + 3.0 * m.gutter <= m.width + 0.01,
                "{m:?}"
            );
            assert!(c.cover_w * m.ratio <= c.list_h, "{m:?}");
        }
    }
}
