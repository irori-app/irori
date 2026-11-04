//! Play status on every screen that shows or sets it: its names, colours and
//! icons, the menu that sets it and the button the game page shows. Setting
//! a status writes the library file, so it runs off the UI thread.

use gpui_kit::assets::IconName;
use gpui_kit::component::button::Button;
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenu, PopupMenuItem};
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, h_flex};
use gpui_kit::{
    AnyElement, App, ElementId, Hsla, IntoElement as _, ParentElement as _, SharedString,
    Styled as _, Window, div, px,
};
use irori_core::GameId;
use irori_services::{EffectiveStatus, PlayStatus};

use crate::i18n::t;
use crate::{backend, models};

/// "Play status", for menus and headings.
pub fn title() -> SharedString {
    t("ui.play_status.title")
}

/// Name of a status ("Plan to play", "Beaten").
pub fn label(status: impl Into<EffectiveStatus>) -> SharedString {
    t(match status.into() {
        EffectiveStatus::Beaten => "ui.play_status.beaten",
        EffectiveStatus::Playing => "ui.play_status.playing",
        EffectiveStatus::PlanToPlay => "ui.play_status.plan_to_play",
        EffectiveStatus::Played => "ui.play_status.played",
        EffectiveStatus::OnHold => "ui.play_status.on_hold",
        EffectiveStatus::Abandoned => "ui.play_status.abandoned",
        EffectiveStatus::NotPlayed => "ui.play_status.not_played",
    })
}

/// Colour of a status in charts, legends and badges, from the theme's own
/// status and chart colours so it reads on every theme.
pub fn color(status: impl Into<EffectiveStatus>, cx: &App) -> Hsla {
    let theme = cx.theme();
    match status.into() {
        EffectiveStatus::Beaten => theme.success,
        EffectiveStatus::Playing => theme.info,
        EffectiveStatus::PlanToPlay => theme.chart_5,
        // The two groups without a status the user set stay neutral.
        EffectiveStatus::Played => theme.muted_foreground.opacity(0.75),
        EffectiveStatus::OnHold => theme.warning,
        EffectiveStatus::Abandoned => theme.danger,
        EffectiveStatus::NotPlayed => theme.muted_foreground.opacity(0.3),
    }
}

pub fn icon(status: impl Into<EffectiveStatus>) -> IconName {
    match status.into() {
        EffectiveStatus::Beaten => IconName::Trophy,
        EffectiveStatus::Playing => IconName::CirclePlay,
        EffectiveStatus::PlanToPlay => IconName::Bookmark,
        EffectiveStatus::Played => IconName::CircleCheck,
        EffectiveStatus::OnHold => IconName::CirclePause,
        EffectiveStatus::Abandoned => IconName::CircleX,
        EffectiveStatus::NotPlayed => IconName::CircleDashed,
    }
}

/// Set a game's status, or clear it with `None`.
pub fn set(id: &GameId, status: Option<PlayStatus>, cx: &mut App) {
    set_many(vec![id.clone()], status, cx);
}

/// Set or clear the status of several games in one change.
pub fn set_many(ids: Vec<GameId>, status: Option<PlayStatus>, cx: &mut App) {
    let b = backend(cx);
    crate::state::run_infallible(cx, async move {
        match ids.as_slice() {
            [id] => b.userdata.set_play_status(id, status),
            ids => b.userdata.set_play_status_many(ids, status),
        }
    })
    .detach();
}

/// The status the user set for a game.
pub fn of(id: &GameId, cx: &App) -> Option<PlayStatus> {
    models(cx).library.read(cx).user.play_status(&id.key())
}

/// Add one checked item per status to `menu`, and an item clearing the
/// status when there is one.
pub fn menu(menu: PopupMenu, id: &GameId, cx: &App) -> PopupMenu {
    menu_many(menu, vec![id.clone()], cx)
}

/// [`menu`] for several games: a status is checked when every one of them
/// has it, and clearing is offered when any has one.
pub fn menu_many(menu: PopupMenu, ids: Vec<GameId>, cx: &App) -> PopupMenu {
    menu_many_with(menu, ids, cx, |ids, status, _, cx| {
        set_many(ids, status, cx)
    })
}

/// [`menu_many`] whose picks go to `on_pick` (with the games and the
/// status, `None` to clear it) instead of being set right away, so callers
/// can offer an undo.
pub fn menu_many_with(
    menu: PopupMenu,
    ids: Vec<GameId>,
    cx: &App,
    on_pick: impl Fn(Vec<GameId>, Option<PlayStatus>, &mut Window, &mut App) + 'static,
) -> PopupMenu {
    let on_pick = std::rc::Rc::new(on_pick);
    let user = models(cx).library.read(cx).user.clone();
    let statuses: Vec<Option<PlayStatus>> =
        ids.iter().map(|id| user.play_status(&id.key())).collect();
    let shared = statuses
        .first()
        .copied()
        .flatten()
        .filter(|first| statuses.iter().all(|s| *s == Some(*first)));
    let mut menu = menu;
    for status in PlayStatus::ALL {
        let (ids, on_pick) = (ids.clone(), on_pick.clone());
        menu = menu.item(
            PopupMenuItem::new(label(status))
                .icon(Icon::new(icon(status)))
                .checked(shared == Some(status))
                .on_click(move |_, window, cx| on_pick(ids.clone(), Some(status), window, cx)),
        );
    }
    if statuses.iter().any(Option::is_some) {
        menu = menu.separator().item(
            PopupMenuItem::new(t("ui.play_status.clear"))
                .icon(Icon::new(IconName::X))
                .on_click(move |_, window, cx| on_pick(ids.clone(), None, window, cx)),
        );
    }
    menu
}

/// An outline pill naming the game's status after a dot in its colour,
/// opening [`menu`]. Without a status it reads "Set status". Long names
/// are cut short to fit the row it sits in.
pub fn button(element_id: impl Into<ElementId>, id: &GameId, cx: &App) -> AnyElement {
    let current = of(id, cx);
    let id = id.clone();
    let text = current.map_or_else(|| t("ui.play_status.set"), label);
    let dot = current.map_or_else(|| cx.theme().muted_foreground, |s| color(s, cx));
    Button::new(element_id)
        .outline()
        .small()
        .rounded(px(999.))
        .min_w_0()
        .max_w_full()
        .tooltip(t("ui.play_status.title"))
        .accessibility_label(text.clone())
        .dropdown_caret(true)
        .child(
            h_flex()
                .min_w_0()
                .gap_1p5()
                .items_center()
                .child(div().size_2().flex_shrink_0().rounded_full().bg(dot))
                .child(div().min_w_0().truncate().child(text)),
        )
        .dropdown_menu(move |m, _, cx| menu(m, &id, cx))
        .into_any_element()
}
