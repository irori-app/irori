use std::collections::BTreeMap;
use std::io::Write as _;
use std::path::Path;
use std::sync::Arc;

use irori_core::{
    AppSettings, EventBus, GameSettings, InstallInfoQuery, InstallPlatform, InstallRequest,
    LaunchEnvironment, LaunchRequest, OpContext, Outcome, RemoveArgs, Store as _,
};
use serde_json::Value;

use crate::EmulationStore;
use crate::exo::conf::{
    LocalMidiAssets, prepare_dosbox_conf, rewrite_host_paths, strip_windows_verbatim,
    translate_conf,
};
use crate::exo::extract::extract_local_archive;
use crate::exo::scummvm::{detect_platform_code, inject_platform_arg, map_platform_code};
use crate::exo::{ExoCollectionKind, ExoEntryMeta};

fn touch(path: &Path, bytes: &[u8]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
}

fn write_zip(path: &Path, entries: &[(&str, &[u8])]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let file = std::fs::File::create(path).unwrap();
    let mut zip = zip::ZipWriter::new(file);
    let opts = zip::write::SimpleFileOptions::default();
    for (name, data) in entries {
        zip.start_file(*name, opts).unwrap();
        zip.write_all(data).unwrap();
    }
    zip.finish().unwrap();
}

#[test]
fn rewrites_host_paths_preserving_dos_guest_backslashes() {
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path().join("My eXo");
    touch(&base.join("eXoDOS/doom/DOOM.EXE"), b"MZ");
    touch(&base.join("eXoDOS/doom/cd/DISC.CUE"), b"FILE");

    let raw = "[sdl]\noutput=openglpp\nfullscreen=false\npixelshader=crt.fx\n\n\
[midi]\nmididevice=default\n\n\
[autoexec]\n@echo off\nmount c .\\eXoDOS\\DOOM\\\nimgmount d .\\eXoDOS\\doom\\cd\\disc.cue -t iso\n\
set PATH=C:\\;Z:\\\nc:\ncd \\DOOM\nDOOM.EXE\nexit\n";

    let midi = LocalMidiAssets {
        mt32_dir: None,
        soundfont: Some(base.join("soundfonts/gm.sf2")),
    };
    let out = translate_conf(raw, &base, &midi, true);

    assert!(out.contains("output = texture\n"));
    assert!(!out.contains("pixelshader"));
    assert!(out.contains("fullscreen = true\n"));
    // Guest paths kept verbatim with backslashes.
    assert!(out.contains("set PATH=C:\\;Z:\\\n"));
    assert!(out.contains("cd \\DOOM\n"));
    // Host mount paths quoted because `My eXo` has a space, and trailing slash stripped.
    // On case-sensitive filesystems (e.g. Linux), mismatched casing in Windows-authored
    // configs resolves to the exact on-disk casing (`doom`, `DISC.CUE`). On case-insensitive
    // filesystems (Windows NTFS, default macOS APFS), the direct path already exists and
    // is kept without extra directory scans.
    let case_sensitive = !base.join("eXoDOS/DOOM").exists();
    let expected_game = base
        .join(if case_sensitive {
            "eXoDOS/doom"
        } else {
            "eXoDOS/DOOM"
        })
        .to_string_lossy()
        .replace('\\', "/");
    assert!(
        out.contains(&format!("mount c \"{expected_game}\"\n")),
        "unexpected conf:\n{out}"
    );
    assert!(Path::new(&expected_game).is_dir());
    let expected_cue = base
        .join(if case_sensitive {
            "eXoDOS/doom/cd/DISC.CUE"
        } else {
            "eXoDOS/doom/cd/disc.cue"
        })
        .to_string_lossy()
        .replace('\\', "/");
    assert!(
        out.contains(&format!("imgmount d \"{expected_cue}\" -t iso\n")),
        "unexpected conf:\n{out}"
    );
    assert!(Path::new(&expected_cue).is_file());
    if !cfg!(windows) {
        assert!(out.contains("mididevice = fluidsynth\n"));
        assert!(out.contains("soundfont = "));
    }
    // Verify pure rewrite helper quotes mount args with spaces but never config values.
    let pure = rewrite_host_paths("romdir=.\\mt32\nmount c .\\game", &|_| "/a b/target".into());
    assert_eq!(pure, "romdir=/a b/target\nmount c \"/a b/target\"");
    assert_eq!(
        strip_windows_verbatim(Path::new(r"\\?\C:\eXo\game.bat")),
        Path::new(r"C:\eXo\game.bat")
    );
    assert_eq!(
        strip_windows_verbatim(Path::new(r"\\?\UNC\server\share\game.bat")),
        Path::new(r"\\?\UNC\server\share\game.bat")
    );
}

#[test]
fn maps_exoscummvm_platform_codes_and_injects_flag() {
    assert_eq!(map_platform_code("dos"), Some("pc"));
    assert_eq!(map_platform_code("SP48"), Some("zx"));
    assert_eq!(map_platform_code("apple2gs"), Some("2gs"));
    assert_eq!(map_platform_code("unknown_sys"), None);

    let dir = tempfile::tempdir().unwrap();
    touch(&dir.path().join("Platform.txt"), b"  AMIGA \n");
    assert_eq!(detect_platform_code(dir.path()), Some("amiga"));

    let mut spec =
        irori_core::process::CommandSpec::new("scummvm").args(["--path=/g", "--auto-detect"]);
    inject_platform_arg(&mut spec, "amiga");
    let args: Vec<String> = spec
        .args
        .iter()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    assert_eq!(args, ["--path=/g", "--platform=amiga", "--auto-detect"]);
}

#[test]
fn extracts_local_zip_strips_shortcode_and_rejects_traversal() {
    let dir = tempfile::tempdir().unwrap();
    let exo_root = dir.path().join("eXoDOS");
    let zip_path = exo_root.join("eXo/eXoDOS/Quake (1996).zip");
    write_zip(
        &zip_path,
        &[
            ("quake/QUAKE.EXE", b"MZ1234"),
            ("quake/ID1/PAK0.PAK", b"PACK"),
        ],
    );
    let meta = ExoEntryMeta {
        collection: ExoCollectionKind::ExoDos,
        platform: "MS-DOS".into(),
        shortcode: "quake".into(),
        exo_root: exo_root.clone(),
        game_dir: exo_root.join("eXo/eXoDOS/quake"),
        config_dir: exo_root.join("eXo/eXoDOS/!dos/quake"),
        conf_path: None,
        archive_path: Some(zip_path),
        script_path: None,
        installed: false,
        language: None,
        developer: None,
        publisher: None,
        release_date: None,
        genres: Vec::new(),
        notes: None,
        manual_path: None,
        local_cover: None,
        local_logo: None,
        local_background: None,
        local_screenshots: Vec::new(),
    };
    let outcome = extract_local_archive(&meta, &OpContext::detached()).unwrap();
    assert_eq!(outcome, Outcome::Done);
    assert!(meta.game_dir.join("QUAKE.EXE").is_file());
    assert!(meta.game_dir.join("ID1/PAK0.PAK").is_file());
    assert!(!meta.game_dir.join("quake").exists());

    // Zip with `../` traversal must fail cleanly without writing outside staging.
    let bad_zip = exo_root.join("eXo/eXoDOS/Bad.zip");
    write_zip(&bad_zip, &[("../escaped.txt", b"nope")]);
    let bad_meta = ExoEntryMeta {
        shortcode: "bad".into(),
        game_dir: exo_root.join("eXo/eXoDOS/bad"),
        archive_path: Some(bad_zip),
        ..meta
    };
    assert!(extract_local_archive(&bad_meta, &OpContext::detached()).is_err());
    assert!(!exo_root.join("eXo/eXoDOS/escaped.txt").exists());
}

#[tokio::test]
async fn installs_launches_and_uninstalls_exo_games_preserving_archive() {
    let base = crate::tests::test_dir("exo-launch-flow");
    let exo = base.join("eXoDOS");
    let xml = r#"<LaunchBox>
  <Game>
    <Title>Quake</Title>
    <Platform>MS-DOS</Platform>
    <ApplicationPath>eXo\eXoDOS\!dos\quake\Quake (1996).bat</ApplicationPath>
  </Game>
</LaunchBox>"#;
    touch(&exo.join("Data/Platforms/MS-DOS.xml"), xml.as_bytes());
    touch(
        &exo.join("eXo/eXoDOS/!dos/quake/dosbox.conf"),
        b"[autoexec]\nmount c .\\eXoDOS\\quake\nc:\nQUAKE.EXE\n",
    );
    if cfg!(windows) {
        touch(
            &exo.join("eXo/eXoDOS/!dos/quake/Quake (1996).bat"),
            b"@echo off\n",
        );
    } else {
        touch(
            &exo.join("eXo/eXoDOS/!dos/quake/Quake (1996).command"),
            b"#!/bin/bash\necho hi\n",
        );
    }
    write_zip(
        &exo.join("eXo/eXoDOS/Quake (1996).zip"),
        &[("quake/QUAKE.EXE", b"MZ9999")],
    );

    let dosbox = crate::tests::test_dosbox();

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

    let quake = s.games().into_iter().find(|g| g.title == "Quake").unwrap();
    assert!(!quake.is_installed);
    assert!(quake.installable);

    // Install info reports 0 download bytes and real uncompressed disk bytes.
    let info = s
        .install_info(
            &quake.app_name,
            InstallPlatform::Linux,
            &InstallInfoQuery::default(),
        )
        .await
        .unwrap();
    assert_eq!(info.download_size, 0);
    assert_eq!(info.disk_size, 6);

    // Launching before install returns an actionable error.
    let req = LaunchRequest {
        settings: GameSettings::default(),
        environment: LaunchEnvironment::default(),
        option: None,
        extra_args: Vec::new(),
        offline: false,
        skip_version_check: false,
        log_file: base.join("game.log"),
    };
    assert!(
        s.launch(&quake.app_name, &req, OpContext::detached())
            .await
            .is_err()
    );

    // Install extracts the local archive and flips the game to installed.
    let res = s
        .install(
            &quake.app_name,
            &InstallRequest::default(),
            OpContext::detached(),
        )
        .await
        .unwrap();
    assert_eq!(res, Outcome::Done);
    assert!(exo.join("eXo/eXoDOS/quake/QUAKE.EXE").is_file());
    let quake_now = s.game(&quake.app_name).unwrap();
    assert!(quake_now.is_installed);
    assert!(s.is_available(&quake.app_name).await);

    // Default launch uses DOSBox Staging with translated conf in cache dir.
    let plan = s
        .launch(&quake.app_name, &req, OpContext::detached())
        .await
        .unwrap();
    assert_eq!(plan.command.program, dosbox);
    assert_eq!(plan.command.cwd.as_deref(), Some(exo.join("eXo").as_path()));
    let conf_arg = plan
        .command
        .args
        .last()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    assert!(conf_arg.ends_with(".conf"));
    let generated = std::fs::read_to_string(&conf_arg).unwrap();
    assert!(generated.contains("eXo/eXoDOS/quake"));

    // Opt-in native script toggle runs eXo's own launch script instead.
    let mut script_req = req.clone();
    script_req.settings.exo_use_native_script = true;
    let script_plan = s
        .launch(&quake.app_name, &script_req, OpContext::detached())
        .await
        .unwrap();
    assert_eq!(
        script_plan.command.cwd.as_deref(),
        Some(exo.join("eXo/eXoDOS/!dos/quake").as_path())
    );
    assert_ne!(script_plan.command.program, dosbox);

    // Uninstalling with delete_files removes ONLY the extracted folder and keeps the .zip.
    s.uninstall(
        &quake.app_name,
        RemoveArgs {
            delete_files: true,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert!(!exo.join("eXo/eXoDOS/quake").exists());
    assert!(exo.join("eXo/eXoDOS/Quake (1996).zip").is_file());
    let after = s.game(&quake.app_name).unwrap();
    assert!(!after.is_installed);
    assert!(after.installable);

    // Exercise prepare_dosbox_conf directly for coverage of option map.
    let mut opts = BTreeMap::new();
    opts.insert("fullscreen".to_string(), Value::Bool(false));
    let entry = s.entry(&quake.app_name).unwrap();
    let conf_path = prepare_dosbox_conf(
        entry.exo.as_ref().unwrap(),
        &base.join("tmpconf"),
        "q",
        &opts,
    )
    .unwrap();
    assert!(conf_path.is_file());
}
