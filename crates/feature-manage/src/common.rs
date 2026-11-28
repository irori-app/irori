//! Small helpers shared by the management screens.

use std::cell::Cell;
use std::rc::Rc;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{ActiveTheme as _, Sizable as _, WindowExt as _, v_flex};
use gpui_kit::{
    AnyElement, AnyView, App, Div, ElementId, InteractiveElement as _, IntoElement as _,
    ParentElement as _, Pixels, SharedString, StatefulInteractiveElement as _, Styled as _, Window,
    canvas, div, px, rems,
};
use irori_services::Backend;
use irori_ui::backend;

/// Gap between cards in a [`card_grid`] (`gap_4`), in rem.
const CARD_GAP_REM: f32 = 1.;
/// Columns a [`card_grid`] uses before its width is known.
const DEFAULT_COLUMNS: u16 = 3;

/// Run a synchronous queue/settings mutation of the backend on its blocking
/// pool (they persist state files), never on the UI thread.
pub(crate) fn backend_op(cx: &App, f: impl FnOnce(&Backend) + Send + 'static) {
    let b = backend(cx);
    let worker = b.clone();
    drop(b.spawn_blocking(move || f(&worker)));
}

/// Show an operation failure: what failed, and the error the backend gave.
pub(crate) fn notify_error(
    title: impl Into<SharedString>,
    error: &anyhow::Error,
    window: &mut Window,
    cx: &mut App,
) {
    window.push_notification(Notification::error(format!("{error:#}")).title(title), cx);
}

/// Show a success message whose result is not otherwise visible.
pub(crate) fn notify_success(message: impl Into<SharedString>, window: &mut Window, cx: &mut App) {
    window.push_notification(Notification::success(message), cx);
}

/// Wrapped plain-language tooltip built lazily on hover.
pub(crate) fn wrapped_tooltip(
    text: impl Into<SharedString>,
    window: &mut Window,
    cx: &mut App,
) -> AnyView {
    let text: SharedString = text.into();
    Tooltip::element(move |_, _| {
        div()
            .max_w(rems(22.))
            .text_xs()
            .whitespace_normal()
            .child(text.clone())
    })
    .py_1p5()
    .px_2p5()
    .build(window, cx)
}

/// A bordered surface for one object (a store, the current download).
pub(crate) fn card(cx: &App) -> Div {
    let theme = cx.theme();
    v_flex()
        .p_4()
        .gap_3()
        .rounded(theme.radius_lg)
        .border_1()
        .border_color(theme.border)
        .bg(theme.group_box)
}

/// Wrapped plain-language tooltip built lazily on hover.
pub(crate) fn plain_tooltip(text: SharedString, window: &mut Window, cx: &mut App) -> AnyView {
    Tooltip::element(move |_, _| {
        div()
            .max_w(rems(22.))
            .text_xs()
            .whitespace_normal()
            .child(text.clone())
    })
    .py_1p5()
    .px_2p5()
    .build(window, cx)
}

/// Compact info icon button that shows `text` in a wrapped hover tooltip.
pub(crate) fn info_tip(id: impl Into<SharedString>, text: SharedString, cx: &App) -> AnyElement {
    let muted = cx.theme().muted_foreground;
    let key = id.into();
    let tip_id = ElementId::Name(format!("info-tip-{key}").into());
    let icon_id = ElementId::Name(format!("info-icon-{key}").into());
    let hover_text = text.clone();
    div()
        .id(tip_id)
        .tooltip(move |window, cx| plain_tooltip(hover_text.clone(), window, cx))
        .child(
            Button::new(icon_id)
                .ghost()
                .xsmall()
                .compact()
                .icon(IconName::Info)
                .text_color(muted)
                .accessibility_label(text),
        )
        .into_any_element()
}

/// Columns of cards at least `min_card` rem wide that fit `available` rem,
/// clamped to `[1, max_columns]`.
fn card_columns(available: f32, min_card: f32, max_columns: u16) -> u16 {
    let cap = max_columns.max(1);
    let fit = ((available + CARD_GAP_REM) / (min_card + CARD_GAP_REM)).floor();
    fit.clamp(1., f32::from(cap)) as u16
}

/// Cards in equal columns that fill the width, with every row as tall as the
/// tallest card, so cards line up whatever their text. `width` keeps the
/// grid's last laid-out width; the grid renders again when it changes.
pub(crate) fn card_grid(
    width: &Rc<Cell<Pixels>>,
    min_card: f32,
    cards: Vec<AnyElement>,
    window: &Window,
) -> Div {
    card_grid_capped(width, min_card, u16::MAX, cards, window)
}

/// Like [`card_grid`], but caps the column count at `max_columns` so cards stay
/// comfortable on wide monitors.
pub(crate) fn card_grid_capped(
    width: &Rc<Cell<Pixels>>,
    min_card: f32,
    max_columns: u16,
    cards: Vec<AnyElement>,
    window: &Window,
) -> Div {
    card_grid_inner(width, min_card, max_columns, cards, true, window)
}

/// Like [`card_grid_capped`], but sizes each row to its own tallest card so a
/// tall card in one row does not leave empty space in other rows.
pub(crate) fn card_grid_natural_rows(
    width: &Rc<Cell<Pixels>>,
    min_card: f32,
    max_columns: u16,
    cards: Vec<AnyElement>,
    window: &Window,
) -> Div {
    card_grid_inner(width, min_card, max_columns, cards, false, window)
}

fn card_grid_inner(
    width: &Rc<Cell<Pixels>>,
    min_card: f32,
    max_columns: u16,
    cards: Vec<AnyElement>,
    equal_rows: bool,
    window: &Window,
) -> Div {
    let cap = max_columns.max(1);
    let measured = width.get();
    let columns = if measured > px(0.) {
        card_columns(measured / window.rem_size(), min_card, cap)
    } else {
        DEFAULT_COLUMNS.min(cap)
    };
    let rows = cards.len().div_ceil(usize::from(columns)).max(1);
    let row_count = u16::try_from(rows).unwrap_or(u16::MAX);
    let width = width.clone();
    let mut grid = div().w_full().grid().grid_cols(columns).gap_4();
    grid = if equal_rows {
        // Fractional rows share the tallest card's height.
        grid.grid_rows_min_content(row_count)
    } else {
        grid.grid_rows_max_content(row_count).items_start()
    };
    div()
        .relative()
        .w_full()
        .child(
            canvas(
                move |bounds, window, _| {
                    // Columns are picked while rendering, so render again once
                    // the width changed.
                    if width.replace(bounds.size.width) != bounds.size.width {
                        window.refresh();
                    }
                },
                |_, _, _, _| {},
            )
            .absolute()
            .top_0()
            .left_0()
            .size_full(),
        )
        .child(grid.children(cards))
}

#[cfg(test)]
mod tests {
    use super::card_columns;

    #[test]
    fn card_columns_fill_the_width() {
        assert_eq!(card_columns(0., 17., u16::MAX), 1);
        assert_eq!(card_columns(17., 17., u16::MAX), 1);
        assert_eq!(card_columns(35., 17., u16::MAX), 2);
        assert_eq!(card_columns(89., 17., u16::MAX), 5);
        assert_eq!(card_columns(61.9, 20., u16::MAX), 2);
        assert_eq!(card_columns(62., 20., u16::MAX), 3);
        assert_eq!(card_columns(120., 20., 3), 3);
        assert_eq!(card_columns(120., 22., 2), 2);
    }
}
