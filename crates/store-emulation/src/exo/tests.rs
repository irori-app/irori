use std::path::{Path, PathBuf};
use std::sync::Arc;

use irori_core::{AppSettings, EventBus, Store as _};

use crate::EmulationStore;
use crate::exo::media::{MediaIndex, media_key, resolve_manual};
use crate::exo::xml::{
    MAX_XML_BYTES, extract_language, extract_shortcode_and_lang, normalize_release_date,
    parse_reader,
};
use crate::exo::{ExoCollectionKind, is_exo_root, scan_root};
use crate::scan::{ScanCache, scan};

fn touch(path: &Path, bytes: &[u8]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
}

#[test]
fn parses_launchbox_xml_with_entities_and_genres() {
    let xml = r#"<?xml version="1.0" standalone="yes"?>
<LaunchBox>
  <Game>
    <Title>Doom &amp; Friends: Part &#39;1&#39;</Title>
    <Platform>MS-DOS</Platform>
    <Developer>id Software</Developer>
    <Publisher>id Software</Publisher>
    <ReleaseDate>1993-12-10T00:00:00+00:00</ReleaseDate>
    <Genre>Action; First-Person Shooter</Genre>
    <Notes>Classic &lt;FPS&gt; game.</Notes>
    <Series>Language: German</Series>
    <Region>Europe</Region>
    <ApplicationPath>eXo\eXoDOS\!dos\!german\doom\DOOM (1993).bat</ApplicationPath>
    <ManualPath>Manuals\MS-DOS\DOOM.pdf</ManualPath>
  </Game>
  <Game>
    <Title></Title>
    <Platform>MS-DOS</Platform>
  </Game>
</LaunchBox>"#;
    let games = parse_reader(xml.as_bytes()).unwrap();
    assert_eq!(games.len(), 1);
    let g = &games[0];
    assert_eq!(g.title, "Doom & Friends: Part '1'");
    assert_eq!(g.platform.as_deref(), Some("MS-DOS"));
    assert_eq!(g.developer.as_deref(), Some("id Software"));
    assert_eq!(g.release_date.as_deref(), Some("1993-12-10"));
    assert_eq!(g.genres, ["Action", "First-Person Shooter"]);
    assert_eq!(g.notes.as_deref(), Some("Classic <FPS> game."));
    assert_eq!(
        extract_shortcode_and_lang(g.application_path.as_deref().unwrap(), "!dos"),
        Some(("doom".into(), Some("!german".into())))
    );
    assert_eq!(
        extract_language(g.series.as_deref(), Some("!german")).as_deref(),
        Some("DE")
    );
    assert_eq!(normalize_release_date("1995"), Some("1995".into()));
    assert_eq!(normalize_release_date("unknown"), None);
}

#[test]
fn rejects_malformed_and_incomplete_xml() {
    const { assert!(MAX_XML_BYTES >= 64 * 1024 * 1024) };
    let bad = "<LaunchBox><Game><Title>Unclosed";
    assert!(parse_reader(bad.as_bytes()).is_ok()); // EOF without closing Game drops partial entry safely
    let malformed = "<LaunchBox><Game></Wrong></LaunchBox>";
    assert!(parse_reader(malformed.as_bytes()).is_err());
}

#[test]
fn resolves_local_media_and_rejects_manual_traversal() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    touch(
        &root.join("Images/MS-DOS/Box - Front/Prince of Persia_ The Sands-01.jpg"),
        b"jpg",
    );
    touch(
        &root.join("Images/MS-DOS/Clear Logo/Prince of Persia_ The Sands-01.png"),
        b"png",
    );
    touch(
        &root.join("Images/MS-DOS/Fanart - Background/Prince of Persia_ The Sands-01.jpg"),
        b"bg",
    );
    touch(
        &root.join("Images/MS-DOS/Screenshot - Gameplay/Prince of Persia_ The Sands-01.png"),
        b"s1",
    );
    touch(
        &root.join("Images/MS-DOS/Screenshot - Game Title/Prince of Persia_ The Sands-02.png"),
        b"s2",
    );
    touch(&root.join("Manuals/MS-DOS/Prince.pdf"), b"%PDF");

    assert_eq!(
        media_key("Prince of Persia: The Sands"),
        media_key("Prince of Persia_ The Sands-01")
    );
    let idx = MediaIndex::load(root, "MS-DOS");
    let found = idx.lookup("Prince of Persia: The Sands");
    assert!(found.cover.is_some());
    assert!(found.logo.is_some());
    assert!(found.background.is_some());
    assert_eq!(found.screenshots.len(), 2);

    assert!(resolve_manual(root, Some(r"Manuals\MS-DOS\Prince.pdf")).is_some());
    assert!(resolve_manual(root, Some(r"..\..\etc\passwd")).is_none());
}

#[test]
fn scans_extracted_zipped_and_skips_lite_stubs() {
    let dir = tempfile::tempdir().unwrap();
    let exo = dir.path().join("eXoDOS");
    let xml = r#"<LaunchBox>
  <Game>
    <Title>DOOM</Title>
    <Platform>MS-DOS</Platform>
    <Developer>id Software</Developer>
    <Publisher>id Software</Publisher>
    <ReleaseDate>1993-12-10T00:00:00+00:00</ReleaseDate>
    <Genre>Action; Shooter</Genre>
    <Notes>Fight demons on Mars.</Notes>
    <ApplicationPath>eXo\eXoDOS\!dos\doom\DOOM (1993).bat</ApplicationPath>
  </Game>
  <Game>
    <Title>Quake</Title>
    <Platform>MS-DOS</Platform>
    <Developer>id Software</Developer>
    <ReleaseDate>1996-06-22T00:00:00+00:00</ReleaseDate>
    <ApplicationPath>eXo\eXoDOS\!dos\quake\Quake (1996).bat</ApplicationPath>
  </Game>
  <Game>
    <Title>Missing Lite Game</Title>
    <Platform>MS-DOS</Platform>
    <ApplicationPath>eXo\eXoDOS\!dos\missing\Missing Lite Game (1995).bat</ApplicationPath>
  </Game>
</LaunchBox>"#;
    touch(&exo.join("Data/Platforms/MS-DOS.xml"), xml.as_bytes());
    // Extracted game with dosbox.conf and native script.
    touch(&exo.join("eXo/eXoDOS/!dos/doom/dosbox.conf"), b"[autoexec]");
    if cfg!(windows) {
        touch(
            &exo.join("eXo/eXoDOS/!dos/doom/DOOM (1993).bat"),
            b"@echo off",
        );
    } else {
        touch(
            &exo.join("eXo/eXoDOS/!dos/doom/DOOM (1993).command"),
            b"#!/bin/sh",
        );
    }
    touch(&exo.join("eXo/eXoDOS/doom/DOOM.EXE"), b"MZ");
    touch(
        &exo.join("Images/MS-DOS/Box - Front/DOOM-01.png"),
        b"\x89PNG\r\n\x1a\n",
    );

    // Packed game (only .zip present on disk, not extracted yet).
    touch(
        &exo.join("eXo/eXoDOS/!dos/quake/dosbox.conf"),
        b"[autoexec]",
    );
    touch(&exo.join("eXo/eXoDOS/Quake (1996).zip"), b"PK\x05\x06");

    // Lite stub: only config folder exists, no extracted dir and no zip.
    touch(
        &exo.join("eXo/eXoDOS/!dos/missing/dosbox.conf"),
        b"[autoexec]",
    );

    // Win3x + ScummVM entries inside the same unified root.
    let win3x_xml = r#"<LaunchBox>
  <Game>
    <Title>SkiFree</Title>
    <Platform>Windows 3x</Platform>
    <ApplicationPath>eXo\eXoWin3x\!win3x\skifree\SkiFree (1991).bat</ApplicationPath>
  </Game>
</LaunchBox>"#;
    touch(
        &exo.join("Data/Platforms/Windows 3x.xml"),
        win3x_xml.as_bytes(),
    );
    touch(
        &exo.join("eXo/eXoWin3x/!win3x/skifree/dosbox.conf"),
        b"[autoexec]",
    );
    touch(&exo.join("eXo/eXoWin3x/skifree/SKI32.EXE"), b"MZ");

    let scumm_xml = r#"<LaunchBox>
  <Game>
    <Title>Day of the Tentacle</Title>
    <Platform>ScummVM</Platform>
    <ApplicationPath>eXo\eXoScummVM\!ScummVM\tentacle\Day of the Tentacle (1993).bat</ApplicationPath>
  </Game>
</LaunchBox>"#;
    touch(
        &exo.join("Data/Platforms/ScummVM.xml"),
        scumm_xml.as_bytes(),
    );
    touch(&exo.join("eXo/eXoScummVM/tentacle/TENTACLE.000"), b"data");

    assert!(is_exo_root(&exo));
    let found = scan_root(&exo, &exo);
    assert_eq!(found.len(), 4);

    let doom = found.iter().find(|e| e.title == "DOOM").unwrap();
    let doom_exo = doom.exo.as_ref().unwrap();
    assert_eq!(doom.system, "dos");
    assert_eq!(doom_exo.collection, ExoCollectionKind::ExoDos);
    assert!(doom_exo.installed);
    assert!(doom_exo.conf_path.is_some());
    assert!(doom_exo.script_path.is_some());
    assert!(doom_exo.local_cover.is_some());
    assert!(doom.tags.contains(&"eXoDOS".to_string()));

    let quake = found.iter().find(|e| e.title == "Quake").unwrap();
    let quake_exo = quake.exo.as_ref().unwrap();
    assert!(!quake_exo.installed);
    assert!(quake_exo.archive_path.is_some());

    let skifree = found.iter().find(|e| e.title == "SkiFree").unwrap();
    assert_eq!(skifree.system, "win3x");
    assert_eq!(
        skifree.exo.as_ref().unwrap().collection,
        ExoCollectionKind::ExoWin3x
    );

    let tentacle = found
        .iter()
        .find(|e| e.title == "Day of the Tentacle")
        .unwrap();
    assert_eq!(tentacle.system, "scummvm");
    assert_eq!(
        tentacle.exo.as_ref().unwrap().collection,
        ExoCollectionKind::ExoScummVm
    );

    // Also works when the parent folder is passed to scan() instead of the eXo root itself.
    let via_parent = scan(
        &[dir.path().to_path_buf()],
        &ScanCache::new(),
        &dir.path().join("playlists"),
        &|_| {},
    );
    assert_eq!(via_parent.entries.len(), 4);
    assert_eq!(via_parent.unrecognized, 0);
}

#[tokio::test]
async fn emulation_store_publishes_exo_metadata_art_and_install_state() {
    let base = crate::tests::test_dir("store-exo-meta");
    let exo = base.join("eXoDOS");
    let xml = r#"<LaunchBox>
  <Game>
    <Title>Commander Keen</Title>
    <Platform>MS-DOS</Platform>
    <Developer>id Software</Developer>
    <Publisher>Apogee</Publisher>
    <ReleaseDate>1990-12-14T00:00:00+00:00</ReleaseDate>
    <Genre>Platform</Genre>
    <Notes>Billy Blaze saves the galaxy.</Notes>
    <ApplicationPath>eXo\eXoDOS\!dos\keen1\Commander Keen (1990).bat</ApplicationPath>
  </Game>
  <Game>
    <Title>Packed Game</Title>
    <Platform>MS-DOS</Platform>
    <ApplicationPath>eXo\eXoDOS\!dos\packed\Packed Game (1992).bat</ApplicationPath>
  </Game>
</LaunchBox>"#;
    touch(&exo.join("Data/Platforms/MS-DOS.xml"), xml.as_bytes());
    touch(
        &exo.join("eXo/eXoDOS/!dos/keen1/dosbox.conf"),
        b"[autoexec]",
    );
    touch(&exo.join("eXo/eXoDOS/keen1/KEEN1.EXE"), b"MZ");
    // Minimal 1x1 PNG (width=1, height=1 -> CoverSlot::Square)
    let png_1x1: &[u8] = &[
        0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, b'I', b'H', b'D',
        b'R', 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x00, 0x00, 0x00,
    ];
    touch(
        &exo.join("Images/MS-DOS/Box - Front/Commander Keen-01.png"),
        png_1x1,
    );
    touch(
        &exo.join("Images/MS-DOS/Screenshot - Gameplay/Commander Keen-01.png"),
        png_1x1,
    );
    touch(
        &exo.join("eXo/eXoDOS/!dos/packed/dosbox.conf"),
        b"[autoexec]",
    );
    touch(
        &exo.join("eXo/eXoDOS/Packed Game (1992).zip"),
        b"PK\x05\x06",
    );

    let app = Arc::new(AppSettings {
        rom_folders: vec![exo.display().to_string()],
        ..Default::default()
    });
    let s = EmulationStore::with_dirs(
        EventBus::new(),
        Arc::new(move || app.clone()),
        base.join("config"),
        base.join("cache"),
    );
    s.init().await.unwrap();
    s.refresh(false).await.unwrap();

    let games = s.games();
    assert_eq!(games.len(), 2);

    let keen = games.iter().find(|g| g.title == "Commander Keen").unwrap();
    assert!(keen.is_installed);
    assert!(!keen.installable);
    assert_eq!(keen.developer.as_deref(), Some("id Software"));
    assert_eq!(keen.publisher.as_deref(), Some("Apogee"));
    assert_eq!(keen.genres, ["Platform"]);
    assert_eq!(keen.release_date.as_deref(), Some("1990-12-14"));
    assert!(
        keen.art_cover
            .as_deref()
            .is_some_and(|u| u.starts_with("file://"))
    );

    let extra = s.extra_info(&keen.app_name).await.unwrap();
    assert_eq!(
        extra.description.as_deref(),
        Some("Billy Blaze saves the galaxy.")
    );
    assert_eq!(extra.screenshots.len(), 1);
    assert!(extra.screenshots[0].starts_with("file://"));
    assert_eq!(extra.details_from.as_deref(), Some("eXoDOS"));

    let packed = games.iter().find(|g| g.title == "Packed Game").unwrap();
    assert!(!packed.is_installed);
    assert!(packed.installable);
    assert!(packed.install.is_none());
    let _ = PathBuf::from(&packed.app_name);
}
