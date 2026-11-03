//! Where the user is with each game: the play status they set (plan to play,
//! playing, beaten…) and the date they set it, kept with the rest of the
//! user's library data. Nothing sets a status on its own; a game without one
//! counts as played or not played from its playtime, and only in views that
//! group games by status (filters, stats).

use serde::{Deserialize, Serialize};

/// A play status the user picked for a game.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PlayStatus {
    NotPlayed,
    PlanToPlay,
    Playing,
    OnHold,
    Beaten,
    Abandoned,
}

impl PlayStatus {
    /// Every status, in the order menus list them.
    pub const ALL: [PlayStatus; 6] = [
        PlayStatus::NotPlayed,
        PlayStatus::PlanToPlay,
        PlayStatus::Playing,
        PlayStatus::OnHold,
        PlayStatus::Beaten,
        PlayStatus::Abandoned,
    ];
}

/// A game's play status and when the user set it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlayStatusRecord {
    pub status: PlayStatus,
    /// RFC 3339 time the status was set.
    pub since: String,
}

/// A game's status as filters and stats group it: the user's own, else
/// [`EffectiveStatus::Played`] for a game with playtime and
/// [`EffectiveStatus::NotPlayed`] for one without.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum EffectiveStatus {
    Beaten,
    Playing,
    PlanToPlay,
    Played,
    OnHold,
    Abandoned,
    NotPlayed,
}

impl EffectiveStatus {
    /// Every group, in the order charts and legends show them: finished and
    /// active games first, untouched ones last.
    pub const ALL: [EffectiveStatus; 7] = [
        EffectiveStatus::Beaten,
        EffectiveStatus::Playing,
        EffectiveStatus::PlanToPlay,
        EffectiveStatus::Played,
        EffectiveStatus::OnHold,
        EffectiveStatus::Abandoned,
        EffectiveStatus::NotPlayed,
    ];
}

impl From<PlayStatus> for EffectiveStatus {
    fn from(status: PlayStatus) -> Self {
        match status {
            PlayStatus::NotPlayed => Self::NotPlayed,
            PlayStatus::PlanToPlay => Self::PlanToPlay,
            PlayStatus::Playing => Self::Playing,
            PlayStatus::OnHold => Self::OnHold,
            PlayStatus::Beaten => Self::Beaten,
            PlayStatus::Abandoned => Self::Abandoned,
        }
    }
}
