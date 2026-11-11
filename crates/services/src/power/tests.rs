//! What is offered, and that the system is asked only once every game
//! session has ended. A fake system stands in, so nothing powers off.

use std::sync::Arc;

use parking_lot::Mutex;

use super::*;

/// Records what it was asked to do.
#[derive(Default)]
struct FakeSystem {
    answers: Vec<(PowerAction, Permission)>,
    done: Mutex<Vec<PowerAction>>,
}

impl PowerSystem for FakeSystem {
    fn permissions(&self) -> BoxFuture<'_, Vec<(PowerAction, Permission)>> {
        let answers = self.answers.clone();
        Box::pin(async move { answers })
    }

    fn perform(&self, action: PowerAction) -> BoxFuture<'_, Result<()>> {
        self.done.lock().push(action);
        Box::pin(async { Ok(()) })
    }
}

#[test]
fn reads_loginds_answers() {
    assert_eq!(Permission::from_logind("yes"), Permission::Yes);
    assert_eq!(
        Permission::from_logind("challenge\n"),
        Permission::Challenge
    );
    assert_eq!(Permission::from_logind("no"), Permission::No);
    assert_eq!(Permission::from_logind("na"), Permission::Unavailable);
    assert_eq!(
        Permission::from_logind("something-newer"),
        Permission::Unavailable
    );
}

#[test]
fn offers_what_the_system_allows_in_a_fixed_order() {
    use Permission::*;
    use PowerAction::*;
    let answers = [
        (LogOut, Yes),
        (Sleep, Unavailable),
        (Restart, Challenge),
        (PowerOff, Yes),
    ];
    assert_eq!(offered(&answers), [PowerOff, Restart, LogOut]);
    assert_eq!(
        offered(&[(PowerOff, No), (Restart, Unavailable)]),
        [],
        "refused and impossible actions are hidden"
    );
    assert_eq!(offered(&[]), [], "an action never answered for is hidden");
    assert_eq!(
        offered(&PowerAction::ALL.map(|a| (a, Yes))),
        PowerAction::ALL
    );
}

#[test]
fn macos_asks_system_events() {
    for action in PowerAction::ALL {
        assert!(apple_script(action).starts_with("tell application \"System Events\" to "));
    }
    assert!(apple_script(PowerAction::PowerOff).ends_with("shut down"));
    assert!(apple_script(PowerAction::LogOut).ends_with("log out"));
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap()
}

#[test]
fn acts_at_once_without_sessions() {
    let system = FakeSystem::default();
    let (_tx, rx) = watch::channel(0);
    runtime()
        .block_on(after_sessions(
            rx,
            Duration::from_secs(1),
            &system,
            PowerAction::Sleep,
        ))
        .unwrap();
    assert_eq!(*system.done.lock(), [PowerAction::Sleep]);
}

#[test]
fn waits_for_sessions_to_end_before_acting() {
    let system = Arc::new(FakeSystem::default());
    let (tx, rx) = watch::channel(2);
    let rt = runtime();
    let s = system.clone();
    let task = rt.spawn(async move {
        after_sessions(rx, Duration::from_secs(60), &*s, PowerAction::PowerOff).await
    });
    rt.block_on(async {
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(system.done.lock().is_empty(), "a session is still open");
        tx.send_modify(|n| *n -= 1);
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(system.done.lock().is_empty(), "the other one too");
        tx.send_modify(|n| *n -= 1);
    });
    rt.block_on(task).unwrap().unwrap();
    assert_eq!(*system.done.lock(), [PowerAction::PowerOff]);
}

#[test]
fn leaves_the_computer_on_when_a_game_never_closes() {
    let system = FakeSystem::default();
    let (_tx, rx) = watch::channel(1);
    let e = runtime()
        .block_on(after_sessions(
            rx,
            Duration::from_millis(100),
            &system,
            PowerAction::Restart,
        ))
        .unwrap_err();
    assert!(format!("{e}").contains("still closing"), "{e}");
    assert!(system.done.lock().is_empty());
}

#[test]
fn backend_waits_for_its_open_sessions() {
    crate::tests::isolate_paths();
    let b = Backend::start_headless(|_, _| Vec::new()).unwrap();
    let system = Arc::new(FakeSystem::default());
    let session = OpenSession::new(&b);
    assert_eq!(b.open_session_count(), 1);
    let (b2, s) = (b.clone(), system.clone());
    let task = b.spawn(async move {
        b2.power_with(&*s, PowerAction::LogOut, Duration::from_secs(60))
            .await
    });
    std::thread::sleep(std::time::Duration::from_millis(200));
    assert!(system.done.lock().is_empty(), "the session is still open");
    drop(session);
    assert_eq!(b.open_session_count(), 0);
    b.runtime().block_on(task).unwrap().unwrap();
    assert_eq!(*system.done.lock(), [PowerAction::LogOut]);
}
