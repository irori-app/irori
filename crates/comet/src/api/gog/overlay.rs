use std::sync::Arc;

use super::achievements::Achievement;

#[derive(Clone, Debug)]
pub enum OverlayPeerMessage {
    InitConnection(String),
    Achievement(Arc<Achievement>),
    DisablePopups(Vec<u8>),
    OpenWebPage(String),
    InvitationDialog(String),
    VisibilityChange(bool),
    GameJoin((u64, String, String)),
    /// Irori: a game signed in as `client_id` unlocked `achievement`, for
    /// the host to announce. No overlay pipe reads it.
    Unlocked {
        client_id: String,
        achievement: Arc<Achievement>,
    },
}
