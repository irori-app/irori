//! Copies of one game from several stores, which the library shows as one
//! entry (`irori_services::copies`): which copies a game has, and the menu
//! items that open, prefer and separate them. Each copy keeps its own
//! [`GameId`], so whatever a screen does with one is done with that copy.
//! The changes write the library file, so they run off the UI thread.

use std::sync::Arc;

use gpui_kit::App;
use gpui_kit::assets::IconName;
use gpui_kit::component::menu::{PopupMenu, PopupMenuItem};
use irori_core::{GameId, GameInfo};

use crate::i18n::{t, t_args};
use crate::{Route, backend, models, navigate};

/// Every copy of `id`'s entry, the default one first; empty when it is the
/// only one.
pub fn of(id: &GameId, cx: &App) -> Vec<Arc<GameInfo>> {
    let library = models(cx).library.read(cx);
    let snapshot = &library.snapshot;
    snapshot
        .copies_of(id)
        .iter()
        .filter_map(|copy| snapshot.get(copy).cloned())
        .collect()
}

/// "Open the Steam copy", for menus and tooltips.
pub fn open_label(store: &str) -> gpui_kit::SharedString {
    t_args("ui.copies.open", &[("store", store)])
}

/// The copies' ids, for the changes below.
fn ids(copies: &[Arc<GameInfo>]) -> Vec<GameId> {
    copies.iter().map(|g| g.id()).collect()
}

/// Make `ids` one library entry.
pub fn combine(ids: Vec<GameId>, cx: &mut App) {
    let b = backend(cx);
    crate::state::run_infallible(cx, async move { b.userdata.combine_copies(&ids) }).detach();
}

/// Take `id` out of its entry.
pub fn separate(id: GameId, group: Vec<GameId>, cx: &mut App) {
    let b = backend(cx);
    crate::state::run_infallible(cx, async move { b.userdata.separate_copy(&id, &group) }).detach();
}

/// Show every copy of `group` on its own.
pub fn separate_all(group: Vec<GameId>, cx: &mut App) {
    let b = backend(cx);
    crate::state::run_infallible(cx, async move { b.userdata.separate_copies(&group) }).detach();
}

/// Make `id` the copy its entry plays and installs.
pub fn make_default(id: GameId, group: Vec<GameId>, cx: &mut App) {
    let b = backend(cx);
    crate::state::run_infallible(cx, async move { b.userdata.set_default_copy(&id, &group) })
        .detach();
}

/// Add items for `id`'s copies to `menu`: open each other copy, make this
/// one the default, separate it or all of them. Nothing for a game with
/// one copy.
pub fn menu_items(mut menu: PopupMenu, id: &GameId, cx: &App) -> PopupMenu {
    let copies = of(id, cx);
    if copies.len() < 2 {
        return menu;
    }
    let group = ids(&copies);
    for copy in copies.iter().filter(|c| c.id() != *id) {
        let copy_id = copy.id();
        menu = menu.item(
            PopupMenuItem::new(open_label(copy.runner().store_name()))
                .icon(IconName::Layers)
                .on_click(move |_, _, cx| navigate(Route::Game(copy_id.clone()), cx)),
        );
    }
    if group.first() != Some(id) {
        let (id, group) = (id.clone(), group.clone());
        menu = menu.item(
            PopupMenuItem::new(t("ui.copies.make_default"))
                .icon(IconName::Star)
                .on_click(move |_, _, cx| make_default(id.clone(), group.clone(), cx)),
        );
    }
    let (one, one_group) = (id.clone(), group.clone());
    menu.item(
        PopupMenuItem::new(t("ui.copies.separate"))
            .icon(IconName::Unlink)
            .on_click(move |_, _, cx| separate(one.clone(), one_group.clone(), cx)),
    )
    .item(
        PopupMenuItem::new(t("ui.copies.separate_all"))
            .icon(IconName::Ungroup)
            .on_click(move |_, _, cx| separate_all(group.clone(), cx)),
    )
}
