//! `--wait`: a launch that lasts as long as the game's session, for programs
//! that follow the process they start, such as Sunshine streaming a game to
//! Moonlight.
//!
//! The waiting invocation hands its request to the running Irori, starting
//! one first (outside the caller's process group or job, so ending the
//! caller leaves Irori running), and exits when the session ends: 0 when the
//! game ran, 1 with the reason on stderr when it couldn't start. Ending the
//! waiting invocation first (Moonlight's Quit) stops the game, or leaves Big
//! Picture for `--big-picture --wait`.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, bail};
use futures::channel::mpsc;
use gpui_kit::App;
use gpui_kit::component::WindowExt as _;
use gpui_kit::component::notification::Notification;
use irori_core::{Event, GameId};
use irori_ui::i18n::t;
use irori_ui::{Route, nav};
use parking_lot::Mutex;

use crate::cli::Cli;
use crate::instance::{self, Waiter};
use crate::{AppHandles, ShellCommand, open_main_window};

/// How long a game whose launch asked the user something (saves in conflict,
/// a missing emulator) may take to start before the wait gives up.
const START_WINDOW: Duration = Duration::from_secs(5 * 60);
/// How long a freshly started Irori gets to listen.
const STARTUP: Duration = Duration::from_secs(90);

/// Run a waiting invocation; returns the exit status.
pub fn run(args: &[String]) -> i32 {
    let outcome = forward(args)
        .and_then(|stream| instance::read_outcome(stream).map_err(|e| anyhow::anyhow!(e)));
    match outcome {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("irori: {e:#}");
            1
        }
    }
}

/// Hand `args` to the running Irori, starting it when none runs, with the
/// Moonlight client Sunshine started this for.
fn forward(args: &[String]) -> Result<std::net::TcpStream> {
    let client = irori_services::StreamClient::from_env(|name| std::env::var(name).ok());
    match instance::acquire_waiting(args, client.clone())? {
        Ok(stream) => return Ok(stream),
        // Let go at once: the Irori started next takes the lock.
        Err(primary) => drop(primary),
    }
    start_detached()?;
    let deadline = Instant::now() + STARTUP;
    loop {
        std::thread::sleep(Duration::from_millis(500));
        match instance::acquire_waiting(args, client.clone())? {
            Ok(stream) => return Ok(stream),
            Err(primary) => drop(primary),
        }
        if Instant::now() > deadline {
            bail!("Irori didn't start");
        }
    }
}

/// Start Irori in the tray, in a process group (on Windows, outside the job)
/// of its own: Sunshine ends the whole group or job of what it started, and
/// Irori must outlive the stream.
fn start_detached() -> Result<()> {
    let program = std::env::var_os("APPIMAGE")
        .filter(|p| !p.is_empty())
        .map(PathBuf::from)
        .map_or_else(std::env::current_exe, Ok)
        .context("Irori couldn't find its own program")?;
    let mut command = std::process::Command::new(program);
    command
        .arg("--minimized")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        command.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
        command.creation_flags(CREATE_NEW_PROCESS_GROUP | CREATE_BREAKAWAY_FROM_JOB);
        if command.spawn().is_ok() {
            return Ok(());
        }
        // A job that forbids leaving it: Irori then ends with the stream.
        command.creation_flags(CREATE_NEW_PROCESS_GROUP);
    }
    command.spawn().context("Could not start Irori")?;
    Ok(())
}

/// One waiting invocation, answered once.
struct Wait {
    waiter: Arc<Waiter>,
    done: AtomicBool,
}

impl Wait {
    fn finish(&self, error: Option<&str>) {
        if !self.done.swap(true, Ordering::SeqCst) {
            self.waiter.finish(error);
        }
    }

    fn is_done(&self) -> bool {
        self.done.load(Ordering::SeqCst)
    }
}

/// Serve a waiting invocation in the running Irori.
pub fn serve(
    cli: &Cli,
    waiter: Arc<Waiter>,
    app: &Arc<Mutex<AppHandles>>,
    tx: mpsc::UnboundedSender<ShellCommand>,
    cx: &mut App,
) {
    let wait = Arc::new(Wait {
        waiter,
        done: AtomicBool::new(false),
    });
    if let Some(id) = &cli.launch {
        game(id.clone(), wait, app, cx);
    } else if cli.big_picture {
        big_picture(wait, app, tx, cx);
    } else {
        wait.finish(Some(
            "--wait needs --launch <runner>/<app> or --big-picture",
        ));
    }
}

/// Launch `id` (unless it runs already), answer when its session ends, and
/// stop it when the waiter goes first.
fn game(id: GameId, wait: Arc<Wait>, app: &Arc<Mutex<AppHandles>>, cx: &mut App) {
    let b = irori_ui::backend(cx);
    let Some(game) = b.game(&id) else {
        wait.finish(Some(&format!("Irori has no game {}", id.key())));
        return;
    };
    if !game.is_installed {
        wait.finish(Some(&format!("{} isn't installed", game.title)));
        return;
    }
    // Listen before launching, so an exit right after the start counts.
    let mut events = b.bus.subscribe();
    let started = Instant::now();
    {
        let (b, id, wait, title) = (b.clone(), id.clone(), wait.clone(), game.title.clone());
        b.clone().spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs(5));
            loop {
                tokio::select! {
                    event = events.recv() => match event {
                        Ok(Event::GameExited { id: exited, crashed, .. }) if exited == id => {
                            let error = crashed.then(|| format!("{title} stopped right after it started"));
                            wait.finish(error.as_deref());
                            return;
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
                        _ => {}
                    },
                    _ = tick.tick() => {
                        if wait.is_done() {
                            return;
                        }
                        if !b.is_running(&id) && started.elapsed() > START_WINDOW {
                            wait.finish(Some(&format!("{title} didn't start")));
                            return;
                        }
                    }
                }
            }
        });
    }
    {
        let (b, id, wait) = (b.clone(), id.clone(), wait.clone());
        std::thread::Builder::new()
            .name("irori-wait".into())
            .spawn(move || {
                wait.waiter.wait_for_hangup();
                if !wait.is_done() {
                    wait.done.store(true, Ordering::SeqCst);
                    tracing::info!("the program waiting for {id} ended; stopping the game");
                    let stop = b.clone();
                    b.spawn(async move {
                        if let Err(e) = stop.stop(&id).await {
                            tracing::warn!("stopping {id}: {e:#}");
                        }
                    });
                }
            })
            .ok();
    }
    if b.is_running(&id) {
        return;
    }
    let window = app.lock().window;
    let params = irori_services::LaunchParams {
        stream: wait.waiter.client().cloned(),
        ..Default::default()
    };
    let task = irori_ui::run(cx, {
        let (b, id) = (b.clone(), id.clone());
        async move { b.launch_with(&id, params).await }
    });
    cx.spawn(async move |cx| {
        let Err(e) = task.await else { return };
        cx.update(|cx| {
            let asked = window.is_some_and(|handle| {
                handle
                    .update(cx, |_, window, cx| {
                        let asked = irori_ui::save_conflict::resolve_launch_error(
                            &id,
                            Default::default(),
                            &e,
                            window,
                            cx,
                        );
                        if !asked {
                            window.push_notification(
                                Notification::error(format!("{e:#}"))
                                    .title(t("shell.launch_failed")),
                                cx,
                            );
                        }
                        asked
                    })
                    .unwrap_or(false)
            });
            // A question on screen may still start the game; the tick above
            // gives up once the start window passes.
            if !asked {
                wait.finish(Some(&format!("{e:#}")));
            }
        });
    })
    .detach();
}

/// Show Big Picture, answer when the user leaves it, and leave it when the
/// waiter goes first.
fn big_picture(
    wait: Arc<Wait>,
    app: &Arc<Mutex<AppHandles>>,
    tx: mpsc::UnboundedSender<ShellCommand>,
    cx: &mut App,
) {
    open_main_window(app, cx);
    nav::navigate(Route::BigPicture, cx);
    let navigator = nav::navigator(cx);
    let watching: Rc<RefCell<Option<gpui_kit::Subscription>>> = Rc::default();
    let left = wait.clone();
    let holder = watching.clone();
    *watching.borrow_mut() = Some(cx.observe(&navigator, move |navigator, cx| {
        if navigator.read(cx).current() != &Route::BigPicture {
            left.finish(None);
            // Dropped once this callback returns.
            holder.borrow_mut().take();
        }
    }));
    std::thread::Builder::new()
        .name("irori-wait".into())
        .spawn(move || {
            wait.waiter.wait_for_hangup();
            if !wait.is_done() {
                wait.done.store(true, Ordering::SeqCst);
                let _ = tx.unbounded_send(ShellCommand::LeaveBigPicture);
            }
        })
        .ok();
}
