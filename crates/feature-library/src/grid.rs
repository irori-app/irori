//! Geometry of the cover grid and keyboard movement within it. Pure
//! functions over logical pixels and rem sizes so they can be unit-tested.

use std::ops::Range;

use irori_core::settings::{CoverCaption, CoverShape, Density, ListColumns, ListRowSize};

/// Gap between tiles, in rem.
pub const GAP_REM: f32 = 1.25;
/// Room under the cover for the title and status lines, in rem.
pub const CAPTION_REM: f32 = 2.75;
/// Room under the cover for the title alone, in rem.
pub const TITLE_ONLY_REM: f32 = 1.75;

/// Room under each cover for what `caption` writes there, in rem.
pub fn caption_rem(caption: CoverCaption) -> f32 {
    match caption {
        CoverCaption::Full => CAPTION_REM,
        CoverCaption::Title => TITLE_ONLY_REM,
        CoverCaption::None => 0.0,
    }
}
/// Space above each row so a focused tile's ring is not clipped, in rem.
pub const ROW_INSET_REM: f32 = 0.25;
/// Horizontal content inset of the scroll region, in rem.
pub const EDGE_REM: f32 = 1.5;
/// Height of one list-layout row, in rem.
pub const LIST_ROW_REM: f32 = 3.75;
/// Height of a section heading row, in rem.
pub const HEADING_REM: f32 = 2.75;
/// Cover aspect ratio (height / width).
pub const COVER_RATIO: f32 = 1.5;
/// Tile width bounds for the size slider, in rem.
pub const MIN_TILE_REM: f32 = 7.0;
pub const MAX_TILE_REM: f32 = 16.0;

/// List column widths, in rem (store, size, playtime, last played, status,
/// compat, actions).
pub const LIST_COLUMNS: [f32; 7] = [6.0, 5.5, 6.0, 7.5, 8.0, 7.5, 10.0];
/// Width of a list row's thumbnail, in rem.
pub const LIST_THUMB_REM: f32 = 2.25;
/// Gap between list cells (`gap_4`), in rem.
const LIST_GAP_REM: f32 = 1.0;
/// Narrowest the title column gets before a data column is dropped, in rem.
const LIST_TITLE_MIN_REM: f32 = 12.0;
/// Data columns in the order narrow lists drop them: size, compat, store,
/// playtime, last played. Status (if enabled) and the actions always stay.
const LIST_DROP_ORDER: [usize; 5] = [1, 5, 0, 2, 3];

/// Height of one list-layout row for `size` and `density`, in rem.
pub fn list_row_rem(size: ListRowSize, density: Density) -> f32 {
    let base = match size {
        ListRowSize::Compact => LIST_ROW_REM,
        ListRowSize::Medium => 5.0,
        ListRowSize::Large => 6.5,
    };
    base * density.row_scale()
}

/// Width of a list row's thumbnail for `size`, in rem.
pub fn list_thumb_rem(size: ListRowSize) -> f32 {
    match size {
        ListRowSize::Compact => LIST_THUMB_REM,
        ListRowSize::Medium => 3.0,
        ListRowSize::Large => 3.9,
    }
}

/// Width of a list row's thumbnail for `size` and `density`, in rem. Scales
/// down alongside compact row heights so a 2:3 cover never overflows its row.
pub fn list_thumb_rem_at(size: ListRowSize, density: Density) -> f32 {
    list_thumb_rem(size) * density.row_scale().min(1.0)
}

/// Which of [`LIST_COLUMNS`] fit in `available_rem` beside the thumbnail and a
/// readable title, keeping only columns enabled in `chosen`, sized for a list
/// row thumbnail of `row_size`.
pub fn list_columns_for(
    available_rem: f32,
    chosen: ListColumns,
    row_size: ListRowSize,
) -> [bool; 7] {
    let thumb = list_thumb_rem(row_size);
    let needed = |shown: &[bool; 7]| {
        let columns: f32 = LIST_COLUMNS
            .iter()
            .zip(shown)
            .filter(|(_, shown)| **shown)
            .map(|(w, _)| w + LIST_GAP_REM)
            .sum();
        thumb + LIST_GAP_REM + LIST_TITLE_MIN_REM + columns
    };
    let mut shown = [
        chosen.store,
        chosen.size,
        chosen.playtime,
        chosen.last_played,
        chosen.status,
        chosen.compat,
        true,
    ];
    for column in LIST_DROP_ORDER {
        if needed(&shown) <= available_rem {
            break;
        }
        shown[column] = false;
    }
    shown
}

/// Resolved layout of the grid for one viewport width.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GridMetrics {
    pub columns: usize,
    /// Tile width in rem: the preferred size stretched so tiles fill the row.
    pub tile_rem: f32,
    /// Height of one grid row in rem (inset, cover, caption and gap).
    pub row_rem: f32,
    /// Gap between tiles, in rem.
    pub gap_rem: f32,
    /// Height of one list-layout row, in rem.
    pub list_row_rem: f32,
}

/// Fit tiles of at least `preferred_rem` into `available_rem` with `GAP_REM`
/// between them, then widen them to fill the row exactly. Tiles never grow
/// past one and a half times the preferred width (very narrow windows with a
/// single column).
pub fn grid_metrics(available_rem: f32, preferred_rem: f32) -> GridMetrics {
    grid_metrics_at(available_rem, preferred_rem, Density::Comfortable)
}

/// [`grid_metrics`] at a density: covers, gaps and list rows scaled by it.
pub fn grid_metrics_at(available_rem: f32, preferred_rem: f32, density: Density) -> GridMetrics {
    grid_metrics_for(available_rem, preferred_rem, density, CoverCaption::Full)
}

/// [`grid_metrics_at`] with `caption` under each cover.
pub fn grid_metrics_for(
    available_rem: f32,
    preferred_rem: f32,
    density: Density,
    caption: CoverCaption,
) -> GridMetrics {
    grid_metrics_shaped(
        available_rem,
        preferred_rem,
        density,
        caption,
        CoverShape::Portrait,
    )
}

/// [`grid_metrics_for`] with covers of `shape` (`coverShape`): the cover
/// size `preferred_rem` is a box's width, which squares and banners widen
/// by [`CoverShape::width_scale`], and rows are as tall as the shape.
pub fn grid_metrics_shaped(
    available_rem: f32,
    preferred_rem: f32,
    density: Density,
    caption: CoverCaption,
    shape: CoverShape,
) -> GridMetrics {
    let preferred = preferred_rem.clamp(MIN_TILE_REM, MAX_TILE_REM)
        * density.cover_scale()
        * shape.width_scale();
    let gap = GAP_REM * density.spacing_scale();
    let available = available_rem.max(preferred);
    let columns = (((available + gap) / (preferred + gap)).floor() as usize).max(1);
    let fill = (available - gap * (columns as f32 - 1.0)) / columns as f32;
    let tile_rem = fill.min(preferred * 1.5).max(preferred.min(available));
    GridMetrics {
        columns,
        tile_rem,
        row_rem: ROW_INSET_REM + tile_rem * shape.ratio() + caption_rem(caption) + gap,
        gap_rem: gap,
        list_row_rem: LIST_ROW_REM * density.row_scale(),
    }
}

/// The Focus view's column: covers exactly `tile_rem` wide (its own sizes,
/// not the grid slider's range) at comfortable spacing, as many to a row as
/// `available_rem` holds.
pub fn focus_metrics(available_rem: f32, tile_rem: f32, caption: CoverCaption) -> GridMetrics {
    let gap = GAP_REM;
    let columns = (((available_rem + gap) / (tile_rem + gap) + 1e-4).floor() as usize).max(1);
    GridMetrics {
        columns,
        tile_rem,
        row_rem: ROW_INSET_REM + tile_rem * COVER_RATIO + caption_rem(caption) + gap,
        gap_rem: gap,
        list_row_rem: LIST_ROW_REM,
    }
}

/// Height over width from which a picture counts as a tall box: boxes of a
/// row share one height. Squarer art (handheld and disc boxes) and wide art
/// keep their own.
pub const TALL_RATIO: f32 = 1.2;

/// Width and height in rem of each cover drawn whole in a row of cells
/// `tile_rem` wide and [`COVER_RATIO`] tall, from each picture's height over
/// width (`None` while unknown: the whole cell). Each picture keeps its shape
/// inside its cell; the tall ones share the lowest of their heights, so a row
/// of boxes stands even instead of stepping by a few pixels.
pub fn whole_sizes(aspects: &[Option<f32>], tile_rem: f32) -> Vec<(f32, f32)> {
    let cell_h = tile_rem * COVER_RATIO;
    let natural = |ratio: f32| {
        if ratio > COVER_RATIO {
            (cell_h / ratio, cell_h)
        } else {
            (tile_rem, tile_rem * ratio)
        }
    };
    let ratios: Vec<Option<f32>> = aspects
        .iter()
        .map(|a| a.filter(|r| r.is_finite() && *r > 0.0))
        .collect();
    let shared = ratios
        .iter()
        .flatten()
        .filter(|r| **r >= TALL_RATIO)
        .map(|r| natural(*r).1)
        .fold(None, |low: Option<f32>, h| {
            Some(low.map_or(h, |l| l.min(h)))
        });
    ratios
        .iter()
        .map(|ratio| match (ratio, shared) {
            (None, _) => (tile_rem, cell_h),
            (Some(r), Some(h)) if *r >= TALL_RATIO => (h / r, h),
            (Some(r), _) => natural(*r),
        })
        .collect()
}

/// Screens of covers the library keeps decoded: the one shown plus one above
/// and one below, so scrolling back a little never reloads art.
const SCREENS_CACHED: usize = 3;
/// Fewest decoded images the library keeps, for tiny windows.
const MIN_CACHED_IMAGES: usize = 24;

/// Decoded images the library's image cache should hold for a viewport
/// `viewport_rem` tall showing rows `row_rem` tall of `per_row` images, plus
/// `extra` images outside the rows (the top section). Each cover costs its
/// pixels twice, in memory and in the GPU atlas, so this follows what fits on
/// screen instead of a fixed number.
pub fn cached_images(per_row: usize, row_rem: f32, viewport_rem: f32, extra: usize) -> usize {
    let rows = (viewport_rem / row_rem.max(1.0)).ceil() as usize + 1;
    (per_row.max(1) * rows * SCREENS_CACHED + extra).max(MIN_CACHED_IMAGES)
}

/// Rows needed for `count` tiles.
pub fn row_count(count: usize, columns: usize) -> usize {
    count.div_ceil(columns.max(1))
}

/// A keyboard movement in the grid.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Move {
    Left,
    Right,
    Up,
    Down,
    /// Start of the library.
    First,
    /// End of the library.
    Last,
    /// One page of `rows` rows up.
    PageUp(usize),
    /// One page of `rows` rows down.
    PageDown(usize),
}

/// Index focused after a movement in a list split into `sections`
/// (consecutive ranges, each starting on a new row; `[0..len]` for one
/// list). With nothing focused yet, any movement focuses the first game.
/// Up/Down keep the column, cross into the next or previous section, and land
/// on a row's last game when it is shorter; Left/Right run through the
/// sections as one list.
pub fn move_in_sections(
    current: Option<usize>,
    sections: &[Range<usize>],
    columns: usize,
    step: Move,
) -> Option<usize> {
    let len = sections.last()?.end;
    if len == 0 {
        return None;
    }
    let columns = columns.max(1);
    let last = len - 1;
    let Some(ix) = current.map(|i| i.min(last)) else {
        return Some(if step == Move::Last { last } else { 0 });
    };
    Some(match step {
        Move::Left => ix.saturating_sub(1),
        Move::Right => (ix + 1).min(last),
        Move::Up => row_step(ix, sections, columns, false),
        Move::Down => row_step(ix, sections, columns, true),
        Move::First => 0,
        Move::Last => last,
        Move::PageUp(rows) => {
            (0..rows.max(1)).fold(ix, |ix, _| row_step(ix, sections, columns, false))
        }
        Move::PageDown(rows) => {
            (0..rows.max(1)).fold(ix, |ix, _| row_step(ix, sections, columns, true))
        }
    })
}

/// The game one row below (or above) `ix` in the same column, clamped to a
/// shorter row; `ix` itself at the end of the list.
fn row_step(ix: usize, sections: &[Range<usize>], columns: usize, down: bool) -> usize {
    let Some(s) = sections.iter().position(|r| r.contains(&ix)) else {
        return ix;
    };
    let section = &sections[s];
    let row_start = |range: &Range<usize>, row: usize| range.start + row * columns;
    let row = (ix - section.start) / columns;
    let column = (ix - section.start) % columns;
    let target =
        |range: &Range<usize>, row: usize| (row_start(range, row) + column).min(range.end - 1);
    if down {
        if row + 1 < row_count(section.len(), columns) {
            target(section, row + 1)
        } else {
            sections[s + 1..]
                .iter()
                .find(|r| !r.is_empty())
                .map_or(ix, |next| target(next, 0))
        }
    } else if row > 0 {
        target(section, row - 1)
    } else {
        sections[..s]
            .iter()
            .rev()
            .find(|r| !r.is_empty())
            .map_or(ix, |prev| target(prev, row_count(prev.len(), columns) - 1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cached_images_cover_three_screens() {
        // 7 columns of 21.3rem rows in a 48.75rem viewport: 4 partial rows on screen.
        assert_eq!(cached_images(7, 21.3, 48.75, 0), 84);
        // The smallest tiles filling a 4K screen at 1x still keep three screens.
        let m = grid_metrics(1920.0 / 16.0 * 2.0 - 3.0, MIN_TILE_REM);
        let on_screen = m.columns * (2160.0 / 16.0 / m.row_rem).ceil() as usize;
        assert!(cached_images(m.columns, m.row_rem, 2160.0 / 16.0, 0) >= 3 * on_screen);
        // Top-section art comes on top, and tiny windows keep a floor.
        assert_eq!(cached_images(7, 21.3, 48.75, 10), 94);
        assert_eq!(cached_images(1, 3.75, 1.0, 0), MIN_CACHED_IMAGES);
    }

    #[test]
    fn rows_leave_room_for_the_caption_shown() {
        let full = grid_metrics_for(60.0, 11.0, Density::Comfortable, CoverCaption::Full);
        let title = grid_metrics_for(60.0, 11.0, Density::Comfortable, CoverCaption::Title);
        let none = grid_metrics_for(60.0, 11.0, Density::Comfortable, CoverCaption::None);
        assert_eq!(full, grid_metrics(60.0, 11.0));
        assert!((full.row_rem - title.row_rem - (CAPTION_REM - TITLE_ONLY_REM)).abs() < 1e-4);
        assert!((full.row_rem - none.row_rem - CAPTION_REM).abs() < 1e-4);
        assert_eq!((full.columns, full.tile_rem), (none.columns, none.tile_rem));
    }

    #[test]
    fn rows_follow_the_cover_shape() {
        let at = |shape| {
            grid_metrics_shaped(60.0, 11.0, Density::Comfortable, CoverCaption::None, shape)
        };
        let (boxes, squares, banners) = (
            at(CoverShape::Portrait),
            at(CoverShape::Square),
            at(CoverShape::Banner),
        );
        assert_eq!(
            boxes,
            grid_metrics_for(60.0, 11.0, Density::Comfortable, CoverCaption::None)
        );
        // Wider covers, fewer to a row, and rows as tall as the shape.
        assert!(squares.columns <= boxes.columns && banners.columns < boxes.columns);
        assert!(banners.tile_rem > boxes.tile_rem);
        for m in [boxes, squares, banners] {
            let cover = m.row_rem - ROW_INSET_REM - m.gap_rem;
            assert!(cover > 0.0);
        }
        let cover = |m: GridMetrics| m.row_rem - ROW_INSET_REM - m.gap_rem;
        assert!((cover(squares) - squares.tile_rem).abs() < 1e-4);
        assert!((cover(banners) - banners.tile_rem * 9.0 / 16.0).abs() < 1e-4);
    }

    #[test]
    fn focus_covers_keep_their_own_size() {
        // Smaller than the grid slider allows, and exactly as wide as asked.
        for tile in [6.0, 7.5, 9.5] {
            let two = 2.0 * tile + GAP_REM;
            let m = focus_metrics(two, tile, CoverCaption::Full);
            assert_eq!((m.columns, m.tile_rem), (2, tile));
            assert_eq!(
                focus_metrics(two - 0.5, tile, CoverCaption::Full).columns,
                1
            );
        }
        // The medium size lays out as the column always did.
        let old = grid_metrics_for(16.25, 7.5, Density::Comfortable, CoverCaption::Title);
        assert_eq!(focus_metrics(16.25, 7.5, CoverCaption::Title), old);
    }

    #[test]
    fn metrics_fill_the_row() {
        // 58rem fits 4 tiles of 11rem with 1.25rem gaps, not 5 (5*11 + 4*1.25 = 60).
        let compact = grid_metrics_at(58.0, 11.0, Density::Compact);
        let spacious = grid_metrics_at(58.0, 11.0, Density::Spacious);
        assert!(compact.columns > grid_metrics(58.0, 11.0).columns);
        assert!(spacious.columns <= grid_metrics(58.0, 11.0).columns);
        assert!(compact.gap_rem < GAP_REM && spacious.gap_rem > GAP_REM);
        assert!(compact.list_row_rem < LIST_ROW_REM && spacious.list_row_rem > LIST_ROW_REM);
        let m = grid_metrics(58.0, 11.0);
        assert_eq!(m.columns, 4);
        let used = m.tile_rem * 4.0 + GAP_REM * 3.0;
        assert!((used - 58.0).abs() < 0.01, "{used}");
        assert!(m.tile_rem >= 11.0);
        assert!(
            (m.row_rem - (ROW_INSET_REM + m.tile_rem * 1.5 + CAPTION_REM + GAP_REM)).abs() < 1e-4
        );

        // Narrower than one tile: one column, tile keeps the preferred width.
        let m = grid_metrics(5.0, 11.0);
        assert_eq!(m.columns, 1);
        assert_eq!(m.tile_rem, 11.0);

        // Preferred width is clamped to the slider range.
        let m = grid_metrics(100.0, 2.0);
        assert!(m.tile_rem >= MIN_TILE_REM);
    }

    #[test]
    fn whole_covers_keep_their_shape_and_tall_boxes_share_a_height() {
        let close = |a: (f32, f32), b: (f32, f32)| {
            assert!(
                (a.0 - b.0).abs() < 1e-4 && (a.1 - b.1).abs() < 1e-4,
                "{a:?} {b:?}"
            );
        };
        // A 2:3 box fills its cell; unknown art keeps the whole cell.
        let sizes = whole_sizes(&[Some(1.5), None], 10.0);
        close(sizes[0], (10.0, 15.0));
        close(sizes[1], (10.0, 15.0));
        // A DVD box (1.4) and a 2:3 one stand at the DVD's height.
        let sizes = whole_sizes(&[Some(1.5), Some(1.4)], 10.0);
        close(sizes[0], (14.0 / 1.5, 14.0));
        close(sizes[1], (10.0, 14.0));
        // Square and wide art keep their own height, full width.
        let sizes = whole_sizes(&[Some(1.0), Some(0.5), Some(1.4)], 10.0);
        close(sizes[0], (10.0, 10.0));
        close(sizes[1], (10.0, 5.0));
        close(sizes[2], (10.0, 14.0));
        // Taller than the cell: as high as it, narrower.
        let sizes = whole_sizes(&[Some(2.0)], 10.0);
        close(sizes[0], (7.5, 15.0));
        // Nothing grows past its cell.
        for (w, h) in whole_sizes(&[Some(1.3), Some(1.7), Some(0.2), Some(f32::NAN)], 8.0) {
            assert!(w <= 8.0 + 1e-4 && h <= 12.0 + 1e-4);
        }
    }

    #[test]
    fn rows() {
        assert_eq!(row_count(0, 4), 0);
        assert_eq!(row_count(4, 4), 1);
        assert_eq!(row_count(5, 4), 2);
        assert_eq!(row_count(5, 0), 5);
    }

    #[test]
    fn keyboard_movement() {
        // 10 games in 4 columns:
        //  0 1 2 3
        //  4 5 6 7
        //  8 9
        let whole = 0..10;
        let mv = |c, s| move_in_sections(c, std::slice::from_ref(&whole), 4, s);
        assert_eq!(mv(None, Move::Down), Some(0));
        assert_eq!(mv(None, Move::Last), Some(9));
        assert_eq!(mv(Some(0), Move::Left), Some(0));
        assert_eq!(mv(Some(3), Move::Right), Some(4));
        assert_eq!(mv(Some(9), Move::Right), Some(9));
        assert_eq!(mv(Some(5), Move::Up), Some(1));
        assert_eq!(mv(Some(1), Move::Up), Some(1));
        assert_eq!(mv(Some(5), Move::Down), Some(9));
        // From the second row's end, down lands on the last game of the partial row.
        assert_eq!(mv(Some(7), Move::Down), Some(9));
        assert_eq!(mv(Some(9), Move::Down), Some(9));
        assert_eq!(mv(Some(6), Move::First), Some(0));
        assert_eq!(mv(Some(6), Move::PageDown(5)), Some(9));
        assert_eq!(mv(Some(6), Move::PageUp(1)), Some(2));
        let empty = 0..0;
        assert_eq!(
            move_in_sections(Some(3), std::slice::from_ref(&empty), 4, Move::Down),
            None
        );
        // A stale index beyond the list is clamped first.
        assert_eq!(mv(Some(42), Move::Left), Some(8));
    }

    #[test]
    fn keyboard_movement_across_sections() {
        // Three sections in 3 columns:
        //  0 1 2      (Playing)
        //  3
        //  4 5        (Plan to play)
        //  6 7 8      (Not played)
        //  9 10 11
        let sections = [0..4, 4..6, 6..12];
        let mv = |c, s| move_in_sections(Some(c), &sections, 3, s);
        // Down keeps the column, landing on a shorter row's last game.
        assert_eq!(mv(1, Move::Down), Some(3));
        assert_eq!(mv(2, Move::Down), Some(3));
        // From a section's last row into the next section's first row.
        assert_eq!(mv(3, Move::Down), Some(4));
        assert_eq!(mv(2, Move::Up), Some(2));
        assert_eq!(mv(5, Move::Down), Some(7));
        assert_eq!(mv(11, Move::Down), Some(11));
        // Up into the previous section's last row, clamped to its length.
        assert_eq!(mv(8, Move::Up), Some(5));
        assert_eq!(mv(5, Move::Up), Some(3));
        assert_eq!(mv(4, Move::Up), Some(3));
        assert_eq!(mv(10, Move::Up), Some(7));
        // Left and Right run through the sections.
        assert_eq!(mv(3, Move::Right), Some(4));
        assert_eq!(mv(6, Move::Left), Some(5));
        // Pages count rows across sections.
        assert_eq!(mv(0, Move::PageDown(3)), Some(6));
        assert_eq!(mv(9, Move::PageUp(2)), Some(4));
        assert_eq!(mv(9, Move::PageUp(10)), Some(0));
        assert_eq!(mv(4, Move::Last), Some(11));
        assert_eq!(move_in_sections(None, &sections, 3, Move::Down), Some(0));
        assert_eq!(move_in_sections(Some(1), &[], 3, Move::Down), None);
        // One column (the list layout) is a plain list.
        assert_eq!(move_in_sections(Some(3), &sections, 1, Move::Down), Some(4));
        assert_eq!(move_in_sections(Some(4), &sections, 1, Move::Up), Some(3));
    }

    #[test]
    fn narrow_lists_drop_columns_in_order() {
        let cols = |w, chosen| list_columns_for(w, chosen, ListRowSize::Medium);
        let all = ListColumns {
            store: true,
            size: true,
            playtime: true,
            last_played: true,
            status: true,
            compat: true,
        };
        assert_eq!(cols(90.0, all), [true; 7]);
        assert_eq!(cols(69.0, all), [true, false, true, true, true, true, true]);
        assert_eq!(
            cols(60.0, all),
            [true, false, true, true, true, false, true]
        );
        assert_eq!(
            cols(52.0, all),
            [false, false, true, true, true, false, true]
        );
        assert_eq!(
            cols(50.0, all),
            [false, false, false, true, true, false, true]
        );
        assert_eq!(
            cols(20.0, all),
            [false, false, false, false, true, false, true]
        );
        // Respects disabled columns even on wide windows, including status.
        let default = ListColumns::default();
        assert_eq!(
            cols(90.0, default),
            [
                true,
                false,
                true,
                true,
                true,
                cfg!(target_os = "linux"),
                true
            ]
        );
        let minimal = ListColumns {
            store: false,
            size: true,
            playtime: false,
            last_played: false,
            status: false,
            compat: true,
        };
        assert_eq!(
            cols(90.0, minimal),
            [false, true, false, false, false, true, true]
        );
    }

    #[test]
    fn list_row_size_scales_row_height_and_thumbnail() {
        let compact_row = list_row_rem(ListRowSize::Compact, Density::Comfortable);
        let medium_row = list_row_rem(ListRowSize::Medium, Density::Comfortable);
        let large_row = list_row_rem(ListRowSize::Large, Density::Comfortable);
        assert!((compact_row - 3.75).abs() < 1e-4);
        assert!((medium_row - 5.0).abs() < 1e-4);
        assert!((large_row - 6.5).abs() < 1e-4);
        assert!(
            list_row_rem(ListRowSize::Large, Density::Compact)
                < list_row_rem(ListRowSize::Large, Density::Comfortable)
        );
        assert!((list_thumb_rem(ListRowSize::Compact) - 2.25).abs() < 1e-4);
        assert!((list_thumb_rem(ListRowSize::Medium) - 3.0).abs() < 1e-4);
        assert!((list_thumb_rem(ListRowSize::Large) - 3.9).abs() < 1e-4);
        // Every (ListRowSize, Density) combination leaves vertical room for
        // the 2:3 thumbnail so covers never clip outside their row.
        for size in [
            ListRowSize::Compact,
            ListRowSize::Medium,
            ListRowSize::Large,
        ] {
            for density in [Density::Compact, Density::Comfortable, Density::Spacious] {
                let row = list_row_rem(size, density);
                let thumb_h = list_thumb_rem_at(size, density) * COVER_RATIO;
                assert!(
                    thumb_h + 0.25 <= row,
                    "thumb height {thumb_h} overflows row height {row} for ({size:?}, {density:?})"
                );
            }
        }
    }
}
