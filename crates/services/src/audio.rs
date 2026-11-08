//! The system's volume and a running game's own, for the in-game overlay.
//!
//! Both go through PulseAudio's `pactl`, which PipeWire's `pipewire-pulse`
//! answers too, so one client covers every Linux desktop. The system volume
//! is the default output's. A game's volume is that of its streams ("sink
//! inputs"): a stream belongs to the game when the process that opened it
//! is in the game's process group or descends from the process Irori
//! started, which is how Wine's and Proton's helper processes are found.
//! Every command runs with `LC_ALL=C`, because `pactl` translates its
//! output. Only Linux has this; elsewhere every call fails plainly.

use std::collections::HashMap;

use anyhow::Result;
#[cfg(target_os = "linux")]
use anyhow::{Context as _, bail};
use irori_core::GameId;
use serde_json::Value;

use crate::Backend;

/// How far one press of the volume controls moves a volume, in percent.
pub const VOLUME_STEP: i32 = 5;
/// The loudest Irori sets a volume to: past 100% PulseAudio amplifies.
pub const MAX_VOLUME: u32 = 100;

/// A volume and whether it is muted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Volume {
    /// The average of the channels, in percent.
    pub percent: u32,
    pub muted: bool,
}

/// What the overlay shows of the sound.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GameAudio {
    /// The default output; `None` when it couldn't be read.
    pub system: Option<Volume>,
    /// The game's streams together (the loudest of them, and muted when all
    /// of them are); `None` while the game plays no sound.
    pub game: Option<Volume>,
}

/// Whose volume to change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VolumeTarget {
    System,
    Game,
}

/// One playing stream, as `pactl` lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SinkInput {
    pub index: u32,
    /// The process ids the stream names: the client's own, then the one the
    /// server saw (they differ inside a PID namespace).
    pub pids: Vec<u32>,
    pub volume: Volume,
}

/// The streams in `pactl -f json list sink-inputs`; `None` when the text is
/// not that JSON (older `pactl` versions write broken JSON for some
/// streams, and then the text form is read instead).
pub fn parse_sink_inputs_json(text: &str) -> Option<Vec<SinkInput>> {
    let list: Vec<Value> = serde_json::from_str(text).ok()?;
    Some(
        list.iter()
            .filter_map(|s| {
                let index = u32::try_from(s.get("index")?.as_u64()?).ok()?;
                let channels: Vec<u32> = s
                    .get("volume")
                    .and_then(Value::as_object)
                    .map(|v| {
                        v.values()
                            .filter_map(|c| c.get("value_percent")?.as_str())
                            .filter_map(parse_percent)
                            .collect()
                    })
                    .unwrap_or_default();
                let props = s.get("properties");
                let prop = |key: &str| {
                    props.and_then(|p| p.get(key)).and_then(|v| match v {
                        Value::String(s) => s.parse().ok(),
                        Value::Number(n) => n.as_u64().and_then(|n| u32::try_from(n).ok()),
                        _ => None,
                    })
                };
                Some(SinkInput {
                    index,
                    pids: pids_of(prop("application.process.id"), prop("pipewire.sec.pid")),
                    volume: Volume {
                        percent: average(&channels)?,
                        muted: s.get("mute").and_then(Value::as_bool).unwrap_or(false),
                    },
                })
            })
            .collect(),
    )
}

/// The streams in `pactl list sink-inputs`, written with `LC_ALL=C`.
pub fn parse_sink_inputs_text(text: &str) -> Vec<SinkInput> {
    let mut out = Vec::new();
    let mut block: Option<(u32, Vec<&str>)> = None;
    for line in text.lines().map(str::trim) {
        if let Some(rest) = line.strip_prefix("Sink Input #") {
            out.extend(block.take().and_then(text_block));
            block = rest.trim().parse().ok().map(|i| (i, Vec::new()));
        } else if let Some((_, lines)) = block.as_mut() {
            lines.push(line);
        }
    }
    out.extend(block.and_then(text_block));
    out
}

/// One `Sink Input #N` block of the text form, its lines trimmed.
fn text_block((index, lines): (u32, Vec<&str>)) -> Option<SinkInput> {
    let mut volume = None;
    let mut muted = false;
    let mut pid = None;
    let mut sec_pid = None;
    for line in lines {
        if let Some(rest) = line.strip_prefix("Volume:") {
            volume = parse_volume_line(rest);
        } else if let Some(rest) = line.strip_prefix("Mute:") {
            muted = rest.trim() == "yes";
        } else if let Some((key, value)) = line.split_once(" = ") {
            let value = value.trim().trim_matches('"');
            match key.trim() {
                "application.process.id" => pid = value.parse().ok(),
                "pipewire.sec.pid" => sec_pid = value.parse().ok(),
                _ => {}
            }
        }
    }
    Some(SinkInput {
        index,
        pids: pids_of(pid, sec_pid),
        volume: Volume {
            percent: volume?,
            muted,
        },
    })
}

fn pids_of(first: Option<u32>, second: Option<u32>) -> Vec<u32> {
    let mut pids: Vec<u32> = first.into_iter().collect();
    if let Some(pid) = second.filter(|p| !pids.contains(p)) {
        pids.push(pid);
    }
    pids
}

/// The volume in `pactl get-sink-volume` (`Volume: front-left: 26214 /  40% /
/// -23.88 dB,   front-right: …`): the average of the channels.
pub fn parse_sink_volume(text: &str) -> Option<u32> {
    text.lines()
        .find_map(|line| line.trim().strip_prefix("Volume:"))
        .and_then(parse_volume_line)
}

/// Whether `pactl get-sink-mute` (`Mute: yes`) says muted.
pub fn parse_mute(text: &str) -> Option<bool> {
    let value = text
        .lines()
        .find_map(|line| line.trim().strip_prefix("Mute:"))?;
    match value.trim() {
        "yes" => Some(true),
        "no" => Some(false),
        _ => None,
    }
}

/// The average of the `NN%` channel volumes on one `Volume:` line.
fn parse_volume_line(rest: &str) -> Option<u32> {
    let channels: Vec<u32> = rest
        .split('/')
        .map(str::trim)
        .filter(|part| part.ends_with('%'))
        .filter_map(parse_percent)
        .collect();
    average(&channels)
}

fn parse_percent(text: &str) -> Option<u32> {
    text.trim().strip_suffix('%')?.trim().parse().ok()
}

fn average(channels: &[u32]) -> Option<u32> {
    if channels.is_empty() {
        return None;
    }
    let sum: u64 = channels.iter().map(|&c| u64::from(c)).sum();
    u32::try_from((sum + channels.len() as u64 / 2) / channels.len() as u64).ok()
}

/// The parent and process group in a `/proc/<pid>/stat` line. The command
/// name is in parentheses and may hold spaces and parentheses itself, so
/// the fields are read after the last `)`.
pub fn parse_stat(text: &str) -> Option<(u32, u32)> {
    let rest = &text[text.rfind(')')? + 1..];
    let mut fields = rest.split_whitespace();
    let _state = fields.next()?;
    let ppid = fields.next()?.parse().ok()?;
    let pgrp = fields.next()?.parse().ok()?;
    Some((ppid, pgrp))
}

/// Whether `pid` belongs to the game Irori started as `game`: it is in the
/// game's process group or descends from it. `procs` maps every process to
/// its parent and process group.
pub fn belongs_to(pid: u32, game: u32, procs: &HashMap<u32, (u32, u32)>) -> bool {
    let mut current = pid;
    // A loop in a stale table can't hang this: the depth is bounded.
    for _ in 0..64 {
        if current == game {
            return true;
        }
        let Some(&(ppid, pgrp)) = procs.get(&current) else {
            return false;
        };
        if pgrp == game {
            return true;
        }
        if ppid == 0 || ppid == current {
            return false;
        }
        current = ppid;
    }
    false
}

/// The streams of the game started as `game`.
pub fn game_streams<'a>(
    inputs: &'a [SinkInput],
    game: u32,
    procs: &HashMap<u32, (u32, u32)>,
) -> Vec<&'a SinkInput> {
    inputs
        .iter()
        .filter(|s| s.pids.iter().any(|&pid| belongs_to(pid, game, procs)))
        .collect()
}

/// The game's streams as one volume: the loudest, muted when all are.
pub fn combined(streams: &[&SinkInput]) -> Option<Volume> {
    let percent = streams.iter().map(|s| s.volume.percent).max()?;
    Some(Volume {
        percent,
        muted: streams.iter().all(|s| s.volume.muted),
    })
}

/// `current` moved by `step`, kept within `0..=MAX_VOLUME`.
pub fn stepped(current: u32, step: i32) -> u32 {
    let next = i64::from(current) + i64::from(step);
    next.clamp(0, i64::from(MAX_VOLUME)) as u32
}

impl Backend {
    /// The system's volume and the running game's.
    pub async fn audio(&self, id: &GameId) -> Result<GameAudio> {
        platform::audio(self.running_pid(id)).await
    }

    /// Move a volume by `step` percent (a game's streams all together) and
    /// unmute it; returns the sound as it is then.
    pub async fn change_volume(
        &self,
        id: &GameId,
        target: VolumeTarget,
        step: i32,
    ) -> Result<GameAudio> {
        let game = self.running_pid(id);
        platform::change(game, target, Change::Step(step)).await?;
        platform::audio(game).await
    }

    /// Mute a volume, or unmute it when it is muted.
    pub async fn toggle_mute(&self, id: &GameId, target: VolumeTarget) -> Result<GameAudio> {
        let game = self.running_pid(id);
        platform::change(game, target, Change::ToggleMute).await?;
        platform::audio(game).await
    }

    /// Move the system's volume by `step` percent, for the controller's
    /// volume shortcut (no window shows it, so a failure is announced).
    pub fn step_system_volume(&self, step: i32) {
        let b = self.clone();
        self.spawn(async move {
            if let Err(e) = platform::change(None, VolumeTarget::System, Change::Step(step)).await {
                tracing::warn!("changing the volume: {e:#}");
                b.bus.notice(
                    irori_core::Notice::new(
                        irori_core::NoticeLevel::Error,
                        "The volume wasn't changed",
                    )
                    .message(format!("{e:#}"))
                    .keys("shell.notice.volume_failed", None, &[]),
                );
            }
        });
    }
}

// Only Linux reads what a change asks for.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
#[derive(Debug, Clone, Copy)]
enum Change {
    Step(i32),
    ToggleMute,
}

#[cfg(target_os = "linux")]
mod platform {
    use super::*;
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

    pub(super) async fn audio(game: Option<u32>) -> Result<GameAudio> {
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
        game: Option<u32>,
        target: VolumeTarget,
        change: Change,
    ) -> Result<()> {
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
}

#[cfg(not(target_os = "linux"))]
mod platform {
    use super::*;

    pub(super) async fn audio(_game: Option<u32>) -> Result<GameAudio> {
        anyhow::bail!("Volume control isn't available on this system yet")
    }

    pub(super) async fn change(
        _game: Option<u32>,
        _target: VolumeTarget,
        _change: Change,
    ) -> Result<()> {
        anyhow::bail!("Volume control isn't available on this system yet")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const JSON: &str = r#"[
      {"index": 42, "driver": "PipeWire", "mute": false,
       "volume": {"front-left": {"value": 39322, "value_percent": "60%", "db": "-13.31 dB"},
                  "front-right": {"value": 45875, "value_percent": "70%", "db": "-9.29 dB"}},
       "properties": {"application.name": "Game.exe", "application.process.id": "1234",
                      "pipewire.sec.pid": 98765}},
      {"index": 7, "mute": true,
       "volume": {"mono": {"value": 65536, "value_percent": "100%", "db": "0.00 dB"}},
       "properties": {"application.name": "Firefox", "application.process.id": "555"}}
    ]"#;

    const TEXT: &str = "Sink Input #42
	Driver: PipeWire
	Sink: 50
	Mute: no
	Volume: front-left: 39322 /  60% / -13.31 dB,   front-right: 45875 /  70% / -9.29 dB
	        balance 0.10
	Properties:
		application.name = \"Game.exe\"
		application.process.id = \"1234\"
		pipewire.sec.pid = \"98765\"

Sink Input #7
	Mute: yes
	Volume: mono: 65536 / 100% / 0.00 dB
	Properties:
		application.process.id = \"555\"
";

    #[test]
    fn reads_streams_from_json_and_text_alike() {
        let json = parse_sink_inputs_json(JSON).unwrap();
        let text = parse_sink_inputs_text(TEXT);
        assert_eq!(json, text);
        assert_eq!(json.len(), 2);
        assert_eq!(json[0].index, 42);
        assert_eq!(json[0].pids, [1234, 98765]);
        assert_eq!(
            json[0].volume,
            Volume {
                percent: 65,
                muted: false
            }
        );
        assert_eq!(json[1].pids, [555]);
        assert!(json[1].volume.muted);
        assert_eq!(parse_sink_inputs_json("Sink Input #1"), None);
        assert!(parse_sink_inputs_text("").is_empty());
    }

    #[test]
    fn reads_the_system_volume_and_mute() {
        let out = "Volume: front-left: 26214 /  40% / -23.88 dB,   front-right: 26214 /  40% / -23.88 dB\n        balance 0.00\n";
        assert_eq!(parse_sink_volume(out), Some(40));
        assert_eq!(parse_sink_volume("nothing"), None);
        assert_eq!(parse_mute("Mute: yes\n"), Some(true));
        assert_eq!(parse_mute("Mute: no"), Some(false));
        assert_eq!(parse_mute(""), None);
    }

    #[test]
    fn reads_parent_and_group_past_odd_command_names() {
        let stat = "4242 (Game (x64).exe) S 4200 4100 4100 0 -1 4194304 1 0 0 0";
        assert_eq!(parse_stat(stat), Some((4200, 4100)));
        assert_eq!(parse_stat("garbage"), None);
    }

    #[test]
    fn finds_the_games_streams_by_group_or_ancestry() {
        // 100 is the process Irori started (its group); 200 is wineserver's
        // child that left the group; 300 belongs to another program.
        let procs: HashMap<u32, (u32, u32)> = [
            (100, (1, 100)),
            (110, (100, 100)),
            (150, (1, 100)),
            (200, (110, 200)),
            (300, (1, 300)),
            (400, (400, 400)),
        ]
        .into_iter()
        .collect();
        assert!(belongs_to(100, 100, &procs));
        assert!(belongs_to(150, 100, &procs), "same group, reparented");
        assert!(belongs_to(200, 100, &procs), "descendant in a new group");
        assert!(!belongs_to(300, 100, &procs));
        assert!(!belongs_to(400, 100, &procs), "its own parent");
        assert!(!belongs_to(999, 100, &procs), "unknown");

        let stream = |index, pids: &[u32], percent, muted| SinkInput {
            index,
            pids: pids.to_vec(),
            volume: Volume { percent, muted },
        };
        let inputs = [
            stream(1, &[200], 80, true),
            stream(2, &[300], 50, false),
            // Seen through a PID namespace: the server's pid matches.
            stream(3, &[5, 150], 40, true),
        ];
        let mine = game_streams(&inputs, 100, &procs);
        assert_eq!(mine.iter().map(|s| s.index).collect::<Vec<_>>(), [1, 3]);
        assert_eq!(
            combined(&mine),
            Some(Volume {
                percent: 80,
                muted: true
            })
        );
        assert_eq!(combined(&[]), None);
    }

    #[test]
    fn volume_steps_stay_in_range() {
        assert_eq!(stepped(50, VOLUME_STEP), 55);
        assert_eq!(stepped(3, -VOLUME_STEP), 0);
        assert_eq!(stepped(98, VOLUME_STEP), MAX_VOLUME);
        assert_eq!(stepped(120, -VOLUME_STEP), MAX_VOLUME);
    }
}
