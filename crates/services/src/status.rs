//! Live game statuses. Progress updates arrive many times per second from tool
//! output; events are throttled per game so the UI repaints at a bounded rate.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use irori_core::process::CancellationToken;
use irori_core::{Event, EventBus, GameId, GameStatus, InstallProgress, StatusKind};
use parking_lot::Mutex;

/// Minimum interval between progress events of one game.
const PROGRESS_INTERVAL: Duration = Duration::from_millis(250);

struct Entry {
    status: GameStatus,
    last_emit: Instant,
}

#[derive(Clone)]
pub struct StatusRegistry {
    inner: Arc<Mutex<HashMap<GameId, Entry>>>,
    /// The tokens of launches being prepared, which cancelling fires.
    launches: Arc<Mutex<HashMap<GameId, CancellationToken>>>,
    bus: EventBus,
}

impl StatusRegistry {
    pub fn new(bus: EventBus) -> Self {
        Self {
            inner: Arc::default(),
            launches: Arc::default(),
            bus,
        }
    }

    pub fn get(&self, id: &GameId) -> Option<GameStatus> {
        self.inner.lock().get(id).map(|e| e.status.clone())
    }

    pub fn all(&self) -> Vec<GameStatus> {
        self.inner
            .lock()
            .values()
            .map(|e| e.status.clone())
            .collect()
    }

    pub fn is_busy(&self, id: &GameId) -> bool {
        self.inner
            .lock()
            .get(id)
            .is_some_and(|e| e.status.kind.is_busy())
    }

    /// Enter a status (resets progress and context).
    pub fn set(&self, id: &GameId, kind: StatusKind) {
        let status = GameStatus::new(id.clone(), kind);
        self.inner.lock().insert(
            id.clone(),
            Entry {
                status: status.clone(),
                last_emit: Instant::now(),
            },
        );
        self.bus.emit(Event::Status(status));
    }

    /// Update progress of the current status (throttled).
    pub fn progress(&self, id: &GameId, progress: InstallProgress) {
        let emit = {
            let mut map = self.inner.lock();
            let Some(entry) = map.get_mut(id) else { return };
            entry.status.progress = Some(progress);
            let due = entry.last_emit.elapsed() >= PROGRESS_INTERVAL
                || progress.percent.is_some_and(|p| p >= 100.0);
            if due {
                entry.last_emit = Instant::now();
                Some(entry.status.clone())
            } else {
                None
            }
        };
        if let Some(s) = emit {
            self.bus.emit(Event::Status(s));
        }
    }

    /// Update the detail line of the current status.
    pub fn context(&self, id: &GameId, context: &str) {
        let emit = {
            let mut map = self.inner.lock();
            let Some(entry) = map.get_mut(id) else { return };
            if entry.status.context.as_deref() == Some(context) {
                return;
            }
            entry.status.context = Some(context.to_string());
            entry.last_emit = Instant::now();
            entry.status.clone()
        };
        self.bus.emit(Event::Status(emit));
    }

    /// Return the game to idle.
    pub fn clear(&self, id: &GameId) {
        if self.inner.lock().remove(id).is_some() {
            self.bus.emit(Event::StatusCleared(id.clone()));
        }
    }

    /// RAII guard that clears the status when dropped (error paths included).
    pub fn guard(&self, id: &GameId, kind: StatusKind) -> StatusGuard {
        self.set(id, kind);
        StatusGuard {
            registry: self.clone(),
            id: id.clone(),
            launch: false,
        }
    }

    /// Enter [`StatusKind::Launching`] with the token that
    /// [`Self::cancel_launch`] fires until the guard is dropped, which the
    /// launch does once the game started (or the launch ended).
    pub fn launch_guard(&self, id: &GameId) -> (StatusGuard, CancellationToken) {
        let token = CancellationToken::new();
        self.launches.lock().insert(id.clone(), token.clone());
        self.set(id, StatusKind::Launching);
        let guard = StatusGuard {
            registry: self.clone(),
            id: id.clone(),
            launch: true,
        };
        (guard, token)
    }

    /// Cancel the launch of `id` while it is being prepared. Whether one was.
    pub fn cancel_launch(&self, id: &GameId) -> bool {
        match self.launches.lock().get(id) {
            Some(token) => {
                token.cancel();
                true
            }
            None => false,
        }
    }
}

pub struct StatusGuard {
    registry: StatusRegistry,
    id: GameId,
    /// Holds the launch token of [`StatusRegistry::launch_guard`].
    launch: bool,
}

impl StatusGuard {
    pub fn set(&self, kind: StatusKind) {
        self.registry.set(&self.id, kind);
    }
}

impl Drop for StatusGuard {
    fn drop(&mut self) {
        if self.launch {
            self.registry.launches.lock().remove(&self.id);
        }
        self.registry.clear(&self.id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use irori_core::Runner;

    #[test]
    fn a_launch_can_be_cancelled_only_while_it_is_prepared() {
        let statuses = StatusRegistry::new(EventBus::new());
        let id = GameId::new(Runner::Gog, "1");
        assert!(!statuses.cancel_launch(&id), "nothing to cancel");
        let (guard, token) = statuses.launch_guard(&id);
        assert_eq!(
            statuses.get(&id).map(|s| s.kind),
            Some(StatusKind::Launching)
        );
        assert!(statuses.cancel_launch(&id));
        assert!(token.is_cancelled());
        drop(guard);
        assert!(statuses.get(&id).is_none());
        assert!(!statuses.cancel_launch(&id), "the launch is over");
    }
}
