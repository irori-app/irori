//! MangoHud in a running game's session, for the in-game overlay and live
//! Settings updates: show or hide it, change its frame limit, and rewrite the
//! base configuration while the game runs. The launch writes the session's
//! MangoHud file (`irori_runtime::mangohud`), MangoHud applies every change to
//! it at once via `inotify`, and the file goes when the session ends
//! ([`Backend::finish_session`](crate::Backend)).

use anyhow::{Context as _, Result};
use irori_core::GameId;
pub use irori_runtime::mangohud::MangoState;

use crate::Backend;

/// Live MangoHud configuration payload written from Settings so Steam games,
/// external launchers, gamescope and running Irori-launched sessions all reload
/// immediately via MangoHud's `inotify`.
#[derive(Debug, Clone)]
pub struct LiveMangoHudSync {
    /// Config stem (`global` or `<runner>_<app>`).
    pub conf_name: String,
    /// Shared `MANGOHUD_CONFIGFILE` body.
    pub config_body: String,
    /// Per-preset `MANGOHUD_PRESETSFILE` body.
    pub presets_body: String,
    /// Explicit FPS limit change from Settings (applied only to sessions where
    /// MangoHud owns the frame limiter).
    pub fps_limit_update: Option<Option<u32>>,
    /// Explicit `Start hidden` toggle change from Settings.
    pub no_display_update: Option<bool>,
}

impl Backend {
    /// MangoHud's state in the running session of `id`; `None` when it
    /// doesn't run with the game, or Irori can't change it (the user's
    /// `MANGOHUD_CONFIG` keeps MangoHud from reading a file).
    pub async fn mangohud(&self, id: &GameId) -> Option<MangoState> {
        let id = id.clone();
        self.spawn_blocking(move || irori_runtime::mangohud::read_session(&id))
            .await
            .ok()
            .flatten()
    }

    /// Change MangoHud's state in the running session of `id`; returns it
    /// as the file then says.
    pub async fn set_mangohud(&self, id: &GameId, state: MangoState) -> Result<MangoState> {
        let id = id.clone();
        self.spawn_blocking(move || {
            irori_runtime::mangohud::write_session(&id, state)?;
            irori_runtime::mangohud::read_session(&id)
                .context("MangoHud isn't running with this game")
        })
        .await
        .context("MangoHud's settings couldn't be changed")?
    }

    /// Write `<data>/mangohud/<conf_name>_presets.conf` and `<conf_name>.conf`
    /// in place and mirror the updated base config into any running game
    /// sessions sourced from `conf_name`.
    pub async fn sync_live_mangohud(&self, sync: LiveMangoHudSync) -> Result<()> {
        let paths = irori_core::Paths::get();
        let presets_path = paths.mangohud_presets_file(&sync.conf_name);
        let conf_path = paths.mangohud_config_file(&sync.conf_name);

        // Write presets before config so MangoHud's IN_MODIFY reload on the
        // config file always reads the updated preset definitions at once.
        irori_runtime::hud::write_config_live(&presets_path, &sync.presets_body).await?;
        irori_runtime::hud::write_config_live(&conf_path, &sync.config_body).await?;

        let running = self.running_games();
        if running.is_empty() {
            return Ok(());
        }
        let source_name = irori_core::hud::sanitize_game_id_for_filename(&sync.conf_name);
        let base_config = sync.config_body;
        let fps_limit_update = sync.fps_limit_update;
        let no_display_update = sync.no_display_update;
        self.spawn_blocking(move || {
            for id in running {
                if let Err(err) = irori_runtime::mangohud::sync_session_base(
                    &id,
                    &source_name,
                    &base_config,
                    fps_limit_update,
                    no_display_update,
                ) {
                    tracing::warn!("updating live MangoHud session for {id}: {err:#}");
                }
            }
        })
        .await
        .context("synchronizing running MangoHud sessions")?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use irori_core::{Paths, Runner};

    use super::*;
    use crate::games::RunningGame;

    #[tokio::test]
    async fn sync_live_mangohud_updates_matching_running_sessions_in_place() {
        static ROOT: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();
        ROOT.get_or_init(|| {
            let root = tempfile::tempdir().expect("temp dir").keep();
            Paths::init(Paths::under(&root));
            root
        });
        let backend = Backend::start_for_tests(|_, _| Vec::new()).expect("backend");

        let g_global = GameId::new(Runner::Gog, "sync_live_global_game");
        let g_limited = GameId::new(Runner::Gog, "sync_live_other_limiter");
        let g_custom = GameId::new(Runner::Gog, "sync_live_custom_game");

        let seed = |id: &GameId, base: &str, state: MangoState, source: &str| {
            let path = irori_runtime::mangohud::session_path(id);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(
                &path,
                irori_runtime::mangohud::session_config_with_source(base, state, Some(source)),
            )
            .unwrap();
            path
        };

        let global_path = seed(
            &g_global,
            "legacy_layout=false\npreset=3,4,1,2\n",
            MangoState {
                visible: true,
                fps_limit: Some(60),
                limits: true,
            },
            "global",
        );
        let other_limiter_path = seed(
            &g_limited,
            "legacy_layout=false\npreset=3,4,1,2\n",
            MangoState {
                visible: false,
                fps_limit: None,
                limits: false,
            },
            "global",
        );
        let custom_path = seed(
            &g_custom,
            "legacy_layout=false\npreset=2,3,4,1\n",
            MangoState {
                visible: true,
                fps_limit: Some(90),
                limits: true,
            },
            "gog_sync_live_custom_game",
        );

        #[cfg(unix)]
        let inode_before = {
            use std::os::unix::fs::MetadataExt as _;
            std::fs::metadata(&global_path).unwrap().ino()
        };

        {
            let mut running = backend.running.lock();
            for id in [&g_global, &g_limited, &g_custom] {
                running.insert(
                    id.clone(),
                    RunningGame {
                        pid: 1,
                        started: Instant::now(),
                        stopping: false,
                        watch: None,
                        sandbox: None,
                        handoff: None,
                    },
                );
            }
        }

        backend
            .sync_live_mangohud(LiveMangoHudSync {
                conf_name: "global".into(),
                config_body: "legacy_layout=false\npreset=5,1,2,3,4\ntoggle_preset=Shift_L+F9\n"
                    .into(),
                presets_body: "[preset 1]\nfps_only=1\n".into(),
                fps_limit_update: Some(Some(120)),
                no_display_update: None,
            })
            .await
            .unwrap();

        let paths = Paths::get();
        assert_eq!(
            std::fs::read_to_string(paths.mangohud_presets_file("global")).unwrap(),
            "[preset 1]\nfps_only=1\n"
        );
        assert!(
            std::fs::read_to_string(paths.mangohud_config_file("global"))
                .unwrap()
                .contains("toggle_preset=Shift_L+F9")
        );

        // Matching session with MangoHud as limiter gets base config + new fps_limit, same inode.
        let global_after = std::fs::read_to_string(&global_path).unwrap();
        assert!(global_after.contains("preset=5,1,2,3,4"));
        assert!(global_after.contains("toggle_preset=Shift_L+F9"));
        assert_eq!(
            irori_runtime::mangohud::parse_state(&global_after),
            Some(MangoState {
                visible: true,
                fps_limit: Some(120),
                limits: true,
            })
        );
        assert_eq!(
            irori_runtime::mangohud::parse_source(&global_after),
            Some("global")
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt as _;
            assert_eq!(
                std::fs::metadata(&global_path).unwrap().ino(),
                inode_before,
                "in-place rewrite must preserve session inode for MangoHud inotify"
            );
        }

        // Matching session with another limiter gets base config but keeps limits=false and visible=false.
        let limited_after = std::fs::read_to_string(&other_limiter_path).unwrap();
        assert!(limited_after.contains("preset=5,1,2,3,4"));
        assert_eq!(
            irori_runtime::mangohud::parse_state(&limited_after),
            Some(MangoState {
                visible: false,
                fps_limit: None,
                limits: false,
            })
        );

        // Unrelated per-game custom session is untouched.
        let custom_after = std::fs::read_to_string(&custom_path).unwrap();
        assert!(custom_after.contains("preset=2,3,4,1"));
        assert!(!custom_after.contains("toggle_preset=Shift_L+F9"));
        assert_eq!(
            irori_runtime::mangohud::parse_state(&custom_after),
            Some(MangoState {
                visible: true,
                fps_limit: Some(90),
                limits: true,
            })
        );

        for id in [&g_global, &g_limited, &g_custom] {
            irori_runtime::mangohud::remove_session(id);
        }
    }
}
