//! Trailers played through the computer's ffmpeg ([`super::ffmpeg`]): it
//! reads the store's file or stream at its own pace (`-re`), scales each
//! frame to the size the page draws it at, and writes raw BGRA frames to a
//! pipe, which [`Trailer`] hands to the game page. With sound on, the same
//! ffmpeg plays it through the system's sound output; ffmpeg has none on
//! Windows, where trailers stay muted.
//!
//! Nothing runs unless a trailer plays: pausing, hiding the page or
//! dropping the [`Trailer`] ends ffmpeg, and playing again starts a new one
//! at the position reached. A trailer that ends starts over, so it loops.
//! Frames the page hasn't drawn yet are dropped rather than queued, so a
//! busy window never makes the trailer fall behind.

use std::ffi::OsString;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Result, bail};
use irori_core::process::{self, CancellationToken};
use tokio::io::{AsyncBufReadExt as _, AsyncReadExt as _, BufReader};
use tokio::sync::mpsc;

use super::ffmpeg::Ffmpeg;

/// Frames a second the page draws a trailer at; trailers made at 60 are
/// halved, which costs half the copying and reads the same at that size.
pub const FPS: u32 = 30;
/// Widest a trailer is decoded at, in pixels; Steam's smaller files are 480p.
pub const MAX_WIDTH: u32 = 960;
/// Narrowest, so a tiny stage still gets a picture worth scaling.
const MIN_WIDTH: u32 = 320;
/// How many frames may wait for the page.
const QUEUE: usize = 2;

/// What to play.
#[derive(Debug, Clone, PartialEq)]
pub struct TrailerRequest {
    /// The `https` file or stream address.
    pub url: String,
    /// Width the page draws the trailer at, in pixels.
    pub width: u32,
    /// Where to start.
    pub start: Duration,
    /// Sound volume from 0 to 1, or `None` for muted.
    pub volume: Option<f32>,
}

/// One frame, BGRA, `width × height`, top row first.
#[derive(Debug)]
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub bgra: Vec<u8>,
    /// Where in the trailer it is.
    pub position: Duration,
}

/// What a playing trailer reports.
#[derive(Debug)]
pub enum TrailerEvent {
    Frame(Frame),
    /// It couldn't be played (the stream failed before its first frame).
    Failed(String),
}

/// A trailer playing: its frames, and what stops it. Dropping `stop` (or
/// the whole trailer) ends ffmpeg.
pub struct Trailer {
    pub events: mpsc::Receiver<TrailerEvent>,
    pub stop: TrailerStop,
}

/// Ends the trailer's ffmpeg when dropped.
pub struct TrailerStop(CancellationToken);

impl Drop for TrailerStop {
    fn drop(&mut self) {
        self.0.cancel();
    }
}

/// The frame size for a stage `width` wide: even, 16:9, within bounds.
pub fn frame_size(width: u32) -> (u32, u32) {
    let w = width.clamp(MIN_WIDTH, MAX_WIDTH) & !1;
    let h = (w * 9 / 16) & !1;
    (w, h)
}

/// ffmpeg's arguments for `request` decoded at `width × height`, with
/// sound through `audio` (an output device) when the request has a volume.
/// Only network protocols are allowed, so an address from a store page
/// can't make ffmpeg read a file of this computer.
pub fn trailer_args(
    request: &TrailerRequest,
    size: (u32, u32),
    audio: Option<&str>,
) -> Vec<OsString> {
    args_with(request, size, audio, PROTOCOLS)
}

/// The protocols ffmpeg may use for a trailer: HTTPS and what HLS and DASH
/// streams need under it.
const PROTOCOLS: &str = "https,tls,tcp,crypto";

fn args_with(
    request: &TrailerRequest,
    (width, height): (u32, u32),
    audio: Option<&str>,
    protocols: &str,
) -> Vec<OsString> {
    let mut args: Vec<String> = [
        "-hide_banner",
        "-nostdin",
        "-loglevel",
        "error",
        "-re",
        "-protocol_whitelist",
        protocols,
    ]
    .map(String::from)
    .to_vec();
    if !request.start.is_zero() {
        args.extend(["-ss".into(), format!("{:.3}", request.start.as_secs_f64())]);
    }
    args.extend(["-i".into(), request.url.clone()]);
    let video = format!(
        "scale={width}:{height}:force_original_aspect_ratio=decrease:flags=bilinear,\
         pad={width}:{height}:(ow-iw)/2:(oh-ih)/2,fps={FPS}"
    );
    args.extend(
        [
            "-map", "0:v:0", "-vf", &video, "-pix_fmt", "bgra", "-f", "rawvideo", "pipe:1",
        ]
        .map(String::from),
    );
    if let (Some(volume), Some(device)) = (request.volume, audio) {
        let volume = format!("volume={:.2}", volume.clamp(0., 1.));
        args.extend(["-map", "0:a:0", "-af", &volume, "-ac", "2"].map(String::from));
        args.extend(["-f".into(), device.to_string()]);
        // PulseAudio names the stream after the "file"; AudioToolbox wants `-`.
        args.push(if device == "pulse" { "Irori" } else { "-" }.into());
    }
    args.into_iter().map(OsString::from).collect()
}

/// Start playing `request` with `ffmpeg`. Must run inside the backend's
/// runtime.
pub fn play(ffmpeg: Arc<Ffmpeg>, request: TrailerRequest) -> Result<Trailer> {
    if !request.url.starts_with("https://") {
        bail!("Trailers play only from secure addresses");
    }
    Ok(start(ffmpeg, request, PROTOCOLS))
}

fn start(ffmpeg: Arc<Ffmpeg>, request: TrailerRequest, protocols: &'static str) -> Trailer {
    let (tx, rx) = mpsc::channel(QUEUE);
    let stop = CancellationToken::new();
    tokio::spawn(run(ffmpeg, request, protocols, tx, stop.clone()));
    Trailer {
        events: rx,
        stop: TrailerStop(stop),
    }
}

/// How one run of ffmpeg ended.
enum RunEnd {
    /// It played to the end after `frames` frames.
    Ended {
        frames: u64,
        error: String,
    },
    Stopped,
}

async fn run(
    ffmpeg: Arc<Ffmpeg>,
    mut request: TrailerRequest,
    protocols: &'static str,
    tx: mpsc::Sender<TrailerEvent>,
    stop: CancellationToken,
) {
    let size = frame_size(request.width);
    loop {
        let end = match run_once(&ffmpeg, &request, size, protocols, &tx, &stop).await {
            Ok(end) => end,
            Err(e) => RunEnd::Ended {
                frames: 0,
                error: format!("{e:#}"),
            },
        };
        match end {
            RunEnd::Stopped => return,
            RunEnd::Ended { frames: 0, error } => {
                // Some trailers have no sound to play: try once without.
                if request.volume.is_some() && ffmpeg.audio_output.is_some() {
                    request.volume = None;
                    continue;
                }
                let _ = tx.send(TrailerEvent::Failed(error)).await;
                return;
            }
            // Played through: from the start again.
            RunEnd::Ended { .. } => request.start = Duration::ZERO,
        }
    }
}

async fn run_once(
    ffmpeg: &Ffmpeg,
    request: &TrailerRequest,
    (width, height): (u32, u32),
    protocols: &str,
    tx: &mpsc::Sender<TrailerEvent>,
    stop: &CancellationToken,
) -> Result<RunEnd> {
    let audio = ffmpeg.audio_output.as_deref();
    let args = args_with(request, (width, height), audio, protocols);
    let mut child = process::spawn_piped(&ffmpeg.command(args))?;
    let Some(mut stdout) = child.stdout.take() else {
        bail!("ffmpeg gave no output");
    };
    // The last thing ffmpeg complained about, for the page to say.
    let stderr = child.stderr.take().map(|err| {
        tokio::spawn(async move {
            let mut last = String::new();
            let mut lines = BufReader::new(err).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if !line.trim().is_empty() {
                    last = line;
                }
            }
            last
        })
    });
    let frame_len = width as usize * height as usize * 4;
    let mut frames = 0u64;
    loop {
        let mut bgra = vec![0u8; frame_len];
        tokio::select! {
            read = stdout.read_exact(&mut bgra) => {
                if read.is_err() {
                    break;
                }
            }
            _ = stop.cancelled() => {
                let _ = child.start_kill();
                let _ = child.wait().await;
                return Ok(RunEnd::Stopped);
            }
        }
        let position = request.start + Duration::from_secs_f64(frames as f64 / f64::from(FPS));
        frames += 1;
        let frame = Frame {
            width,
            height,
            bgra,
            position,
        };
        match tx.try_send(TrailerEvent::Frame(frame)) {
            Ok(()) | Err(mpsc::error::TrySendError::Full(_)) => {}
            // The page let go of the trailer.
            Err(mpsc::error::TrySendError::Closed(_)) => {
                let _ = child.start_kill();
                let _ = child.wait().await;
                return Ok(RunEnd::Stopped);
            }
        }
    }
    let _ = child.wait().await;
    let error = match stderr {
        Some(task) => task.await.unwrap_or_default(),
        None => String::new(),
    };
    Ok(RunEnd::Ended { frames, error })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(volume: Option<f32>, start: u64) -> TrailerRequest {
        TrailerRequest {
            url: "https://cdn.example/movie480.mp4".into(),
            width: 1000,
            start: Duration::from_secs(start),
            volume,
        }
    }

    fn strings(args: Vec<OsString>) -> Vec<String> {
        args.into_iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn sizes_frames_for_the_stage() {
        assert_eq!(frame_size(1000), (960, 540));
        assert_eq!(frame_size(641), (640, 360));
        assert_eq!(frame_size(10), (320, 180));
        let (w, h) = frame_size(853);
        assert!(w % 2 == 0 && h % 2 == 0);
    }

    #[test]
    fn decodes_to_raw_frames_from_the_network_only() {
        let args = strings(trailer_args(&request(None, 0), (960, 540), Some("pulse")));
        let at = |a: &str| args.iter().position(|x| x == a).unwrap();
        assert_eq!(args[at("-protocol_whitelist") + 1], "https,tls,tcp,crypto");
        // Read at its own pace, from the start.
        assert!(at("-re") < at("-i") && !args.contains(&"-ss".into()));
        assert_eq!(args[at("-pix_fmt") + 1], "bgra");
        assert!(
            args[at("-vf") + 1].contains("scale=960:540")
                && args[at("-vf") + 1].ends_with("fps=30")
        );
        assert_eq!(args.last().unwrap(), "pipe:1");
        // Muted: no sound output at all.
        assert!(!args.contains(&"0:a:0".into()));
    }

    #[test]
    fn plays_sound_through_the_systems_output_and_resumes_where_it_was() {
        let args = strings(trailer_args(
            &request(Some(0.4), 12),
            (960, 540),
            Some("pulse"),
        ));
        let at = |a: &str| args.iter().position(|x| x == a).unwrap();
        assert_eq!(args[at("-ss") + 1], "12.000");
        assert!(at("-ss") < at("-i"));
        assert_eq!(args[at("-af") + 1], "volume=0.40");
        assert_eq!(&args[args.len() - 3..], ["-f", "pulse", "Irori"]);
        let mac = strings(trailer_args(
            &request(Some(2.), 0),
            (960, 540),
            Some("audiotoolbox"),
        ));
        assert_eq!(&mac[mac.len() - 3..], ["-f", "audiotoolbox", "-"]);
        assert!(mac.contains(&"volume=1.00".into()));
        // No output device (Windows): muted whatever the request says.
        let none = strings(trailer_args(&request(Some(0.5), 0), (960, 540), None));
        assert!(!none.contains(&"0:a:0".into()));
    }

    #[test]
    fn refuses_addresses_that_arent_secure() {
        crate::tests::isolate_paths();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _enter = rt.enter();
        let ffmpeg = Arc::new(Ffmpeg {
            program: "ffmpeg".into(),
            host: false,
            audio_output: None,
        });
        for url in ["file:///etc/passwd", "http://cdn/x.mp4", "concat:a|b"] {
            let req = TrailerRequest {
                url: url.into(),
                ..request(None, 0)
            };
            assert!(play(ffmpeg.clone(), req).is_err());
        }
    }

    /// Runs where the computer has ffmpeg: the arguments decode a trailer it
    /// makes itself into whole BGRA frames, it starts over at its end, and
    /// dropping it ends ffmpeg. Sound is asked for and, the clip having
    /// none, left out.
    #[test]
    fn plays_loops_and_stops() {
        crate::tests::isolate_paths();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let Some(ffmpeg) = rt.block_on(super::super::ffmpeg::find()) else {
            return;
        };
        // A local file stands in for the store's stream, so this test alone
        // lets ffmpeg read files.
        let dir = tempfile::tempdir().unwrap();
        let clip = dir.path().join("t.mp4");
        let make = ffmpeg.command([
            "-hide_banner",
            "-loglevel",
            "error",
            "-f",
            "lavfi",
            "-i",
            "testsrc=duration=1:size=320x180:rate=30",
            "-pix_fmt",
            "yuv420p",
            "-y",
            clip.to_str().unwrap(),
        ]);
        let made = rt
            .block_on(process::run(&make, process::RunOptions::default()))
            .unwrap();
        if !made.success() {
            return;
        }
        let req = TrailerRequest {
            url: clip.to_string_lossy().into_owned(),
            width: 320,
            start: Duration::from_millis(500),
            volume: Some(0.5),
        };
        let (w, h) = frame_size(320);
        let positions = rt.block_on(async {
            let mut trailer = start(ffmpeg, req, "file");
            let mut positions = Vec::new();
            while positions.len() < 30 {
                match tokio::time::timeout(Duration::from_secs(20), trailer.events.recv()).await {
                    Ok(Some(TrailerEvent::Frame(f))) => {
                        assert_eq!((f.width, f.height), (w, h));
                        assert_eq!(f.bgra.len(), (w * h * 4) as usize);
                        // testsrc isn't black: the frame has colour.
                        assert!(f.bgra.iter().any(|b| *b > 16));
                        positions.push(f.position);
                    }
                    other => panic!("no frame: {other:?}"),
                }
            }
            drop(trailer);
            positions
        });
        // From half a second in to the end, then from the start again.
        assert!(positions[0] >= Duration::from_millis(500));
        assert!(positions.windows(2).any(|p| p[1] < p[0]));
    }
}
