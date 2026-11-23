//! The command palette's rows for one game: for the game whose page is
//! open, its play status, the user's own actions, troubleshooting, Steam
//! Play, playing a ROM as a PC game, ReShade and RenoDX, Steam shortcuts and
//! save backups; and for any game, the level → opens, which adds playing,
//! stopping, going back to it, its page, its store page, favourite, its
//! links and Steam link, hiding and its folder.

use super::entry::Entry;
use gpui_kit::SharedString;
use gpui_kit::assets::IconName;
use irori_core::{GameInfo, Runner, StatusKind, UserAction};
use irori_services::PlayStatus;
use irori_ui::i18n::{t, t_args};

use super::PaletteTarget;

/// The rows for `game`, whose play status is `current` and whose own
/// actions are `actions` (with their place), in the order the palette lists
/// them.
pub(super) fn rows(
    game: &GameInfo,
    current: Option<PlayStatus>,
    actions: &[(usize, UserAction)],
) -> Vec<(Entry, PaletteTarget)> {
    let mut rows = Vec::new();
    let mut push = |item: Entry, target: PaletteTarget| rows.push((item, target));
    if game.is_installed {
        for (index, action) in actions {
            push(
                Entry::new()
                    .label(t_args(
                        "shell.palette.user_action",
                        &[("action", &action.label()), ("title", &game.title)],
                    ))
                    .keywords([t("shell.palette.user_action_keywords")])
                    .icon(irori_ui::user_actions::icon(action)),
                PaletteTarget::UserAction(game.id(), *index),
            );
        }
        push(
            Entry::new()
                .label(t_args(
                    "shell.palette.edit_actions",
                    &[("title", &game.title)],
                ))
                .keywords([t("shell.palette.user_action_keywords")])
                .icon(IconName::Pencil),
            PaletteTarget::EditActions(game.id()),
        );
    }
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
            Entry::new()
                .label(label)
                .keywords(keywords.clone())
                .icon(irori_ui::play_status::icon(status)),
            PaletteTarget::PlayStatus(game.id(), Some(status)),
        );
    }
    if current.is_some() {
        push(
            Entry::new()
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
            Entry::new()
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
            Entry::new()
                .label(t_args(
                    "shell.palette.steam_play",
                    &[("title", &game.title)],
                ))
                .keywords([t("shell.palette.steam_play_keywords")])
                .icon(IconName::Wine),
            PaletteTarget::SteamPlay(game.id()),
        );
    }
    if game.runner() == Runner::Emulation {
        push(
            Entry::new()
                .label(t_args(
                    "shell.palette.play_as_pc_game",
                    &[("title", &game.title)],
                ))
                .keywords([t("shell.palette.play_as_pc_game_keywords")])
                .icon(IconName::Monitor),
            PaletteTarget::PlayAsPcGame(game.id()),
        );
    }
    if irori_services::supports_reshade(game) {
        push(
            Entry::new()
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
            Entry::new()
                .label(t_args(
                    "shell.palette.add_to_steam",
                    &[("title", &game.title)],
                ))
                .keywords(keywords.clone())
                .icon(IconName::Gamepad2),
            PaletteTarget::AddToSteam(game.id()),
        );
        push(
            Entry::new()
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
            Entry::new()
                .label(t_args(
                    "shell.palette.back_up_saves",
                    &[("title", &game.title)],
                ))
                .keywords(keywords.clone())
                .icon(IconName::DatabaseBackup),
            PaletteTarget::BackUpSaves(game.id()),
        );
        push(
            Entry::new()
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

/// What the palette knows about a game when it lists the game's actions.
#[derive(Debug, Clone, Default)]
pub(super) struct GameFacts {
    /// Running, starting, installing…
    pub(super) kind: Option<StatusKind>,
    pub(super) status: Option<PlayStatus>,
    pub(super) favourite: bool,
    pub(super) hidden: bool,
    /// Its store keeps cloud saves Irori syncs.
    pub(super) syncable: bool,
    pub(super) actions: Vec<(usize, UserAction)>,
}

/// Every action of `game`, as the level → opens lists them: what it is
/// doing decides the first row (back to it while it runs, cancel while it
/// starts, else play), then its page and store page, favourite, the rows
/// [`rows`] gives, its folder and hiding, and Stop last, out of the way of
/// Enter.
pub(super) fn level_rows(game: &GameInfo, facts: &GameFacts) -> Vec<(Entry, PaletteTarget)> {
    let id = game.id();
    let title = game.title.as_str();
    let mut out = Vec::new();
    let mut push = |item: Entry, target: PaletteTarget| out.push((item, target));
    let playing = facts.kind == Some(StatusKind::Playing);
    match facts.kind {
        Some(StatusKind::Playing) => push(
            Entry::new()
                .label(t_args("shell.palette.back_to_game", &[("title", title)]))
                .keywords([t("shell.palette.back_to_game_keywords")])
                .icon(IconName::AppWindow),
            PaletteTarget::BackToGame(id.clone()),
        ),
        Some(StatusKind::Launching) => push(
            Entry::new()
                .label(t_args("shell.palette.cancel_launch", &[("title", title)]))
                .keywords([t("shell.palette.cancel_launch_keywords")])
                .icon(IconName::X),
            PaletteTarget::CancelLaunch(id.clone()),
        ),
        Some(kind) if kind.is_busy() => {}
        _ if game.is_installed => push(
            Entry::new()
                .label(t_args("shell.palette.play", &[("title", title)]))
                .icon(IconName::Play),
            PaletteTarget::Play(id.clone()),
        ),
        _ => {}
    }
    push(
        Entry::new()
            .label(t_args("shell.palette.show_game", &[("title", title)]))
            .icon(IconName::Gamepad2),
        PaletteTarget::Open(id.clone()),
    );
    if let Some(url) = game.store_url.clone().filter(|u| !u.is_empty()) {
        push(
            Entry::new()
                .label(t_args(
                    "shell.palette.store_page",
                    &[("store", game.runner().store_name())],
                ))
                .icon(IconName::Store),
            PaletteTarget::StorePage(game.runner(), url),
        );
    }
    push(
        Entry::new()
            .label(t(if facts.favourite {
                "shell.palette.remove_favourite"
            } else {
                "shell.palette.add_favourite"
            }))
            .icon(IconName::Star),
        PaletteTarget::Favourite(id.clone(), !facts.favourite),
    );
    for (item, target) in rows(game, facts.status, &facts.actions) {
        push(item, target);
    }
    push(
        Entry::new()
            .label(t_args("shell.palette.game_links", &[("title", title)]))
            .keywords([t("shell.palette.game_links_keywords")])
            .icon(IconName::ExternalLink),
        PaletteTarget::GameLinks(id.clone()),
    );
    push(
        Entry::new()
            .label(t("shell.palette.detail_rows"))
            .keywords([t("shell.palette.detail_rows_keywords")])
            .icon(IconName::Settings2),
        PaletteTarget::DetailRows,
    );
    if !matches!(game.runner(), Runner::Steam | Runner::Emulation) {
        push(
            Entry::new()
                .label(t_args("shell.palette.steam_link", &[("title", title)]))
                .keywords([t("shell.palette.steam_link_keywords")])
                .icon(IconName::Link),
            PaletteTarget::SteamLink(id.clone()),
        );
    }
    if facts.syncable {
        push(
            Entry::new()
                .label(t_args("shell.palette.sync_saves", &[("title", title)]))
                .keywords([t("shell.palette.sync_saves_keywords")])
                .icon(IconName::RefreshCw),
            PaletteTarget::SyncSaves(id.clone()),
        );
    }
    if game.is_installed && game.install_path().is_some() {
        push(
            Entry::new()
                .label(t("shell.palette.open_folder"))
                .icon(IconName::FolderOpen),
            PaletteTarget::OpenFolder(id.clone()),
        );
    }
    push(
        Entry::new()
            .label(t(if facts.hidden {
                "shell.palette.unhide_game"
            } else {
                "shell.palette.hide_game"
            }))
            .icon(if facts.hidden {
                IconName::Eye
            } else {
                IconName::EyeOff
            }),
        PaletteTarget::Hide(id.clone(), !facts.hidden),
    );
    if playing {
        push(
            Entry::new()
                .label(t_args("shell.palette.stop_game", &[("title", title)]))
                .icon(IconName::Square),
            PaletteTarget::Stop(id.clone()),
        );
    }
    push(
        Entry::new()
            .label(t("shell.palette.back"))
            .icon(IconName::ArrowLeft),
        PaletteTarget::Back,
    );
    out
}
