//! Big Picture's geometry: the sizes of its screens worked out from the
//! window's size in rems (so the 200 % interface size on a Steam Deck still
//! fits a whole shelf), and which part of a shelf or the grid shows for the
//! focused game.
//!
//! Shelves and the grid scroll by animating one offset toward the focused
//! item instead of through a scroll area, so they glide, and only the items
//! near the view are built (they may hold thousands of games).
//!
//! Home's other layouts have their geometry in [`home`].

pub mod home;

use std::ops::Range;

pub use home::HomeLayout;

use crate::theme::CoverStyle;

/// Aspect ratio (height / width) of box art, the default cover shape.
pub const TILE_RATIO: f32 = 1.5;
/// Scale of the focused cover on a shelf.
pub const FOCUS_SCALE: f32 = 1.1;
/// Scale of the focused cover in the grid, smaller as neighbours are close.
pub const GRID_FOCUS_SCALE: f32 = 1.06;
/// Space between covers, in rem.
pub const TILE_GAP: f32 = 1.25;
/// Height of a shelf's heading, in rem.
pub const SHELF_HEADING: f32 = 2.25;
/// Height of the Library tab's chips and of its title line, in rem.
pub const CHIPS: f32 = 3.5;
pub const GRID_HEADER: f32 = 3.25;

const MAX_TILE: f32 = 11.5;
const MIN_TILE: f32 = 5.0;
const GRID_TILE: f32 = 9.5;
/// Below this height in rem, screens drop the description and shrink the
/// title (a Steam Deck at 175 % and above).
const COMPACT_BELOW: f32 = 40.0;

/// Sizes in rem for a window of `width` × `height` rem.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Metrics {
    pub width: f32,
    pub height: f32,
    /// Covers' height over their width.
    pub ratio: f32,
    /// How much larger or smaller than the layout's choice covers are.
    pub size: f32,
    /// Short screens leave out descriptions and use smaller headings.
    pub compact: bool,
    /// Space at the left and right of every screen.
    pub gutter: f32,
    pub top_bar: f32,
    pub hints: f32,
    /// Home's title, facts and description above the shelves.
    pub hero: f32,
    /// Cover width on Home's shelves, at rest.
    pub tile: f32,
    /// Cover width in the Library grid, at rest.
    pub grid_tile: f32,
    pub grid_cols: usize,
}

/// How far a cover of `ratio` may grow or shrink next to box art's limits:
/// wider covers get wider limits, so all shapes cover about the same area.
pub fn shape_scale(ratio: f32) -> f32 {
    (TILE_RATIO / ratio.max(0.1)).sqrt()
}

/// Covers `tile` wide (at rest) that fit whole on a strip across `width`,
/// starting at `gutter`.
pub fn strip_tiles(width: f32, gutter: f32, tile: f32) -> usize {
    (((width - gutter) / (tile * FOCUS_SCALE + TILE_GAP)).floor() as usize).max(1)
}

impl Metrics {
    /// Sizes for box-art covers of the default size.
    #[cfg(test)]
    pub fn new(width: f32, height: f32) -> Self {
        Self::styled(width, height, CoverStyle::default())
    }

    /// Sizes for covers of `style`.
    pub fn styled(width: f32, height: f32, style: CoverStyle) -> Self {
        let ratio = style.shape.ratio();
        let size = style.size.factor();
        let limit = shape_scale(ratio);
        let compact = height < COMPACT_BELOW;
        let gutter = if width < 60.0 { 2.0 } else { 3.0 };
        let top_bar = if compact { 3.75 } else { 5.0 };
        let hints = if compact { 3.25 } else { 4.25 };
        let hero = if compact { 5.5 } else { 13.0 };
        // One whole shelf fits under the hero.
        let shelves = height - top_bar - hints - hero;
        let fit = (shelves - SHELF_HEADING - TILE_GAP) / (ratio * FOCUS_SCALE);
        // At least two covers fit a shelf, whatever their shape.
        let two = ((width - gutter) / 2.0 - TILE_GAP) / FOCUS_SCALE;
        let tile = fit
            .min(MAX_TILE * limit * size)
            .max(MIN_TILE * limit * size.min(1.0))
            .min(two);
        // One whole grid row fits under the chips and title.
        let grid = height - top_bar - hints - CHIPS - GRID_HEADER;
        let grid_tile = ((grid - TILE_GAP) / (ratio * GRID_FOCUS_SCALE))
            .min(GRID_TILE * limit * size)
            .max((MIN_TILE - 0.5) * limit * size.min(1.0));
        let room = width - 2.0 * gutter + TILE_GAP;
        let grid_cols =
            ((room / (grid_tile * GRID_FOCUS_SCALE + TILE_GAP)).floor() as usize).max(2);
        // Spread the columns over the whole width.
        let grid_tile =
            ((room / grid_cols as f32 - TILE_GAP) / GRID_FOCUS_SCALE).min(grid_tile * 1.15);
        Self {
            width,
            height,
            ratio,
            size,
            compact,
            gutter,
            top_bar,
            hints,
            hero,
            tile,
            grid_tile,
            grid_cols,
        }
    }

    /// Height of a shelf: heading and covers.
    pub fn shelf(&self) -> f32 {
        SHELF_HEADING + self.tile * self.ratio * FOCUS_SCALE + TILE_GAP
    }

    /// Height of Home's shelf area.
    pub fn shelves_height(&self) -> f32 {
        self.height - self.top_bar - self.hints - self.hero
    }

    /// Shelves that fit whole.
    pub fn shelves_shown(&self) -> usize {
        ((self.shelves_height() / self.shelf()).floor() as usize).max(1)
    }

    /// Width of one grid column, room for the focused scale included.
    pub fn grid_slot(&self) -> f32 {
        self.grid_tile * GRID_FOCUS_SCALE + TILE_GAP
    }

    /// Height of one grid row.
    pub fn grid_row(&self) -> f32 {
        self.grid_tile * self.ratio * GRID_FOCUS_SCALE + TILE_GAP
    }

    /// Height of the area between the top bar and the hints.
    pub fn area(&self) -> f32 {
        self.height - self.top_bar - self.hints
    }

    /// Height of the grid area.
    pub fn grid_height(&self) -> f32 {
        self.height - self.top_bar - self.hints - CHIPS - GRID_HEADER
    }

    /// Grid rows that fit whole.
    pub fn grid_rows_shown(&self) -> usize {
        ((self.grid_height() / self.grid_row()).floor() as usize).max(1)
    }
}

/// The first item shown when `focus` is focused in a strip of `len` items of
/// which `shown` fit: the focused item leads, until the strip's end comes
/// into view.
pub fn strip_start(focus: usize, len: usize, shown: usize) -> usize {
    focus.min(len.saturating_sub(shown.max(1)))
}

/// Items to build around `start`: those in view plus `margin` on each side,
/// which slide in and out while the strip moves.
pub fn strip_range(start: usize, len: usize, shown: usize, margin: usize) -> Range<usize> {
    let from = start.saturating_sub(margin).min(len);
    let to = (start + shown + margin + 1).min(len);
    from..to
}

/// How far a list of rows `row_h` tall scrolls so the focused row shows in
/// `height`: not at all until the focus would pass the bottom, then just far
/// enough, a row at a time. A caption `caption_h` tall above row
/// `caption_before` comes into view with that row.
pub fn list_scroll(
    focus: usize,
    caption_before: Option<usize>,
    row_h: f32,
    caption_h: f32,
    height: f32,
) -> f32 {
    let caption = |ix: usize| caption_before.is_some_and(|c| c < ix);
    let top = |ix: usize| ix as f32 * row_h + if caption(ix) { caption_h } else { 0.0 };
    let bottom = top(focus)
        + row_h
        + if caption_before == Some(focus) {
            caption_h
        } else {
            0.0
        };
    (0..=focus)
        .map(top)
        .find(|t| bottom - t <= height + 0.01)
        .unwrap_or_else(|| top(focus))
}

/// A move in the grid.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GridStep {
    Up,
    Down,
    Left,
    Right,
}

/// Where `step` leads from item `ix` in a grid of `len` items, `cols` wide.
/// `None` when it leaves the grid upward (to the chips) or there is nowhere
/// to go. Down onto a shorter last row lands on its last item; Left and
/// Right run on into the neighbouring rows.
pub fn grid_step(ix: usize, len: usize, cols: usize, step: GridStep) -> Option<usize> {
    let cols = cols.max(1);
    if len == 0 {
        return None;
    }
    let ix = ix.min(len - 1);
    match step {
        GridStep::Up => ix.checked_sub(cols),
        GridStep::Down => (ix / cols + 1 < len.div_ceil(cols)).then(|| (ix + cols).min(len - 1)),
        GridStep::Left => ix.checked_sub(1),
        GridStep::Right => (ix + 1 < len).then_some(ix + 1),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_whole_shelf_fits_at_every_interface_size() {
        // 1280 × 800 (Steam Deck) and 1920 × 1080 at 100 %…200 % of 16 px.
        for (w, h) in [(1280.0, 800.0), (1920.0, 1080.0), (3840.0, 2160.0)] {
            for scale in [1.0, 1.25, 1.5, 1.75, 2.0] {
                let rem = 16.0 * scale;
                let m = Metrics::new(w / rem, h / rem);
                let used = m.top_bar + m.hero + m.shelf() + m.hints;
                assert!(
                    used <= m.height + 0.01 || m.tile == MIN_TILE,
                    "{w}×{h} at {scale}: {used} of {}",
                    m.height
                );
                assert!(
                    strip_tiles(m.width, m.gutter, m.tile) >= 2,
                    "{w}×{h} at {scale}"
                );
                assert!(m.grid_cols >= 2);
                let grid_used = (m.grid_cols as f32) * m.grid_slot() - TILE_GAP;
                assert!(grid_used <= m.width - 2.0 * m.gutter + 0.01);
            }
        }
        let deck = Metrics::new(1280.0 / 32.0, 800.0 / 32.0);
        assert!(deck.compact);
        assert!(!Metrics::new(80.0, 50.0).compact);
    }

    #[test]
    fn every_cover_style_keeps_a_whole_shelf_and_grid_row() {
        use crate::theme::{CoverShape, CoverSize};
        for shape in CoverShape::ALL {
            for size in CoverSize::ALL {
                let style = CoverStyle {
                    shape,
                    size,
                    ..Default::default()
                };
                for (w, h) in [(80.0, 50.0), (120.0, 67.5), (40.0, 25.0)] {
                    let m = Metrics::styled(w, h, style);
                    let used = m.top_bar + m.hero + m.shelf() + m.hints;
                    assert!(used <= h + 0.01 || m.tile <= MIN_TILE * shape_scale(m.ratio));
                    assert!(m.grid_cols >= 2);
                    assert!(
                        strip_tiles(m.width, m.gutter, m.tile) >= 2,
                        "{shape:?} {size:?} {w}×{h}"
                    );
                }
            }
        }
        let normal = Metrics::new(120.0, 67.5);
        let small = Metrics::styled(
            120.0,
            67.5,
            CoverStyle {
                size: CoverSize::Small,
                ..Default::default()
            },
        );
        assert!(small.tile < normal.tile);
    }

    #[test]
    fn strips_lead_with_the_focus_until_their_end() {
        assert_eq!(strip_start(0, 20, 6), 0);
        assert_eq!(strip_start(5, 20, 6), 5);
        assert_eq!(strip_start(17, 20, 6), 14);
        assert_eq!(strip_start(3, 4, 6), 0);
        assert_eq!(strip_range(5, 20, 6, 2), 3..14);
        assert_eq!(strip_range(0, 3, 6, 2), 0..3);
        assert_eq!(strip_range(0, 0, 6, 2), 0..0);
    }

    #[test]
    fn lists_scroll_only_to_keep_the_focus_in_view() {
        assert_eq!(list_scroll(0, Some(4), 2.75, 2.25, 8.0), 0.0);
        assert_eq!(list_scroll(1, Some(4), 2.75, 2.25, 8.0), 0.0);
        // Rows 1 and 2 show: 2.75…8.25.
        assert_eq!(list_scroll(2, Some(4), 2.75, 2.25, 8.0), 2.75);
        // Row 3, the caption and row 4 show: 8.25…16.
        assert_eq!(list_scroll(4, Some(4), 2.75, 2.25, 8.0), 8.25);
        // The caption, row 4 and row 5 show: 11…18.75.
        assert_eq!(list_scroll(5, Some(4), 2.75, 2.25, 8.0), 11.0);
        // Taller than the space: the row's top leads.
        assert_eq!(list_scroll(3, None, 10.0, 0.0, 8.0), 30.0);
    }

    #[test]
    fn grid_steps() {
        // 0 1 2 3
        // 4 5 6 7
        // 8 9
        assert_eq!(grid_step(1, 10, 4, GridStep::Up), None);
        assert_eq!(grid_step(5, 10, 4, GridStep::Up), Some(1));
        assert_eq!(grid_step(6, 10, 4, GridStep::Down), Some(9));
        assert_eq!(grid_step(9, 10, 4, GridStep::Down), None);
        assert_eq!(grid_step(3, 10, 4, GridStep::Right), Some(4));
        assert_eq!(grid_step(4, 10, 4, GridStep::Left), Some(3));
        assert_eq!(grid_step(0, 10, 4, GridStep::Left), None);
        assert_eq!(grid_step(9, 10, 4, GridStep::Right), None);
        assert_eq!(grid_step(0, 0, 4, GridStep::Down), None);
    }
}
