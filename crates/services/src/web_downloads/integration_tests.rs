//! Integration tests for direct HTTP/HTTPS web downloads, archive extraction,
//! `SideloadBridge` auto-add, `fs_guard` safe deletion, and systemic backend
//! integration (`WhenDone`, `stay_awake`, `download_disks`, offline auto-pause,
//! and bandwidth budget reconciliation).

use std::io::Write as _;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use irori_core::{GameId, InstallPlatform, Runner};
use irori_depot::test_server::{Resp, TestServer};
use parking_lot::Mutex;

use super::aria2::{Aria2TaskStatus, resolve_bt_output_path_opt};
use super::http_probe::{
    DirectProbe, fetch_remote_torrent_bytes, parse_content_disposition_filename, probe_direct_url,
    unique_file_name,
};
use super::model::{
    PostAction, SideloadBridge, SideloadCandidate, WebDownload, WebDownloadId, WebDownloadKind,
    WebDownloadRequest, WebDownloadState,
};
use super::post_process::{self, clean_game_title, delete_download_files};
use super::{RunningWebTask, StopReason};
use crate::{Backend, PowerAction, split_bandwidth_budget};

#[test]
fn clean_game_title_strips_versions_and_platform_tags() {
    assert_eq!(clean_game_title("Hades_v1.38_win64_portable.zip"), "Hades");
    assert_eq!(
        clean_game_title("Hollow.Knight.v1.5.78.Linux.x64.tar.gz"),
        "Hollow Knight"
    );
    assert_eq!(clean_game_title("Celeste [win64] (v1.4.0).7z"), "Celeste");
    assert_eq!(
        clean_game_title("setup_Stardew_Valley_1.6.exe"),
        "Stardew Valley"
    );
    assert_eq!(
        clean_game_title("Cyberpunk.2077.v2.12.zip"),
        "Cyberpunk 2077"
    );
}

#[test]
fn content_disposition_and_collision_dedup_work() {
    assert_eq!(
        parse_content_disposition_filename(
            "attachment; filename=\"fallback.zip\"; filename*=UTF-8''My%20Game%20v1.zip"
        )
        .as_deref(),
        Some("My Game v1.zip")
    );
    assert_eq!(
        parse_content_disposition_filename("attachment; filename=\"demo.7z\"").as_deref(),
        Some("demo.7z")
    );
    let tmp = tempfile::tempdir().unwrap();
    assert_eq!(unique_file_name(tmp.path(), "game.zip"), "game.zip");
    std::fs::write(tmp.path().join("game.zip"), b"existing").unwrap();
    assert_eq!(unique_file_name(tmp.path(), "game.zip"), "game (2).zip");
}

struct FakeBridge {
    registered: Mutex<Vec<SideloadCandidate>>,
}

#[irori_core::async_trait]
impl SideloadBridge for FakeBridge {
    async fn scan_extracted(&self, root: &Path) -> Option<SideloadCandidate> {
        let exe = root.join("Hades.exe");
        exe.is_file().then(|| SideloadCandidate {
            title: "Hades".into(),
            executable: exe,
            install_folder: Some(root.to_path_buf()),
            platform: InstallPlatform::Windows,
            launcher_args: String::new(),
            cover_art: None,
            background_art: None,
        })
    }

    async fn register_game(&self, candidate: SideloadCandidate) -> anyhow::Result<GameId> {
        let mut reg = self.registered.lock();
        if let Some(pos) = reg
            .iter()
            .position(|c| c.executable == candidate.executable)
        {
            return Ok(GameId::new(Runner::Sideload, format!("sl-{pos}")));
        }
        let idx = reg.len();
        reg.push(candidate);
        Ok(GameId::new(Runner::Sideload, format!("sl-{idx}")))
    }
}

static TEST_LOCK: Mutex<()> = Mutex::new(());

struct IsolatedBackend {
    backend: Backend,
    _state_dir: tempfile::TempDir,
    _guard: parking_lot::MutexGuard<'static, ()>,
}

impl IsolatedBackend {
    fn start() -> Self {
        let guard = TEST_LOCK.lock();
        crate::tests::isolate_paths();
        let state_dir = tempfile::tempdir().unwrap();
        let backend = Backend::start_headless(|_, _| Vec::new()).unwrap();
        {
            let mut w = backend.web_downloads.lock();
            w.state_path = Some(state_dir.path().join("web-downloads.json"));
            w.paused = false;
            w.active.clear();
            w.finished.clear();
            w.running.clear();
            w.save();
        }
        Self {
            backend,
            _state_dir: state_dir,
            _guard: guard,
        }
    }
}

impl Drop for IsolatedBackend {
    fn drop(&mut self) {
        let mut w = self.backend.web_downloads.lock();
        w.paused = false;
        w.active.clear();
        w.finished.clear();
        w.running.clear();
    }
}

#[test]
fn direct_url_download_extracts_portable_game_and_registers_play_action() {
    let ctx = IsolatedBackend::start();
    let b = &ctx.backend;
    let mut zip_buf = std::io::Cursor::new(Vec::new());
    {
        let mut zip = zip::ZipWriter::new(&mut zip_buf);
        let opts = zip::write::SimpleFileOptions::default();
        zip.start_file("Hades.exe", opts).unwrap();
        zip.write_all(b"MZ game").unwrap();
        zip.finish().unwrap();
    }
    let payload = zip_buf.into_inner();
    let tmp = tempfile::tempdir().unwrap();
    let install_root = tmp.path().join("Games");
    let dl_root = tmp.path().join("Downloads");
    std::fs::create_dir_all(&install_root).unwrap();
    std::fs::create_dir_all(&dl_root).unwrap();

    let _rt_guard = b.runtime().enter();
    let server = {
        let payload = payload.clone();
        TestServer::start(move |req| {
            if req.path == "/dl" {
                Resp {
                    status: 200,
                    headers: vec![
                        ("Content-Type".into(), "application/zip".into()),
                        (
                            "Content-Disposition".into(),
                            "attachment; filename=\"Hades_v1.38_win64_portable.zip\"".into(),
                        ),
                    ],
                    body: payload.clone(),
                    ..Default::default()
                }
            } else if req.path == "/interstitial" {
                Resp {
                    status: 200,
                    headers: vec![("Content-Type".into(), "text/html; charset=utf-8".into())],
                    body: b"<html><body>Click here</body></html>".to_vec(),
                    ..Default::default()
                }
            } else if req.path == "/plain" {
                Resp {
                    status: 200,
                    headers: vec![("Content-Type".into(), "application/octet-stream".into())],
                    body: b"rom-image-bytes".to_vec(),
                    ..Default::default()
                }
            } else {
                Resp::status(404)
            }
        })
    };
    b.settings.update_app(|s| {
        s.default_install_path = install_root.display().to_string();
        s.web_downloads.default_folder = dl_root.display().to_string();
        s.web_downloads.auto_extract = true;
        s.web_downloads.auto_add_to_library = true;
    });
    let bridge = Arc::new(FakeBridge {
        registered: Mutex::new(Vec::new()),
    });
    b.set_sideload_bridge(bridge.clone());

    let wait_finished = |id: &WebDownloadId| {
        b.runtime().block_on(async {
            for _ in 0..200 {
                if let Some(done) = b.web_downloads().finished.into_iter().find(|w| &w.id == id) {
                    return Some(done);
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
            None
        })
    };

    // 1. Portable game `.zip` -> extracts to default_install_path and registers `PostAction::Play`.
    let id = b
        .enqueue_web_download(WebDownloadRequest {
            source: server.url("/dl"),
            destination: Some(dl_root.clone()),
            ..Default::default()
        })
        .unwrap();
    let done = wait_finished(&id).expect("web download finished");
    assert_eq!(done.state, WebDownloadState::Done, "err={:?}", done.error);
    assert!(
        matches!(done.post_action, PostAction::Play(ref gid) if gid.runner == Runner::Sideload),
        "expected PostAction::Play, got {:?}",
        done.post_action
    );
    let installed_dir = done
        .installed_dir
        .expect("extracted into default_install_path");
    assert!(installed_dir.starts_with(&install_root));
    assert!(installed_dir.join("Hades.exe").is_file());
    assert_eq!(bridge.registered.lock().len(), 1);
    assert_eq!(bridge.registered.lock()[0].title, "Hades");

    // 2. Non-game file concludes cleanly as `Done` with `ShowInFolder` and zero error.
    let plain_id = b
        .enqueue_web_download(WebDownloadRequest {
            source: server.url("/plain"),
            destination: Some(dl_root.clone()),
            ..Default::default()
        })
        .unwrap();
    let plain_done = wait_finished(&plain_id).expect("plain download finished");
    assert_eq!(plain_done.state, WebDownloadState::Done);
    assert_eq!(plain_done.post_action, PostAction::ShowInFolder);
    assert!(plain_done.error.is_none());

    // 3. `text/html` interstitial fails clearly instead of saving bogus HTML as a file.
    let html_id = b
        .enqueue_web_download(WebDownloadRequest {
            source: server.url("/interstitial"),
            destination: Some(dl_root.clone()),
            ..Default::default()
        })
        .unwrap();
    let html_done = wait_finished(&html_id).expect("interstitial settled");
    assert_eq!(html_done.state, WebDownloadState::Failed);
    assert!(
        html_done
            .error
            .as_deref()
            .is_some_and(|e| e.contains("HTML")),
        "{:?}",
        html_done.error
    );

    // 4. Explicit user-supplied `req.title` is preserved instead of overwritten by Content-Disposition.
    let custom_id = b
        .enqueue_web_download(WebDownloadRequest {
            source: server.url("/dl"),
            title: Some("My Custom Game Title".into()),
            destination: Some(dl_root),
            auto_extract: Some(false),
            auto_add_to_library: Some(false),
            ..Default::default()
        })
        .unwrap();
    let custom_done = wait_finished(&custom_id).expect("custom-title download finished");
    assert_eq!(custom_done.state, WebDownloadState::Done);
    assert_eq!(custom_done.title, "My Custom Game Title");
}

#[test]
fn fs_guard_refuses_deleting_download_root_and_cleans_partials() {
    let tmp = tempfile::tempdir().unwrap();
    let dl_root = tmp.path().join("Downloads");
    std::fs::create_dir_all(&dl_root).unwrap();
    let app = irori_core::AppSettings {
        default_install_path: tmp.path().join("Games").display().to_string(),
        web_downloads: irori_core::WebDownloadPrefs {
            default_folder: dl_root.display().to_string(),
            ..Default::default()
        },
        ..Default::default()
    };

    let bad_item = WebDownload {
        id: WebDownloadId::new(),
        destination: dl_root.clone(),
        output_path: Some(dl_root.clone()),
        ..Default::default()
    };
    assert!(delete_download_files(&bad_item, &app).is_err());

    let file = dl_root.join("sample.bin");
    let part = irori_depot::path::part_path(&file);
    let seg = irori_depot::path::seg_path(&file);
    let aria2 = dl_root.join("sample.bin.aria2");
    std::fs::write(&part, b"p").unwrap();
    std::fs::write(&seg, b"s").unwrap();
    std::fs::write(&aria2, b"a").unwrap();

    let item = WebDownload {
        id: WebDownloadId::new(),
        destination: dl_root,
        output_path: Some(file),
        ..Default::default()
    };
    delete_download_files(&item, &app).unwrap();
    assert!(!part.exists());
    assert!(!seg.exists());
    assert!(!aria2.exists());
}

#[test]
fn systemic_when_done_stay_awake_offline_pause_and_bandwidth_budget() {
    let ctx = IsolatedBackend::start();
    let b = &ctx.backend;
    b.set_power_countdown(Duration::from_millis(200));

    assert_eq!(split_bandwidth_budget(0, true, true), (0, 0));
    assert_eq!(
        split_bandwidth_budget(10_000, true, false),
        (10_000, 10_000)
    );
    assert_eq!(
        split_bandwidth_budget(10_000, false, true),
        (10_000, 10_000)
    );
    let (d, a) = split_bandwidth_budget(10_001, true, true);
    assert_eq!(d + a, 10_001);

    let dl_dir = tempfile::tempdir().unwrap();
    let extracting_id = WebDownloadId::new();
    let user_paused_id = WebDownloadId::new();
    let queued_id = WebDownloadId::new();
    {
        let mut w = b.web_downloads.lock();
        w.active.clear();
        w.active.push(WebDownload {
            id: extracting_id.clone(),
            title: "ExtractingGame".into(),
            kind: WebDownloadKind::DirectUrl,
            destination: dl_dir.path().to_path_buf(),
            state: WebDownloadState::Extracting,
            bytes_total: 1000,
            bytes_done: 1000,
            ..Default::default()
        });
        w.active.push(WebDownload {
            id: user_paused_id.clone(),
            title: "UserPaused".into(),
            kind: WebDownloadKind::DirectUrl,
            destination: dl_dir.path().to_path_buf(),
            state: WebDownloadState::Paused,
            user_paused: true,
            bytes_total: 5000,
            bytes_done: 1000,
            ..Default::default()
        });
        w.active.push(WebDownload {
            id: queued_id.clone(),
            title: "QueuedGame".into(),
            kind: WebDownloadKind::DirectUrl,
            destination: dl_dir.path().to_path_buf(),
            state: WebDownloadState::Queued,
            user_paused: false,
            bytes_total: 2000,
            bytes_done: 500,
            ..Default::default()
        });
        // Prevent the background worker from picking up this synthetic item
        // while we exercise offline pause/resume state transitions directly.
        w.running.insert(
            queued_id.clone(),
            RunningWebTask::new(irori_core::process::CancellationToken::new()),
        );
    }

    assert!(b.has_active_transfers());

    let disks = b.download_disks();
    assert!(!disks.is_empty());
    assert_eq!(disks.iter().map(|d| d.needed).sum::<u64>(), 5500);

    b.set_when_downloads_done(Some(PowerAction::Sleep));
    b.queue_ran();
    assert!(b.downloads().countdown.is_none());

    b.pause_downloads_while_offline(true, false);
    assert!(b.web_downloads().paused);
    b.pause_downloads_while_offline(false, true);
    let snap = b.web_downloads();
    assert!(!snap.paused);
    let up = snap.active.iter().find(|w| w.id == user_paused_id).unwrap();
    assert_eq!(up.state, WebDownloadState::Paused);
    assert!(
        up.user_paused,
        "user-paused item must stay paused after reconnect"
    );
    let q = snap.active.iter().find(|w| w.id == queued_id).unwrap();
    assert_eq!(q.state, WebDownloadState::Queued);

    {
        let mut w = b.web_downloads.lock();
        w.active.retain(|item| item.id == user_paused_id);
    }
    assert!(!b.has_active_transfers(), "only paused item remains");
    b.queue_ran();
    assert!(
        b.downloads().countdown.is_none(),
        "WhenDone must not trigger when a web item is paused manually"
    );

    b.web_downloads.lock().active.clear();
    b.set_when_downloads_done(None);
}

#[test]
fn post_process_detects_password_required_errors() {
    let rar_pw = anyhow::Error::new(irori_mods::archive::PasswordRequired);
    assert!(post_process::is_password_error(&rar_pw));
    let zip_pw = anyhow::anyhow!("Password required to decrypt file");
    assert!(post_process::is_password_error(&zip_pw));
    let other = anyhow::anyhow!("corrupt CRC mismatch");
    assert!(!post_process::is_password_error(&other));
}

#[tokio::test]
async fn probe_sends_identity_encoding_and_remote_torrent_fetch_is_cancellable() {
    let seen = Arc::new(Mutex::new(Vec::<(String, String, Option<String>)>::new()));
    let seen_srv = seen.clone();
    let server = TestServer::start(move |req| {
        let ae = req
            .headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("accept-encoding"))
            .map(|(_, v)| v.clone());
        seen_srv
            .lock()
            .push((req.method.clone(), req.path.clone(), ae));
        match (req.method.as_str(), req.path.as_str()) {
            ("HEAD", "/head") => Resp {
                status: 200,
                headers: vec![("Content-Length".into(), "42".into())],
                ..Default::default()
            },
            ("HEAD", "/range") => Resp::status(405),
            ("GET", "/range") => Resp {
                status: 206,
                headers: vec![("Content-Range".into(), "bytes 0-0/1234".into())],
                body: vec![b'x'],
                ..Default::default()
            },
            ("GET", "/slow.torrent") => Resp {
                status: 200,
                hang_after: Some(10),
                body: b"d4:infod6:lengthi1e4:name1:a12:piece lengthi1e6:pieces20:12345678901234567890ee".to_vec(),
                ..Default::default()
            },
            _ => Resp::status(404),
        }
    });

    let cancel = irori_core::process::CancellationToken::new();
    match probe_direct_url(&server.url("/head"), &cancel)
        .await
        .unwrap()
    {
        DirectProbe::File { size, .. } => assert_eq!(size, 42),
        other => panic!("expected DirectProbe::File, got {other:?}"),
    }
    match probe_direct_url(&server.url("/range"), &cancel)
        .await
        .unwrap()
    {
        DirectProbe::File { size, .. } => assert_eq!(size, 1234),
        other => panic!("expected DirectProbe::File, got {other:?}"),
    }

    let recorded = seen.lock().clone();
    assert_eq!(recorded.len(), 3);
    for (method, path, ae) in &recorded {
        assert_eq!(
            ae.as_deref(),
            Some("identity"),
            "missing Accept-Encoding: identity on {method} {path}"
        );
    }

    let fetch_cancel = irori_core::process::CancellationToken::new();
    let trigger = fetch_cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(50)).await;
        trigger.cancel();
    });
    let started = std::time::Instant::now();
    let err = fetch_remote_torrent_bytes(&server.url("/slow.torrent"), &fetch_cancel)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("cancelled"), "{err:#}");
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "remote torrent fetch did not cancel promptly"
    );
}

#[test]
fn pause_and_offline_leave_local_extracting_and_installing_running() {
    let ctx = IsolatedBackend::start();
    let b = &ctx.backend;
    let dir = tempfile::tempdir().unwrap();

    let net_id = WebDownloadId::new();
    let ext_id = WebDownloadId::new();
    let inst_id = WebDownloadId::new();
    let net_cancel = irori_core::process::CancellationToken::new();
    let ext_cancel = irori_core::process::CancellationToken::new();
    let inst_cancel = irori_core::process::CancellationToken::new();

    {
        let mut w = b.web_downloads.lock();
        w.active.clear();
        w.running.clear();
        for (id, state, tok) in [
            (&net_id, WebDownloadState::Running, &net_cancel),
            (&ext_id, WebDownloadState::Extracting, &ext_cancel),
            (&inst_id, WebDownloadState::Installing, &inst_cancel),
        ] {
            w.active.push(WebDownload {
                id: id.clone(),
                title: format!("{state:?}"),
                destination: dir.path().to_path_buf(),
                state,
                ..Default::default()
            });
            w.running
                .insert(id.clone(), RunningWebTask::new(tok.clone()));
        }
    }

    b.pause_web_download(&ext_id);
    b.pause_web_download(&inst_id);
    assert!(!ext_cancel.is_cancelled());
    assert!(!inst_cancel.is_cancelled());

    b.pause_web_downloads_for_offline();
    assert!(net_cancel.is_cancelled());
    assert!(matches!(
        b.web_downloads
            .lock()
            .running
            .get(&net_id)
            .and_then(|r| r.stop),
        Some(StopReason::Pause {
            user_initiated: false
        })
    ));
    assert!(
        !ext_cancel.is_cancelled(),
        "offline drop must not cancel local extraction"
    );
    assert!(
        !inst_cancel.is_cancelled(),
        "offline drop must not cancel local installer"
    );

    b.pause_web_downloads();
    assert!(!ext_cancel.is_cancelled());
    assert!(!inst_cancel.is_cancelled());

    b.pause_web_downloads_for_shutdown();
    assert!(ext_cancel.is_cancelled());
    assert!(inst_cancel.is_cancelled());

    let mut w = b.web_downloads.lock();
    w.active.clear();
    w.running.clear();
}

#[test]
fn running_remove_waits_for_file_handle_and_bt_cancel_deletes_early_output() {
    let ctx = IsolatedBackend::start();
    let b = &ctx.backend;
    let tmp = tempfile::tempdir().unwrap();
    let dl_root = tmp.path().join("Downloads");
    std::fs::create_dir_all(&dl_root).unwrap();

    b.settings.update_app(|s| {
        s.web_downloads.bittorrent = true;
        s.web_downloads.default_folder = dl_root.display().to_string();
    });
    b.pause_web_downloads();

    // 1. Enqueuing a `.torrent` populates `output_path` immediately so cancel/remove cleans up
    //    both partial payload and `.aria2` control file before aria2 ever reaches `Done`.
    let torrent_bytes =
        b"d4:infod6:lengthi10e4:name10:SampleGame12:piece lengthi16384e6:pieces20:12345678901234567890ee";
    let torrent_file = tmp.path().join("sample.torrent");
    std::fs::write(&torrent_file, torrent_bytes).unwrap();
    let bt_id = b
        .enqueue_web_download(WebDownloadRequest {
            source: torrent_file.display().to_string(),
            destination: Some(dl_root.clone()),
            ..Default::default()
        })
        .unwrap();
    let queued = b
        .web_downloads()
        .active
        .into_iter()
        .find(|w| w.id == bt_id)
        .unwrap();
    assert_eq!(queued.output_path, Some(dl_root.join("SampleGame")));

    let partial_payload = dl_root.join("SampleGame");
    let aria2_ctrl = dl_root.join("SampleGame.aria2");
    std::fs::write(&partial_payload, b"partial-bt").unwrap();
    std::fs::write(&aria2_ctrl, b"aria2-state").unwrap();

    b.runtime()
        .block_on(b.remove_web_download(&bt_id, true))
        .unwrap();
    assert!(!partial_payload.exists());
    assert!(!aria2_ctrl.exists());

    // 2. `resolve_bt_output_path_opt` returns `None` when empty (never bare root) and
    //    `Some(<dest>/<name>)` as soon as `bittorrent_name` arrives in progress.
    let empty_status = Aria2TaskStatus {
        dir: dl_root.clone(),
        ..Default::default()
    };
    assert_eq!(resolve_bt_output_path_opt(&empty_status, &dl_root), None);
    let named_status = Aria2TaskStatus {
        dir: dl_root.clone(),
        bittorrent_name: Some("MagnetGame".into()),
        ..Default::default()
    };
    assert_eq!(
        resolve_bt_output_path_opt(&named_status, &dl_root),
        Some(dl_root.join("MagnetGame"))
    );

    // 3. `remove_web_download(id, true)` waits for a running worker to close its open file handle
    //    before deleting files (avoiding Windows sharing violations).
    let active_id = WebDownloadId::new();
    let locked_file = dl_root.join("in-flight.bin");
    let handle = std::fs::File::create(&locked_file).unwrap();
    let cancel = irori_core::process::CancellationToken::new();
    {
        let mut w = b.web_downloads.lock();
        w.active.push(WebDownload {
            id: active_id.clone(),
            title: "InFlight".into(),
            destination: dl_root.clone(),
            output_path: Some(locked_file.clone()),
            state: WebDownloadState::Running,
            ..Default::default()
        });
        w.running
            .insert(active_id.clone(), RunningWebTask::new(cancel.clone()));
    }
    let b_worker = b.clone();
    let id_worker = active_id.clone();
    b.spawn(async move {
        let file_guard = handle;
        cancel.cancelled().await;
        tokio::time::sleep(Duration::from_millis(60)).await;
        drop(file_guard);
        b_worker.web_downloads.lock().running.remove(&id_worker);
    });

    b.runtime()
        .block_on(b.remove_web_download(&active_id, true))
        .unwrap();
    assert!(!locked_file.exists());
    let snap = b.web_downloads();
    assert!(!snap.active.iter().any(|w| w.id == active_id));
    assert!(!snap.finished.iter().any(|w| w.id == active_id));
}
