//! Linux: PulseAudio's `pactl`, which PipeWire's `pipewire-pulse` answers
//! too (see the parent module).

use super::*;
use anyhow::{Context as _, bail};
use irori_core::process::{self, CommandSpec, RunOptions};

async fn pactl(args: &[&str]) -> Result<String> {
    let program = irori_core::host::find_program("pactl")
        .await
        .context("pactl isn't installed (it comes with PulseAudio's or PipeWire's tools)")?;
    let spec = irori_core::host::command(
        CommandSpec::new(program)
            .args(args.iter().copied())
            .env("LC_ALL", "C"),
    );
    let out = process::run(&spec, RunOptions::default()).await?;
    if !out.success() {
        bail!("pactl: {}", out.error_summary());
    }
    Ok(out.stdout)
}

async fn sink_inputs() -> Result<Vec<SinkInput>> {
    if let Ok(text) = pactl(&["-f", "json", "list", "sink-inputs"]).await
        && let Some(list) = parse_sink_inputs_json(&text)
    {
        return Ok(list);
    }
    Ok(parse_sink_inputs_text(
        &pactl(&["list", "sink-inputs"]).await?,
    ))
}

/// Every process's parent and group, from `/proc`.
fn process_table() -> HashMap<u32, (u32, u32)> {
    let Ok(dir) = std::fs::read_dir("/proc") else {
        return HashMap::new();
    };
    dir.flatten()
        .filter_map(|entry| {
            let pid: u32 = entry.file_name().to_str()?.parse().ok()?;
            let stat = std::fs::read_to_string(entry.path().join("stat")).ok()?;
            Some((pid, parse_stat(&stat)?))
        })
        .collect()
}

async fn streams_of(game: u32) -> Result<Vec<SinkInput>> {
    let inputs = sink_inputs().await?;
    let procs = tokio::task::spawn_blocking(process_table)
        .await
        .unwrap_or_default();
    Ok(game_streams(&inputs, game, &procs)
        .into_iter()
        .cloned()
        .collect())
}

async fn system() -> Result<Volume> {
    let volume = pactl(&["get-sink-volume", "@DEFAULT_SINK@"]).await?;
    let mute = pactl(&["get-sink-mute", "@DEFAULT_SINK@"]).await?;
    Ok(Volume {
        percent: parse_sink_volume(&volume).context("Could not read the volume")?,
        muted: parse_mute(&mute).unwrap_or(false),
    })
}

pub(super) async fn audio(game: Option<GameProcess>) -> Result<GameAudio> {
    let game = game.map(|g| g.pid);
    let system = system().await;
    let streams = match game {
        Some(game) => streams_of(game).await.unwrap_or_default(),
        None => Vec::new(),
    };
    if let (Err(e), true) = (&system, streams.is_empty()) {
        bail!("{e:#}");
    }
    let refs: Vec<&SinkInput> = streams.iter().collect();
    Ok(GameAudio {
        system: system.ok(),
        game: combined(&refs),
    })
}

pub(super) async fn change(
    game: Option<GameProcess>,
    target: VolumeTarget,
    change: Change,
) -> Result<()> {
    let game = game.map(|g| g.pid);
    match target {
        VolumeTarget::System => {
            let sink = "@DEFAULT_SINK@";
            match change {
                Change::Step(step) => {
                    let now = system().await?;
                    let next = format!("{}%", stepped(now.percent, step));
                    pactl(&["set-sink-volume", sink, &next]).await?;
                    if now.muted {
                        pactl(&["set-sink-mute", sink, "0"]).await?;
                    }
                }
                Change::ToggleMute => {
                    pactl(&["set-sink-mute", sink, "toggle"]).await?;
                }
            }
        }
        VolumeTarget::Game => {
            let game = game.context("The game isn't running")?;
            let streams = streams_of(game).await?;
            let refs: Vec<&SinkInput> = streams.iter().collect();
            let Some(now) = combined(&refs) else {
                bail!("The game isn't playing any sound");
            };
            for stream in &streams {
                let index = stream.index.to_string();
                match change {
                    Change::Step(step) => {
                        let next = format!("{}%", stepped(now.percent, step));
                        pactl(&["set-sink-input-volume", &index, &next]).await?;
                        if stream.volume.muted {
                            pactl(&["set-sink-input-mute", &index, "0"]).await?;
                        }
                    }
                    Change::ToggleMute => {
                        let mute = if now.muted { "0" } else { "1" };
                        pactl(&["set-sink-input-mute", &index, mute]).await?;
                    }
                }
            }
        }
    }
    Ok(())
}

pub(super) async fn outputs() -> Result<Vec<AudioOutput>> {
    let default = pactl(&["get-default-sink"]).await.unwrap_or_default();
    let text = pactl(&["-f", "json", "list", "sinks"]).await?;
    parse_sinks_json(&text, &default).context("pactl is too old to list the sound outputs")
}

pub(super) async fn set_output(game: Option<GameProcess>, id: String) -> Result<()> {
    pactl(&["set-default-sink", &id]).await?;
    // Streams PipeWire placed on an output of their own stay there; move
    // the game's along.
    if let Some(game) = game {
        for stream in streams_of(game.pid).await.unwrap_or_default() {
            let index = stream.index.to_string();
            if let Err(e) = pactl(&["move-sink-input", &index, &id]).await {
                tracing::info!("moving the game's sound to {id}: {e:#}");
            }
        }
    }
    Ok(())
}
