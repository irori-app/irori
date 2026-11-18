//! Emulators the user adds on the Emulators page: a name, the program, the
//! argument line it takes and the systems it plays. Each is a manifest of its
//! own in `<config>/engines/<id>.json`, marked [`EngineManifest::custom`],
//! with its program recorded like any program the user picked
//! ([`EngineState::custom`](crate::state::EngineState::custom)). Discovery,
//! the system pickers, "Play with…" and launching then treat it like every
//! other engine: the program is started directly with the expanded
//! arguments, never through a shell.

use std::path::PathBuf;

use anyhow::{Context as _, Result, bail, ensure};
use irori_core::json_store;

use crate::manifest::{Arg, EngineKind, EngineManifest, LaunchTemplate};

/// Ids of emulators the user adds start with this, so they never take the
/// id of an engine a later Irori ships.
pub const ID_PREFIX: &str = "custom-";

/// Placeholders an argument line may use (see [`crate::launch`]).
pub const PLACEHOLDERS: &[&str] = &[
    "rom",
    "rom_dir",
    "home",
    "engine_home",
    "engine_dir",
    "xdg_data",
];

/// Longest name kept.
const NAME_MAX: usize = 80;

/// What the user enters for an emulator of their own.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CustomEngine {
    pub name: String,
    pub program: PathBuf,
    /// The arguments as typed, split by this system's rules. `{rom}` stands
    /// for the game; without it the game goes last.
    pub args: String,
    /// System ids.
    pub systems: Vec<String>,
}

/// Split an argument line into arguments, refusing placeholders Irori does
/// not fill in (a typo would otherwise reach the emulator as text).
pub fn parse_args(line: &str) -> Result<Vec<String>> {
    let args = irori_core::process::split_args(line);
    for arg in &args {
        let mut rest = arg.as_str();
        while let Some(open) = rest.find('{') {
            let after = &rest[open + 1..];
            let close = after.find('}').with_context(|| {
                format!("“{arg}” opens a placeholder with {{ but never closes it")
            })?;
            let name = &after[..close];
            ensure!(
                PLACEHOLDERS.contains(&name),
                "{{{name}}} is not a placeholder Irori fills in; use {}",
                PLACEHOLDERS
                    .iter()
                    .map(|p| format!("{{{p}}}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            );
            rest = &after[close + 1..];
        }
    }
    Ok(args)
}

/// The argument line of a launch template, as [`parse_args`] reads it back.
pub fn args_line(args: &[Arg]) -> String {
    args.iter()
        .filter_map(|a| match a {
            Arg::Plain(s) => Some(quote(s)),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn quote(arg: &str) -> String {
    if !arg.is_empty() && !arg.contains(|c: char| c.is_whitespace() || c == '"' || c == '\'') {
        return arg.to_string();
    }
    format!("\"{}\"", arg.replace('"', "\\\""))
}

/// A new id for an emulator named `name`, one `taken` doesn't hold.
pub fn new_id(name: &str, taken: impl Fn(&str) -> bool) -> String {
    let mut slug = String::new();
    for c in name.chars().flat_map(char::to_lowercase) {
        if c.is_ascii_alphanumeric() {
            slug.push(c);
        } else if !slug.ends_with('-') && !slug.is_empty() {
            slug.push('-');
        }
        if slug.len() >= 32 {
            break;
        }
    }
    let slug = slug.trim_end_matches('-');
    let base = format!(
        "{ID_PREFIX}{}",
        if slug.is_empty() { "emulator" } else { slug }
    );
    if !taken(&base) {
        return base;
    }
    (2..)
        .map(|n| format!("{base}-{n}"))
        .find(|id| !taken(id))
        .expect("some suffix is free")
}

/// The manifest of the emulator `id` the user described.
pub fn manifest(id: &str, def: &CustomEngine) -> Result<EngineManifest> {
    let name = def.name.trim();
    ensure!(!name.is_empty(), "Give the emulator a name");
    let name: String = name.chars().take(NAME_MAX).collect();
    let mut systems: Vec<String> = Vec::new();
    for s in &def.systems {
        ensure!(
            crate::systems::get(s).is_some(),
            "Irori does not know the system {s}"
        );
        if !systems.contains(s) {
            systems.push(s.clone());
        }
    }
    ensure!(!systems.is_empty(), "Choose at least one system");
    let args = parse_args(&def.args)?;
    let manifest = EngineManifest {
        id: id.to_string(),
        name,
        license: String::new(),
        homepage: String::new(),
        kind: EngineKind::Standalone,
        systems,
        sources: Vec::new(),
        binaries: Default::default(),
        launch: LaunchTemplate {
            args: args.into_iter().map(Arg::Plain).collect(),
            env: Default::default(),
        },
        options: Vec::new(),
        bios_dir: Default::default(),
        portable_marker: Default::default(),
        exit_hotkey: String::new(),
        experimental: false,
        cpu_features: Vec::new(),
        cores: Vec::new(),
        custom: true,
    };
    manifest.validate()?;
    Ok(manifest)
}

/// Save the emulator the user described: a new one when `id` is `None`,
/// else the custom emulator `id`. Returns its id. Blocking.
pub fn save(id: Option<&str>, def: &CustomEngine) -> Result<String> {
    ensure!(
        def.program.exists(),
        "{} does not exist",
        def.program.display()
    );
    let id = match id {
        Some(id) => {
            ensure!(
                crate::engine(id).is_some_and(|m| m.custom),
                "{id} is not an emulator you added"
            );
            id.to_string()
        }
        None => new_id(&def.name, |id| {
            crate::engine(id).is_some() || file(id).exists()
        }),
    };
    let manifest = manifest(&id, def)?;
    json_store::write(&file(&id), &manifest)
        .with_context(|| format!("saving the emulator {}", manifest.name))?;
    let program = def.program.clone();
    crate::state::update(|s| {
        s.custom.insert(id.clone(), program);
    })?;
    crate::reload();
    Ok(id)
}

/// Forget the custom emulator `id`: its manifest and its program. The
/// program itself stays where it is. Blocking.
pub fn remove(id: &str) -> Result<()> {
    let Some(manifest) = crate::engine(id) else {
        bail!("{id} is not an emulator Irori knows");
    };
    ensure!(
        manifest.custom,
        "{} is not an emulator you added",
        manifest.name
    );
    // `validate` allows only letters, digits and dashes, so the id names a
    // file inside the folder.
    manifest.validate()?;
    match std::fs::remove_file(file(id)) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => {
            return Err(e).with_context(|| format!("removing the emulator {}", manifest.name));
        }
    }
    crate::state::update(|s| {
        s.custom.remove(id);
        s.preferred.remove(id);
        s.hotkey_shown.remove(id);
    })?;
    crate::reload();
    Ok(())
}

fn file(id: &str) -> PathBuf {
    crate::dirs::overrides().join(format!("{id}.json"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::discovery::{Installation, Origin};
    use crate::launch::{GameFile, LaunchInput};

    fn def(args: &str) -> CustomEngine {
        CustomEngine {
            name: "My Emu 2000!".into(),
            program: PathBuf::from("/opt/emu/emu"),
            args: args.into(),
            systems: vec!["snes".into(), "nes".into(), "snes".into()],
        }
    }

    #[test]
    fn parses_argument_lines() {
        assert_eq!(
            parse_args("--fullscreen -L \"my core.so\" {rom}").unwrap(),
            ["--fullscreen", "-L", "my core.so", "{rom}"]
        );
        assert_eq!(
            parse_args("--dir={rom_dir} --cfg={engine_home}/x").unwrap(),
            ["--dir={rom_dir}", "--cfg={engine_home}/x"]
        );
        assert!(parse_args("").unwrap().is_empty());
        let e = parse_args("--game {romfile}").unwrap_err();
        assert!(format!("{e:#}").contains("{romfile}"));
        assert!(parse_args("--x {rom").is_err());
        assert!(parse_args("{opt.fullscreen}").is_err());
    }

    #[test]
    fn argument_lines_read_back_the_same() {
        let line = "--fullscreen -L \"my core.so\" \"say \\\"hi\\\"\" {rom}";
        let args = parse_args(line).unwrap();
        let template: Vec<Arg> = args.iter().cloned().map(Arg::Plain).collect();
        assert_eq!(parse_args(&args_line(&template)).unwrap(), args);
    }

    #[test]
    fn makes_ids_from_names() {
        assert_eq!(new_id("My Emu 2000!", |_| false), "custom-my-emu-2000");
        assert_eq!(new_id("  ", |_| false), "custom-emulator");
        assert_eq!(new_id("Émulateur", |_| false), "custom-mulateur");
        assert_eq!(
            new_id("Emu", |id| id == "custom-emu" || id == "custom-emu-2"),
            "custom-emu-3"
        );
        let long = new_id(&"x".repeat(200), |_| false);
        assert!(long.len() <= ID_PREFIX.len() + 32);
        // Every id is one a manifest accepts.
        for name in ["../../etc", "a/b\\c", "日本語", "-x-"] {
            let id = new_id(name, |_| false);
            manifest(&id, &def("{rom}")).unwrap();
        }
    }

    #[test]
    fn builds_and_checks_manifests() {
        let m = manifest("custom-my-emu", &def("--fullscreen {rom}")).unwrap();
        assert!(m.custom && m.sources.is_empty());
        assert_eq!(m.systems, ["snes", "nes"]);
        assert_eq!(args_line(&m.launch.args), "--fullscreen {rom}");
        // It reads back like any manifest in the overrides folder.
        let json = serde_json::to_string(&m).unwrap();
        assert_eq!(EngineManifest::parse(&json).unwrap(), m);

        let mut no_name = def("{rom}");
        no_name.name = " ".into();
        assert!(manifest("custom-x", &no_name).is_err());
        let mut no_systems = def("{rom}");
        no_systems.systems.clear();
        assert!(manifest("custom-x", &no_systems).is_err());
        let mut unknown = def("{rom}");
        unknown.systems = vec!["gizmondo".into()];
        assert!(manifest("custom-x", &unknown).is_err());
        assert!(manifest("custom-x", &def("{nope}")).is_err());
    }

    #[test]
    fn launches_the_program_with_the_arguments() {
        let root = crate::test_root("custom-launch");
        let program = root.join("emu");
        std::fs::write(&program, b"x").unwrap();
        let rom = root.join("Game (USA).sfc");
        std::fs::write(&rom, b"x").unwrap();
        let game = GameFile {
            path: rom.clone(),
            system: "snes".into(),
        };
        let installation = Installation {
            engine: "custom-my-emu".into(),
            origin: Origin::Custom,
            version: None,
            program: program.clone(),
            flatpak: None,
            dir: None,
            size: 0,
        };
        let bios = crate::bios::BiosIndex::default();
        let values = Default::default();
        let run = |args: &str| {
            let m = manifest("custom-my-emu", &def(args)).unwrap();
            crate::launch::command(&LaunchInput {
                manifest: &m,
                installation: &installation,
                game: &game,
                core: None,
                values: &values,
                bios: &bios,
                own_settings: false,
            })
            .unwrap()
        };
        let spec = run("--fullscreen \"--rom={rom}\"");
        assert_eq!(spec.program, program);
        let args: Vec<String> = spec
            .args
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            args,
            [
                "--fullscreen".to_string(),
                format!("--rom={}", rom.display())
            ]
        );
        // Without `{rom}`, the game goes last.
        let spec = run("-f");
        let last = spec.args.last().unwrap();
        assert_eq!(std::path::Path::new(last), rom);
    }

    #[test]
    fn saves_edits_and_removes_custom_emulators() {
        let root = crate::test_root("custom-save");
        let program = root.join("emu");
        std::fs::write(&program, b"x").unwrap();
        let mut d = def("{rom}");
        d.program = program.clone();
        let id = save(None, &d).unwrap();
        assert_eq!(id, "custom-my-emu-2000");
        let m = crate::engine(&id).unwrap();
        assert!(m.custom);
        assert!(crate::choices_for("nes").iter().any(|c| c.engine == id));
        assert_eq!(crate::state::load().custom.get(&id), Some(&program));

        // A second one of the same name gets an id of its own.
        let other = save(None, &d).unwrap();
        assert_eq!(other, "custom-my-emu-2000-2");

        d.name = "Renamed".into();
        d.systems = vec!["gba".into()];
        assert_eq!(save(Some(&id), &d).unwrap(), id);
        let m = crate::engine(&id).unwrap();
        assert_eq!(m.name, "Renamed");
        assert!(!crate::choices_for("nes").iter().any(|c| c.engine == id));

        // Built-in engines are neither edited nor removed this way.
        assert!(save(Some("duckstation"), &d).is_err());
        assert!(remove("duckstation").is_err());
        assert!(crate::engine("duckstation").is_some());

        remove(&id).unwrap();
        remove(&other).unwrap();
        assert!(crate::engine(&id).is_none());
        assert!(!crate::state::load().custom.contains_key(&id));
        assert!(program.exists(), "the program itself stays");

        let mut missing = d.clone();
        missing.program = root.join("nope");
        assert!(save(None, &missing).is_err());
    }
}
