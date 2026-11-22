//! Backend behaviour across stores, over an in-memory store.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Once};

use anyhow::{Result, bail};
use irori_core::{
    Event, ExtraInfo, GameInfo, InstallInfo, InstallInfoQuery, InstallPlatform, InstallRequest,
    LaunchPlan, LaunchRequest, LoginFlow, OpContext, Outcome, Paths, RemoveArgs, Runner,
    SessionExpired, Store, UpdateRequest, UserInfo, async_trait,
};

use crate::Backend;

/// Keep every file the backend writes in a private temporary directory.
///
/// Panics when another test has already resolved the real folders: the
/// tests would otherwise write to the user's own library and settings.
pub(crate) fn isolate_paths() {
    static ONCE: Once = Once::new();
    let root = std::env::temp_dir().join(format!("irori-services-{}", std::process::id()));
    ONCE.call_once(|| {
        let _ = std::fs::remove_dir_all(&root);
        Paths::init(Paths::under(&root));
    });
    assert!(
        Paths::get().config.starts_with(&root),
        "a test used Irori's folders before isolate_paths(); call it first there"
    );
}

/// A GOG-like store whose saved login can be refused and whose games can
/// have updates.
#[derive(Default)]
struct LoginStore {
    refused: AtomicBool,
    updates: parking_lot::Mutex<Vec<String>>,
    /// The games removed, and whether their files went too.
    removed: parking_lot::Mutex<Vec<(String, bool)>>,
}

#[async_trait]
impl Store for LoginStore {
    fn runner(&self) -> Runner {
        Runner::Gog
    }
    async fn init(&self) -> Result<()> {
        Ok(())
    }
    fn user(&self) -> Option<UserInfo> {
        Some(UserInfo::default())
    }
    fn login_flow(&self) -> LoginFlow {
        LoginFlow::None
    }
    async fn login(&self, _: &str) -> Result<UserInfo> {
        bail!("not supported")
    }
    async fn logout(&self) -> Result<()> {
        Ok(())
    }
    async fn refresh(&self, _: bool) -> Result<()> {
        if self.refused.load(Ordering::SeqCst) {
            return Err(anyhow::Error::new(SessionExpired::new(
                Runner::Gog,
                "GOG refused the login (401)",
            ))
            .context("Could not refresh the GOG library"));
        }
        Ok(())
    }
    fn games(&self) -> Vec<Arc<GameInfo>> {
        Vec::new()
    }
    fn game(&self, _: &str) -> Option<Arc<GameInfo>> {
        None
    }
    async fn extra_info(&self, _: &str) -> Result<ExtraInfo> {
        Ok(ExtraInfo::default())
    }
    async fn install_info(
        &self,
        _: &str,
        _: InstallPlatform,
        _: &InstallInfoQuery,
    ) -> Result<InstallInfo> {
        bail!("not supported")
    }
    async fn updatable_games(&self) -> Result<Vec<String>> {
        Ok(self.updates.lock().clone())
    }
    async fn install(&self, _: &str, _: &InstallRequest, _: OpContext) -> Result<Outcome> {
        bail!("not supported")
    }
    async fn update(&self, _: &str, _: &UpdateRequest, _: OpContext) -> Result<Outcome> {
        bail!("not supported")
    }
    async fn repair(&self, _: &str, _: OpContext) -> Result<Outcome> {
        bail!("not supported")
    }
    async fn uninstall(&self, app: &str, args: RemoveArgs) -> Result<()> {
        self.removed
            .lock()
            .push((app.to_string(), args.delete_files));
        Ok(())
    }
    async fn import(&self, _: &str, _: &Path, _: InstallPlatform) -> Result<()> {
        bail!("not supported")
    }
    async fn set_install_path(&self, _: &str, _: &Path) -> Result<()> {
        bail!("not supported")
    }
    async fn launch(&self, _: &str, _: &LaunchRequest, _: OpContext) -> Result<LaunchPlan> {
        bail!("not supported")
    }
}

#[test]
fn canceling_a_stopped_install_deletes_what_it_downloaded() {
    isolate_paths();
    let store = Arc::new(LoginStore::default());
    let dyn_store: irori_core::DynStore = store.clone();
    let b = Backend::start_headless(move |_, _| vec![dyn_store]).unwrap();
    // Paused, so the items wait as a paused download (or one a restart left
    // queued) does, with whatever it wrote already on disk.
    b.pause_downloads();
    let install = |app: &str| {
        let id = irori_core::GameId::new(Runner::Gog, app);
        b.enqueue_install(id.clone(), app.into(), InstallRequest::default(), 0);
        id
    };
    let (kept, deleted) = (install("kept"), install("deleted"));
    b.enqueue_update(
        irori_core::GameId::new(Runner::Gog, "updated"),
        "updated".into(),
        UpdateRequest::default(),
    );
    b.cancel_download(&kept, true);
    b.cancel_download(&deleted, false);
    b.cancel_download(&irori_core::GameId::new(Runner::Gog, "updated"), false);
    let removed = b.runtime().block_on(async {
        for _ in 0..200 {
            if !store.removed.lock().is_empty() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        store.removed.lock().clone()
    });
    assert_eq!(removed, [("deleted".to_string(), true)]);
    b.resume_downloads();
}

/// Title keys of the notices sent on `rx` so far.
fn notices(rx: &mut tokio::sync::broadcast::Receiver<Event>) -> Vec<&'static str> {
    let mut keys = Vec::new();
    while let Ok(event) = rx.try_recv() {
        if let Event::Notice(notice) = event
            && let Some(key) = notice.title_key
        {
            keys.push(key);
        }
    }
    keys
}

#[test]
fn asks_once_to_log_in_again_and_forgets_after_a_good_refresh() {
    isolate_paths();
    let store = Arc::new(LoginStore::default());
    let dyn_store: irori_core::DynStore = store.clone();
    let b = Backend::start_headless(move |_, _| vec![dyn_store]).unwrap();
    let mut rx = b.subscribe();

    store.refused.store(true, Ordering::SeqCst);
    for _ in 0..2 {
        let e = b
            .runtime()
            .block_on(b.refresh_store(Runner::Gog, false))
            .unwrap_err();
        assert!(SessionExpired::of(&e).is_some());
    }
    assert_eq!(
        b.expired_logins().get(&Runner::Gog).map(String::as_str),
        Some("GOG refused the login (401)")
    );
    assert_eq!(
        notices(&mut rx),
        ["shell.notice.login_expired"],
        "told once, not on every refresh"
    );

    store.refused.store(false, Ordering::SeqCst);
    b.runtime()
        .block_on(b.refresh_store(Runner::Gog, false))
        .unwrap();
    assert!(b.expired_logins().is_empty());
}

#[test]
fn tells_about_updates_released_while_running() {
    isolate_paths();
    let store = Arc::new(LoginStore::default());
    let dyn_store: irori_core::DynStore = store.clone();
    let b = Backend::start_headless(move |_, _| vec![dyn_store]).unwrap();
    let rt = b.runtime();
    // The first check of a session only fills the badges.
    *store.updates.lock() = vec!["a".into()];
    rt.block_on(b.refresh_updates());
    let mut rx = b.subscribe();
    rt.block_on(b.refresh_updates());
    assert!(notices(&mut rx).is_empty(), "nothing new");
    *store.updates.lock() = vec!["a".into(), "b".into()];
    rt.block_on(b.refresh_updates());
    assert_eq!(notices(&mut rx), ["shell.notice.update_available"]);
    *store.updates.lock() = vec!["a".into(), "b".into(), "c".into(), "d".into()];
    rt.block_on(b.refresh_updates());
    assert_eq!(notices(&mut rx), ["shell.notice.updates_available"]);
}

#[test]
fn never_touches_files_of_games_steam_manages() {
    isolate_paths();
    let b = Backend::start_headless(|_, _| Vec::new()).unwrap();
    let id = irori_core::GameId::new(Runner::Steam, "620");
    let dir = tempfile::tempdir().unwrap();
    let rt = b.runtime();
    let refusals = [
        rt.block_on(b.move_install(&id, dir.path().to_path_buf())),
        rt.block_on(b.uninstall(&id, RemoveArgs::default())),
        rt.block_on(b.repair(&id)),
        rt.block_on(b.set_install_path(&id, dir.path())),
        rt.block_on(b.import(&id, dir.path(), InstallPlatform::Windows)),
    ];
    for result in refusals {
        let e = result.unwrap_err().to_string();
        assert!(e.contains("Steam installs, updates and removes"), "{e}");
    }
}

const MENU_SCRIPT: &str = r#"
name: Test Game
game_slug: test-game
version: Local
slug: test-game-local
runner: linux
script:
  game:
    exe: $GAMEDIR/lang.cfg
  installer:
  - input_menu:
      id: lang
      description: Choose the language
      options:
      - en: English
      - de: Deutsch
      preselect: en
  - write_file:
      file: $GAMEDIR/lang.cfg
      content: lang=$INPUT_lang
"#;

fn script_install(game_dir: &Path) -> crate::ScriptInstall {
    let script = irori_scripts::Script::parse(MENU_SCRIPT).unwrap();
    let ctx = crate::scripts::script_context(&Default::default(), &script, game_dir);
    let approvals = irori_scripts::Approvals::all(&irori_scripts::consent(&script, &ctx));
    crate::ScriptInstall {
        script: Arc::new(script),
        game_dir: game_dir.to_path_buf(),
        approvals,
        picked: Default::default(),
        source_url: Some("file:///scripts/test-game.yml".into()),
        resolution: None,
        sandbox: Default::default(),
    }
}

#[test]
fn script_installs_ask_the_ui_and_produce_a_library_entry() {
    use futures::StreamExt as _;
    isolate_paths();
    let b = Backend::start_headless(|_, _| Vec::new()).unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let game_dir = tmp.path().join("test-game");
    let mut run = b.start_script_install(script_install(&game_dir));
    let rt = b.runtime();
    match rt.block_on(run.prompts.next()) {
        Some(crate::ScriptPrompt::Menu { request, reply }) => {
            assert_eq!(request.preselect.as_deref(), Some("en"));
            reply.send(Some("de".into())).unwrap();
        }
        other => panic!("expected the language menu, got {other:?}"),
    }
    let outcome = rt.block_on(run.result).unwrap().unwrap();
    assert_eq!(
        std::fs::read_to_string(game_dir.join("lang.cfg")).unwrap(),
        "lang=de"
    );
    let progress = run.progress.lock().clone();
    assert_eq!(progress.step.map(|s| (s.number, s.total)), Some((2, 2)));
    assert!(progress.wine.is_none(), "a Linux script uses no Wine");
    assert!(run.log_file.is_file());

    let game =
        crate::scripts::script_game(&outcome, &game_dir, None, None, Default::default()).unwrap();
    assert!(matches!(
        game.entry,
        crate::scripts::ScriptLibraryEntry::App { windows: false, .. }
    ));
    let record = game
        .settings
        .iter()
        .find(|(k, _)| *k == "iroriLutrisScript")
        .map(|(_, v)| v.clone())
        .unwrap();
    assert_eq!(record["slug"], "test-game");
    assert_eq!(record["sourceUrl"], "file:///scripts/test-game.yml");
}

#[test]
fn unanswered_script_questions_cancel_and_roll_back() {
    use futures::StreamExt as _;
    isolate_paths();
    let b = Backend::start_headless(|_, _| Vec::new()).unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let game_dir = tmp.path().join("test-game");
    let mut run = b.start_script_install(script_install(&game_dir));
    let rt = b.runtime();
    // The window closed the question: its reply sender is dropped.
    drop(rt.block_on(run.prompts.next()));
    let e = rt.block_on(run.result).unwrap().unwrap_err();
    assert!(irori_runtime::is_cancelled(&e), "{e:#}");
    assert!(!game_dir.exists());
    assert!(run.progress.lock().rolling_back);
}

/// An added Linux game that runs a shell command.
#[cfg(unix)]
pub(crate) struct PlayStore {
    pub(crate) game: Arc<GameInfo>,
    pub(crate) command: irori_core::process::CommandSpec,
    /// Has cloud saves, and the cloud can't be reached.
    pub(crate) saves_fail: bool,
}

#[cfg(unix)]
#[async_trait]
impl Store for PlayStore {
    fn runner(&self) -> Runner {
        Runner::Sideload
    }
    async fn init(&self) -> Result<()> {
        Ok(())
    }
    fn user(&self) -> Option<UserInfo> {
        None
    }
    fn login_flow(&self) -> LoginFlow {
        LoginFlow::None
    }
    async fn login(&self, _: &str) -> Result<UserInfo> {
        bail!("no accounts")
    }
    async fn logout(&self) -> Result<()> {
        Ok(())
    }
    async fn refresh(&self, _: bool) -> Result<()> {
        Ok(())
    }
    fn games(&self) -> Vec<Arc<GameInfo>> {
        vec![self.game.clone()]
    }
    fn game(&self, app: &str) -> Option<Arc<GameInfo>> {
        (*self.game.app_name == *app).then(|| self.game.clone())
    }
    async fn extra_info(&self, _: &str) -> Result<ExtraInfo> {
        Ok(ExtraInfo::default())
    }
    async fn install_info(
        &self,
        _: &str,
        _: InstallPlatform,
        _: &InstallInfoQuery,
    ) -> Result<InstallInfo> {
        bail!("added apps aren't installed")
    }
    async fn updatable_games(&self) -> Result<Vec<String>> {
        Ok(Vec::new())
    }
    async fn install(&self, _: &str, _: &InstallRequest, _: OpContext) -> Result<Outcome> {
        bail!("added apps aren't installed")
    }
    async fn update(&self, _: &str, _: &UpdateRequest, _: OpContext) -> Result<Outcome> {
        bail!("added apps aren't updated")
    }
    async fn repair(&self, _: &str, _: OpContext) -> Result<Outcome> {
        bail!("added apps aren't repaired")
    }
    async fn uninstall(&self, _: &str, _: RemoveArgs) -> Result<()> {
        Ok(())
    }
    async fn import(&self, _: &str, _: &Path, _: InstallPlatform) -> Result<()> {
        Ok(())
    }
    async fn set_install_path(&self, _: &str, _: &Path) -> Result<()> {
        Ok(())
    }
    fn supports_cloud_saves(&self) -> bool {
        self.saves_fail
    }
    async fn default_save_path(&self, _: &str, _: Option<&Path>) -> Result<String> {
        Ok("/saves".into())
    }
    async fn sync_saves(
        &self,
        _: &str,
        _: irori_core::SyncDirection,
        _: &str,
        _: OpContext,
    ) -> Result<String> {
        bail!("the cloud can't be reached")
    }
    async fn launch(&self, _: &str, request: &LaunchRequest, _: OpContext) -> Result<LaunchPlan> {
        Ok(LaunchPlan {
            command: self.command.clone().envs(request.environment.env.clone()),
            track_wineserver: false,
            watch: None,
        })
    }
}

#[cfg(target_os = "linux")]
#[test]
fn programs_alongside_start_and_stop_with_the_game() {
    isolate_paths();
    let dir = tempfile::tempdir().unwrap();
    let marks = dir.path().join("marks");
    let game = Arc::new(
        GameInfo {
            runner: Some(Runner::Sideload),
            app_name: "programs-alongside".into(),
            title: "Programs Alongside".into(),
            is_installed: true,
            install: Some(irori_core::InstalledInfo {
                install_path: dir.path().display().to_string(),
                platform: InstallPlatform::Linux,
                ..Default::default()
            }),
            ..Default::default()
        }
        .finalize(),
    );
    let id = game.id();
    let store: irori_core::DynStore = Arc::new(PlayStore {
        game,
        command: irori_core::process::CommandSpec::new("sh").args(["-c", "sleep 2"]),
        saves_fail: false,
    });
    let b = Backend::start_headless(move |_, _| vec![store]).unwrap();
    // The first program writes its game's name and then lives on until it is
    // stopped; the second would start after the game is over.
    let programs = serde_json::json!([
        {
            "name": "Voice",
            "path": "sh",
            "args": format!("-c 'echo \"$IRORI_GAME_TITLE\" > {}; exec sleep 60'", marks.display()),
        },
        {"name": "Trainer", "path": "sh", "args": "-c true", "delaySeconds": 30},
    ]);
    b.settings
        .set_game_value(&id, "iroriLaunchPrograms", Some(programs));
    let mut rx = b.subscribe();
    b.runtime()
        .block_on(b.launch(&id, None, Vec::new()))
        .unwrap();
    let exited = b.runtime().block_on(async {
        tokio::time::timeout(std::time::Duration::from_secs(30), async {
            loop {
                if let Ok(Event::GameExited { id: e, .. }) = rx.recv().await
                    && e == id
                {
                    return;
                }
            }
        })
        .await
    });
    assert!(exited.is_ok(), "the game ended");
    assert_eq!(
        std::fs::read_to_string(&marks).unwrap().trim(),
        "Programs Alongside"
    );
    let log = std::fs::read_to_string(b.game_log(&id)).unwrap();
    assert!(log.contains("Starting Voice"), "{log}");
    assert!(log.contains("Stopping Voice: the game ended"), "{log}");
    assert!(
        log.contains("Not starting Trainer: the game ended first"),
        "{log}"
    );
}

/// A backend with one added game that runs `script` through `sh -c`.
#[cfg(unix)]
fn play(app: &str, script: &str) -> (Backend, irori_core::GameId) {
    play_with(app, script, false)
}

/// [`play`], with cloud saves that can't be reached when `saves_fail`.
#[cfg(unix)]
fn play_with(app: &str, script: &str, saves_fail: bool) -> (Backend, irori_core::GameId) {
    isolate_paths();
    let game = Arc::new(
        GameInfo {
            runner: Some(Runner::Sideload),
            app_name: app.into(),
            title: app.into(),
            is_installed: true,
            install: Some(irori_core::InstalledInfo {
                install_path: std::env::temp_dir().display().to_string(),
                platform: if cfg!(target_os = "macos") {
                    InstallPlatform::Mac
                } else {
                    InstallPlatform::Linux
                },
                ..Default::default()
            }),
            cloud_save_enabled: saves_fail,
            ..Default::default()
        }
        .finalize(),
    );
    let id = game.id();
    let store: irori_core::DynStore = Arc::new(PlayStore {
        game,
        command: irori_core::process::CommandSpec::new("sh").args(["-c", script]),
        saves_fail,
    });
    (
        Backend::start_headless(move |_, _| vec![store]).unwrap(),
        id,
    )
}

#[cfg(unix)]
#[test]
fn a_failed_save_download_stops_the_launch_until_the_user_plays_anyway() {
    let (b, id) = play_with("saves-unreachable", "exit 0", true);
    b.settings
        .set_game_value(&id, "autoSyncSaves", Some(serde_json::Value::Bool(true)));
    let e = b
        .runtime()
        .block_on(b.launch(&id, None, Vec::new()))
        .unwrap_err();
    let failed = irori_core::SaveDownloadFailed::of(&e).expect("a failed download");
    assert!(failed.reason.contains("can't be reached"), "{e:#}");
    assert!(!b.is_running(&id));

    let mut rx = b.subscribe();
    let params = crate::LaunchParams {
        skip_save_download: true,
        ..Default::default()
    };
    b.runtime().block_on(b.launch_with(&id, params)).unwrap();
    let ended = session_end(&b, &mut rx, &id, std::time::Duration::from_secs(15));
    assert!(ended.is_some(), "the game ran");
}

/// Heroic #5968: the force-upload question says when the save folder holds
/// no files, since the cloud saves are then deleted.
#[cfg(unix)]
#[test]
fn knows_when_a_forced_upload_deletes_cloud_saves() {
    let (b, id) = play_with("saves-empty-folder", "exit 0", true);
    let dir = tempfile::tempdir().unwrap();
    let saves = dir.path().join("saves");
    std::fs::create_dir_all(&saves).unwrap();
    b.settings.set_game_value(
        &id,
        "savesPath",
        Some(serde_json::Value::String(saves.display().to_string())),
    );
    let deletes = || {
        b.runtime()
            .block_on(b.forced_upload_deletes_cloud_saves(&id))
            .unwrap()
    };
    assert!(deletes());
    std::fs::write(saves.join("slot1.sav"), "x").unwrap();
    assert!(!deletes());
}

/// Wait up to `limit` for `id`'s session to end; how long that took.
#[cfg(unix)]
fn session_end(
    b: &Backend,
    rx: &mut tokio::sync::broadcast::Receiver<Event>,
    id: &irori_core::GameId,
    limit: std::time::Duration,
) -> Option<std::time::Duration> {
    let started = std::time::Instant::now();
    b.runtime().block_on(async {
        tokio::time::timeout(limit, async {
            loop {
                if let Ok(Event::GameExited { id: e, .. }) = rx.recv().await
                    && e == *id
                {
                    return started.elapsed();
                }
            }
        })
        .await
        .ok()
    })
}

#[cfg(unix)]
#[test]
fn a_launcher_that_hands_over_keeps_the_session_until_the_game_ends() {
    // The launcher starts the "game" and exits at once, as crack and
    // publisher launchers do.
    let (b, id) = play("hand-over", "sleep 3 & exit 0");
    let mut rx = b.subscribe();
    b.runtime()
        .block_on(b.launch(&id, None, Vec::new()))
        .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(1500));
    assert!(b.is_running(&id), "the session outlives the launcher");
    let took = session_end(&b, &mut rx, &id, std::time::Duration::from_secs(15));
    assert!(took.is_some(), "the session ended with the game");
    let log = std::fs::read_to_string(b.game_log(&id)).unwrap();
    assert!(log.contains("left the game running"), "{log}");
}

#[cfg(unix)]
#[test]
fn stopping_ends_the_game_a_launcher_handed_over_to() {
    let dir = tempfile::tempdir().unwrap();
    let pid_file = dir.path().join("game.pid");
    let (b, id) = play(
        "hand-over-stop",
        &format!("sleep 60 & echo $! > {}; exit 0", pid_file.display()),
    );
    let mut rx = b.subscribe();
    b.runtime()
        .block_on(b.launch(&id, None, Vec::new()))
        .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(800));
    assert!(b.is_running(&id));
    b.runtime().block_on(b.stop(&id)).unwrap();
    let took = session_end(&b, &mut rx, &id, std::time::Duration::from_secs(15));
    assert!(took.is_some(), "stopping ended the session");
    let pid = std::fs::read_to_string(&pid_file).unwrap();
    assert!(!signal(pid.trim(), "-0"), "the handed-over game is gone");
}

/// Send `kill <flag> <pid>`; whether it reached the process.
#[cfg(unix)]
fn signal(pid: &str, flag: &str) -> bool {
    std::process::Command::new("kill")
        .args([flag, pid])
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

#[cfg(unix)]
#[test]
fn a_daemon_the_launcher_starts_does_not_hold_the_session() {
    let dir = tempfile::tempdir().unwrap();
    let pid_file = dir.path().join("daemon.pid");
    // A session of its own, as daemons (and wineserver) start.
    let script = format!(
        r#"perl -MPOSIX -e 'POSIX::setsid(); open(F, ">{}"); print F $$; close F; sleep 30' & sleep 1; exit 0"#,
        pid_file.display()
    );
    let (b, id) = play("hand-over-daemon", &script);
    let mut rx = b.subscribe();
    b.runtime()
        .block_on(b.launch(&id, None, Vec::new()))
        .unwrap();
    let took = session_end(&b, &mut rx, &id, std::time::Duration::from_secs(10));
    let pid = std::fs::read_to_string(&pid_file).ok();
    if let Some(pid) = &pid {
        signal(pid.trim(), "-9");
    }
    assert!(pid.is_some(), "the daemon started");
    let took = took.expect("the session ended without the daemon");
    assert!(took < std::time::Duration::from_secs(6), "{took:?}");
}

/// Wait up to five seconds for `done`.
#[cfg(unix)]
fn eventually(done: impl Fn() -> bool) -> bool {
    for _ in 0..100 {
        if done() {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    done()
}

#[cfg(unix)]
#[test]
fn a_launch_cancelled_while_its_script_runs_starts_nothing() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let started = dir.path().join("started");
    let script_pid = dir.path().join("script.pid");
    let script = dir.path().join("before.sh");
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\necho $$ > {}.tmp; mv {0}.tmp {0}\nexec sleep 60\n",
            script_pid.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    let (b, id) = play(
        "cancel-while-preparing",
        &format!("touch {}", started.display()),
    );
    b.settings.set_game_value(
        &id,
        "beforeLaunchScriptPath",
        Some(serde_json::Value::String(script.display().to_string())),
    );
    let launch = {
        let (b, id) = (b.clone(), id.clone());
        b.runtime()
            .spawn(async move { b.launch(&id, None, Vec::new()).await })
    };
    assert!(eventually(|| script_pid.is_file()), "the script runs");
    assert_eq!(
        b.statuses.get(&id).map(|s| s.kind),
        Some(irori_core::StatusKind::Launching)
    );
    assert!(b.cancel_launch(&id));
    let result = b
        .runtime()
        .block_on(async { tokio::time::timeout(std::time::Duration::from_secs(15), launch).await })
        .expect("the launch ends at once")
        .unwrap();
    let e = result.expect_err("cancelled");
    assert!(crate::LaunchCancelled::of(&e), "{e:#}");
    assert!(!b.is_running(&id));
    assert!(b.statuses.get(&id).is_none(), "the status is cleared");
    assert!(!b.cancel_launch(&id), "nothing left to cancel");
    let pid = std::fs::read_to_string(&script_pid).unwrap();
    assert!(
        eventually(|| !signal(pid.trim(), "-0")),
        "the script was stopped"
    );
    std::thread::sleep(std::time::Duration::from_millis(300));
    assert!(!started.exists(), "the game never started");
}

/// Whether `id` has a session recorded as running on disk.
#[cfg(unix)]
fn recorded_running(id: &irori_core::GameId) -> Option<crate::running::Record> {
    crate::running::read()
        .into_iter()
        .find(|r| r.game == id.key())
}

#[cfg(unix)]
#[test]
fn a_running_session_is_kept_on_disk_until_it_ends() {
    let (b, id) = play("kept-on-disk", "sleep 2");
    let mut rx = b.subscribe();
    b.runtime()
        .block_on(b.launch(&id, None, Vec::new()))
        .unwrap();
    let record = recorded_running(&id).expect("recorded while it runs");
    assert_eq!(Some(record.pid), b.running_pid(&id));
    assert!(record.process_start.is_some());
    assert!(session_end(&b, &mut rx, &id, std::time::Duration::from_secs(15)).is_some());
    assert!(recorded_running(&id).is_none(), "forgotten when it ended");
}

#[cfg(unix)]
#[test]
fn a_game_still_running_after_a_restart_is_followed_to_its_end() {
    use std::os::unix::process::CommandExt as _;
    let (b, id) = play("followed-again", "exit 0");
    // The game an Irori that closed left running, in its own process group
    // as Irori starts games.
    let mut game = std::process::Command::new("sh")
        .args(["-c", "sleep 4"])
        .process_group(0)
        .spawn()
        .unwrap();
    let pid = game.id();
    // Nobody else reaps it: a zombie would look alive.
    let reaper = std::thread::spawn(move || game.wait());
    let record = crate::running::Record {
        game: id.key(),
        pid,
        started_at: (chrono::Utc::now() - chrono::Duration::seconds(90)).to_rfc3339(),
        process_start: crate::running::process_start(pid),
        folder: None,
        mods: false,
    };
    crate::running::remember(record.clone());
    // A game that ended while Irori was closed is only forgotten.
    let ended = std::process::Command::new("true").spawn().unwrap().wait();
    assert!(ended.is_ok());
    let gone = irori_core::GameId::new(Runner::Sideload, "ended-meanwhile");
    let stale = crate::running::Record {
        game: gone.key(),
        pid: 999_999,
        ..record.clone()
    };
    crate::running::remember(stale.clone());

    let mut rx = b.subscribe();
    b.runtime().block_on(b.adopt_records(vec![record, stale]));
    assert!(b.is_running(&id));
    assert_eq!(
        b.statuses.get(&id).map(|s| s.kind),
        Some(irori_core::StatusKind::Playing)
    );
    assert!(b.playing_for(&id).unwrap() >= std::time::Duration::from_secs(89));
    assert!(!b.is_running(&gone));
    assert!(recorded_running(&gone).is_none());

    let seconds = b.runtime().block_on(async {
        tokio::time::timeout(std::time::Duration::from_secs(20), async {
            loop {
                if let Ok(Event::GameExited { id: e, seconds, .. }) = rx.recv().await
                    && e == id
                {
                    return seconds;
                }
            }
        })
        .await
        .expect("the session ended with the game")
    });
    let _ = reaper.join();
    assert!(seconds >= 90, "counted from its start: {seconds}");
    assert!(!b.is_running(&id));
    assert!(recorded_running(&id).is_none());
    let session = b
        .sessions()
        .into_iter()
        .rfind(|s| s.game == id.key())
        .expect("the session was recorded");
    assert!(!session.crashed);
}

#[test]
fn launch_presets_save_apply_and_go() {
    isolate_paths();
    let b = Backend::start_headless(|_, _| Vec::new()).unwrap();
    let (from, to) = (
        irori_core::GameId::new(Runner::Sideload, "preset-from"),
        irori_core::GameId::new(Runner::Sideload, "preset-to"),
    );
    assert!(b.settings.save_launch_preset("Empty", Some(&from)).is_err());
    b.settings
        .set_game_value(&from, "showMangohud", Some(serde_json::Value::Bool(true)));
    b.settings.set_game_value(
        &from,
        "launcherArgs",
        Some(serde_json::Value::String("-windowed".into())),
    );
    let preset = b
        .settings
        .save_launch_preset(" Overlay ", Some(&from))
        .unwrap();
    assert_eq!(preset.name, "Overlay");
    assert!(b.settings.launch_presets().contains(&preset));

    b.settings
        .apply_launch_preset(&preset, std::slice::from_ref(&to));
    let overrides = b.settings.overrides(&to);
    assert!(overrides.is_overridden("showMangohud"));
    assert!(
        !overrides.is_overridden("launcherArgs"),
        "not a launch preset key"
    );
    assert!(b.settings.game(&to).show_mangohud);

    b.settings.remove_launch_preset(&preset.id);
    assert!(
        !b.settings
            .launch_presets()
            .iter()
            .any(|p| p.id == preset.id)
    );
    assert!(b.settings.game(&to).show_mangohud, "applied settings stay");
}

#[cfg(unix)]
#[test]
fn backups_while_playing_repeat_tell_one_failure_and_stop_with_the_session() {
    let (b, id) = play_with("backups-while-playing", "exit 0", false);
    // A chosen Ludusavi that is gone: every backup fails, none runs.
    b.settings
        .update_app(|a| a.ludusavi_path = "/nonexistent/irori-test/ludusavi".into());
    let mut rx = b.subscribe();
    let stop = irori_core::process::CancellationToken::new();
    let task = b.spawn({
        let (b, id, stop) = (b.clone(), id.clone(), stop.clone());
        async move {
            b.back_up_while_playing(
                id,
                "Backups".into(),
                std::time::Duration::from_millis(20),
                stop,
            )
            .await
        }
    });
    std::thread::sleep(std::time::Duration::from_millis(300));
    stop.cancel();
    let ended = b
        .runtime()
        .block_on(async { tokio::time::timeout(std::time::Duration::from_secs(5), task).await });
    assert!(ended.is_ok(), "the backups end with the session");
    let mut notices = 0;
    while let Ok(event) = rx.try_recv() {
        if let Event::Notice(n) = event
            && n.title_key == Some("shell.notice.ludusavi_backup_failed")
            && n.game.as_ref() == Some(&id)
        {
            notices += 1;
        }
    }
    assert_eq!(notices, 1, "a failure is told once per session");
    assert_eq!(
        crate::ludusavi::backup_interval(0),
        std::time::Duration::from_secs(60)
    );
    assert_eq!(
        crate::ludusavi::backup_interval(15),
        std::time::Duration::from_secs(900)
    );
}

/// A store of fixed games, for library tests.
struct ListStore {
    runner: Runner,
    games: Vec<Arc<GameInfo>>,
}

impl ListStore {
    fn store(runner: Runner, games: &[(&str, &str)]) -> irori_core::DynStore {
        let games = games
            .iter()
            .map(|(app, title)| {
                Arc::new(
                    GameInfo {
                        runner: Some(runner),
                        app_name: (*app).into(),
                        title: (*title).into(),
                        ..Default::default()
                    }
                    .finalize(),
                )
            })
            .collect();
        Arc::new(Self { runner, games })
    }
}

#[async_trait]
impl Store for ListStore {
    fn runner(&self) -> Runner {
        self.runner
    }
    async fn init(&self) -> Result<()> {
        Ok(())
    }
    fn user(&self) -> Option<UserInfo> {
        Some(UserInfo::default())
    }
    fn login_flow(&self) -> LoginFlow {
        LoginFlow::None
    }
    async fn login(&self, _: &str) -> Result<UserInfo> {
        bail!("not supported")
    }
    async fn logout(&self) -> Result<()> {
        Ok(())
    }
    async fn refresh(&self, _: bool) -> Result<()> {
        Ok(())
    }
    fn games(&self) -> Vec<Arc<GameInfo>> {
        self.games.clone()
    }
    fn game(&self, app: &str) -> Option<Arc<GameInfo>> {
        self.games.iter().find(|g| g.app_name == app).cloned()
    }
    async fn extra_info(&self, _: &str) -> Result<ExtraInfo> {
        Ok(ExtraInfo::default())
    }
    async fn install_info(
        &self,
        _: &str,
        _: InstallPlatform,
        _: &InstallInfoQuery,
    ) -> Result<InstallInfo> {
        bail!("not supported")
    }
    async fn updatable_games(&self) -> Result<Vec<String>> {
        Ok(Vec::new())
    }
    async fn install(&self, _: &str, _: &InstallRequest, _: OpContext) -> Result<Outcome> {
        bail!("not supported")
    }
    async fn update(&self, _: &str, _: &UpdateRequest, _: OpContext) -> Result<Outcome> {
        bail!("not supported")
    }
    async fn repair(&self, _: &str, _: OpContext) -> Result<Outcome> {
        bail!("not supported")
    }
    async fn uninstall(&self, _: &str, _: RemoveArgs) -> Result<()> {
        bail!("not supported")
    }
    async fn import(&self, _: &str, _: &Path, _: InstallPlatform) -> Result<()> {
        bail!("not supported")
    }
    async fn set_install_path(&self, _: &str, _: &Path) -> Result<()> {
        bail!("not supported")
    }
    async fn launch(&self, _: &str, _: &LaunchRequest, _: OpContext) -> Result<LaunchPlan> {
        bail!("not supported")
    }
}

#[test]
fn copies_combine_separate_and_prefer_as_the_user_says() {
    use irori_core::GameId;
    isolate_paths();
    let b = Backend::start_headless(|_, _| {
        vec![
            ListStore::store(
                Runner::Steam,
                &[
                    ("copies-620", "Copies: Portal 2"),
                    ("copies-1", "Copies: A"),
                ],
            ),
            ListStore::store(
                Runner::Gog,
                &[("copies-p2", "Copies: Portal 2"), ("copies-b", "Copies: B")],
            ),
        ]
    })
    .unwrap();
    let ids = |b: &Backend, id: &GameId| -> Vec<String> {
        b.library().copies_of(id).iter().map(GameId::key).collect()
    };
    let (steam, gog) = (
        GameId::new(Runner::Steam, "copies-620"),
        GameId::new(Runner::Gog, "copies-p2"),
    );
    let (a, other) = (
        GameId::new(Runner::Steam, "copies-1"),
        GameId::new(Runner::Gog, "copies-b"),
    );
    b.rebuild_library();
    assert_eq!(ids(&b, &steam), ["gog:copies-p2", "steam:copies-620"]);
    assert!(ids(&b, &a).is_empty());

    // The user's choice leads.
    b.userdata
        .set_default_copy(&steam, &[gog.clone(), steam.clone()]);
    b.rebuild_library();
    assert_eq!(ids(&b, &gog), ["steam:copies-620", "gog:copies-p2"]);
    assert_eq!(b.library().entry_of(&gog).unwrap().id(), steam);

    // Games of other titles combine only when the user says so.
    b.userdata.combine_copies(&[a.clone(), other.clone()]);
    b.rebuild_library();
    assert_eq!(ids(&b, &other), ["steam:copies-1", "gog:copies-b"]);

    // Separated, each stands alone; editing a game's title keeps that.
    b.userdata
        .separate_copy(&steam, &[steam.clone(), gog.clone()]);
    b.userdata.set_overrides(
        &steam,
        irori_core::game::GameOverrides {
            title: Some("Copies: Portal 2".into()),
            ..Default::default()
        },
    );
    b.rebuild_library();
    assert!(ids(&b, &steam).is_empty());
    assert!(ids(&b, &gog).is_empty());
    b.userdata.separate_copies(&[a.clone(), other.clone()]);
    b.rebuild_library();
    assert!(ids(&b, &a).is_empty());
    assert_eq!(b.library().all_games().count(), 4);

    // Turned off, nothing is grouped.
    b.userdata.combine_copies(&[a.clone(), other.clone()]);
    b.settings.update_app(|s| s.ui.group_copies = false);
    b.rebuild_library();
    assert!(ids(&b, &a).is_empty());
    b.settings.update_app(|s| s.ui.group_copies = true);
}

#[test]
fn third_party_launchers_are_rescanned_on_their_own_timer() {
    use std::time::Duration;
    let mut app = irori_core::AppSettings::default();
    assert_eq!(app.launchers_rescan, 5);
    let five = Duration::from_secs(5 * 60);
    assert!(!super::launchers_rescan_due(&app, five), "launchers off");
    app.third_party_launchers = true;
    assert!(!super::launchers_rescan_due(
        &app,
        Duration::from_secs(4 * 60)
    ));
    assert!(super::launchers_rescan_due(&app, five));
    app.launchers_rescan = 0;
    assert!(
        !super::launchers_rescan_due(&app, Duration::from_secs(86_400)),
        "0 is off"
    );
}

#[test]
fn igdb_fills_store_gaps_only_while_on_and_after_the_users_choice() {
    use irori_core::GameId;
    use irori_depot::test_server::{Resp, TestServer};
    isolate_paths();
    let b = Backend::start_headless(|_, _| {
        vec![ListStore::store(
            Runner::Sideload,
            &[("igdb-lone", "IGDB: Lone Game")],
        )]
    })
    .unwrap();
    let _rt = b.runtime().enter();
    let server = TestServer::start(|req| match req.path.as_str() {
        "/token" => Resp::json(serde_json::json!({"access_token": "t", "expires_in": 3600})),
        "/v4/games" => {
            let body = String::from_utf8_lossy(&req.body).to_string();
            if body.contains("where id = 7;") {
                Resp::json(serde_json::json!([{"id": 7, "name": "Chosen", "summary": "Picked."}]))
            } else {
                Resp::json(serde_json::json!([
                    {"id": 1, "name": "IGDB: Lone Game", "summary": "Found by title.",
                     "genres": [{"id": 1, "name": "Puzzle"}]}
                ]))
            }
        }
        _ => Resp::status(404),
    });
    b.set_igdb_endpoints(irori_extras::igdb::Endpoints {
        token: server.url("/token"),
        api: server.url("/v4"),
    });
    b.rebuild_library();
    let id = GameId::new(Runner::Sideload, "igdb-lone");
    let creds = irori_extras::igdb::Credentials::new("client", "secret").unwrap();
    let rt = b.runtime();
    rt.block_on(b.set_igdb_credentials(Some(creds))).unwrap();
    assert_eq!(
        rt.block_on(b.igdb_status()).client_id.as_deref(),
        Some("client")
    );

    // Off: nothing is asked, nothing filled.
    let asked = server.requests().len();
    let info = rt.block_on(b.extra_info(&id)).unwrap();
    assert_eq!((info.description, info.details_from), (None, None));
    assert_eq!(server.requests().len(), asked);

    b.settings.update_app(|s| s.igdb = true);
    let info = rt.block_on(b.extra_info(&id)).unwrap();
    assert_eq!(info.description.as_deref(), Some("Found by title."));
    assert_eq!(info.genres, ["Puzzle"]);
    assert_eq!(info.details_from.as_deref(), Some("IGDB"));

    // The user's match wins over the title search.
    b.set_igdb_match(&id, Some("7".into()));
    let info = rt.block_on(b.extra_info(&id)).unwrap();
    assert_eq!(info.description.as_deref(), Some("Picked."));
    b.set_igdb_match(&id, Some(irori_extras::igdb::IGDB_NONE.into()));
    let info = rt.block_on(b.extra_info(&id)).unwrap();
    assert_eq!(info.details_from, None);

    rt.block_on(b.set_igdb_credentials(None)).unwrap();
    assert_eq!(rt.block_on(b.igdb_status()).client_id, None);
    b.settings.update_app(|s| s.igdb = false);
}

#[test]
fn a_linked_steam_app_lends_its_screenshots() {
    use irori_core::GameId;
    use irori_depot::test_server::{Resp, TestServer};
    isolate_paths();
    let b = Backend::start_headless(|_, _| {
        vec![ListStore::store(
            Runner::Sideload,
            &[("steam-linked", "Linked Game")],
        )]
    })
    .unwrap();
    let _rt = b.runtime().enter();
    let server = TestServer::start(|req| {
        if req.path.starts_with("/api/appdetails?appids=4242") {
            Resp::json(serde_json::json!({"4242": {"success": true, "data": {
                "screenshots": [{"path_full": "https://cdn/shot.jpg"}]
            }}}))
        } else {
            Resp::status(404)
        }
    });
    crate::media::steam::set_endpoints(irori_steam_api::Endpoints {
        api: server.base(),
        store: server.base(),
        assets: server.base(),
    });
    b.rebuild_library();
    let id = GameId::new(Runner::Sideload, "steam-linked");
    let rt = b.runtime();

    // Not linked: Steam isn't asked.
    let info = rt.block_on(b.extra_info(&id)).unwrap();
    assert!(info.screenshots.is_empty());
    assert!(server.requests().is_empty());

    b.settings.set_game_value(
        &id,
        "iroriSteamAppId",
        Some(serde_json::Value::String("4242".into())),
    );
    let info = rt.block_on(b.extra_info(&id)).unwrap();
    assert_eq!(info.screenshots, ["https://cdn/shot.jpg"]);
    // Kept for the run: the page is asked once.
    rt.block_on(b.extra_info(&id)).unwrap();
    assert_eq!(server.requests().len(), 1);
}

#[cfg(unix)]
#[test]
fn removing_a_prefix_forgets_it() {
    isolate_paths();
    let dir = tempfile::tempdir().unwrap();
    let prefix = dir.path().join("prefix");
    std::fs::create_dir_all(prefix.join("drive_c")).unwrap();
    std::fs::write(prefix.join("system.reg"), "").unwrap();
    let game = Arc::new(
        GameInfo {
            runner: Some(Runner::Sideload),
            app_name: "prefix-gone".into(),
            title: "Prefix Gone".into(),
            is_installed: true,
            install: Some(irori_core::InstalledInfo {
                install_path: dir.path().join("game").display().to_string(),
                platform: InstallPlatform::Windows,
                ..Default::default()
            }),
            ..Default::default()
        }
        .finalize(),
    );
    let id = game.id();
    let store: irori_core::DynStore = Arc::new(PlayStore {
        game,
        command: irori_core::process::CommandSpec::new("true"),
        saves_fail: false,
    });
    let b = Backend::start_headless(move |_, _| vec![store]).unwrap();
    b.settings.set_game_value(
        &id,
        "winePrefix",
        Some(serde_json::Value::String(prefix.display().to_string())),
    );
    let args = RemoveArgs {
        remove_prefix: true,
        ..Default::default()
    };
    b.runtime().block_on(b.uninstall(&id, args)).unwrap();
    assert!(!prefix.exists());
    assert!(!b.settings.overrides(&id).is_overridden("winePrefix"));
}

/// A ROM's title and ids find nothing in Ludusavi; its emulator manifest's
/// name does, and is kept so the search runs once.
#[cfg(unix)]
#[test]
fn emulated_games_find_their_manifest_name_once() {
    use std::os::unix::fs::PermissionsExt as _;
    isolate_paths();
    let dir = tempfile::tempdir().unwrap();
    // An added game played in an emulator, its title the ROM's, tags and all.
    let game = Arc::new(
        GameInfo {
            runner: Some(Runner::Sideload),
            app_name: "ludusavi-emulated".into(),
            title: "Crash Bandicoot (USA)".into(),
            is_installed: true,
            install: Some(irori_core::InstalledInfo {
                install_path: dir.path().display().to_string(),
                executable: dir
                    .path()
                    .join("Crash Bandicoot (USA).cue")
                    .display()
                    .to_string(),
                ..Default::default()
            }),
            ..Default::default()
        }
        .finalize(),
    );
    let id = game.id();
    let store: irori_core::DynStore = Arc::new(PlayStore {
        game,
        command: irori_core::process::CommandSpec::new("true"),
        saves_fail: false,
    });
    let b = Backend::start_headless(move |_, _| vec![store]).unwrap();
    let calls = dir.path().join("calls");
    let script = dir.path().join("ludusavi");
    std::fs::write(
        &script,
        format!(
            r#"#!/bin/sh
[ "$1" = --version ] && {{ echo "ludusavi 0.31.0"; exit 0; }}
echo "$*" >> '{calls}'
case "$*" in
  *--fuzzy*) echo '{{"games": {{"Crash Bandicoot (RetroArch)": {{"score": 0.95}},
    "Crash Bandicoot (PS1)": {{"score": 0.9}}, "Crash Team Racing (PS1)": {{"score": 0.8}}}}}}' ;;
  *) echo '{{"errors": {{"unknownGames": ["x"]}}, "games": {{}}}}'; exit 1 ;;
esac
"#,
            calls = calls.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    b.settings
        .update_app(|a| a.ludusavi_path = script.display().to_string());
    b.settings.set_game_value(
        &id,
        "iroriEngine",
        Some(serde_json::Value::String("duckstation".into())),
    );
    let rt = b.runtime();
    let title = rt.block_on(b.ludusavi_title(&id)).unwrap();
    assert_eq!(title.as_deref(), Some("Crash Bandicoot (PS1)"));
    // Found, not chosen: the user's own choice stays empty.
    assert!(
        !b.settings
            .overrides(&id)
            .is_overridden("iroriLudusaviTitle")
    );

    // A new session (no matches in memory) runs only the plain lookup.
    let fuzzy = || {
        std::fs::read_to_string(&calls)
            .unwrap()
            .lines()
            .filter(|l| l.contains("--fuzzy"))
            .count()
    };
    let asked = fuzzy();
    crate::ludusavi::forget_matches_for_test();
    let again = rt.block_on(b.ludusavi_title(&id)).unwrap();
    assert_eq!(again.as_deref(), Some("Crash Bandicoot (PS1)"));
    assert_eq!(fuzzy(), asked, "the kept name needs no search");

    // The user's choice wins over the found name.
    b.set_ludusavi_title(&id, Some("Crash Bandicoot (RetroArch)"));
    let chosen = rt.block_on(b.ludusavi_title(&id)).unwrap();
    assert_eq!(chosen.as_deref(), Some("Crash Bandicoot (RetroArch)"));
}

/// The clean-up after a store removed a game goes on past a step that fails,
/// and the one notice names what was left.
#[cfg(unix)]
#[test]
fn uninstall_cleans_up_past_a_prefix_it_cannot_delete() {
    use std::os::unix::fs::PermissionsExt;
    isolate_paths();
    let dir = tempfile::tempdir().unwrap();
    // A prefix in a folder nobody may change: deleting it fails.
    let locked = dir.path().join("locked");
    let prefix = locked.join("prefix");
    std::fs::create_dir_all(&prefix).unwrap();
    std::fs::write(prefix.join("system.reg"), "WINE REGISTRY Version 2\n").unwrap();
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o555)).unwrap();
    let game = Arc::new(
        GameInfo {
            runner: Some(Runner::Sideload),
            app_name: "cleanup-on".into(),
            title: "Cleanup On".into(),
            is_installed: true,
            install: Some(irori_core::InstalledInfo {
                install_path: dir.path().display().to_string(),
                platform: InstallPlatform::Windows,
                ..Default::default()
            }),
            ..Default::default()
        }
        .finalize(),
    );
    let id = game.id();
    let store: irori_core::DynStore = Arc::new(PlayStore {
        game,
        command: irori_core::process::CommandSpec::new("true"),
        saves_fail: false,
    });
    let b = Backend::start_headless(move |_, _| vec![store]).unwrap();
    b.settings.set_game_value(
        &id,
        "winePrefix",
        Some(serde_json::Value::String(prefix.display().to_string())),
    );
    let settings_file = Paths::get().game_settings_file(&id);
    assert!(settings_file.is_file());
    let mut rx = b.subscribe();
    let args = RemoveArgs {
        remove_prefix: true,
        delete_files: false,
        remove_settings: true,
    };
    let result = b.runtime().block_on(b.uninstall(&id, args));
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
    result.unwrap();
    assert!(prefix.is_dir(), "the prefix could not go");
    assert!(!settings_file.exists(), "the settings went anyway");
    let mut library_changed = false;
    let mut notices = Vec::new();
    while let Ok(event) = rx.try_recv() {
        match event {
            Event::LibraryChanged(Runner::Sideload) => library_changed = true,
            Event::Notice(n) => notices.push(n),
            _ => {}
        }
    }
    assert!(library_changed);
    let [notice] = notices.as_slice() else {
        panic!("one notice: {notices:?}");
    };
    assert_eq!(
        notice.title_key,
        Some("shell.notice.uninstalled_with_problems")
    );
    assert_eq!(notice.level, irori_core::NoticeLevel::Warning);
    let message = notice.message.as_deref().unwrap_or_default();
    assert!(message.contains(&prefix.display().to_string()), "{message}");
}

/// A game whose folder is gone stops with a typed error the UI turns into
/// "locate the files or forget the install", never a plain message.
#[cfg(unix)]
#[test]
fn missing_game_files_stop_the_launch_with_a_typed_error() {
    isolate_paths();
    let gone = tempfile::tempdir().unwrap().path().join("Moved Away");
    let game = Arc::new(
        GameInfo {
            runner: Some(Runner::Sideload),
            app_name: "files-gone".into(),
            title: "Files Gone".into(),
            is_installed: true,
            install: Some(irori_core::InstalledInfo {
                install_path: gone.display().to_string(),
                platform: InstallPlatform::Linux,
                ..Default::default()
            }),
            ..Default::default()
        }
        .finalize(),
    );
    let id = game.id();
    let store: irori_core::DynStore = Arc::new(PlayStore {
        game,
        command: irori_core::process::CommandSpec::new("sh").args(["-c", "exit 0"]),
        saves_fail: false,
    });
    let b = Backend::start_headless(move |_, _| vec![store]).unwrap();
    let e = b
        .runtime()
        .block_on(b.launch(&id, None, Vec::new()))
        .unwrap_err();
    let missing = irori_core::GameFilesMissing::of(&e).expect("a typed error");
    assert_eq!(missing.title, "Files Gone");
    assert_eq!(missing.path.as_deref(), Some(gone.as_path()));
    assert!(!b.is_running(&id));
    // An added game is located again, never "forgotten".
    assert!(!Backend::can_forget_install(&id));
    assert!(b.runtime().block_on(b.forget_install(&id)).is_err());
    assert!(Backend::can_forget_install(&irori_core::GameId::new(
        Runner::Gog,
        "1"
    )));
    assert!(!Backend::can_forget_install(&irori_core::GameId::new(
        Runner::Steam,
        "1"
    )));
}

/// A store with one game whose achievements the test unlocks, reporting
/// unlocks the way `signal` says.
struct UnlockStore {
    list: ListStore,
    achievements: parking_lot::Mutex<Vec<irori_core::Achievement>>,
    signal: irori_core::AchievementSignal,
    /// How often the store was asked for the achievements.
    asked: std::sync::atomic::AtomicUsize,
}

impl UnlockStore {
    fn new(app: &str, signal: irori_core::AchievementSignal) -> Self {
        let achievement = |id: &str, at: Option<&str>| irori_core::Achievement {
            id: id.into(),
            name: id.to_uppercase(),
            visible: true,
            unlocked_at: at.map(String::from),
            ..Default::default()
        };
        Self {
            list: ListStore {
                runner: Runner::Sideload,
                games: vec![Arc::new(
                    GameInfo {
                        runner: Some(Runner::Sideload),
                        app_name: app.into(),
                        title: "Live".into(),
                        ..Default::default()
                    }
                    .finalize(),
                )],
            },
            achievements: parking_lot::Mutex::new(vec![
                achievement("a", Some("2026-01-01T00:00:00Z")),
                achievement("b", None),
            ]),
            signal,
            asked: Default::default(),
        }
    }

    fn unlock(&self, id: &str) {
        for a in self.achievements.lock().iter_mut().filter(|a| a.id == id) {
            a.unlocked_at = Some("2026-02-01T00:00:00Z".into());
        }
    }

    fn asked(&self) -> usize {
        self.asked.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl Store for UnlockStore {
    fn runner(&self) -> Runner {
        self.list.runner()
    }
    async fn init(&self) -> Result<()> {
        Ok(())
    }
    fn user(&self) -> Option<UserInfo> {
        self.list.user()
    }
    fn login_flow(&self) -> LoginFlow {
        LoginFlow::None
    }
    async fn login(&self, _: &str) -> Result<UserInfo> {
        bail!("not supported")
    }
    async fn logout(&self) -> Result<()> {
        Ok(())
    }
    async fn refresh(&self, _: bool) -> Result<()> {
        Ok(())
    }
    fn games(&self) -> Vec<Arc<GameInfo>> {
        self.list.games()
    }
    fn game(&self, app: &str) -> Option<Arc<GameInfo>> {
        self.list.game(app)
    }
    async fn extra_info(&self, _: &str) -> Result<ExtraInfo> {
        Ok(ExtraInfo::default())
    }
    async fn install_info(
        &self,
        _: &str,
        _: InstallPlatform,
        _: &InstallInfoQuery,
    ) -> Result<InstallInfo> {
        bail!("not supported")
    }
    async fn updatable_games(&self) -> Result<Vec<String>> {
        Ok(Vec::new())
    }
    async fn install(&self, _: &str, _: &InstallRequest, _: OpContext) -> Result<Outcome> {
        bail!("not supported")
    }
    async fn update(&self, _: &str, _: &UpdateRequest, _: OpContext) -> Result<Outcome> {
        bail!("not supported")
    }
    async fn repair(&self, _: &str, _: OpContext) -> Result<Outcome> {
        bail!("not supported")
    }
    async fn uninstall(&self, _: &str, _: RemoveArgs) -> Result<()> {
        bail!("not supported")
    }
    async fn import(&self, _: &str, _: &Path, _: InstallPlatform) -> Result<()> {
        bail!("not supported")
    }
    async fn set_install_path(&self, _: &str, _: &Path) -> Result<()> {
        bail!("not supported")
    }
    async fn launch(&self, _: &str, _: &LaunchRequest, _: OpContext) -> Result<LaunchPlan> {
        bail!("not supported")
    }
    async fn achievements(&self, _: &str, _: &str) -> Result<Vec<irori_core::Achievement>> {
        self.asked.fetch_add(1, Ordering::SeqCst);
        Ok(self.achievements.lock().clone())
    }
    async fn achievement_signal(&self, _: &str) -> irori_core::AchievementSignal {
        self.signal.clone()
    }
}

/// Wait up to ten seconds for `done`.
async fn soon(done: impl Fn() -> bool) -> bool {
    for _ in 0..200 {
        if done() {
            return true;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    false
}

/// Whether an `AchievementsUnlocked` for `id` comes within ten seconds.
async fn unlocked(
    events: &mut tokio::sync::broadcast::Receiver<Event>,
    id: &irori_core::GameId,
) -> bool {
    let wait = async {
        loop {
            match events.recv().await {
                Ok(Event::AchievementsUnlocked(game)) if game == *id => return true,
                Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                Err(_) => return false,
            }
        }
    };
    tokio::time::timeout(std::time::Duration::from_secs(10), wait)
        .await
        .unwrap_or(false)
}

/// A session's achievement watch over `store`'s game, with notices on.
fn watch(
    store: &Arc<UnlockStore>,
    app: &str,
) -> (
    Backend,
    irori_core::GameId,
    tokio::sync::broadcast::Receiver<Event>,
    tokio_util::sync::CancellationToken,
) {
    let shared: irori_core::DynStore = store.clone();
    let b = Backend::start_headless(move |_, _| vec![shared.clone()]).unwrap();
    b.settings.update_app(|a| a.achievement_notices = true);
    let id = irori_core::GameId::new(Runner::Sideload, app);
    let events = b.subscribe();
    let stop = tokio_util::sync::CancellationToken::new();
    let (watcher, game, until) = (b.clone(), id.clone(), stop.clone());
    b.runtime().spawn(async move {
        watcher
            .watch_achievements_while_playing(game, "Live".into(), until)
            .await
    });
    (b, id, events, stop)
}

#[test]
fn unlocks_follow_the_stores_reports_while_playing() {
    isolate_paths();
    let app = "live-unlocks-events";
    let store = Arc::new(UnlockStore::new(app, irori_core::AchievementSignal::Events));
    let (b, id, mut events, stop) = watch(&store, app);
    let rt = b.runtime();
    // What is unlocked when the session starts is only noted.
    assert!(rt.block_on(soon(|| store.asked() == 1)));
    assert!(b.take_live_unlocks(&id).is_empty());

    store.unlock("b");
    b.bus.emit(Event::AchievementSignal(id.clone()));
    assert!(rt.block_on(unlocked(&mut events, &id)));
    let live: Vec<String> = b.take_live_unlocks(&id).into_iter().map(|a| a.id).collect();
    assert_eq!(live, ["b"]);
    assert!(b.take_live_unlocks(&id).is_empty(), "taken once");
    // Nothing was asked in between: the store's reports are followed.
    assert_eq!(store.asked(), 2);
    stop.cancel();
}

#[test]
fn unlocks_follow_the_stores_files_while_playing() {
    isolate_paths();
    let dir = tempfile::tempdir().unwrap();
    let stats = dir.path().join("UserGameStats_1_2.bin");
    std::fs::write(&stats, b"before").unwrap();
    let app = "live-unlocks-files";
    let store = Arc::new(UnlockStore::new(
        app,
        irori_core::AchievementSignal::Files(vec![stats.clone()]),
    ));
    let (b, id, mut events, stop) = watch(&store, app);
    let rt = b.runtime();
    assert!(rt.block_on(soon(|| store.asked() == 1)));
    // Let the watch begin before the store writes its file.
    std::thread::sleep(std::time::Duration::from_millis(500));
    store.unlock("b");
    std::fs::write(&stats, b"after").unwrap();
    assert!(rt.block_on(unlocked(&mut events, &id)));
    let live: Vec<String> = b.take_live_unlocks(&id).into_iter().map(|a| a.id).collect();
    assert_eq!(live, ["b"]);
    stop.cancel();
}
