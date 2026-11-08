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
struct PlayStore {
    game: Arc<GameInfo>,
    command: irori_core::process::CommandSpec,
    /// Has cloud saves, and the cloud can't be reached.
    saves_fail: bool,
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
