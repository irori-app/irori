//! Concrete store handles the shell hands to the settings feature for
//! store-specific panels (the backend only exposes `dyn Store`).

use std::sync::Arc;

use gpui_kit::{App, Global};
use irori_epic::EpicStore;
use irori_steam::SteamStore;

struct StoreHandles {
    epic: Option<Arc<EpicStore>>,
    steam: Option<Arc<SteamStore>>,
}

impl Global for StoreHandles {}

/// Give the settings feature the concrete Epic store, enabling the "Epic
/// Games" group (EOS overlay and Epic Games Launcher sync) in Advanced, and
/// the Steam store, enabling the compatibility tool choice of Steam games
/// on Linux.
///
/// Call once at startup, after `irori_ui::init` and before the settings page
/// is first opened; forms built earlier keep hiding the groups. Passing
/// `None` (the store disabled) hides its group. A later call replaces the
/// handles.
pub fn install_store_handles(
    epic: Option<Arc<EpicStore>>,
    steam: Option<Arc<SteamStore>>,
    cx: &mut App,
) {
    cx.set_global(StoreHandles { epic, steam });
}

/// The Epic store, when the shell installed it.
pub(crate) fn epic(cx: &App) -> Option<Arc<EpicStore>> {
    cx.try_global::<StoreHandles>().and_then(|h| h.epic.clone())
}

/// The Steam store, when the shell installed it.
pub(crate) fn steam(cx: &App) -> Option<Arc<SteamStore>> {
    cx.try_global::<StoreHandles>()
        .and_then(|h| h.steam.clone())
}
