use std::collections::HashSet;

use irori_extras::anticheat::{AntiCheatInfo, AntiCheatStatus};

use super::analysis::*;
use super::pe::{self, PeInfo};

const AMD64: u16 = 0x8664;

fn facts(lines: &[&str]) -> Facts {
    Facts {
        lines: lines.iter().map(|l| l.to_string()).collect(),
        ..Default::default()
    }
}

fn wine() -> FixContext {
    FixContext {
        needs_wine: true,
        verifiable: true,
        anticheat_runtimes_available: true,
        ..Default::default()
    }
}

fn hints(d: &Diagnosis) -> Vec<LaunchHint> {
    d.findings.iter().map(|f| f.hint).collect()
}

fn offered(d: &Diagnosis) -> Vec<Fix> {
    d.fixes.iter().map(|o| o.fix.clone()).collect()
}

fn program(dlls: &[&str], dotnet: bool) -> PeInfo {
    let file = pe::build_test_pe(AMD64, true, dlls, dotnet);
    pe::parse(&mut std::io::Cursor::new(file)).unwrap()
}

#[test]
fn recognises_causes_in_logs() {
    let log = [
        "wine: Read access denied for device L\"\\\\??\\\\Z:\\\\\"",
        "0024:err:module:import_dll Library VCRUNTIME140.dll (which is needed by L\"game.exe\") not found",
        "EasyAntiCheat: launcher error",
        "err:module:import_dll Library MSVCP140.dll not found",
    ];
    assert_eq!(
        launch_hints(log),
        vec![LaunchHint::AntiCheat, LaunchHint::MissingLibrary]
    );
    assert_eq!(
        launch_hints(["err:vulkan:wine_vk_init Failed to load Vulkan"]),
        vec![LaunchHint::Vulkan]
    );
    assert_eq!(
        launch_hints(["sh: ./game.x86_64: Permission denied"]),
        vec![LaunchHint::PermissionDenied]
    );
    assert!(launch_hints(["Started", "fixme:d3d:stub"]).is_empty());
}

#[test]
fn finds_missing_programs_wrong_builds_and_crashes_with_their_lines() {
    let log = [
        "wine: '/home/me/.wine32' is a 32-bit installation, it cannot support 64-bit applications.",
        "wine: cannot find L\"C:\\Games\\game.exe\"",
        "Could not start Trainer: failed to start /opt/trainer",
        "wine: Unhandled page fault on read access to 00000000",
        "wine: Unhandled page fault on read access to 00000000",
        "wine: Unhandled exception 0xc0000005",
    ];
    let mut f = facts(&log);
    f.exit_code = Some(53);
    let d = diagnose(&f, &HashSet::new());
    assert_eq!(
        hints(&d),
        [
            LaunchHint::MissingExecutable,
            LaunchHint::WrongWineBuild,
            LaunchHint::ProgramFailed,
            LaunchHint::Crash
        ]
    );
    assert_eq!(d.findings[0].line(), Some(log[1]));
    // Each line once, in order.
    assert_eq!(d.findings[3].evidence, [log[3], log[5]]);

    // The shell's own exit codes say what the log may not.
    let mut f = facts(&["Starting"]);
    f.exit_code = Some(127);
    let d = diagnose(&f, &HashSet::new());
    assert_eq!(hints(&d), [LaunchHint::MissingExecutable]);
    assert!(d.findings[0].evidence.is_empty());
    let mut f = facts(&[]);
    f.exit_code = Some(126);
    assert_eq!(
        hints(&diagnose(&f, &HashSet::new())),
        [LaunchHint::PermissionDenied]
    );
    // Games log missing optional files; only the shell's line counts.
    let mut f = facts(&["open(config.ini): No such file or directory"]);
    f.exit_code = Some(1);
    assert!(diagnose(&f, &HashSet::new()).findings.is_empty());
    assert_eq!(
        launch_hints(["sh: ./game.x86_64: No such file or directory"]),
        [LaunchHint::MissingExecutable]
    );
    assert_eq!(
        launch_hints(["./game: error while loading shared libraries: libSDL2-2.0.so.0"]),
        [LaunchHint::MissingNativeLibrary]
    );
}

#[test]
fn maps_missing_windows_libraries_to_winetricks_verbs() {
    let log = [
        "0024:err:module:import_dll Library VCRUNTIME140.dll (which is needed by L\"C:\\\\game\\\\msvcp120.dll\") not found",
        "0024:err:module:import_dll Library MSVCP140.dll not found",
        "0024:err:module:import_dll Library d3dx9_43.dll not found",
        "0024:err:mscoree:LoadLibraryShim error reading registry key for mscoree.dll",
        "fixme:d3d:stub vcruntime140 mentioned without an error",
        "wine: Call from 00006FFFFF1C2E48 to unimplemented function d3dcompiler_47.dll.D3DCompile2, aborting",
    ];
    // The library needing a DLL isn't missing itself.
    assert_eq!(
        runtime_verbs(log),
        ["vcrun2022", "d3dx9", "dotnet48", "d3dcompiler_47"]
    );
    assert_eq!(
        missing_dlls(log),
        [
            "vcruntime140.dll",
            "msvcp140.dll",
            "d3dx9_43.dll",
            "d3dcompiler_47.dll"
        ]
    );
    assert!(runtime_verbs(["Started", "msvcp140 loaded"]).is_empty());
    assert_eq!(verb_for_dll("D3DX9_43.DLL"), Some("d3dx9"));
    assert_eq!(verb_for_dll("libcef.dll"), None);
}

#[test]
fn a_silent_proton_log_with_err_module_names_the_dll_and_its_fix() {
    // What `WINEDEBUG=-all,err+module` leaves of a failed start.
    let log = [
        "0130:err:module:import_dll Library MSVCP140.dll (which is needed by L\"Z:\\\\games\\\\Game\\\\Game.exe\") not found",
        "0130:err:module:LdrInitializeThunk Importing dlls for L\"Z:\\\\games\\\\Game\\\\Game.exe\" failed, status c0000135",
    ];
    let mut f = facts(&log);
    f.context = wine();
    f.exit_code = Some(53);
    let d = diagnose(&f, &HashSet::new());
    assert_eq!(hints(&d), [LaunchHint::MissingLibrary]);
    let found = &d.findings[0];
    assert!(found.confirmed);
    assert_eq!(found.evidence, [log[0]]);
    assert_eq!(
        found.detail,
        Some(Detail::Libraries {
            dlls: vec!["msvcp140.dll".into()],
            installed: vec![],
        })
    );
    assert_eq!(offered(&d), [Fix::Winetricks(vec!["vcrun2022"])]);

    // Already in the prefix: say so instead of offering it again.
    f.installed_verbs = vec!["vcrun2022".into()];
    let d = diagnose(&f, &HashSet::new());
    assert!(d.fixes.is_empty());
    assert_eq!(
        d.findings[0].detail,
        Some(Detail::Libraries {
            dlls: vec!["msvcp140.dll".into()],
            installed: vec!["vcrun2022"],
        })
    );

    // Native games never get winetricks.
    f.installed_verbs.clear();
    f.context.needs_wine = false;
    assert!(diagnose(&f, &HashSet::new()).fixes.is_empty());
}

#[test]
fn dotnet_errors_offer_dotnet48_unless_installed() {
    let log = ["0024:err:mscoree:CLRRuntimeInfo_GetRuntimeHost Wine Mono is not installed"];
    let mut f = facts(&log);
    f.context = wine();
    let d = diagnose(&f, &HashSet::new());
    assert_eq!(hints(&d), [LaunchHint::DotNet]);
    assert_eq!(offered(&d), [Fix::Winetricks(vec!["dotnet48"])]);
    f.installed_verbs = vec!["dotnet48".into()];
    assert!(diagnose(&f, &HashSet::new()).fixes.is_empty());
}

#[test]
fn the_program_s_imports_are_worth_checking_when_the_log_says_nothing() {
    let mut f = facts(&["Starting the game"]);
    f.context = wine();
    f.program = Some(program(
        &[
            "KERNEL32.dll",
            "MSVCP140.dll",
            "VCRUNTIME140.dll",
            "d3dcompiler_47.dll",
        ],
        true,
    ));
    let d = diagnose(&f, &HashSet::new());
    assert_eq!(hints(&d), [LaunchHint::MissingLibrary, LaunchHint::DotNet]);
    assert!(d.findings.iter().all(|f| !f.confirmed));
    assert_eq!(
        d.findings[0].detail,
        Some(Detail::ProgramLibraries {
            dlls: vec![
                "msvcp140.dll".into(),
                "vcruntime140.dll".into(),
                "d3dcompiler_47.dll".into()
            ]
        })
    );
    assert_eq!(
        offered(&d),
        [
            Fix::Winetricks(vec!["vcrun2022", "d3dcompiler_47"]),
            Fix::Winetricks(vec!["dotnet48"])
        ]
    );

    // Any Visual C++ 2015–2022 runtime brings the 140 DLLs.
    f.installed_verbs = vec!["vcrun2019".into(), "dotnet472".into()];
    let d = diagnose(&f, &HashSet::new());
    assert_eq!(
        offered(&d),
        [Fix::Winetricks(vec!["d3dcompiler_47"])],
        "{d:?}"
    );

    // A cause the log shows wins; the imports aren't guessed at then.
    f.installed_verbs.clear();
    f.lines = vec!["err:vulkan:wine_vk_init Failed to load Vulkan".into()];
    assert_eq!(hints(&diagnose(&f, &HashSet::new())), [LaunchHint::Vulkan]);
}

#[test]
fn anticheat_entries_explain_what_the_log_cant() {
    let entry = |status| AntiCheatInfo {
        name: "Game".into(),
        status,
        anticheats: vec!["Easy Anti-Cheat".into()],
        ..Default::default()
    };
    let mut f = facts(&["Starting"]);
    f.context = wine();
    f.anticheat = Some(entry(AntiCheatStatus::Denied));
    let d = diagnose(&f, &HashSet::new());
    assert_eq!(hints(&d), [LaunchHint::AntiCheat]);
    assert!(d.findings[0].confirmed);
    assert_eq!(
        d.findings[0].detail,
        Some(Detail::AntiCheat {
            names: vec!["Easy Anti-Cheat".into()],
            status: AntiCheatStatus::Denied
        })
    );
    // Turning the runtimes on doesn't help a denied anticheat.
    assert!(d.fixes.is_empty());

    // Supported with the runtimes, which are off.
    f.anticheat = Some(entry(AntiCheatStatus::Supported));
    let d = diagnose(&f, &HashSet::new());
    assert_eq!(offered(&d), [Fix::AntiCheatRuntimes]);
    assert!(!d.findings[0].confirmed);
    f.context.anticheat_runtimes_on = true;
    assert!(diagnose(&f, &HashSet::new()).findings.is_empty());

    // Only an online mode is blocked: the game itself starts.
    f.context.anticheat_runtimes_on = false;
    let mut online = entry(AntiCheatStatus::Denied);
    online.online_only = Some("GTA Online".into());
    f.anticheat = Some(online);
    assert!(diagnose(&f, &HashSet::new()).findings.is_empty());

    // The log's own anticheat line, with the runtimes on offer (Linux).
    f.anticheat = None;
    f.lines = vec!["EasyAntiCheat: launcher error".into()];
    assert_eq!(
        offered(&diagnose(&f, &HashSet::new())),
        [Fix::AntiCheatRuntimes]
    );
    f.context.anticheat_runtimes_available = false;
    assert!(diagnose(&f, &HashSet::new()).fixes.is_empty());
}

#[test]
fn a_low_map_count_is_explained() {
    let mut f = facts(&["Starting"]);
    f.context = wine();
    f.map_count = Some(65_530);
    let d = diagnose(&f, &HashSet::new());
    assert_eq!(hints(&d), [LaunchHint::MapCount]);
    assert!(!d.findings[0].confirmed);
    assert_eq!(
        d.findings[0].detail,
        Some(Detail::MapCount { current: 65_530 })
    );
    assert_eq!(offered(&d), [Fix::MapCount]);
    // Out-of-memory lines confirm it.
    f.lines = vec!["Fatal: mmap failed: Cannot allocate memory".into()];
    let d = diagnose(&f, &HashSet::new());
    assert!(d.findings[0].confirmed);
    assert_eq!(d.findings[0].line(), Some(f.lines[0].as_str()));
    // High enough: nothing to say.
    f.map_count = Some(MAP_COUNT_RECOMMENDED);
    assert!(diagnose(&f, &HashSet::new()).findings.is_empty());
    assert_eq!(parse_map_count("65530\n"), Some(65_530));
    assert_eq!(parse_map_count("nope"), None);
    assert!(map_count_commands()[0].contains("vm.max_map_count=2147483642"));
}

#[test]
fn offers_the_fixes_that_apply() {
    let mut f = facts(&[
        "wine: cannot find L\"C:\\\\game.exe\"",
        "EasyAntiCheat: launcher error",
        "err:module:import_dll Library VCRUNTIME140.dll not found",
    ]);
    f.context = wine();
    assert_eq!(
        offered(&diagnose(&f, &HashSet::new())),
        [
            Fix::VerifyFiles,
            Fix::AntiCheatRuntimes,
            Fix::Winetricks(vec!["vcrun2022"]),
        ]
    );
    // Runtimes already on, added apps can't be verified, a missing
    // alternative executable comes first.
    f.lines.truncate(2);
    f.context = FixContext {
        verifiable: false,
        anticheat_runtimes_on: true,
        target_exe_missing: true,
        ..wine()
    };
    assert_eq!(
        offered(&diagnose(&f, &HashSet::new())),
        [Fix::ClearTargetExe]
    );
    // Native games: the Steam runtime for missing libraries.
    let mut f = facts(&[
        "./game: error while loading shared libraries: libSDL2-2.0.so.0",
        "Could not start Trainer: failed to start /opt/trainer",
    ]);
    f.context = FixContext {
        native_without_runtime: true,
        ..Default::default()
    };
    assert_eq!(
        offered(&diagnose(&f, &HashSet::new())),
        [Fix::SteamRuntime, Fix::ProgramSettings]
    );
    let mut f = facts(&["wine: Unhandled exception 0xc0000005"]);
    f.context = wine();
    assert!(diagnose(&f, &HashSet::new()).fixes.is_empty());
}

#[test]
fn a_fix_tried_this_session_comes_last() {
    let mut f = facts(&[
        "err:module:import_dll Library VCRUNTIME140.dll not found",
        "0024:err:mscoree:CLRRuntimeInfo_GetRuntimeHost Wine Mono is not installed",
    ]);
    f.context = wine();
    let first = Fix::Winetricks(vec!["vcrun2022"]);
    let tried: HashSet<String> = [first.key()].into();
    let d = diagnose(&f, &tried);
    assert_eq!(
        d.fixes,
        [
            OfferedFix {
                fix: Fix::Winetricks(vec!["dotnet48"]),
                tried: false
            },
            OfferedFix {
                fix: first,
                tried: true
            },
        ]
    );
}

#[test]
fn reads_only_what_the_game_wrote() {
    let log = [
        "Irori 0.1.0 - Game (sideload:game)",
        r#"Settings: {"battlEyeRuntime":true,"eacRuntime":true}"#,
        "",
        "Launch command: /games/start.sh",
        "",
        "Starting",
    ];
    let start = game_output_start(log);
    assert_eq!(start, 4);
    assert!(launch_hints(log[start..].iter().copied()).is_empty());
    assert_eq!(game_output_start(["no header", "at all"]), 0);

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("game.log");
    std::fs::write(&path, log.join("\n")).unwrap();
    assert_eq!(super::read_game_output(&path), ["", "Starting"]);
    // A long log is read from its end, from a whole line.
    let long = format!("{}\nlast line\n", "x".repeat(200));
    std::fs::write(&path, long).unwrap();
    assert_eq!(super::read_tail(&path, 20).unwrap(), "last line\n");
}

#[test]
fn reads_the_verbs_of_wine_and_proton_prefixes() {
    let dir = tempfile::tempdir().unwrap();
    let prefix = dir.path();
    std::fs::create_dir_all(prefix.join("pfx")).unwrap();
    std::fs::write(
        prefix.join("pfx/winetricks.log"),
        "vcrun2022\n-q\n\nd3dcompiler_47\n",
    )
    .unwrap();
    std::fs::write(prefix.join("winetricks.log"), "vcrun2022\ncorefonts\n").unwrap();
    assert_eq!(
        super::installed_verbs(prefix),
        ["vcrun2022", "d3dcompiler_47", "corefonts"]
    );
    assert!(super::installed_verbs(&prefix.join("none")).is_empty());
}
