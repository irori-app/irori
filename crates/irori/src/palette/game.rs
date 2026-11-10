//! The command palette's rows for the game whose page is open: its play
//! status, troubleshooting, Steam Play, ReShade and RenoDX, Steam shortcuts
//! and save backups.

use gpui_kit::SharedString;
use gpui_kit::assets::IconName;
use gpui_kit::component::command::CommandItem;
use irori_core::{GameInfo, Runner};
use irori_services::PlayStatus;
use irori_ui::i18n::{t, t_args};

use super::PaletteTarget;

/// The rows for `game`, whose play status is `current`, in the order the
/// palette lists them.
pub(super) fn rows(
    game: &GameInfo,
    current: Option<PlayStatus>,
) -> Vec<(CommandItem, PaletteTarget)> {
    let mut rows = Vec::new();
    let mut push = |item: CommandItem, target: PaletteTarget| rows.push((item, target));
    let keywords = [irori_ui::play_status::title()];
    for status in PlayStatus::ALL.into_iter().filter(|s| current != Some(*s)) {
        let label = t_args(
            "shell.palette.play_status",
            &[
                ("title", &game.title),
                ("status", &irori_ui::play_status::label(status)),
            ],
        );
        push(
            CommandItem::new()
                .label(label)
                .keywords(keywords.clone())
                .icon(irori_ui::play_status::icon(status)),
            PaletteTarget::PlayStatus(game.id(), Some(status)),
        );
    }
    if current.is_some() {
        push(
            CommandItem::new()
                .label(t_args(
                    "shell.palette.clear_play_status",
                    &[("title", &game.title)],
                ))
                .keywords(keywords)
                .icon(IconName::X),
            PaletteTarget::PlayStatus(game.id(), None),
        );
    }
    if game.is_installed && game.runner().manages_installs() {
        push(
            CommandItem::new()
                .label(t_args(
                    "shell.palette.troubleshoot",
                    &[("title", &game.title)],
                ))
                .keywords([t("shell.palette.troubleshoot_keywords")])
                .icon(IconName::Stethoscope),
            PaletteTarget::Troubleshoot(game.id()),
        );
    }
    if irori_game::ops::offers_steam_play(&game.id()) {
        push(
            CommandItem::new()
                .label(t_args(
                    "shell.palette.steam_play",
                    &[("title", &game.title)],
                ))
                .keywords([t("shell.palette.steam_play_keywords")])
                .icon(IconName::Wine),
            PaletteTarget::SteamPlay(game.id()),
        );
    }
    if irori_services::supports_reshade(game) {
        push(
            CommandItem::new()
                .label(t_args(
                    "shell.palette.graphics_mods",
                    &[("title", &game.title)],
                ))
                .keywords([t("shell.palette.graphics_mods_keywords")])
                .icon(IconName::Sparkles),
            PaletteTarget::GraphicsMods(game.id()),
        );
    }
    if game.runner() != Runner::Steam && (game.is_installed || game.installable) {
        let keywords = [t("shell.palette.steam_shortcut_keywords")];
        push(
            CommandItem::new()
                .label(t_args(
                    "shell.palette.add_to_steam",
                    &[("title", &game.title)],
                ))
                .keywords(keywords.clone())
                .icon(IconName::Gamepad2),
            PaletteTarget::AddToSteam(game.id()),
        );
        push(
            CommandItem::new()
                .label(t_args(
                    "shell.palette.remove_from_steam",
                    &[("title", &game.title)],
                ))
                .keywords(keywords)
                .icon(IconName::CircleMinus),
            PaletteTarget::RemoveFromSteam(game.id()),
        );
    }
    if irori_ui::save_backups::offers_backups(game) {
        let keywords = [SharedString::new_static("Ludusavi")];
        push(
            CommandItem::new()
                .label(t_args(
                    "shell.palette.back_up_saves",
                    &[("title", &game.title)],
                ))
                .keywords(keywords.clone())
                .icon(IconName::DatabaseBackup),
            PaletteTarget::BackUpSaves(game.id()),
        );
        push(
            CommandItem::new()
                .label(t_args(
                    "shell.palette.save_backups",
                    &[("title", &game.title)],
                ))
                .keywords(keywords)
                .icon(IconName::ArchiveRestore),
            PaletteTarget::SaveBackups(game.id()),
        );
    }
    rows
}
