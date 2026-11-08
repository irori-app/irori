//! macOS: the default output's volume through `osascript`'s volume
//! settings (Standard Additions, which need no permission). macOS has no
//! per-program volume, so a game's own can't be read or changed.

use super::*;
use anyhow::{Context as _, bail};
use irori_core::process::{self, CommandSpec, RunOptions};

async fn osascript(statements: &[&str]) -> Result<String> {
    let mut spec = CommandSpec::new("/usr/bin/osascript");
    for statement in statements {
        spec = spec.arg("-e").arg(*statement);
    }
    let out = process::run(&spec, RunOptions::default()).await?;
    if !out.success() {
        bail!("osascript: {}", out.error_summary());
    }
    Ok(out.stdout)
}

async fn system() -> Result<Volume> {
    let text = osascript(&["get volume settings"]).await?;
    parse_volume_settings(&text).context("This sound output has no volume of its own")
}

pub(super) async fn audio(_game: Option<GameProcess>) -> Result<GameAudio> {
    Ok(GameAudio {
        system: Some(system().await?),
        game: None,
    })
}

pub(super) async fn change(
    _game: Option<GameProcess>,
    target: VolumeTarget,
    change: Change,
) -> Result<()> {
    if target == VolumeTarget::Game {
        bail!("macOS has no volume per program; change the system volume instead");
    }
    let now = system().await?;
    match change {
        Change::Step(step) => {
            let set = format!("set volume output volume {}", stepped(now.percent, step));
            if now.muted {
                osascript(&[&set, "set volume without output muted"]).await?;
            } else {
                osascript(&[&set]).await?;
            }
        }
        Change::ToggleMute => {
            let set = if now.muted {
                "set volume without output muted"
            } else {
                "set volume with output muted"
            };
            osascript(&[set]).await?;
        }
    }
    Ok(())
}
