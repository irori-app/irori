//! MangoHud in a running game's session, for the in-game overlay: show or
//! hide it and change its frame limit while the game runs. The launch
//! writes the session's MangoHud file (`irori_runtime::mangohud`), MangoHud
//! applies every change to it at once, and the file goes when the session
//! ends ([`Backend::finish_session`](crate::Backend)).

use anyhow::{Context as _, Result};
use irori_core::GameId;
pub use irori_runtime::mangohud::MangoState;

use crate::Backend;

impl Backend {
    /// MangoHud's state in the running session of `id`; `None` when it
    /// doesn't run with the game, or Irori can't change it (the user's
    /// `MANGOHUD_CONFIG` keeps MangoHud from reading a file).
    pub async fn mangohud(&self, id: &GameId) -> Option<MangoState> {
        let id = id.clone();
        self.spawn_blocking(move || irori_runtime::mangohud::read_session(&id))
            .await
            .ok()
            .flatten()
    }

    /// Change MangoHud's state in the running session of `id`; returns it
    /// as the file then says.
    pub async fn set_mangohud(&self, id: &GameId, state: MangoState) -> Result<MangoState> {
        let id = id.clone();
        self.spawn_blocking(move || {
            irori_runtime::mangohud::write_session(&id, state)?;
            irori_runtime::mangohud::read_session(&id)
                .context("MangoHud isn't running with this game")
        })
        .await
        .context("MangoHud's settings couldn't be changed")?
    }
}
