//! Steam shortcut changes that waited for Steam to close
//! (`irori_extras::shortcuts::steam::queue`): made from the periodic tick
//! once Steam is gone, and announced to the Steam card when their number
//! changes. Nothing is checked while nothing waits.

use std::sync::atomic::Ordering;

use irori_core::{Event, Notice, NoticeLevel, Runner};
use irori_extras::shortcuts::steam::queue;

use crate::Backend;

impl Backend {
    /// How many Steam shortcut changes wait for Steam to close.
    pub fn steam_shortcuts_waiting(&self) -> usize {
        queue::waiting()
    }

    /// Read the queue an earlier run left (at start).
    pub(crate) async fn load_steam_shortcut_queue(&self) {
        let waiting = queue::load().await;
        self.note_steam_shortcuts_waiting(waiting);
    }

    /// Make the waiting changes when Steam has closed.
    pub(crate) async fn steam_shortcuts_when_due(&self) {
        if queue::waiting() > 0 {
            let app = self.settings.app();
            match queue::apply(&app, |id| self.game(id)).await {
                Ok(Some(report)) => {
                    tracing::info!(
                        "Steam shortcuts after Steam closed: {} added, {} removed",
                        report.added,
                        report.removed
                    );
                    for e in &report.errors {
                        tracing::warn!("{e}");
                    }
                }
                Ok(None) => {}
                Err(e) => self.bus.notice(
                    Notice::new(NoticeLevel::Error, "Steam shortcuts weren't changed")
                        .message(format!("{e:#}"))
                        .keys("shell.notice.steam_shortcuts_failed", None, &[])
                        .account(Runner::Steam),
                ),
            }
        }
        self.note_steam_shortcuts_waiting(queue::waiting());
    }

    /// Tell the Steam card when the number of waiting changes moved.
    fn note_steam_shortcuts_waiting(&self, waiting: usize) {
        if self.steam_shortcuts_shown.swap(waiting, Ordering::Relaxed) != waiting {
            self.bus.emit(Event::AccountChanged(Runner::Steam));
        }
    }
}
