//! "Open" an emulator without a game: from its card on the Emulators page
//! and from the command palette. The palette builds its rows without
//! waiting, so the emulators that can open are kept here, read again in
//! the background when the page reads them or the list is older than a
//! minute; nothing is read while nobody asks.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use gpui_kit::{App, AppContext as _, Entity, Global, SharedString, Window};
use irori_engines::open::can_open;
use irori_services::EngineStatus;
use irori_ui::backend;
use irori_ui::i18n::{t, t_args};

use crate::common::{notify_error, notify_success};

/// How long the palette's list stands before it is read again.
const FRESH_FOR: Duration = Duration::from_secs(60);

/// An emulator installed on this computer that has a window to open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenableEngine {
    pub id: String,
    pub name: SharedString,
}

/// The emulators that open, and the ones being started.
#[derive(Default)]
pub(crate) struct Openable {
    list: Vec<OpenableEngine>,
    read_at: Option<Instant>,
    reading: bool,
    /// Engines whose start is under way (their Open button spins).
    pub opening: HashSet<String>,
}

struct OpenableHandle(Entity<Openable>);
impl Global for OpenableHandle {}

pub(crate) fn openable(cx: &mut App) -> Entity<Openable> {
    if let Some(h) = cx.try_global::<OpenableHandle>() {
        return h.0.clone();
    }
    let e = cx.new(|_| Openable::default());
    cx.set_global(OpenableHandle(e.clone()));
    e
}

/// The installed emulators that open, from `statuses`, by name.
pub(crate) fn openable_of(statuses: &[EngineStatus]) -> Vec<OpenableEngine> {
    let mut list: Vec<OpenableEngine> = statuses
        .iter()
        .filter(|s| s.is_installed() && can_open(&s.manifest))
        .map(|s| OpenableEngine {
            id: s.manifest.id.clone(),
            name: s.manifest.name.clone().into(),
        })
        .collect();
    list.sort_by_key(|e| e.name.to_lowercase());
    list
}

/// Keep the list the Emulators page just read.
pub(crate) fn remember(statuses: &[EngineStatus], cx: &mut App) {
    let list = openable_of(statuses);
    openable(cx).update(cx, |o, cx| {
        o.list = list;
        o.read_at = Some(Instant::now());
        cx.notify();
    });
}

/// The emulators the palette offers to open. Answers from memory at once
/// and reads them again in the background when the list is stale, so the
/// next look is current.
pub fn openable_engines(cx: &mut App) -> Vec<OpenableEngine> {
    let entity = openable(cx);
    let (list, stale) = {
        let o = entity.read(cx);
        let stale = !o.reading && o.read_at.is_none_or(|at| at.elapsed() > FRESH_FOR);
        (o.list.clone(), stale)
    };
    if stale {
        entity.update(cx, |o, _| o.reading = true);
        let b = backend(cx);
        let task = irori_ui::run(cx, async move { Ok(b.engine_statuses().await) });
        cx.spawn(async move |cx| {
            let statuses = task.await.unwrap_or_default();
            cx.update(|cx| {
                entity.update(cx, |o, cx| {
                    o.reading = false;
                    o.list = openable_of(&statuses);
                    o.read_at = Some(Instant::now());
                    cx.notify();
                })
            });
        })
        .detach();
    }
    list
}

/// Start the emulator `id` without a game, saying when it started or why
/// it couldn't.
pub fn open_engine(id: String, name: SharedString, window: &mut Window, cx: &mut App) {
    crate::i18n::register();
    let entity = openable(cx);
    let started = entity.update(cx, |o, cx| {
        cx.notify();
        o.opening.insert(id.clone())
    });
    if !started {
        return;
    }
    let b = backend(cx);
    let engine = id.clone();
    let task = irori_ui::run(cx, async move { b.open_engine(&engine).await });
    window
        .spawn(cx, async move |cx| {
            let result = task.await;
            let _ = cx.update(|window, cx| {
                entity.update(cx, |o, cx| {
                    o.opening.remove(&id);
                    cx.notify();
                });
                match result {
                    Ok(()) => notify_success(
                        t_args("manage.emulators.open.started", &[("engine", &name)]),
                        window,
                        cx,
                    ),
                    Err(e) => notify_error(t("manage.emulators.open.failed"), &e, window, cx),
                }
            });
        })
        .detach();
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use irori_engines::{Installation, Origin};

    use super::*;

    fn status(id: &str, installed: bool) -> EngineStatus {
        let (_, json) = irori_engines::BUILT_IN
            .iter()
            .find(|(built_in, _)| *built_in == id)
            .expect("built-in engine");
        let manifest = Arc::new(irori_engines::EngineManifest::parse(json).unwrap());
        EngineStatus {
            installations: if installed {
                vec![Installation {
                    engine: id.into(),
                    origin: Origin::System,
                    version: None,
                    program: format!("/usr/bin/{id}").into(),
                    flatpak: None,
                    dir: None,
                    size: 0,
                }]
            } else {
                Vec::new()
            },
            default: None,
            cores: Vec::new(),
            manifest,
        }
    }

    #[test]
    fn only_installed_emulators_with_a_window_open() {
        let list = openable_of(&[
            status("ppsspp", true),
            status("dolphin", false),
            status("anyps5", true),
            status("duckstation", true),
        ]);
        let ids: Vec<&str> = list.iter().map(|e| e.id.as_str()).collect();
        assert_eq!(ids, ["duckstation", "ppsspp"], "sorted by name");
    }
}
