//! The folders other launchers (Lutris, Bottles, Faugus Launcher) keep
//! games, prefixes and Wine builds in, read from their own data (only read)
//! into `irori_core::fs_guard::OtherLaunchers`, so no deletion of Irori's
//! (uninstall, prefix removal, storage cleanup) reaches them, imported or
//! not.

use irori_core::fs_guard::OtherLaunchers;

use crate::Backend;

impl Backend {
    /// Read the other launchers' folders again.
    pub(crate) async fn note_other_launchers(&self) {
        let done = tokio::task::spawn_blocking(|| {
            for (launcher, folders) in irori_extras::wine_launchers::found_folders() {
                OtherLaunchers::set_found(launcher, folders)?;
            }
            anyhow::Ok(())
        })
        .await;
        match done {
            Ok(Err(e)) => tracing::warn!("recording other launchers' folders: {e:#}"),
            Err(e) => tracing::warn!("reading other launchers' folders stopped: {e}"),
            Ok(Ok(())) => {}
        }
    }
}
