//! The Moonlight client a stream starts a game for, as Sunshine (and Apollo)
//! describe it to the command they run: `SUNSHINE_CLIENT_WIDTH`, `_HEIGHT`,
//! `_FPS` and `_HDR`. The game gets the same variables, and a game running in
//! gamescope takes the client's resolution, frame rate and HDR unless its
//! settings name their own.

use irori_core::{EnvVar, GameSettings};
use serde::{Deserialize, Serialize};

/// The streaming client a game was started for.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StreamClient {
    pub width: u32,
    pub height: u32,
    /// Frames per second, as the host wrote it ("60", or Apollo's "59.94").
    pub fps: String,
    pub hdr: bool,
}

impl StreamClient {
    /// The client a streaming host described in the variables `var` reads,
    /// when it gave a size.
    pub fn from_env(var: impl Fn(&str) -> Option<String>) -> Option<Self> {
        let number = |name: &str| var(name).and_then(|v| v.trim().parse::<u32>().ok());
        let width = number("SUNSHINE_CLIENT_WIDTH").filter(|w| *w > 0)?;
        let height = number("SUNSHINE_CLIENT_HEIGHT").filter(|h| *h > 0)?;
        let fps = var("SUNSHINE_CLIENT_FPS")
            .map(|v| v.trim().to_string())
            .filter(|v| v.parse::<f32>().is_ok_and(|f| f > 0.0))
            .unwrap_or_default();
        let hdr = var("SUNSHINE_CLIENT_HDR").is_some_and(|v| v.trim() == "true");
        Some(Self {
            width,
            height,
            fps,
            hdr,
        })
    }

    /// The client's frame rate as gamescope takes it: a whole number.
    fn whole_fps(&self) -> Option<String> {
        let fps = self.fps.parse::<f32>().ok()?;
        (fps >= 1.0).then(|| format!("{}", fps.round() as u32))
    }

    /// Fit `settings` to this client: the stream's variables for the game,
    /// and with `match_display`, gamescope's output size, frame limit and
    /// HDR where the game's settings leave them open.
    pub fn apply(&self, settings: &mut GameSettings, match_display: bool) {
        let vars = [
            ("SUNSHINE_CLIENT_WIDTH", self.width.to_string()),
            ("SUNSHINE_CLIENT_HEIGHT", self.height.to_string()),
            ("SUNSHINE_CLIENT_FPS", self.fps.clone()),
            ("SUNSHINE_CLIENT_HDR", self.hdr.to_string()),
        ];
        for (key, value) in vars {
            if value.is_empty() || settings.environment_options.iter().any(|v| v.key == key) {
                continue;
            }
            settings.environment_options.push(EnvVar {
                key: key.into(),
                value,
            });
        }
        let gs = &mut settings.gamescope;
        if !match_display || !gs.is_enabled() {
            return;
        }
        // Output size: the client's, unless the game asks for its own.
        if gs.upscale_width.trim().is_empty() && gs.upscale_height.trim().is_empty() {
            gs.upscale_width = self.width.to_string();
            gs.upscale_height = self.height.to_string();
            // Without a size of its own the game renders at the client's too.
            if gs.game_width.trim().is_empty() && gs.game_height.trim().is_empty() {
                gs.game_width = self.width.to_string();
                gs.game_height = self.height.to_string();
            }
            // The sizes only reach gamescope when it sets the window.
            gs.enabled = Some(true);
        }
        if !gs.enable_limiter
            && let Some(fps) = self.whole_fps()
        {
            gs.enable_limiter = true;
            gs.fps_limiter = fps;
        }
        if self.hdr && !gs.additional_options.contains("--hdr-enabled") {
            gs.additional_options = format!("{} --hdr-enabled", gs.additional_options)
                .trim()
                .to_string();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use irori_core::GamescopeSettings;

    use super::*;

    fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |k| map.get(k).cloned()
    }

    fn deck() -> StreamClient {
        StreamClient::from_env(env(&[
            ("SUNSHINE_CLIENT_WIDTH", "1280"),
            ("SUNSHINE_CLIENT_HEIGHT", "800"),
            ("SUNSHINE_CLIENT_FPS", "59.94"),
            ("SUNSHINE_CLIENT_HDR", "true"),
        ]))
        .unwrap()
    }

    #[test]
    fn reads_what_the_host_set() {
        let client = deck();
        assert_eq!((client.width, client.height), (1280, 800));
        assert_eq!(client.fps, "59.94");
        assert!(client.hdr);
        assert_eq!(StreamClient::from_env(env(&[])), None);
        let odd = StreamClient::from_env(env(&[
            ("SUNSHINE_CLIENT_WIDTH", "1920"),
            ("SUNSHINE_CLIENT_HEIGHT", "1080"),
            ("SUNSHINE_CLIENT_FPS", "fast"),
        ]))
        .unwrap();
        assert_eq!(odd.fps, "");
        assert!(!odd.hdr);
    }

    #[test]
    fn games_get_the_variables_and_keep_their_own() {
        let mut settings = GameSettings::default();
        settings.environment_options.push(EnvVar {
            key: "SUNSHINE_CLIENT_FPS".into(),
            value: "30".into(),
        });
        deck().apply(&mut settings, true);
        let get = |k: &str| {
            settings
                .environment_options
                .iter()
                .find(|v| v.key == k)
                .map(|v| v.value.clone())
        };
        assert_eq!(get("SUNSHINE_CLIENT_WIDTH").as_deref(), Some("1280"));
        assert_eq!(get("SUNSHINE_CLIENT_FPS").as_deref(), Some("30"));
        assert_eq!(get("SUNSHINE_CLIENT_HDR").as_deref(), Some("true"));
        // Without gamescope, nothing else changes.
        assert_eq!(settings.gamescope, GamescopeSettings::default());
    }

    #[test]
    fn gamescope_takes_the_clients_display() {
        let mut settings = GameSettings::default();
        settings.gamescope.enabled = Some(true);
        deck().apply(&mut settings, true);
        let gs = &settings.gamescope;
        assert_eq!(
            (gs.upscale_width.as_str(), gs.upscale_height.as_str()),
            ("1280", "800")
        );
        assert_eq!(
            (gs.game_width.as_str(), gs.game_height.as_str()),
            ("1280", "800")
        );
        assert!(gs.enable_limiter);
        assert_eq!(gs.fps_limiter, "60");
        assert_eq!(gs.additional_options, "--hdr-enabled");
        let args = irori_runtime::wrappers::gamescope_args(gs, false, true);
        assert!(args.windows(2).any(|w| w == ["-W", "1280"]));
        assert!(args.windows(2).any(|w| w == ["-r", "60"]));
        assert!(args.iter().any(|a| a == "--hdr-enabled"));
    }

    #[test]
    fn a_games_own_display_settings_win() {
        let mut settings = GameSettings::default();
        settings.gamescope.enabled = Some(true);
        settings.gamescope.game_width = "960".into();
        settings.gamescope.game_height = "600".into();
        settings.gamescope.upscale_width = "1920".into();
        settings.gamescope.upscale_height = "1200".into();
        settings.gamescope.enable_limiter = true;
        settings.gamescope.fps_limiter = "40".into();
        let before = settings.gamescope.clone();
        let mut client = deck();
        client.hdr = false;
        client.apply(&mut settings, true);
        assert_eq!(settings.gamescope, before);

        // Turned off, only the variables change.
        let mut settings = GameSettings::default();
        settings.gamescope.enabled = Some(true);
        let before = settings.gamescope.clone();
        deck().apply(&mut settings, false);
        assert_eq!(settings.gamescope, before);
    }
}
