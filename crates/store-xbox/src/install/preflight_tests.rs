use super::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[test]
fn streaming_repair_selection_includes_short_ciphertext_even_with_matching_package_metadata() {
    for (size, included) in [
        (0usize, true),
        (5000, true),
        (8191, true),
        (8192, false),
        (9000, false),
    ] {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("Game.exe");
        let bytes = vec![9u8; size];
        std::fs::write(&path, &bytes).unwrap();
        let make = || {
            HashMap::from([(
                "Game.exe".to_owned(),
                SegmentFile {
                    offset: 0,
                    length: 5000,
                    keep_encrypted: true,
                    data_hashs: vec![[7; 20]],
                },
            )])
        };
        assert_eq!(
            !pending_files(root.path(), &make(), &make())
                .unwrap()
                .is_empty(),
            included
        );
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
    }
    let root = tempfile::tempdir().unwrap();
    let make = || {
        HashMap::from([(
            "Missing.exe".to_owned(),
            SegmentFile {
                offset: 0,
                length: 5000,
                keep_encrypted: true,
                data_hashs: vec![[7; 20]],
            },
        )])
    };
    assert_eq!(
        pending_files(root.path(), &make(), &make()).unwrap().len(),
        1
    );
}

#[test]
fn streaming_repair_selection_preserves_unchanged_files_and_only_targets_current_package() {
    let root = tempfile::tempdir().unwrap();
    for file in [
        "Game.exe",
        "Dlc.exe",
        "Plain.bin",
        "OtherGame.exe",
        "Save.sav",
    ] {
        std::fs::write(root.path().join(file), vec![3u8; 8192]).unwrap();
    }
    let make = || {
        HashMap::from([
            (
                "Game.exe".to_owned(),
                SegmentFile {
                    offset: 0,
                    length: 5000,
                    keep_encrypted: true,
                    data_hashs: vec![[7; 20]],
                },
            ),
            (
                "Plain.bin".to_owned(),
                SegmentFile {
                    offset: 0,
                    length: 5000,
                    keep_encrypted: false,
                    data_hashs: vec![[7; 20]],
                },
            ),
        ])
    };
    assert!(
        pending_files(root.path(), &make(), &make())
            .unwrap()
            .is_empty()
    );
    let local = make();
    let mut changed = make();
    changed.get_mut("Game.exe").unwrap().data_hashs[0] = [8; 20];
    assert_eq!(
        pending_files(root.path(), &changed, &local).unwrap()[0].0,
        "Game.exe"
    );
    let mut resized = make();
    resized.get_mut("Plain.bin").unwrap().length = 0;
    assert_eq!(
        pending_files(root.path(), &resized, &local).unwrap()[0].0,
        "Plain.bin"
    );
    for file in [
        "Game.exe",
        "Dlc.exe",
        "Plain.bin",
        "OtherGame.exe",
        "Save.sav",
    ] {
        assert_eq!(
            std::fs::read(root.path().join(file)).unwrap(),
            vec![3u8; 8192]
        );
    }
}

#[test]
fn streaming_repair_selection_rejects_nonfile_and_symlinked_encrypted_paths() {
    let root = tempfile::tempdir().unwrap();
    let make = || {
        HashMap::from([(
            "Game.exe".to_owned(),
            SegmentFile {
                offset: 0,
                length: 5000,
                keep_encrypted: true,
                data_hashs: vec![[7; 20]],
            },
        )])
    };
    std::fs::create_dir(root.path().join("Game.exe")).unwrap();
    assert!(pending_files(root.path(), &make(), &make()).is_err());
    std::fs::remove_dir(root.path().join("Game.exe")).unwrap();
    let outside = tempfile::NamedTempFile::new().unwrap();
    std::os::unix::fs::symlink(outside.path(), root.path().join("Game.exe")).unwrap();
    assert!(pending_files(root.path(), &make(), &make()).is_err());
}

#[tokio::test]
async fn streaming_metadata_failure_returns_error_without_panic_or_touching_existing_game() {
    for response in [
        "HTTP/1.1 403 Test\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        "HTTP/1.1 200 OK\r\nContent-Length: 3\r\nConnection: close\r\n\r\nabc",
    ] {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("game.exe"), b"existing-game").unwrap();
        std::fs::write(root.path().join("dlc.bin"), b"existing-dlc").unwrap();
        std::fs::write(root.path().join(PACKAGE_FILE), b"existing-marker").unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = vec![];
            while !request.ends_with(b"\r\n\r\n") {
                request.push(socket.read_u8().await.unwrap());
            }
            socket.write_all(response.as_bytes()).await.unwrap();
        });
        let client = irori_msixvc::http_client().unwrap();
        let url = format!("http://{address}/?token=PRIVATE-SECRET");
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            Remote::open(&client, &url, root.path()),
        )
        .await
        .unwrap();
        let Err(error) = result else {
            panic!("a broken package was accepted");
        };
        assert!(!error.to_string().contains("PRIVATE-SECRET"));
        for (file, expected) in [
            ("game.exe", "existing-game"),
            ("dlc.bin", "existing-dlc"),
            (PACKAGE_FILE, "existing-marker"),
        ] {
            assert_eq!(
                std::fs::read(root.path().join(file)).unwrap(),
                expected.as_bytes()
            );
        }
        server.await.unwrap();
    }
}

#[test]
fn streaming_update_preserves_uninstalled_optional_archive_placeholders() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("optional.ucas"), []).unwrap();
    let files = HashMap::from([(
        "optional.ucas".to_owned(),
        SegmentFile {
            offset: 0,
            length: 8192,
            keep_encrypted: false,
            data_hashs: vec![[7; 20]; 2],
        },
    )]);
    assert!(
        pending_files(root.path(), &files, &files)
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        std::fs::metadata(root.path().join("optional.ucas"))
            .unwrap()
            .len(),
        0
    );
}
