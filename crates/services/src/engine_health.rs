//! How each emulator stands, for the Emulators page: how many of the ROM
//! library's games play with it, and a program Irori was given or installed
//! that is gone from the disk (a moved custom program, a deleted managed
//! version), which discovery otherwise leaves out without a word.

use std::collections::HashMap;
use std::path::PathBuf;

use irori_core::GameId;
use irori_engines::EngineRef;
use irori_engines::state::EngineState;

use crate::{Backend, EngineStatus};

/// One emulator's standing.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EngineHealth {
    /// Games of the ROM library that play with it: their own choice, else
    /// their system's default.
    pub games: usize,
    /// Programs it was set up with that aren't on the disk any more.
    pub missing: Vec<PathBuf>,
}

/// The programs `state` names for an engine that no longer exist, by
/// engine id. `exists` is the disk, so tests can stand in for it.
pub(crate) fn missing_programs(
    state: &EngineState,
    exists: impl Fn(&std::path::Path) -> bool,
) -> HashMap<String, Vec<PathBuf>> {
    let mut out: HashMap<String, Vec<PathBuf>> = HashMap::new();
    for (engine, program) in &state.custom {
        if !exists(program) {
            out.entry(engine.clone()).or_default().push(program.clone());
        }
    }
    for (engine, versions) in &state.managed {
        for version in versions {
            if !exists(&version.program) {
                out.entry(engine.clone())
                    .or_default()
                    .push(version.program.clone());
            }
        }
    }
    out
}

/// The emulator each game plays with: its own choice (`iroriEngine`),
/// else its system's default (`system_default`), counted by engine id.
pub(crate) fn count_games(
    games: &[(GameId, Option<String>, String)],
    system_default: impl Fn(&str) -> Option<EngineRef>,
) -> HashMap<String, usize> {
    let mut out = HashMap::new();
    for (_, own, system) in games {
        let engine = own
            .as_deref()
            .and_then(EngineRef::parse)
            .or_else(|| system_default(system));
        if let Some(engine) = engine {
            *out.entry(engine.engine).or_insert(0) += 1;
        }
    }
    out
}

impl Backend {
    /// Every emulator's standing, by engine id (`statuses` from
    /// [`Backend::engine_statuses`]). Reads games' settings and the disk on
    /// a blocking thread.
    pub async fn engine_health(
        &self,
        statuses: Vec<EngineStatus>,
    ) -> HashMap<String, EngineHealth> {
        let b = self.clone();
        self.spawn_blocking(move || {
            let games: Vec<(GameId, Option<String>, String)> = b
                .library()
                .all_games()
                .filter(|g| g.runner() == irori_core::Runner::Emulation)
                .filter_map(|g| {
                    let system = g.system.clone()?;
                    let id = g.id();
                    let own = Some(b.settings.game(&id).engine)
                        .map(|e| e.trim().to_string())
                        .filter(|e| !e.is_empty());
                    Some((id, own, system))
                })
                .collect();
            let counts = count_games(&games, |system| b.system_engine(system, &statuses));
            let missing = missing_programs(&irori_engines::state::load(), |p| p.exists());
            statuses
                .iter()
                .map(|s| {
                    let id = s.manifest.id.clone();
                    let health = EngineHealth {
                        games: counts.get(&id).copied().unwrap_or(0),
                        missing: missing.get(&id).cloned().unwrap_or_default(),
                    };
                    (id, health)
                })
                .collect()
        })
        .await
        .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use irori_core::Runner;
    use irori_engines::state::ManagedInstall;
    use std::path::Path;

    fn managed(version: &str, program: &str) -> ManagedInstall {
        ManagedInstall {
            version: version.into(),
            dir: PathBuf::from(program).parent().unwrap().to_path_buf(),
            program: PathBuf::from(program),
            size: 0,
            installed_at: String::new(),
            url: String::new(),
        }
    }

    #[test]
    fn programs_gone_from_the_disk_are_named() {
        let mut state = EngineState::default();
        state
            .custom
            .insert("duckstation".into(), PathBuf::from("/gone/duckstation"));
        state
            .custom
            .insert("pcsx2".into(), PathBuf::from("/here/pcsx2"));
        state.managed.insert(
            "ppsspp".into(),
            vec![
                managed("1.17", "/here/ppsspp"),
                managed("1.18", "/gone/ppsspp"),
            ],
        );
        let missing = missing_programs(&state, |p: &Path| p.starts_with("/here"));
        assert_eq!(missing["duckstation"], [PathBuf::from("/gone/duckstation")]);
        assert_eq!(missing["ppsspp"], [PathBuf::from("/gone/ppsspp")]);
        assert!(!missing.contains_key("pcsx2"));
    }

    #[test]
    fn games_count_for_the_emulator_they_play_with() {
        let id = |n: &str| GameId::new(Runner::Emulation, n);
        let games = vec![
            (id("a"), None, "psx".to_string()),
            (
                id("b"),
                Some("retroarch:swanstation".to_string()),
                "psx".to_string(),
            ),
            (id("c"), None, "psx".to_string()),
            (id("d"), None, "unknown".to_string()),
        ];
        let counts = count_games(&games, |system| match system {
            "psx" => EngineRef::parse("duckstation"),
            _ => None,
        });
        assert_eq!(counts["duckstation"], 2);
        assert_eq!(counts["retroarch"], 1);
        assert_eq!(counts.len(), 2);
    }
}
