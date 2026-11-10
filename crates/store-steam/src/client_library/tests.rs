use super::*;
use irori_core::{AppSettings, EventBus, Store as _};
use irori_steam_api::Endpoints;

fn answer(apps: serde_json::Value) -> serde_json::Value {
    serde_json::json!({"v": 1, "state": "ready", "apps": apps})
}

#[test]
fn reads_owned_and_borrowed_games() {
    let games = parse_answer(answer(serde_json::json!([
        {"id": 620, "name": " Portal 2 ", "owned": true, "borrowed": false,
         "installed": true, "minutes": 95, "lastPlayed": 1700000000},
        {"id": 70, "name": "Half-Life", "owned": false, "borrowed": true, "owner": 2,
         "minutes": 30},
        {"id": 400, "name": "Portal", "owned": false, "borrowed": false},
        {"id": 228980, "name": "Steamworks Common Redistributables", "owned": true},
        {"id": 3000000000u64, "name": "A shortcut", "owned": true},
        {"id": 10, "name": "", "owned": true},
        {"id": 620, "name": "Portal 2 again", "owned": true},
        {"id": 220, "name": "Half-Life 2"}
    ])))
    .unwrap()
    .games;
    let ids: Vec<&str> = games.iter().map(|g| g.app_id.as_str()).collect();
    assert_eq!(ids, ["220", "620", "70"]);
    let portal = &games[1];
    assert_eq!(portal.name, "Portal 2");
    assert_eq!(portal.playtime_minutes, 95);
    assert_eq!(portal.last_played, 1700000000);
    assert_eq!(portal.shared_by, None);
    let borrowed = &games[2];
    assert_eq!(borrowed.shared_by.as_deref(), Some("76561197960265730"));
    assert_eq!(borrowed.playtime_minutes, 0, "the owner's time isn't ours");
    // Without the ownership getters a game in the library counts as owned.
    assert_eq!(games[0].name, "Half-Life 2");
}

#[test]
fn reads_the_games_hidden_in_steam() {
    let mut value = answer(serde_json::json!([
        {"id": 620, "name": "Portal 2", "owned": true, "hidden": true},
        {"id": 70, "name": "Half-Life", "owned": true}
    ]));
    let read = parse_answer(value.clone()).unwrap();
    assert!(!read.hidden_known, "a client that didn't say");
    value["hiddenKnown"] = true.into();
    let read = parse_answer(value).unwrap();
    assert!(read.hidden_known);
    let hidden: Vec<(&str, bool)> = read
        .games
        .iter()
        .map(|g| (g.app_id.as_str(), g.hidden))
        .collect();
    assert_eq!(hidden, [("620", true), ("70", false)]);
    // The library cache only writes the flag when set, and reads files
    // written before it existed.
    let json = serde_json::to_value(&read.games[1]).unwrap();
    assert!(json.get("hidden").is_none());
    let old: LibraryGame =
        serde_json::from_str(r#"{"appId": "70", "name": "Half-Life", "unknown": 1}"#).unwrap();
    assert!(!old.hidden);
}

#[test]
fn tells_loading_from_a_changed_client() {
    let loading = serde_json::json!({"v": 1, "state": "loading"});
    assert_eq!(parse_answer(loading), Err(ClientLibraryError::Starting));
    let missing = serde_json::json!({"v": 1, "state": "missing"});
    assert_eq!(parse_answer(missing), Err(ClientLibraryError::Unsupported));
    let newer = serde_json::json!({"v": 2, "state": "ready", "apps": []});
    assert_eq!(parse_answer(newer), Err(ClientLibraryError::Unsupported));
    assert_eq!(
        parse_answer(serde_json::Value::Null),
        Err(ClientLibraryError::Unsupported)
    );
    assert_eq!(
        parse_answer(serde_json::json!("text")),
        Err(ClientLibraryError::Unsupported)
    );
}

#[test]
fn the_expression_only_reads() {
    // A guard against edits that would make the expression change Steam:
    // it reads the library collection and calls ownership getters only.
    for forbidden in [
        "SteamClient",
        "RunGame",
        "Install",
        "Uninstall",
        "fetch(",
        "localStorage",
    ] {
        assert!(!EXPRESSION.contains(forbidden), "{forbidden}");
    }
    assert!(EXPRESSION.contains("collectionStore"));
    assert!(EXPRESSION.contains("allGamesCollection"));
    assert!(EXPRESSION.contains(r#"GetCollection("hidden")"#));
    // No setter of collections.
    for setter in ["SetAppsAsHidden", "AddApps", "RemoveApps", "Save"] {
        assert!(!EXPRESSION.contains(setter), "{setter}");
    }
}

fn state() -> ClientLibraryState {
    ClientLibraryState {
        steam_found: true,
        can_restart_steam: true,
        ..Default::default()
    }
}

#[test]
fn suggests_only_while_it_would_help() {
    assert!(state().suggest());
    for unhelpful in [
        ClientLibraryState {
            steam_found: false,
            ..state()
        },
        ClientLibraryState {
            signed_in: true,
            ..state()
        },
        ClientLibraryState {
            read_at: Some(1),
            ..state()
        },
        ClientLibraryState {
            mode: ClientLibraryMode::Once,
            ..state()
        },
        ClientLibraryState {
            hint_dismissed: true,
            ..state()
        },
        ClientLibraryState {
            can_restart_steam: false,
            ..state()
        },
    ] {
        assert!(!unhelpful.suggest(), "{unhelpful:?}");
    }
}

#[test]
fn waits_only_for_what_starting_steam_fixes() {
    let wait = |e: ClientLibraryError| worth_waiting(&anyhow::Error::new(e));
    assert!(wait(ClientLibraryError::NotRunning));
    assert!(wait(ClientLibraryError::DebuggingOff { millennium: false }));
    assert!(wait(ClientLibraryError::Starting));
    assert!(!wait(ClientLibraryError::Unsupported));
    assert!(!wait(ClientLibraryError::NotSteam { port: 8080 }));
    let refused = anyhow::Error::new(std::io::Error::from(std::io::ErrorKind::ConnectionRefused))
        .context("connecting");
    assert!(worth_waiting(&refused));
    assert!(!worth_waiting(&anyhow::anyhow!("something else")));
}

fn store(steam: &Path) -> Arc<SteamStore> {
    let settings = Arc::new(AppSettings::default());
    let dead = "http://127.0.0.1:9".to_string();
    SteamStore::with_layout(
        EventBus::new(),
        Arc::new(move || settings.clone()),
        steam.to_path_buf(),
        &steam.join("irori"),
        Endpoints {
            api: dead.clone(),
            store: dead.clone(),
            assets: dead,
        },
    )
}

fn steam_folder() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let steam = dir.path().join("Steam");
    std::fs::create_dir_all(steam.join("steamapps/common/Portal 2")).unwrap();
    std::fs::write(
        steam.join("steamapps/appmanifest_620.acf"),
        "\"AppState\" { \"appid\" \"620\" \"name\" \"Portal 2\" \"StateFlags\" \"4\" \"installdir\" \"Portal 2\" }",
    )
    .unwrap();
    std::fs::create_dir_all(marker::marker_path(&steam).parent().unwrap()).unwrap();
    (dir, steam)
}

#[tokio::test]
async fn turns_the_switch_on_and_off_without_touching_anothers_file() {
    let (_dir, steam) = steam_folder();
    let s = store(&steam);
    s.init().await.unwrap();
    assert!(s.client_library_state().suggest());

    s.enable_client_library(ClientLibraryMode::Once)
        .await
        .unwrap();
    let on = s.client_library_state();
    assert!(on.marker && on.own_marker);
    assert_eq!(on.mode, ClientLibraryMode::Once);
    assert!(marker::exists(&steam));
    assert!(!on.suggest());

    // A new start remembers the choice.
    let again = store(&steam);
    again.init().await.unwrap();
    assert_eq!(again.client_library_state(), on);

    s.disable_client_library().await.unwrap();
    let off = s.client_library_state();
    assert!(!off.marker && !off.own_marker);
    assert_eq!(off.mode, ClientLibraryMode::Off);
    assert!(!marker::exists(&steam));

    // A file another program put there stays when Irori switches off.
    std::fs::write(marker::marker_path(&steam), b"").unwrap();
    s.enable_client_library(ClientLibraryMode::Keep)
        .await
        .unwrap();
    assert!(!s.client_library_state().own_marker);
    s.disable_client_library().await.unwrap();
    assert!(marker::exists(&steam), "Decky's or the user's file stays");
    assert!(s.client_library_state().marker);

    s.dismiss_client_library_hint().await.unwrap();
    let again = store(&steam);
    again.init().await.unwrap();
    assert!(again.client_library_state().hint_dismissed);
}

#[tokio::test]
async fn lists_what_was_read_until_an_account_signs_in() {
    let (_dir, steam) = steam_folder();
    let s = store(&steam);
    s.init().await.unwrap();
    assert_eq!(s.games().len(), 1);

    let read = ClientRead {
        games: vec![
            LibraryGame {
                app_id: "620".into(),
                name: "Portal 2".into(),
                playtime_minutes: 95,
                ..Default::default()
            },
            LibraryGame {
                app_id: "70".into(),
                name: "Half-Life".into(),
                playtime_minutes: 42,
                hidden: true,
                ..Default::default()
            },
        ],
        hidden_known: true,
    };
    s.enable_client_library(ClientLibraryMode::Once)
        .await
        .unwrap();
    let mut events = s.bus.subscribe();
    assert_eq!(s.keep_client_games(read).await.unwrap(), 2);
    let ids: Vec<String> = s.games().iter().map(|g| g.app_name.clone()).collect();
    assert_eq!(ids, ["620", "70"]);
    assert!(s.game("620").unwrap().is_installed);
    let half_life = s.game("70").unwrap();
    assert!(!half_life.is_installed && half_life.installable);
    assert!(half_life.cover_art().unwrap().contains("/70/"));
    // Hidden in Steam, as the client said; no files of Steam say otherwise.
    assert!(half_life.store_hidden);
    assert!(!s.game("620").unwrap().store_hidden);
    assert_eq!(
        s.hidden_games(),
        crate::SteamHiddenGames {
            known: true,
            count: 1,
            signed_in: false,
            steam_found: true,
        }
    );
    assert_eq!(s.remote_playtime().await.unwrap()["70"], 42);
    let mut seen = Vec::new();
    while let Ok(e) = events.try_recv() {
        seen.push(e);
    }
    assert!(
        seen.iter()
            .any(|e| matches!(e, Event::LibraryChanged(Runner::Steam)))
    );
    // Read once: the file Irori created is gone again; the games stay.
    let state = s.client_library_state();
    assert_eq!(state.mode, ClientLibraryMode::Off);
    assert!(!state.marker && !marker::exists(&steam));
    assert_eq!(state.games, 2);
    assert!(state.read_at.is_some());
    assert!(!state.suggest());

    // A new start lists them from the cache.
    let again = store(&steam);
    again.init().await.unwrap();
    assert_eq!(again.games().len(), 2);

    // A signed-in account's library wins.
    *s.session.write() = Some(
        crate::account::parse_login_input(
            r#"{"accountName":"a","steamid":"1","refreshToken":"r","accessToken":"x"}"#,
        )
        .unwrap(),
    );
    s.scan().await.unwrap();
    assert_eq!(s.games().len(), 1);
    assert!(s.client_library_state().signed_in);
    *s.session.write() = None;
    s.scan().await.unwrap();
    assert_eq!(s.games().len(), 2);

    s.forget_client_library().await.unwrap();
    assert_eq!(s.games().len(), 1);
    assert_eq!(s.client_library_state().read_at, None);
}

#[tokio::test]
async fn keeps_reading_on_refresh_only_when_asked() {
    let (_dir, steam) = steam_folder();
    let s = store(&steam);
    s.init().await.unwrap();
    // Off: a refresh never looks at the client.
    assert!(!s.refresh_client_library(true).await);
    s.enable_client_library(ClientLibraryMode::Keep)
        .await
        .unwrap();
    s.keep_client_games(ClientRead {
        games: vec![LibraryGame {
            app_id: "70".into(),
            name: "Half-Life".into(),
            ..Default::default()
        }],
        hidden_known: false,
    })
    .await
    .unwrap();
    let state = s.client_library_state();
    assert_eq!(state.mode, ClientLibraryMode::Keep, "kept on");
    assert!(state.marker && marker::exists(&steam));
    // Read moments ago, so a plain refresh doesn't read again.
    assert!(!s.refresh_client_library(false).await);
}

/// A stand-in for Steam's DevTools endpoint on a free port: the page list,
/// then one WebSocket answering the evaluation with `value`.
async fn devtools_server(value: serde_json::Value) -> (u16, tokio::task::JoinHandle<String>) {
    use futures::{SinkExt as _, StreamExt as _};
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let handle = tokio::spawn(async move {
        let (mut http, _) = listener.accept().await.unwrap();
        let mut buf = vec![0; 4096];
        let n = http.read(&mut buf).await.unwrap();
        assert!(String::from_utf8_lossy(&buf[..n]).starts_with("GET /json HTTP/1.1"));
        let list = format!(
            r#"[{{"title": "SharedJSContext", "type": "page",
                "url": "https://steamloopback.host/index.html",
                "webSocketDebuggerUrl": "ws://127.0.0.1:{port}/devtools/page/X"}}]"#
        );
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{list}",
            list.len()
        );
        http.write_all(response.as_bytes()).await.unwrap();
        drop(http);
        let (socket, _) = listener.accept().await.unwrap();
        let mut ws = tokio_tungstenite::accept_async(socket).await.unwrap();
        let request = ws.next().await.unwrap().unwrap().into_text().unwrap();
        let request: serde_json::Value = serde_json::from_str(request.as_str()).unwrap();
        // An event without an id comes first, as Steam's pages send them.
        ws.send(tokio_tungstenite::tungstenite::Message::text(
            r#"{"method": "Runtime.consoleAPICalled", "params": {}}"#,
        ))
        .await
        .unwrap();
        let reply = serde_json::json!({
            "id": request["id"],
            "result": {"result": {"type": "object", "value": value}}
        });
        ws.send(tokio_tungstenite::tungstenite::Message::text(
            reply.to_string(),
        ))
        .await
        .unwrap();
        request["method"].as_str().unwrap_or_default().to_string()
            + " "
            + request["params"]["expression"].as_str().unwrap_or_default()
    });
    (port, handle)
}

#[tokio::test]
async fn reads_the_library_over_devtools() {
    let (port, server) = devtools_server(answer(serde_json::json!([
        {"id": 70, "name": "Half-Life", "owned": true, "minutes": 3}
    ])))
    .await;
    let games = read_port(port).await.unwrap().games;
    assert_eq!(games.len(), 1);
    assert_eq!(games[0].app_id, "70");
    let asked = server.await.unwrap();
    assert!(asked.starts_with("Runtime.evaluate "), "{asked}");
    assert!(asked.ends_with(EXPRESSION));
}

#[tokio::test]
async fn reports_a_client_still_loading() {
    let (port, _server) = devtools_server(serde_json::json!({"v": 1, "state": "loading"})).await;
    let e = read_port(port).await.unwrap_err();
    assert_eq!(
        e.downcast_ref::<ClientLibraryError>(),
        Some(&ClientLibraryError::Starting)
    );
}

#[tokio::test]
async fn nothing_listening_is_not_an_error_of_its_own() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    assert!(devtools::list(port).await.unwrap().is_none());
}

/// Another program holding Steam's debugging port is told apart from
/// Steam, however it answers, so the user hears why and what helps.
#[tokio::test]
async fn another_program_on_the_port_is_not_steam() {
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    let answers: [&[u8]; 4] = [
        b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n",
        b"HTTP/1.1 200 OK\r\nContent-Type: text/html\r\n\r\n<html>Router</html>",
        b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n2\r\n[]\r\n0\r\n\r\n",
        b"SSH-2.0-OpenSSH_9.6\r\n",
    ];
    for answer in answers {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buf = vec![0; 4096];
            let _ = socket.read(&mut buf).await;
            let _ = socket.write_all(answer).await;
        });
        let e = read_port(port).await.unwrap_err();
        assert_eq!(
            e.downcast_ref::<ClientLibraryError>(),
            Some(&ClientLibraryError::NotSteam { port }),
            "{}: {e:#}",
            String::from_utf8_lossy(answer)
        );
    }
}
