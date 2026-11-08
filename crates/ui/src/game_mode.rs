//! Game modes on every screen that shows them: their names and icons, for
//! the library's filter and sections, the game page's chips and the Modify
//! dialog.

use gpui_kit::SharedString;
use gpui_kit::assets::IconName;
use irori_core::GameMode;

use crate::i18n::t;

/// "Game modes", for headings and filter sections.
pub fn title() -> SharedString {
    t("ui.game_mode.title")
}

/// Name of a mode ("Co-op", "Online multiplayer").
pub fn label(mode: GameMode) -> SharedString {
    t(match mode {
        GameMode::Single => "ui.game_mode.single",
        GameMode::Coop => "ui.game_mode.coop",
        GameMode::Local => "ui.game_mode.local",
        GameMode::Online => "ui.game_mode.online",
        GameMode::Mmo => "ui.game_mode.mmo",
    })
}

/// Name of the section of games whose modes nobody knows.
pub fn unknown_label() -> SharedString {
    t("ui.game_mode.unknown")
}

pub fn icon(mode: GameMode) -> IconName {
    match mode {
        GameMode::Single => IconName::User,
        GameMode::Coop => IconName::Handshake,
        GameMode::Local => IconName::Sofa,
        GameMode::Online => IconName::Globe,
        GameMode::Mmo => IconName::Castle,
    }
}

/// Icon of the section of games whose modes nobody knows.
pub fn unknown_icon() -> IconName {
    IconName::CircleDashed
}
