//! Poster frames and lengths of the user's clips, made with ffmpeg when the
//! computer has it ([`super::ffmpeg`]). A clip's poster is a representative
//! keyframe from about two thirds in (ffmpeg's `thumbnail` filter picks the
//! frame closest to the average of a few, which passes over black and faded
//! frames), so it shows the play rather than a loading screen.
//!
//! Posters are JPEG files in `<cache>/clip-posters/`, and their lengths are
//! kept in `<cache>/clip-posters.json`, both by the clip's path, size and
//! modified time: clips are only read, and one that changed gets a new
//! poster. Posters are made one at a time with one decoder thread, and only
//! when a view asks for one, so a big clip folder never takes the computer.

use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::{Duration, UNIX_EPOCH};

use anyhow::{Context as _, Result, bail};
use irori_core::process::{self, RunOptions};
use irori_core::{Paths, json_store};
use once_cell::sync::{Lazy, OnceCell};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use tokio::sync::Semaphore;

/// Pixel width of a poster; the art store cuts smaller thumbnails from it.
const POSTER_WIDTH: u32 = 1280;
/// Keyframes the poster is picked from.
const CANDIDATES: u32 = 6;
const CACHE_VERSION: u32 = 1;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
struct Entry {
    size: u64,
    /// Unix time in milliseconds.
    modified: i64,
    /// The clip's length, when ffmpeg could tell.
    length_ms: Option<u64>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
struct Cache {
    version: u32,
    /// By the clip's path.
    clips: HashMap<String, Entry>,
}

static CACHE: OnceCell<RwLock<Cache>> = OnceCell::new();
/// One poster at a time.
static QUEUE: Lazy<Semaphore> = Lazy::new(|| Semaphore::new(1));

fn cache_file() -> PathBuf {
    Paths::get().cache.join("clip-posters.json")
}

fn poster_dir() -> PathBuf {
    Paths::get().cache.join("clip-posters")
}

/// The cache, read from disk the first time (callers are off the UI thread).
fn cache() -> &'static RwLock<Cache> {
    CACHE.get_or_init(|| {
        let cache: Cache = json_store::read_or_default(&cache_file());
        RwLock::new(if cache.version == CACHE_VERSION {
            cache
        } else {
            Cache::default()
        })
    })
}

/// Size and modified time of the clip at `path`.
fn stamp(path: &Path) -> Result<(u64, i64)> {
    let meta = std::fs::metadata(path).with_context(|| format!("reading {}", path.display()))?;
    let modified = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_millis() as i64);
    Ok((meta.len(), modified))
}

/// Where the poster of a clip of this path, size and time goes.
fn poster_path(path: &Path, size: u64, modified: i64) -> PathBuf {
    let digest = irori_core::paths::art_digest(&format!("{}|{size}|{modified}", path.display()));
    poster_dir().join(format!("{digest}.jpg"))
}

/// The length of the clip at `path`, if its poster was made and ffmpeg could
/// tell. Never reads the disk, so views may ask while drawing.
pub fn known_length(path: &Path) -> Option<Duration> {
    let cache = CACHE.get()?.read();
    let entry = cache.clips.get(path.to_str()?)?;
    entry.length_ms.map(Duration::from_millis)
}

/// The poster of the clip at `path`, made now when it isn't there yet.
/// Fails when the computer has no ffmpeg or it can't read the clip.
pub async fn poster(path: &Path) -> Result<PathBuf> {
    let clip = path.to_path_buf();
    let (size, modified) = tokio::task::spawn_blocking({
        let clip = clip.clone();
        move || {
            // Reading the cache here keeps its first read off the caller.
            cache();
            stamp(&clip)
        }
    })
    .await??;
    let out = poster_path(&clip, size, modified);
    let key = clip.to_string_lossy().into_owned();
    let known = |cache: &Cache| {
        cache
            .clips
            .get(&key)
            .is_some_and(|e| e.size == size && e.modified == modified)
    };
    if known(&cache().read()) && out.is_file() {
        return Ok(out);
    }
    let _turn = QUEUE.acquire().await?;
    if known(&cache().read()) && out.is_file() {
        return Ok(out);
    }
    let Some(ffmpeg) = super::ffmpeg::find().await else {
        bail!("ffmpeg isn't installed");
    };
    let length = probe_length(&ffmpeg, &clip).await;
    tokio::fs::create_dir_all(poster_dir()).await?;
    let partial = out.with_extension("part.jpg");
    let mut run = process::run(
        &ffmpeg.command(poster_args(&clip, length, &partial)),
        RunOptions::default(),
    )
    .await?;
    if !run.success() && length.is_some() {
        // A length the container got wrong can seek past the frames: the
        // start of the clip will do.
        let spec = ffmpeg.command(poster_args(&clip, None, &partial));
        run = process::run(&spec, RunOptions::default()).await?;
    }
    if !run.success() || !partial.is_file() {
        let _ = tokio::fs::remove_file(&partial).await;
        bail!(
            "ffmpeg couldn't read {}: {}",
            clip.display(),
            run.error_summary()
        );
    }
    tokio::fs::rename(&partial, &out).await?;
    let previous = {
        let mut cache = cache().write();
        cache.version = CACHE_VERSION;
        cache.clips.insert(
            key,
            Entry {
                size,
                modified,
                length_ms: length.map(|l| l.as_millis() as u64),
            },
        )
    };
    // A clip that changed leaves its old poster behind.
    if let Some(old) = previous.filter(|e| (e.size, e.modified) != (size, modified)) {
        let _ = tokio::fs::remove_file(poster_path(&clip, old.size, old.modified)).await;
    }
    let snapshot = cache().read().clone();
    tokio::task::spawn_blocking(move || json_store::write(&cache_file(), &snapshot)).await??;
    Ok(out)
}

/// The clip's length as ffmpeg reads it from the container.
async fn probe_length(ffmpeg: &super::ffmpeg::Ffmpeg, clip: &Path) -> Option<Duration> {
    let mut args: Vec<OsString> = ["-hide_banner", "-nostdin", "-i"]
        .into_iter()
        .map(OsString::from)
        .collect();
    args.push(clip.into());
    // Without an output ffmpeg describes the input and exits with an error.
    let run = process::run(&ffmpeg.command(args), RunOptions::default())
        .await
        .ok()?;
    parse_length(&run.stderr)
}

/// The `Duration:` ffmpeg prints for its input (`00:01:23.45`).
pub fn parse_length(text: &str) -> Option<Duration> {
    let value = text
        .lines()
        .find_map(|l| l.trim_start().strip_prefix("Duration:"))?
        .split(',')
        .next()?
        .trim();
    let mut parts = value.split(':');
    let (h, m, s) = (parts.next()?, parts.next()?, parts.next()?);
    let secs = h.parse::<u64>().ok()? as f64 * 3600.
        + m.parse::<u64>().ok()? as f64 * 60.
        + s.parse::<f64>().ok()?;
    (secs.is_finite() && secs > 0.).then(|| Duration::from_secs_f64(secs))
}

/// Where in a clip of `length` the poster is looked for: two thirds in.
fn poster_time(length: Option<Duration>) -> Duration {
    length.map_or(Duration::ZERO, |l| l * 2 / 3)
}

/// ffmpeg's arguments for a poster of `clip` into `out`: seek to the poster
/// time, decode keyframes only with one thread, pick one of
/// [`CANDIDATES`] and scale it to [`POSTER_WIDTH`] (never up).
fn poster_args(clip: &Path, length: Option<Duration>, out: &Path) -> Vec<OsString> {
    let at = poster_time(length);
    let mut args: Vec<OsString> = Vec::new();
    let mut push = |a: &str| args.push(a.into());
    for a in [
        "-hide_banner",
        "-nostdin",
        "-loglevel",
        "error",
        "-threads",
        "1",
    ] {
        push(a);
    }
    // Without accurate seeking the keyframe before the poster time is kept,
    // which a clip with few keyframes needs: decoding keyframes only would
    // otherwise drop everything up to the next one, maybe past the end.
    for a in ["-skip_frame", "nokey", "-noaccurate_seek", "-ss"] {
        push(a);
    }
    push(&format!("{:.3}", at.as_secs_f64()));
    push("-i");
    args.push(clip.into());
    let filter = format!("thumbnail={CANDIDATES},scale='min({POSTER_WIDTH},iw)':-2");
    let mut push = |a: &str| args.push(a.into());
    for a in [
        "-an",
        "-sn",
        "-dn",
        "-vf",
        &filter,
        "-frames:v",
        "1",
        "-q:v",
        "3",
        "-y",
    ] {
        push(a);
    }
    args.push(out.into());
    args
}

/// A clip's length as the album shows it: `0:42`, `12:05`, `1:02:09`.
pub fn length_label(length: Duration) -> String {
    let total = length.as_secs_f64().round() as u64;
    let (h, m, s) = (total / 3600, total / 60 % 60, total % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_ffmpegs_duration() {
        let text = "Input #0, mov,mp4,m4a,3gp,3g2,mj2, from 'clip.mp4':\n  Metadata:\n    \
                    major_brand     : isom\n  Duration: 00:01:23.45, start: 0.000000, \
                    bitrate: 8000 kb/s\n";
        assert_eq!(parse_length(text), Some(Duration::from_secs_f64(83.45)));
        assert_eq!(
            parse_length("  Duration: 01:00:00.00, start: 0"),
            Some(Duration::from_secs(3600))
        );
        assert_eq!(parse_length("  Duration: N/A, start: 0.000000"), None);
        assert_eq!(parse_length("no input here"), None);
    }

    #[test]
    fn looks_two_thirds_in() {
        assert_eq!(
            poster_time(Some(Duration::from_secs(30))),
            Duration::from_secs(20)
        );
        assert_eq!(poster_time(None), Duration::ZERO);
        let args = poster_args(
            Path::new("/clips/Hades 2026.mp4"),
            Some(Duration::from_secs(90)),
            Path::new("/cache/p.jpg"),
        );
        let args: Vec<String> = args.iter().map(|a| a.to_string_lossy().into()).collect();
        let at = args.iter().position(|a| a == "-ss").unwrap();
        assert_eq!(args[at + 1], "60.000");
        // The seek and the keyframe skip are input options: before `-i`.
        let input = args.iter().position(|a| a == "-i").unwrap();
        assert!(at < input && args.iter().position(|a| a == "-skip_frame").unwrap() < input);
        assert_eq!(args[input + 1], "/clips/Hades 2026.mp4");
        assert_eq!(args.last().unwrap(), "/cache/p.jpg");
        assert!(args.contains(&"-frames:v".to_string()));
    }

    #[test]
    fn labels_lengths() {
        assert_eq!(length_label(Duration::from_secs(42)), "0:42");
        assert_eq!(length_label(Duration::from_millis(725_400)), "12:05");
        assert_eq!(length_label(Duration::from_secs(3729)), "1:02:09");
    }

    /// Runs where the computer has ffmpeg (it makes its own clip, so it
    /// reads nothing of the user's); elsewhere there is nothing to run.
    #[test]
    fn makes_a_poster_and_learns_the_length() {
        crate::tests::isolate_paths();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let Some(ffmpeg) = rt.block_on(super::super::ffmpeg::find()) else {
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let clip = dir.path().join("Game 2026-11-23 10-00-00.mp4");
        let make = ffmpeg.command([
            "-hide_banner",
            "-loglevel",
            "error",
            "-f",
            "lavfi",
            "-i",
            "testsrc=duration=3:size=320x180:rate=30",
            "-pix_fmt",
            "yuv420p",
            "-y",
            clip.to_str().unwrap(),
        ]);
        let made = rt
            .block_on(process::run(&make, RunOptions::default()))
            .unwrap();
        if !made.success() {
            // An ffmpeg without an encoder for mp4 can't make the clip.
            return;
        }
        assert_eq!(known_length(&clip), None);
        let poster = rt.block_on(poster(&clip)).unwrap();
        assert!(poster.is_file() && poster.starts_with(poster_dir()));
        assert_eq!(known_length(&clip), Some(Duration::from_secs(3)));
        // Made once: asking again finds it.
        assert_eq!(rt.block_on(super::poster(&clip)).unwrap(), poster);
    }

    #[test]
    fn a_changed_clip_gets_another_poster() {
        crate::tests::isolate_paths();
        let p = Path::new("/clips/a.mp4");
        assert_ne!(poster_path(p, 1, 2), poster_path(p, 1, 3));
        assert_eq!(poster_path(p, 1, 2), poster_path(p, 1, 2));
    }
}
