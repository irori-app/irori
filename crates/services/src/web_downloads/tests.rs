//! Unit tests for torrent bencode hardening, source classification, aria2
//! config/spawn security, mock WebSocket JSON-RPC (`followedBy` + immediate
//! zero-seed removal + BEP-9 preview), and `aria2_download` SHA-256 pinning.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use futures::{SinkExt as _, StreamExt as _};
use irori_core::process::CancellationToken;
use irori_depot::test_server::{Resp, TestServer};
use parking_lot::Mutex;
use serde_json::Value;
use sha2::{Digest as _, Sha256};
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::Message;

use super::aria2::{
    Aria2Client, BtRunOutcome, build_aria2_conf, build_spawn_spec,
    fetch_magnet_metadata_with_client, run_bt_download_with_client,
};
use super::aria2_download;
use super::model::DownloadSource;
use super::torrent_meta::{self, classify_source, format_select_file, parse_torrent};

fn bencode_bytes(data: &[u8]) -> Vec<u8> {
    let mut out = format!("{}:", data.len()).into_bytes();
    out.extend_from_slice(data);
    out
}

fn bencode_dict(entries: &[(&[u8], Vec<u8>)]) -> Vec<u8> {
    let mut out = vec![b'd'];
    for (k, v) in entries {
        out.extend(bencode_bytes(k));
        out.extend(v);
    }
    out.push(b'e');
    out
}

fn bencode_list(items: &[Vec<u8>]) -> Vec<u8> {
    let mut out = vec![b'l'];
    for item in items {
        out.extend(item);
    }
    out.push(b'e');
    out
}

fn bencode_int(n: i64) -> Vec<u8> {
    format!("i{n}e").into_bytes()
}

fn sample_multifile_torrent_with_pad() -> Vec<u8> {
    let file1 = bencode_dict(&[
        (b"length", bencode_int(1000)),
        (
            b"path",
            bencode_list(&[bencode_bytes(b"bin"), bencode_bytes(b"game.exe")]),
        ),
    ]);
    let pad = bencode_dict(&[
        (b"attr", bencode_bytes(b"p")),
        (b"length", bencode_int(512)),
        (
            b"path",
            bencode_list(&[bencode_bytes(b".pad"), bencode_bytes(b"512")]),
        ),
    ]);
    let file3 = bencode_dict(&[
        (b"length", bencode_int(2500)),
        (b"path", bencode_list(&[bencode_bytes(b"data.pak")])),
    ]);
    let info = bencode_dict(&[
        (b"files", bencode_list(&[file1, pad, file3])),
        (b"name", bencode_bytes(b"CoolGame")),
        (b"piece length", bencode_int(16384)),
        (b"pieces", bencode_bytes(&[0xAB; 20])),
    ]);
    bencode_dict(&[(b"info", info)])
}

#[test]
fn bencode_parses_multifile_and_preserves_pad_indices() {
    let raw = sample_multifile_torrent_with_pad();
    let preview = parse_torrent(&raw).unwrap();
    assert_eq!(preview.name, "CoolGame");
    assert_eq!(preview.info_hash.len(), 40);
    assert_eq!(preview.total_bytes, 3500);
    assert_eq!(preview.files.len(), 2);
    assert_eq!(preview.files[0].index, 1);
    assert_eq!(preview.files[0].path, "bin/game.exe");
    assert_eq!(preview.files[1].index, 3);
    assert_eq!(preview.files[1].path, "data.pak");
    assert_eq!(format_select_file(&[3, 1, 3, 0]), Some("1,3".to_string()));
}

#[test]
fn bencode_rejects_traversal_bad_utf8_deep_nesting_and_v2_only() {
    let evil_file = bencode_dict(&[
        (b"length", bencode_int(10)),
        (
            b"path",
            bencode_list(&[bencode_bytes(b".."), bencode_bytes(b"escape.exe")]),
        ),
    ]);
    let traversal = bencode_dict(&[(
        b"info",
        bencode_dict(&[
            (b"files", bencode_list(&[evil_file])),
            (b"name", bencode_bytes(b"Bad")),
            (b"pieces", bencode_bytes(&[0u8; 20])),
        ]),
    )]);
    assert!(parse_torrent(&traversal).is_err());

    let bad_utf8 = bencode_dict(&[(
        b"info",
        bencode_dict(&[
            (b"length", bencode_int(10)),
            (b"name", bencode_bytes(&[0xFF, 0xFE])),
            (b"pieces", bencode_bytes(&[0u8; 20])),
        ]),
    )]);
    assert!(parse_torrent(&bad_utf8).is_err());

    let mut deep = Vec::new();
    deep.extend(std::iter::repeat_n(b'l', 70));
    deep.extend(std::iter::repeat_n(b'e', 70));
    assert!(parse_torrent(&deep).is_err());

    let huge = vec![b'0'; torrent_meta::MAX_TORRENT_BYTES + 1];
    assert!(parse_torrent(&huge).is_err());

    let v2_only = bencode_dict(&[(
        b"info",
        bencode_dict(&[
            (b"file tree", bencode_dict(&[])),
            (b"meta version", bencode_int(2)),
            (b"name", bencode_bytes(b"V2OnlyGame")),
        ]),
    )]);
    let err = parse_torrent(&v2_only).unwrap_err().to_string();
    assert!(err.contains("BitTorrent v2-only"), "{err}");

    let hybrid = bencode_dict(&[(
        b"info",
        bencode_dict(&[
            (b"file tree", bencode_dict(&[])),
            (b"length", bencode_int(4096)),
            (b"meta version", bencode_int(2)),
            (b"name", bencode_bytes(b"HybridGame.zip")),
            (b"pieces", bencode_bytes(&[0x11; 20])),
        ]),
    )]);
    let parsed = parse_torrent(&hybrid).unwrap();
    assert_eq!(parsed.name, "HybridGame.zip");
    assert_eq!(parsed.total_bytes, 4096);
}

#[test]
fn source_classifier_handles_valid_inputs_and_rejects_unsafe_schemes() {
    assert!(matches!(
        classify_source("https://example.com/files/game.zip").unwrap(),
        DownloadSource::DirectUrl(_)
    ));
    assert!(matches!(
        classify_source("https://example.com/releases/demo.torrent").unwrap(),
        DownloadSource::RemoteTorrentUrl(_)
    ));
    assert!(matches!(
        classify_source(r"C:\Downloads\game.torrent").unwrap(),
        DownloadSource::LocalTorrent(_)
    ));
    assert!(matches!(
        classify_source("/tmp/game.torrent").unwrap(),
        DownloadSource::LocalTorrent(_)
    ));
    let hex_hash = "0123456789abcdef0123456789abcdef01234567";
    let mag = classify_source(&format!("magnet:?xt=urn:btih:{hex_hash}&dn=Sample%20Game")).unwrap();
    assert_eq!(
        mag,
        DownloadSource::Magnet {
            uri: format!("magnet:?xt=urn:btih:{hex_hash}&dn=Sample%20Game"),
            display_name: Some("Sample Game".into()),
            info_hash: hex_hash.into(),
        }
    );
    let b32 = classify_source("magnet:?xt=urn:btih:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA").unwrap();
    assert!(matches!(b32, DownloadSource::Magnet { info_hash, .. } if info_hash == "0".repeat(40)));
    let v2_err = classify_source(
        "magnet:?xt=urn:btmh:1220cafebabecafebabecafebabecafebabecafebabecafebabecafebabecafebabe",
    )
    .unwrap_err()
    .to_string();
    assert!(v2_err.contains("v2-only"), "{v2_err}");

    for bad in [
        "file:///etc/passwd",
        "file:///C:/Users/me/game.torrent",
        "ftp://example.com/game.zip",
        "javascript:alert(1)",
        "data:text/plain,hello",
    ] {
        assert!(classify_source(bad).is_err(), "{bad} must be rejected");
    }
}

#[test]
fn aria2_conf_and_spawn_spec_enforce_loopback_zero_seed_and_no_secret_in_argv() {
    let cache = PathBuf::from("/cache/aria2");
    let conf = build_aria2_conf(45123, "deadbeefsecret", &cache, 1024 * 500);
    assert!(conf.contains("enable-rpc=true"));
    assert!(conf.contains("rpc-listen-all=false"));
    assert!(conf.contains("rpc-listen-port=45123"));
    assert!(conf.contains("rpc-secret=deadbeefsecret"));
    assert!(conf.contains("seed-time=0"));
    assert!(conf.contains("bt-seed-unverified=false"));
    assert!(conf.contains("bt-save-metadata=true"));
    assert!(conf.contains("bt-remove-unselected-file=true"));
    assert!(conf.contains("http-accept-gzip=false"));
    assert!(conf.contains("file-allocation=none"));
    assert!(conf.contains("max-overall-download-limit=512000"));

    let conf_path = Path::new("/cache/aria2/rpc-test.conf");
    let native_spec = build_spawn_spec(Path::new("/usr/bin/aria2c"), conf_path, false, 4242);
    let native_args: Vec<String> = native_spec
        .args
        .iter()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    assert!(native_args.iter().any(|a| a == "--stop-with-process=4242"));
    assert!(native_args.iter().any(|a| a.starts_with("--conf-path=")));
    assert!(
        !native_args.iter().any(|a| a.contains("rpc-secret")),
        "rpc-secret must never appear in argv"
    );

    let host_spec = build_spawn_spec(Path::new("/usr/bin/aria2c"), conf_path, true, 4242);
    let host_args: Vec<String> = host_spec
        .args
        .iter()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    assert!(
        !host_args.iter().any(|a| a.contains("--stop-with-process")),
        "Flatpak host execution must not pass --stop-with-process"
    );
}

#[tokio::test]
async fn mock_ws_aria2_handles_followed_by_and_immediate_zero_seed_removal() {
    let dest = tempfile::tempdir().unwrap();
    let payload_file = dest.path().join("CoolGame").join("game.exe");
    std::fs::create_dir_all(payload_file.parent().unwrap()).unwrap();
    std::fs::write(&payload_file, b"MZ").unwrap();

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let secret = "test-secret-token";
    type RpcCallLog = Arc<Mutex<Vec<(String, Vec<Value>)>>>;
    let calls: RpcCallLog = Arc::new(Mutex::new(Vec::new()));
    let calls_srv = calls.clone();
    let dest_str = dest.path().to_string_lossy().into_owned();
    let file_str = payload_file.to_string_lossy().into_owned();

    let server = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let mut ws = tokio_tungstenite::accept_async(socket).await.unwrap();
        let mut pushed_meta_complete = false;
        let mut pushed_bt_complete = false;

        while let Some(Ok(Message::Text(raw))) = ws.next().await {
            let req: Value = serde_json::from_str(raw.as_str()).unwrap();
            let id = req["id"].as_str().unwrap().to_string();
            let method = req["method"].as_str().unwrap().to_string();
            let params = req["params"].as_array().cloned().unwrap_or_default();
            assert_eq!(params[0].as_str(), Some("token:test-secret-token"));
            calls_srv
                .lock()
                .push((method.clone(), params[1..].to_vec()));

            let result = match method.as_str() {
                "aria2.addUri" => Value::String("gid-meta-1".into()),
                "aria2.tellStatus" => {
                    let gid = params[1].as_str().unwrap();
                    if gid == "gid-meta-1" {
                        serde_json::json!({
                            "gid": "gid-meta-1",
                            "status": "complete",
                            "followedBy": ["gid-payload-2"],
                            "dir": dest_str,
                            "totalLength": "0",
                            "completedLength": "0",
                            "downloadSpeed": "0",
                            "files": []
                        })
                    } else {
                        serde_json::json!({
                            "gid": "gid-payload-2",
                            "status": "complete",
                            "dir": dest_str,
                            "totalLength": "4096",
                            "completedLength": "4096",
                            "downloadSpeed": "0",
                            "uploadSpeed": "0",
                            "connections": "2",
                            "numSeeders": "1",
                            "bittorrent": { "info": { "name": "CoolGame" } },
                            "files": [
                                { "path": file_str, "selected": "true", "length": "4096", "completedLength": "4096" }
                            ]
                        })
                    }
                }
                "aria2.changeOption"
                | "aria2.forceRemove"
                | "aria2.remove"
                | "aria2.removeDownloadResult"
                | "aria2.changeGlobalOption" => Value::String("OK".into()),
                _ => Value::Null,
            };
            let reply = serde_json::json!({ "jsonrpc": "2.0", "id": id, "result": result });
            ws.send(Message::text(reply.to_string())).await.unwrap();

            if method == "aria2.addUri" && !pushed_meta_complete {
                pushed_meta_complete = true;
                ws.send(Message::text(
                    serde_json::json!({
                        "jsonrpc": "2.0",
                        "method": "aria2.onDownloadComplete",
                        "params": [{ "gid": "gid-meta-1" }]
                    })
                    .to_string(),
                ))
                .await
                .unwrap();
            }
            if method == "aria2.changeOption" && !pushed_bt_complete {
                pushed_bt_complete = true;
                ws.send(Message::text(
                    serde_json::json!({
                        "jsonrpc": "2.0",
                        "method": "aria2.onBtDownloadComplete",
                        "params": [{ "gid": "gid-payload-2" }]
                    })
                    .to_string(),
                ))
                .await
                .unwrap();
            }
        }
    });

    let ws_url = format!("ws://127.0.0.1:{port}/jsonrpc");
    let client = Aria2Client::connect(&ws_url, secret.into()).await.unwrap();
    client.change_global_download_limit(500_000).await.unwrap();

    let cancel = CancellationToken::new();
    let outcome = run_bt_download_with_client(
        &client,
        None,
        Some("magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567"),
        dest.path(),
        &[1, 3],
        &cancel,
        &|| false,
        &|_| {},
    )
    .await
    .unwrap();

    match outcome {
        BtRunOutcome::Done {
            output_path,
            total_bytes,
            title,
        } => {
            assert_eq!(output_path, payload_file);
            assert_eq!(total_bytes, 4096);
            assert_eq!(title.as_deref(), Some("CoolGame"));
        }
        other => panic!("expected Done, got {other:?}"),
    }

    let recorded = calls.lock().clone();
    assert!(recorded.iter().any(|(m, p)| {
        m == "aria2.changeGlobalOption" && p[0]["max-overall-download-limit"] == "500000"
    }));
    assert!(
        recorded
            .iter()
            .any(|(m, p)| m == "aria2.removeDownloadResult" && p[0] == "gid-meta-1")
    );
    assert!(
        recorded
            .iter()
            .any(|(m, p)| m == "aria2.forceRemove" && p[0] == "gid-payload-2"),
        "expected immediate forceRemove on onBtDownloadComplete: {recorded:?}"
    );
    assert!(
        recorded
            .iter()
            .any(|(m, p)| m == "aria2.removeDownloadResult" && p[0] == "gid-payload-2")
    );

    drop(client);
    let _ = tokio::time::timeout(Duration::from_secs(2), server).await;
}

#[tokio::test]
async fn mock_ws_bep9_magnet_metadata_preview_parses_saved_torrent() {
    let scratch = tempfile::tempdir().unwrap();
    let scratch_path = scratch.path().join("meta-preview");
    let torrent_bytes = sample_multifile_torrent_with_pad();

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let saved_dir = scratch_path.clone();

    let server = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let mut ws = tokio_tungstenite::accept_async(socket).await.unwrap();
        while let Some(Ok(Message::Text(raw))) = ws.next().await {
            let req: Value = serde_json::from_str(raw.as_str()).unwrap();
            let id = req["id"].as_str().unwrap().to_string();
            let method = req["method"].as_str().unwrap().to_string();
            let result = match method.as_str() {
                "aria2.addUri" => {
                    std::fs::create_dir_all(&saved_dir).unwrap();
                    std::fs::write(saved_dir.join("abcdef.torrent"), &torrent_bytes).unwrap();
                    Value::String("gid-bep9".into())
                }
                _ => Value::String("OK".into()),
            };
            ws.send(Message::text(
                serde_json::json!({ "jsonrpc": "2.0", "id": id, "result": result }).to_string(),
            ))
            .await
            .unwrap();
            if method == "aria2.addUri" {
                ws.send(Message::text(
                    serde_json::json!({
                        "jsonrpc": "2.0",
                        "method": "aria2.onDownloadComplete",
                        "params": [{ "gid": "gid-bep9" }]
                    })
                    .to_string(),
                ))
                .await
                .unwrap();
            }
        }
    });

    let client = Aria2Client::connect(&format!("ws://127.0.0.1:{port}/jsonrpc"), "sec".into())
        .await
        .unwrap();
    let preview = fetch_magnet_metadata_with_client(
        &client,
        "magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567",
        &scratch_path,
        Duration::from_secs(5),
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    assert_eq!(preview.name, "CoolGame");
    assert_eq!(preview.files.len(), 2);
    assert_eq!(preview.files[1].index, 3);
    assert!(preview.torrent_bytes.is_some());
    assert!(!scratch_path.exists(), "scratch dir cleaned up");

    drop(client);
    let _ = tokio::time::timeout(Duration::from_secs(2), server).await;
}

#[tokio::test]
async fn aria2_download_verifies_sha256_and_extracts_only_aria2c_exe() {
    let mut buf = std::io::Cursor::new(Vec::new());
    {
        let mut zip = zip::ZipWriter::new(&mut buf);
        let opts = zip::write::SimpleFileOptions::default();
        zip.start_file("aria2-1.37.0-win-64bit-build1/aria2c.exe", opts)
            .unwrap();
        zip.write_all(b"MZ aria2").unwrap();
        zip.start_file("aria2-1.37.0-win-64bit-build1/README.html", opts)
            .unwrap();
        zip.write_all(b"<html></html>").unwrap();
        zip.start_file("../../evil.exe", opts).unwrap();
        zip.write_all(b"evil").unwrap();
        zip.finish().unwrap();
    }
    let archive = buf.into_inner();
    let good_sha = hex::encode(Sha256::digest(&archive));
    let bad_sha = "0".repeat(64);
    let server = {
        let archive = archive.clone();
        TestServer::start(move |req| {
            if req.path.ends_with(aria2_download::ARCHIVE_NAME) {
                Resp {
                    status: 200,
                    body: archive.clone(),
                    ..Default::default()
                }
            } else {
                Resp::status(404)
            }
        })
    };

    let dir = tempfile::tempdir().unwrap();
    let cancel = CancellationToken::new();
    let err = aria2_download::install_from(
        &server.url(""),
        aria2_download::ARCHIVE_NAME,
        &bad_sha,
        dir.path(),
        &cancel,
    )
    .await
    .unwrap_err();
    assert!(format!("{err:#}").contains("SHA-256"), "{err:#}");
    assert!(!dir.path().join("aria2c.exe").exists());

    let installed = aria2_download::install_from(
        &server.url(""),
        aria2_download::ARCHIVE_NAME,
        &good_sha,
        dir.path(),
        &cancel,
    )
    .await
    .unwrap();
    assert_eq!(installed, dir.path().join("aria2c.exe"));
    assert_eq!(std::fs::read(&installed).unwrap(), b"MZ aria2");
    assert!(!dir.path().parent().unwrap().join("evil.exe").exists());
    let entries: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(entries, ["aria2c.exe"]);
}
