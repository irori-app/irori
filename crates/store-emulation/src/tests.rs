use std::path::PathBuf;
use std::sync::Arc;

use irori_core::{AppSettings, GameSettings, LaunchEnvironment, LaunchRequest, OpContext};
use once_cell::sync::Lazy;

use super::*;

static ROOT: Lazy<tempfile::TempDir> = Lazy::new(|| {
    let dir = tempfile::tempdir().expect("temp dir");
    Paths::init(Paths::under(dir.path()));
    dir
});

/// A fresh folder under the test root, with Paths pointing there.
fn test_dir(name: &str) -> PathBuf {
    let root = ROOT.path();
    assert!(Paths::get().config.starts_with(root));
    let dir = root.join(name);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn touch(path: &std::path::Path, bytes: &[u8]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
}

fn store(dir: &std::path::Path, app: AppSettings) -> Arc<EmulationStore> {
    let app = Arc::new(app);
    EmulationStore::with_dirs(
        EventBus::new(),
        Arc::new(move || app.clone()),
        dir.join("config"),
        dir.join("cache"),
    )
}

fn request(settings: GameSettings, dir: &std::path::Path) -> LaunchRequest {
    LaunchRequest {
        settings,
        environment: LaunchEnvironment::default(),
        option: None,
        extra_args: Vec::new(),
        offline: false,
        skip_version_check: false,
        log_file: dir.join("game.log"),
    }
}

#[tokio::test]
async fn scans_folders_and_remembers_removals() {
    let dir = test_dir("scan");
    let roms = dir.join("roms");
    touch(
        &roms.join("SNES/Super Metroid (Japan, USA) (En,Ja).sfc"),
        b"x",
    );
    touch(&roms.join("GBA/Golden Sun (USA).gba"), b"x");
    let app = AppSettings {
        rom_folders: vec![roms.display().to_string()],
        ..Default::default()
    };
    let s = store(&dir, app.clone());
    s.init().await.unwrap();
    assert!(s.games().is_empty());
    s.refresh(false).await.unwrap();
    let games = s.games();
    assert_eq!(games.len(), 2);
    let metroid = games.iter().find(|g| g.title == "Super Metroid").unwrap();
    assert_eq!(metroid.system.as_deref(), Some("snes"));
    assert_eq!(metroid.tags, ["Japan, USA"]);
    assert!(metroid.is_installed);
    assert_eq!(metroid.runner(), Runner::Emulation);
    let art = metroid.art_cover.as_deref().unwrap();
    assert!(art.starts_with(
        "https://thumbnails.libretro.com/Nintendo%20-%20Super%20Nintendo%20Entertainment%20System/Named_Boxarts/"
    ));
    assert!(s.is_available(&metroid.app_name).await);

    // Removing keeps the file and survives a rescan and a restart.
    s.uninstall(&metroid.app_name, RemoveArgs::default())
        .await
        .unwrap();
    assert!(
        roms.join("SNES/Super Metroid (Japan, USA) (En,Ja).sfc")
            .exists()
    );
    s.refresh(false).await.unwrap();
    assert_eq!(s.games().len(), 1);
    let again = store(&dir, app);
    again.init().await.unwrap();
    assert_eq!(again.games().len(), 1);
}

#[tokio::test]
async fn launches_with_the_chosen_emulator_and_checks_bios() {
    let dir = test_dir("launch");
    let roms = dir.join("roms");
    touch(&roms.join("DOS/Keen/KEEN1.EXE"), b"MZ");
    touch(&roms.join("PS1/Game (USA).chd"), b"x");
    let dosbox = dir.join("bin/dosbox");
    let duck = dir.join("bin/duckstation");
    touch(&dosbox, b"#!/bin/sh");
    touch(&duck, b"#!/bin/sh");
    irori_engines::install::set_custom("dosbox-staging", Some(&dosbox)).unwrap();
    irori_engines::install::set_custom("duckstation", Some(&duck)).unwrap();
    let bios_dir = dir.join("bios");
    std::fs::create_dir_all(&bios_dir).unwrap();
    let app = AppSettings {
        rom_folders: vec![roms.display().to_string()],
        bios_folder: bios_dir.display().to_string(),
        ..Default::default()
    };
    let s = store(&dir, app);
    s.init().await.unwrap();
    s.refresh(false).await.unwrap();
    let keen = s.games().into_iter().find(|g| g.title == "Keen").unwrap();
    let plan = s
        .launch(
            &keen.app_name,
            &request(GameSettings::default(), &dir),
            OpContext::detached(),
        )
        .await
        .unwrap();
    assert_eq!(plan.command.program, dosbox);
    assert!(
        plan.command
            .args
            .iter()
            .any(|a| a.to_string_lossy().ends_with("KEEN1.EXE"))
    );

    // PS1 needs a BIOS: without one the launch says so, typed.
    let ps1 = s.games().into_iter().find(|g| g.title == "Game").unwrap();
    let err = s
        .launch(
            &ps1.app_name,
            &request(GameSettings::default(), &dir),
            OpContext::detached(),
        )
        .await
        .unwrap_err();
    let missing = BiosMissing::of(&err).expect("typed BIOS error");
    assert_eq!(missing.system, "psx");
    assert!(missing.files.contains(&"scph5501.bin".to_string()));

    // A file with the expected name is used even when its digest is unknown.
    touch(&bios_dir.join("scph5501.bin"), b"dump");
    irori_engines::bios::scan(&bios_dir, &irori_engines::bios::BiosIndex::default()).unwrap();
    let plan = s
        .launch(
            &ps1.app_name,
            &request(GameSettings::default(), &dir),
            OpContext::detached(),
        )
        .await
        .unwrap();
    assert_eq!(plan.command.program, duck);

    // A game pinned to a version that is gone fails with a typed error.
    let pinned = GameSettings {
        engine_version: "managed:0.1".into(),
        ..Default::default()
    };
    let err = s
        .launch(&ps1.app_name, &request(pinned, &dir), OpContext::detached())
        .await
        .unwrap_err();
    assert_eq!(
        EngineMissing::of(&err)
            .and_then(|m| m.pinned.clone())
            .as_deref(),
        Some("0.1")
    );
}

#[tokio::test]
async fn adds_games_outside_the_folders() {
    let dir = test_dir("add");
    let rom = dir.join("elsewhere/Kirby's Dream Land (USA, Europe).gb");
    touch(&rom, b"x");
    let keen = dir.join("elsewhere/Keen");
    touch(&keen.join("KEEN4E.EXE"), b"MZ");
    let s = store(&dir, AppSettings::default());
    s.init().await.unwrap();
    let id = s.add_game(&rom, None, None).await.unwrap();
    let game = s.game(&id.app_name).unwrap();
    assert_eq!(game.title, "Kirby's Dream Land");
    assert_eq!(game.system.as_deref(), Some("gb"));
    // Folders need their system; DOS folders get their program.
    assert!(s.add_game(&keen, None, None).await.is_err());
    let keen_id = s
        .add_game(&keen, Some("dos"), Some("Commander Keen 4".into()))
        .await
        .unwrap();
    assert!(
        s.entry(&keen_id.app_name)
            .unwrap()
            .path
            .ends_with("KEEN4E.EXE")
    );
    // Adding twice keeps one entry; a rescan keeps added games.
    s.add_game(&rom, None, None).await.unwrap();
    s.refresh(false).await.unwrap();
    assert_eq!(s.games().len(), 2);
    // Removing an added game forgets it.
    s.uninstall(&id.app_name, RemoveArgs::default())
        .await
        .unwrap();
    assert_eq!(s.games().len(), 1);
    let again = store(&dir, AppSettings::default());
    again.init().await.unwrap();
    assert_eq!(again.games().len(), 1);
}

/// LaunchBox's database as served: a zip holding `Metadata.xml`.
fn launchbox_archive(xml: &str) -> Vec<u8> {
    use std::io::Write as _;
    let mut buf = std::io::Cursor::new(Vec::new());
    {
        let mut w = zip::ZipWriter::new(&mut buf);
        w.start_file("Metadata.xml", zip::write::SimpleFileOptions::default())
            .unwrap();
        w.write_all(xml.as_bytes()).unwrap();
        w.finish().unwrap();
    }
    buf.into_inner()
}

const LAUNCHBOX_XML: &str = r#"<?xml version="1.0" standalone="yes"?>
<LaunchBox>
  <Game>
    <Name>Super Mario Bros. 3</Name>
    <DatabaseID>10</DatabaseID>
    <Platform>Nintendo Entertainment System</Platform>
  </Game>
  <GameImage>
    <DatabaseID>10</DatabaseID>
    <FileName>us-box.jpg</FileName>
    <Type>Box - Front</Type>
    <Region>North America</Region>
  </GameImage>
  <GameImage>
    <DatabaseID>10</DatabaseID>
    <FileName>eu-box.jpg</FileName>
    <Type>Box - Front</Type>
    <Region>Europe</Region>
  </GameImage>
</LaunchBox>"#;

/// The game titled `title` once its cover is `cover`, waiting for the
/// background download and lookup.
async fn cover_of(s: &EmulationStore, title: &str, cover: &str) -> Arc<GameInfo> {
    for _ in 0..200 {
        if let Some(g) = s.games().into_iter().find(|g| g.title == title)
            && g.art_cover.as_deref() == Some(cover)
        {
            return g;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    panic!("{title} never got {cover}: {:?}", s.games());
}

#[tokio::test]
async fn covers_are_launchboxs_box_of_the_games_region() {
    use irori_depot::test_server::{Resp, TestServer};
    let dir = test_dir("launchbox");
    let roms = dir.join("roms");
    touch(
        &roms.join("NES/Super Mario Bros. 3 (Europe).nes"),
        b"NES\x1a",
    );
    touch(&roms.join("NES/Homebrew Quest (World).nes"), b"NES\x1a");
    let body = launchbox_archive(LAUNCHBOX_XML);
    let server = TestServer::start(move |_| Resp {
        status: 200,
        body: body.clone(),
        ..Default::default()
    });
    let app = Arc::new(AppSettings {
        rom_folders: vec![roms.display().to_string()],
        launchbox: true,
        ..Default::default()
    });
    let make = || {
        let app = app.clone();
        EmulationStore::with_parts(
            EventBus::new(),
            Arc::new(move || app.clone()),
            dir.join("config"),
            dir.join("cache"),
            Some(server.url("/Metadata.zip")),
        )
    };
    let s = make();
    s.init().await.unwrap();
    // Nothing is downloaded while the library is empty.
    assert!(server.requests().is_empty());
    assert_eq!(s.launchbox_status(), LaunchBoxStatus::Missing);

    // Once it has games, the database comes on its own.
    s.refresh(false).await.unwrap();
    let mario = cover_of(
        &s,
        "Super Mario Bros. 3",
        "https://images.launchbox-app.com/eu-box.jpg",
    )
    .await;
    assert_eq!(s.launchbox_status(), LaunchBoxStatus::Ready);
    // A game LaunchBox doesn't have keeps libretro's thumbnail.
    let other = s
        .games()
        .into_iter()
        .find(|g| g.title == "Homebrew Quest")
        .unwrap();
    assert!(
        other
            .art_cover
            .as_deref()
            .unwrap()
            .starts_with("https://thumbnails.libretro.com/")
    );

    // The picker gets every box, the game's region first.
    let game = s.launchbox_game(&mario.app_name).unwrap();
    let files: Vec<&str> = game.boxes.iter().map(|b| b.file.as_str()).collect();
    assert_eq!(files, ["eu-box.jpg", "us-box.jpg"]);
    let found = s.search_launchbox(&other.app_name, "mario").await.unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].name, "Super Mario Bros. 3");
    assert!(s.launchbox_game(&other.app_name).is_none());

    // A week isn't over: a rescan asks nothing.
    s.refresh(false).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    assert_eq!(server.requests().len(), 1);

    // After a restart the covers come from the index on disk.
    let again = make();
    again.init().await.unwrap();
    cover_of(
        &again,
        "Super Mario Bros. 3",
        "https://images.launchbox-app.com/eu-box.jpg",
    )
    .await;
    assert_eq!(server.requests().len(), 1);

    // Removed, the covers go back to libretro's and nothing is stored.
    assert!(again.launchbox_stored().await.is_some());
    again.remove_launchbox().await.unwrap();
    assert!(again.launchbox_stored().await.is_none());
    assert_eq!(again.launchbox_status(), LaunchBoxStatus::Missing);
    let mario = again
        .games()
        .into_iter()
        .find(|g| g.title == "Super Mario Bros. 3")
        .unwrap();
    assert!(
        mario
            .art_cover
            .as_deref()
            .unwrap()
            .starts_with("https://thumbnails.libretro.com/")
    );
}

#[tokio::test]
async fn launchbox_off_never_downloads_on_its_own() {
    use irori_depot::test_server::{Resp, TestServer};
    let dir = test_dir("launchbox-off");
    let roms = dir.join("roms");
    touch(
        &roms.join("NES/Super Mario Bros. 3 (Europe).nes"),
        b"NES\x1a",
    );
    let body = launchbox_archive(LAUNCHBOX_XML);
    let server = TestServer::start(move |_| Resp {
        status: 200,
        body: body.clone(),
        ..Default::default()
    });
    let app = Arc::new(AppSettings {
        rom_folders: vec![roms.display().to_string()],
        launchbox: false,
        ..Default::default()
    });
    let s = EmulationStore::with_parts(
        EventBus::new(),
        Arc::new(move || app.clone()),
        dir.join("config"),
        dir.join("cache"),
        Some(server.url("/Metadata.zip")),
    );
    s.init().await.unwrap();
    s.refresh(false).await.unwrap();
    let rom = dir.join("roms/Other (USA).nes");
    touch(&rom, b"NES\x1a");
    s.add_game(&rom, None, None).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    assert!(server.requests().is_empty());
    assert_eq!(s.launchbox_status(), LaunchBoxStatus::Missing);

    // Asked for, it downloads all the same.
    s.download_launchbox().await.unwrap();
    assert_eq!(server.requests().len(), 1);
    assert_eq!(s.launchbox_status(), LaunchBoxStatus::Ready);
}

#[tokio::test]
async fn test_stores_never_download_launchbox() {
    let dir = test_dir("launchbox-offline");
    let roms = dir.join("roms");
    touch(
        &roms.join("NES/Super Mario Bros. 3 (Europe).nes"),
        b"NES\x1a",
    );
    let s = store(
        &dir,
        AppSettings {
            rom_folders: vec![roms.display().to_string()],
            ..Default::default()
        },
    );
    s.init().await.unwrap();
    s.refresh(false).await.unwrap();
    assert!(s.download_launchbox().await.is_err());
    assert_eq!(s.launchbox_status(), LaunchBoxStatus::Missing);
}
