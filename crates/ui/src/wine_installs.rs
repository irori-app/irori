//! Wine and Proton builds being installed through the Wine manager, kept in
//! a global entity so any screen can start one and show its progress: the
//! Wine manager's rows, and the install sheet's "Download GE-Proton". An
//! install keeps its progress and cancel handle when the screen that started
//! it closes, and one build is never installed twice at once.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use gpui_kit::{App, AppContext as _, Entity, Global};
use irori_core::process::CancellationToken;
use irori_core::{Event, InstallProgress, Notice, NoticeLevel, OpContext, WineInstallation};
use irori_runtime::manager::{self, Release};

use crate::backend;
use crate::i18n::t_args;

/// Stable key of a release across list refreshes.
pub fn release_key(r: &Release) -> String {
    format!("{}:{}", r.kind, r.version)
}

/// Live state of one install, written from the backend runtime.
#[derive(Debug, Clone, Default)]
pub struct OpLive {
    pub progress: Option<InstallProgress>,
    pub step: Option<String>,
}

impl OpLive {
    /// The archive is being unpacked (the step names it so).
    pub fn extracting(&self) -> bool {
        self.step
            .as_deref()
            .is_some_and(|s| s.starts_with("Extracting"))
    }
}

pub struct WineOp {
    pub version: String,
    pub live: Arc<Mutex<OpLive>>,
    pub cancel: CancellationToken,
}

impl WineOp {
    pub fn snapshot(&self) -> OpLive {
        self.live.lock().map(|l| l.clone()).unwrap_or_default()
    }
}

/// How an install ended.
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    Installed(WineInstallation),
    Failed(String),
    Cancelled,
}

/// Installs in flight by [`release_key`], and how finished ones ended.
/// `revision` bumps when one ends so views reload their release lists.
#[derive(Default)]
pub struct WineOps {
    pub running: HashMap<String, WineOp>,
    pub finished: HashMap<String, Outcome>,
    pub revision: u64,
}

struct WineOpsHandle(Entity<WineOps>);
impl Global for WineOpsHandle {}

/// The installs entity, created on first use.
pub fn wine_ops(cx: &mut App) -> Entity<WineOps> {
    if let Some(h) = cx.try_global::<WineOpsHandle>() {
        return h.0.clone();
    }
    let e = cx.new(|_| WineOps::default());
    cx.set_global(WineOpsHandle(e.clone()));
    e
}

/// Start installing (or updating) `release`; returns its key. Nothing new
/// starts while it is already installing. The outcome is reported through a
/// backend notice, so it reaches the user even when the screen is closed.
/// With `make_default`, the build becomes the default Wine for every game
/// when the current default can't be used (there was none).
pub fn start_install(release: Release, make_default: bool, cx: &mut App) -> String {
    let ops = wine_ops(cx);
    let key = release_key(&release);
    if ops.read(cx).running.contains_key(&key) {
        return key;
    }
    let live = Arc::new(Mutex::new(OpLive::default()));
    let cancel = CancellationToken::new();
    let (progress_live, step_live) = (live.clone(), live.clone());
    let ctx = OpContext {
        progress: Arc::new(move |p| {
            if let Ok(mut l) = progress_live.lock() {
                l.progress = Some(p);
            }
        }),
        cancel: cancel.clone(),
        step: Arc::new(move |s| {
            if let Ok(mut l) = step_live.lock() {
                l.step = Some(s.to_string());
            }
        }),
        log_file: None,
        settings: None,
    };
    let version = release.version.clone();
    ops.update(cx, |o, cx| {
        o.finished.remove(&key);
        o.running.insert(
            key.clone(),
            WineOp {
                version: version.clone(),
                live,
                cancel: cancel.clone(),
            },
        );
        cx.notify();
    });
    let b = backend(cx);
    let app = b.settings.app();
    let task = crate::run(cx, async move {
        let result = manager::install(&release, &app, &ctx).await;
        let outcome = match result {
            Ok(installed) => match installed.installation() {
                Some(wine) => {
                    if make_default
                        && !irori_runtime::discovery::is_usable(&b.settings.app().game.wine_version)
                    {
                        let (chosen, writer) = (wine.clone(), b.clone());
                        let _ = b
                            .spawn_blocking(move || {
                                writer
                                    .settings
                                    .update_app(move |s| s.game.wine_version = chosen)
                            })
                            .await;
                    }
                    b.bus.notice(Notice::new(
                        NoticeLevel::Success,
                        t_args("ui.wine.installed", &[("version", &version)]).to_string(),
                    ));
                    Outcome::Installed(wine)
                }
                None => Outcome::Failed("the build could not be found after installing".into()),
            },
            Err(_) if ctx.cancel.is_cancelled() => Outcome::Cancelled,
            Err(e) => Outcome::Failed(format!("{e:#}")),
        };
        if let Outcome::Failed(message) = &outcome {
            b.bus.notice(
                Notice::new(
                    NoticeLevel::Error,
                    t_args("ui.wine.install_failed", &[("version", &version)]).to_string(),
                )
                .message(message.clone()),
            );
        }
        b.bus.emit(Event::WineVersionsChanged);
        Ok(outcome)
    });
    let done_key = key.clone();
    cx.spawn(async move |cx| {
        let outcome = task.await.unwrap_or(Outcome::Cancelled);
        ops.update(cx, |o, cx| {
            o.running.remove(&done_key);
            o.finished.insert(done_key, outcome);
            o.revision += 1;
            cx.notify();
        });
    })
    .detach();
    key
}

/// Cancel a running install; partial files are cleaned up by the runtime.
pub fn cancel_install(key: &str, cx: &mut App) {
    if let Some(op) = wine_ops(cx).read(cx).running.get(key) {
        op.cancel.cancel();
    }
}
