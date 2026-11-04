//! The library's three ways to look at games (`iroriUi.libraryLayout`): the
//! cover grid, whose click opens a drawer about the game beside it; the
//! list; and Focus, a column of covers beside the selected game's page.
//! Pure rules over rem sizes and indices, so they can be unit-tested.

use irori_core::settings::LibraryLayout;

use crate::grid;

/// The views in the order of the header's switch and of Alt+1…3.
pub const LAYOUTS: [LibraryLayout; 3] = [
    LibraryLayout::Grid,
    LibraryLayout::List,
    LibraryLayout::Focus,
];

/// The view after `layout` when cycling through them.
pub fn next_layout(layout: LibraryLayout) -> LibraryLayout {
    let ix = LAYOUTS.iter().position(|l| *l == layout).unwrap_or(0);
    LAYOUTS[(ix + 1) % LAYOUTS.len()]
}

/// Narrowest the grid's drawer gets, in rem.
pub const DRAWER_MIN_REM: f32 = 22.0;
/// Widest the grid's drawer gets, in rem.
pub const DRAWER_MAX_REM: f32 = 40.0;
/// Room the grid keeps beside the drawer, in rem: two small covers.
pub const DRAWER_GRID_MIN_REM: f32 =
    2.0 * grid::EDGE_REM + 2.0 * grid::MIN_TILE_REM + grid::GAP_REM;
/// How far one key press on the drawer's edge moves it, in rem.
pub const DRAWER_STEP_REM: f32 = 2.0;

/// The drawer's width for a `requested` width beside a library `available`
/// rem wide: within [`DRAWER_MIN_REM`] and [`DRAWER_MAX_REM`], and leaving
/// the grid [`DRAWER_GRID_MIN_REM`] while the window allows it. The minimum
/// wins in very narrow windows: a drawer narrower than that is unreadable.
pub fn drawer_width(requested: f32, available: f32) -> f32 {
    let max = DRAWER_MAX_REM
        .min(available - DRAWER_GRID_MIN_REM)
        .max(DRAWER_MIN_REM);
    if requested.is_finite() {
        requested.clamp(DRAWER_MIN_REM, max)
    } else {
        DRAWER_MIN_REM
    }
}

/// Cover width in the Focus view's column, in rem.
pub const FOCUS_TILE_REM: f32 = 7.5;
/// Most covers in a row of the Focus view's column.
const FOCUS_MAX_COLUMNS: usize = 3;
/// Room the game page keeps beside the Focus view's column, in rem: the
/// width at which the page sets its cover beside its content.
pub const FOCUS_PAGE_MIN_REM: f32 = 50.0;

/// Width of the Focus view's column with `columns` covers to a row, in rem.
pub fn focus_column_rem(columns: usize) -> f32 {
    let n = columns.max(1) as f32;
    2.0 * grid::EDGE_REM + n * FOCUS_TILE_REM + (n - 1.0) * grid::GAP_REM
}

/// Covers to a row in the Focus view's column for a library `width` rem
/// wide: as many as fit (up to three) while the page keeps
/// [`FOCUS_PAGE_MIN_REM`], and always one.
pub fn focus_columns(width: f32) -> usize {
    (1..=FOCUS_MAX_COLUMNS)
        .rev()
        .find(|n| width - focus_column_rem(*n) >= FOCUS_PAGE_MIN_REM)
        .unwrap_or(1)
}

/// What a click on a game does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Click {
    /// Open the game's page.
    Open,
    /// Select the game: the drawer (grid) or the page beside (Focus) shows it.
    Select,
    /// Pick or unpick it for bulk actions.
    Pick,
    /// Pick every game from the last pick to it.
    Extend,
}

/// What a click with `count` presses does in `layout`. Shift and Ctrl (Cmd
/// on macOS) pick games as before, and so does any click in selection mode.
/// The list opens the page; the grid and Focus select on the first click
/// and open the page on the second.
pub fn click(
    layout: LibraryLayout,
    count: usize,
    shift: bool,
    secondary: bool,
    picking: bool,
) -> Click {
    if shift {
        Click::Extend
    } else if secondary || picking {
        Click::Pick
    } else if layout == LibraryLayout::List || count >= 2 {
        Click::Open
    } else {
        Click::Select
    }
}

/// The index the view selects in a list of `len` games after it changed (a
/// filter, a search, a new view): `found`, the selected game's new place,
/// when it is still shown. Otherwise Focus (`always`) moves to the game now
/// at the selection's old place `last`, or the first, so its page never
/// stands empty; the grid's drawer closes (`None`).
pub fn keep_selection(
    found: Option<usize>,
    last: Option<usize>,
    len: usize,
    always: bool,
) -> Option<usize> {
    if len == 0 {
        return None;
    }
    match found {
        Some(ix) if ix < len => Some(ix),
        _ if always => Some(last.unwrap_or(0).min(len - 1)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layouts_cycle_in_the_switch_order() {
        assert_eq!(next_layout(LibraryLayout::Grid), LibraryLayout::List);
        assert_eq!(next_layout(LibraryLayout::List), LibraryLayout::Focus);
        assert_eq!(next_layout(LibraryLayout::Focus), LibraryLayout::Grid);
    }

    #[test]
    fn the_drawer_stays_within_its_limits() {
        // A roomy window: the request is kept within the bounds.
        assert_eq!(drawer_width(26.0, 100.0), 26.0);
        assert_eq!(drawer_width(10.0, 100.0), DRAWER_MIN_REM);
        assert_eq!(drawer_width(80.0, 100.0), DRAWER_MAX_REM);
        // A narrower window keeps room for the grid first.
        let available = DRAWER_GRID_MIN_REM + 30.0;
        assert_eq!(drawer_width(38.0, available), 30.0);
        // Too narrow for both: the drawer keeps its minimum.
        assert_eq!(drawer_width(30.0, 30.0), DRAWER_MIN_REM);
        assert_eq!(drawer_width(f32::NAN, 100.0), DRAWER_MIN_REM);
    }

    #[test]
    fn the_focus_column_gives_way_to_the_page() {
        assert_eq!(focus_column_rem(1), 10.5);
        assert_eq!(focus_column_rem(3), 3.0 + 22.5 + 2.5);
        // A wide library (a 1920 px window at 16 px rem, less the sidebar).
        assert_eq!(focus_columns(104.0), 3);
        // A 1440 px window: two covers keep the page beside them.
        assert_eq!(focus_columns(74.0), 2);
        assert_eq!(focus_columns(FOCUS_PAGE_MIN_REM + focus_column_rem(2)), 2);
        assert_eq!(focus_columns(40.0), 1);
        assert_eq!(focus_columns(0.0), 1);
    }

    #[test]
    fn clicks_select_in_the_grid_and_focus_and_open_in_the_list() {
        use LibraryLayout::*;
        assert_eq!(click(Grid, 1, false, false, false), Click::Select);
        assert_eq!(click(Grid, 2, false, false, false), Click::Open);
        assert_eq!(click(Focus, 1, false, false, false), Click::Select);
        assert_eq!(click(Focus, 2, false, false, false), Click::Open);
        assert_eq!(click(List, 1, false, false, false), Click::Open);
        // Picking for bulk actions wins in every view.
        for layout in LAYOUTS {
            assert_eq!(click(layout, 1, true, false, false), Click::Extend);
            assert_eq!(click(layout, 1, false, true, false), Click::Pick);
            assert_eq!(click(layout, 2, false, false, true), Click::Pick);
        }
    }

    #[test]
    fn the_selection_follows_the_list() {
        // Still shown: it stays, wherever it moved.
        assert_eq!(keep_selection(Some(4), Some(1), 10, false), Some(4));
        assert_eq!(keep_selection(Some(4), Some(1), 10, true), Some(4));
        // Gone: the drawer closes, Focus takes the game now in its place.
        assert_eq!(keep_selection(None, Some(3), 10, false), None);
        assert_eq!(keep_selection(None, Some(3), 10, true), Some(3));
        assert_eq!(keep_selection(None, Some(12), 10, true), Some(9));
        assert_eq!(keep_selection(None, None, 10, true), Some(0));
        // Nothing shown, nothing selected.
        assert_eq!(keep_selection(None, Some(3), 0, true), None);
        assert_eq!(keep_selection(Some(0), None, 0, true), None);
    }
}
