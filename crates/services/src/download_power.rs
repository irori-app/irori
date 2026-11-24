//! Sleep or power off once the download queue is done, for a night's
//! downloads. The choice lasts for this run only and is used once: a
//! forgotten "power off" must never surprise anyone days later.
//!
//! When the queue empties the computer isn't asked at once: a countdown of
//! [`POWER_COUNTDOWN`] runs first, which every screen shows with a way to
//! cancel, and nothing is asked of the user meanwhile. It is never done while
//! a game runs or a session is still ending: a game playing when the queue
//! empties, or when the countdown ends, leaves the computer on and says so.

use std::time::{Duration, Instant};

use irori_core::process::CancellationToken;
use irori_core::{Event, Notice, NoticeLevel};

use crate::{Backend, PowerAction};

/// How long the computer waits after the queue is done before acting.
pub const POWER_COUNTDOWN: Duration = Duration::from_secs(60);

/// A countdown to sleeping or powering off, as screens show it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PowerCountdown {
    pub action: PowerAction,
    /// When the computer is asked.
    pub at: Instant,
}

/// What a finished queue does, and the countdown under way.
#[derive(Default)]
pub(crate) struct WhenDone {
    pub(crate) choice: Option<PowerAction>,
    countdown: Option<(PowerCountdown, CancellationToken)>,
    /// The countdown's length; shorter in tests.
    length: Option<Duration>,
}

impl WhenDone {
    pub(crate) fn countdown(&self) -> Option<PowerCountdown> {
        self.countdown.as_ref().map(|(c, _)| *c)
    }

    /// Stop the countdown under way: the queue got work again, or the user
    /// said no. The choice stays for the next time the queue is done.
    pub(crate) fn stop_countdown(&mut self) {
        if let Some((_, cancel)) = self.countdown.take() {
            cancel.cancel();
        }
    }
}

/// Whether a queue that just ran an item starts counting down: a choice is
/// made, nothing waits or is paused, and no countdown runs yet.
pub(crate) fn countdown_due(
    choice: Option<PowerAction>,
    queue_empty: bool,
    paused: bool,
    counting: bool,
) -> bool {
    choice.is_some() && queue_empty && !paused && !counting
}

/// Translation keys of the notices about `action`: (soon, left on because a
/// game runs, failed).
fn notice_keys(action: PowerAction) -> (&'static str, &'static str, &'static str) {
    match action {
        PowerAction::PowerOff => (
            "shell.notice.power_off_soon",
            "shell.notice.power_off_skipped",
            "shell.notice.power_off_failed",
        ),
        _ => (
            "shell.notice.sleep_soon",
            "shell.notice.sleep_skipped",
            "shell.notice.sleep_failed",
        ),
    }
}

/// The notice's title for every notice about a finished queue.
pub const DOWNLOADS_FINISHED_NOTICE: &str = "shell.notice.downloads_finished";

impl Backend {
    /// What to do with the computer once the download queue is done:
    /// `Some(Sleep)` or `Some(PowerOff)`, or `None` to do nothing, which
    /// also cancels a countdown under way.
    pub fn set_when_downloads_done(&self, choice: Option<PowerAction>) {
        {
            let mut q = self.downloads.lock();
            q.when_done.choice = choice;
            match choice {
                None => q.when_done.stop_countdown(),
                // Switching between sleep and power off keeps the time.
                Some(action) => {
                    if let Some((c, _)) = q.when_done.countdown.as_mut() {
                        c.action = action;
                    }
                }
            }
        }
        self.bus.emit(Event::DownloadQueueChanged);
    }

    /// Count down from `length` instead of [`POWER_COUNTDOWN`].
    #[cfg(test)]
    pub(crate) fn set_power_countdown(&self, length: Duration) {
        self.downloads.lock().when_done.length = Some(length);
    }

    /// After the worker ran an item: start the countdown when the queue is
    /// done and a choice is made.
    pub(crate) fn queue_ran(&self) {
        let (action, length) = {
            let q = self.downloads.lock();
            let w = &q.when_done;
            if !countdown_due(
                w.choice,
                q.active.is_empty(),
                q.paused,
                w.countdown.is_some(),
            ) {
                return;
            }
            let Some(action) = w.choice else { return };
            (action, w.length.unwrap_or(POWER_COUNTDOWN))
        };
        let (soon, skipped, _) = notice_keys(action);
        if self.game_running() {
            self.downloads.lock().when_done.choice = None;
            self.finished_notice(NoticeLevel::Warning, skipped);
            self.bus.emit(Event::DownloadQueueChanged);
            return;
        }
        let cancel = CancellationToken::new();
        self.downloads.lock().when_done.countdown = Some((
            PowerCountdown {
                action,
                at: Instant::now() + length,
            },
            cancel.clone(),
        ));
        self.finished_notice(NoticeLevel::Info, soon);
        self.bus.emit(Event::DownloadQueueChanged);
        let b = self.clone();
        self.spawn(async move {
            tokio::select! {
                () = cancel.cancelled() => {}
                () = tokio::time::sleep(length) => b.countdown_ended(&cancel).await,
            }
        });
    }

    /// The countdown ran out: act, unless it was cancelled meanwhile or a
    /// game runs now. Either way the choice is used up.
    async fn countdown_ended(&self, cancel: &CancellationToken) {
        let action = {
            let mut q = self.downloads.lock();
            let w = &mut q.when_done;
            if cancel.is_cancelled() || w.countdown.is_none() {
                return;
            }
            let action = w.countdown.take().map(|(c, _)| c.action);
            w.choice = None;
            action
        };
        self.bus.emit(Event::DownloadQueueChanged);
        let Some(action) = action else { return };
        let (_, skipped, failed) = notice_keys(action);
        match self.power_when_idle(action).await {
            Ok(true) => {}
            Ok(false) => self.finished_notice(NoticeLevel::Warning, skipped),
            Err(e) => {
                tracing::warn!("{action:?} after the downloads: {e:#}");
                self.bus.notice(
                    Notice::new(NoticeLevel::Error, "The computer was left on")
                        .keys(failed, None, &[])
                        .message(format!("{e:#}")),
                );
            }
        }
    }

    fn finished_notice(&self, level: NoticeLevel, message: &'static str) {
        self.bus
            .notice(Notice::new(level, "Downloads finished").keys(
                DOWNLOADS_FINISHED_NOTICE,
                Some(message),
                &[],
            ));
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use futures::future::BoxFuture;
    use parking_lot::Mutex;

    use super::*;
    use crate::power::{Permission, PowerSystem};

    #[test]
    fn counts_down_only_for_a_finished_unpaused_queue() {
        let sleep = Some(PowerAction::Sleep);
        assert!(countdown_due(sleep, true, false, false));
        assert!(!countdown_due(None, true, false, false), "no choice");
        assert!(!countdown_due(sleep, false, false, false), "work left");
        assert!(!countdown_due(sleep, true, true, false), "paused");
        assert!(!countdown_due(sleep, true, false, true), "already counting");
    }

    #[derive(Default)]
    struct FakeSystem {
        done: Mutex<Vec<PowerAction>>,
    }

    impl PowerSystem for FakeSystem {
        fn permissions(&self) -> BoxFuture<'_, Vec<(PowerAction, Permission)>> {
            Box::pin(async { Vec::new() })
        }

        fn perform(&self, action: PowerAction) -> BoxFuture<'_, anyhow::Result<()>> {
            self.done.lock().push(action);
            Box::pin(async { Ok(()) })
        }
    }

    fn backend() -> (Backend, Arc<FakeSystem>) {
        crate::tests::isolate_paths();
        let b = Backend::start_headless(|_, _| Vec::new()).unwrap();
        let system = Arc::new(FakeSystem::default());
        b.set_power_system(system.clone());
        b.set_power_countdown(Duration::from_millis(100));
        (b, system)
    }

    fn settle() {
        std::thread::sleep(Duration::from_millis(400));
    }

    #[test]
    fn acts_once_after_the_countdown() {
        let (b, system) = backend();
        b.set_when_downloads_done(Some(PowerAction::Sleep));
        b.queue_ran();
        let countdown = b.downloads().countdown.expect("counting down");
        assert_eq!(countdown.action, PowerAction::Sleep);
        assert!(
            system.done.lock().is_empty(),
            "nothing before the countdown ends"
        );
        settle();
        assert_eq!(*system.done.lock(), [PowerAction::Sleep]);
        let after = b.downloads();
        assert_eq!(after.countdown, None);
        assert_eq!(after.when_done, None, "the choice is used once");
        b.queue_ran();
        settle();
        assert_eq!(system.done.lock().len(), 1);
    }

    #[test]
    fn cancelling_keeps_the_computer_on() {
        let (b, system) = backend();
        b.set_when_downloads_done(Some(PowerAction::PowerOff));
        b.queue_ran();
        assert!(b.downloads().countdown.is_some());
        b.set_when_downloads_done(None);
        assert_eq!(b.downloads().countdown, None);
        settle();
        assert!(system.done.lock().is_empty());
    }

    #[test]
    fn never_while_a_game_runs() {
        let (b, system) = backend();
        b.set_when_downloads_done(Some(PowerAction::PowerOff));
        // A game running when the queue empties: no countdown at all.
        let session = crate::power::OpenSession::new(&b);
        b.queue_ran();
        assert_eq!(b.downloads().countdown, None);
        assert_eq!(b.downloads().when_done, None);
        drop(session);
        // One starting during the countdown: nothing when it ends.
        b.set_when_downloads_done(Some(PowerAction::Sleep));
        b.queue_ran();
        let session = crate::power::OpenSession::new(&b);
        settle();
        assert!(system.done.lock().is_empty());
        assert_eq!(b.downloads().countdown, None);
        drop(session);
    }
}
