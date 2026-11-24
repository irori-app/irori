//! Big Picture's navigation sounds (`iroriBigPicture.sounds`): a short tone
//! when the focus moves, something opens, something closes or the tab
//! changes.
//!
//! The tones are made here (a few milliseconds of samples, see [`samples`])
//! and written once per style, cue and volume as WAV files in the cache.
//! Irori links no audio library: the system plays them. Windows plays them
//! itself (`PlaySoundW`, asynchronous, no process); macOS with `afplay`;
//! Linux with PipeWire's `pw-play`, else PulseAudio's `paplay`, else ALSA's
//! `aplay`, whichever is on `PATH`. Without one there is no sound and the
//! setting's help says what is needed.
//!
//! A held direction repeats many times a second, so a move plays only when
//! the previous tone has finished and a little time has passed; opening and
//! closing always play. Nothing runs while no sound is due.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use gpui_kit::{App, Global};
use irori_core::big_picture_prefs::{BigPicturePrefs, NavSounds};
use irori_core::{Paths, json_store};
use once_cell::sync::Lazy;
use parking_lot::Mutex;

/// What a tone stands for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Cue {
    /// The focus moved.
    Move,
    /// Something opened or was chosen.
    Confirm,
    /// Something closed.
    Back,
    /// The tab changed.
    Tab,
}

impl Cue {
    fn id(self) -> &'static str {
        match self {
            Cue::Move => "move",
            Cue::Confirm => "confirm",
            Cue::Back => "back",
            Cue::Tab => "tab",
        }
    }
}

/// Samples per second of the tones.
pub const RATE: u32 = 44_100;

/// One note of a tone: frequency in Hz and length in milliseconds.
type Note = (f32, u32);

fn notes(style: NavSounds, cue: Cue) -> &'static [Note] {
    match (style, cue) {
        (NavSounds::Classic, Cue::Move) => &[(1318.5, 22)],
        (NavSounds::Classic, Cue::Confirm) => &[(987.8, 30), (1318.5, 45)],
        (NavSounds::Classic, Cue::Back) => &[(1318.5, 30), (987.8, 45)],
        (NavSounds::Classic, Cue::Tab) => &[(1174.7, 35)],
        (_, Cue::Move) => &[(880.0, 45)],
        (_, Cue::Confirm) => &[(659.3, 55), (987.8, 80)],
        (_, Cue::Back) => &[(987.8, 55), (659.3, 80)],
        (_, Cue::Tab) => &[(739.9, 70)],
    }
}

/// The tone for `cue` in `style` at `gain` (0…1), as 16-bit mono samples
/// at [`RATE`]. Each note rises in 3 ms and fades out, so no note clicks.
/// Soft tones are sines; classic ones add odd harmonics for a square-ish
/// blip. Empty for [`NavSounds::Off`].
pub fn samples(style: NavSounds, cue: Cue, gain: f32) -> Vec<i16> {
    if style == NavSounds::Off {
        return Vec::new();
    }
    // Navigation sounds sit well under other sound even at full volume.
    let peak = 0.35 * gain.clamp(0.0, 1.0) * f32::from(i16::MAX);
    let attack = (RATE as f32 * 0.003) as usize;
    let mut out = Vec::new();
    for &(freq, ms) in notes(style, cue) {
        let len = (RATE as usize * ms as usize) / 1000;
        for i in 0..len {
            let t = i as f32 / RATE as f32;
            let phase = std::f32::consts::TAU * freq * t;
            let wave = match style {
                NavSounds::Classic => {
                    phase.sin() + (3.0 * phase).sin() / 3.0 + (5.0 * phase).sin() / 5.0
                }
                _ => phase.sin(),
            } / if style == NavSounds::Classic {
                1.4
            } else {
                1.0
            };
            let rise = (i as f32 / attack as f32).min(1.0);
            let fall = (1.0 - i as f32 / len as f32).powi(2);
            out.push((wave * peak * rise * fall) as i16);
        }
    }
    out
}

/// `samples` as a WAV file (PCM, mono, 16-bit, [`RATE`]).
pub fn wav(samples: &[i16]) -> Vec<u8> {
    let data = (samples.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + data as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // mono
    out.extend_from_slice(&RATE.to_le_bytes());
    out.extend_from_slice(&(RATE * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data.to_le_bytes());
    for s in samples {
        out.extend_from_slice(&s.to_le_bytes());
    }
    out
}

/// Whether a cue at `now` plays, given the last one and whether a player
/// is still sounding: moves wait for the last tone and a short gap, so a
/// held direction doesn't stack tones; other cues always play.
pub fn should_play(cue: Cue, now: Instant, last: Option<Instant>, sounding: bool) -> bool {
    /// The least time between two moves' tones.
    const MOVE_GAP: Duration = Duration::from_millis(70);
    if cue != Cue::Move {
        return true;
    }
    !sounding && last.is_none_or(|at| now.saturating_duration_since(at) >= MOVE_GAP)
}

/// The cache file of a tone.
fn file(style: NavSounds, cue: Cue, volume: u32) -> PathBuf {
    let style = match style {
        NavSounds::Classic => "classic",
        _ => "soft",
    };
    Paths::get()
        .cache
        .join("bigpicture-sounds")
        .join(format!("{style}-{}-{volume}.wav", cue.id()))
}

/// Write the tone's file unless it is there.
fn ensure(path: &Path, style: NavSounds, cue: Cue, gain: f32) -> anyhow::Result<()> {
    if path.is_file() {
        return Ok(());
    }
    json_store::write_bytes(path, &wav(&samples(style, cue, gain)))
}

/// State shared by the cues of a session.
#[derive(Default)]
struct Player {
    last: Mutex<Option<Instant>>,
    /// A player process is still sounding.
    sounding: AtomicBool,
}

static PLAYER: Lazy<Player> = Lazy::new(Player::default);

/// The sound settings last seen, so a change plays a sample.
#[derive(Default)]
struct Previous(Option<(NavSounds, u32)>);

impl Global for Previous {}

/// Play a sample when the style or volume changes outside Big Picture, so
/// the choice can be heard. Call once.
pub(crate) fn install(cx: &mut App) {
    let Some(settings) = cx
        .try_global::<irori_ui::Models>()
        .map(|m| m.settings.clone())
    else {
        return;
    };
    let now = &settings.read(cx).app.big_picture;
    cx.set_global(Previous(Some((now.sounds, now.sound_volume))));
    cx.observe(&settings, |settings, cx| {
        let prefs = settings.read(cx).app.big_picture.clone();
        let key = (prefs.sounds, prefs.sound_volume);
        let changed = cx.global::<Previous>().0 != Some(key);
        if changed {
            cx.set_global(Previous(Some(key)));
            play_with(&prefs, Cue::Confirm, cx);
        }
    })
    .detach();
}

/// Play `cue` with the user's sound settings.
pub fn play(cue: Cue, cx: &App) {
    let Some(prefs) = cx
        .try_global::<irori_ui::Models>()
        .map(|m| m.settings.read(cx).app.big_picture.clone())
    else {
        return;
    };
    play_with(&prefs, cue, cx);
}

fn play_with(prefs: &BigPicturePrefs, cue: Cue, cx: &App) {
    if prefs.sounds == NavSounds::Off {
        return;
    }
    let now = Instant::now();
    {
        let mut last = PLAYER.last.lock();
        if !should_play(cue, now, *last, PLAYER.sounding.load(Ordering::Relaxed)) {
            return;
        }
        *last = Some(now);
    }
    let (style, volume, gain) = (
        prefs.sounds,
        prefs.sound_volume.clamp(1, 100),
        prefs.sound_gain(),
    );
    let backend = irori_ui::backend(cx);
    let blocking = backend.clone();
    PLAYER.sounding.store(true, Ordering::Relaxed);
    backend.spawn(async move {
        let path = file(style, cue, volume);
        let written = {
            let path = path.clone();
            blocking
                .spawn_blocking(move || ensure(&path, style, cue, gain))
                .await
        };
        match written {
            Ok(Ok(())) => sound(&path).await,
            Ok(Err(e)) => tracing::debug!("navigation sound: {e:#}"),
            Err(e) => tracing::debug!("navigation sound: {e}"),
        }
        PLAYER.sounding.store(false, Ordering::Relaxed);
    });
}

/// The program that plays a WAV file here, found once.
#[cfg(not(windows))]
fn player() -> Option<&'static Path> {
    static FOUND: once_cell::sync::OnceCell<Option<PathBuf>> = once_cell::sync::OnceCell::new();
    FOUND
        .get_or_init(|| {
            if cfg!(target_os = "macos") {
                return Some(PathBuf::from("/usr/bin/afplay")).filter(|p| p.is_file());
            }
            ["pw-play", "paplay", "aplay"]
                .into_iter()
                .find_map(|name| which::which(name).ok())
        })
        .as_deref()
}

#[cfg(not(windows))]
async fn sound(path: &Path) {
    use irori_core::process::{self, CommandSpec, RunOptions};
    let Some(program) = player() else {
        return;
    };
    let quiet = program
        .file_name()
        .is_some_and(|name| name == "aplay")
        .then_some("-q");
    let spec = CommandSpec::new(program).args(quiet).arg(path);
    if let Err(e) = process::run(&spec, RunOptions::default()).await {
        tracing::debug!("navigation sound: {e:#}");
    }
}

#[cfg(windows)]
async fn sound(path: &Path) {
    use std::os::windows::ffi::OsStrExt as _;
    use windows_sys::Win32::Media::Audio::{PlaySoundW, SND_ASYNC, SND_FILENAME, SND_NODEFAULT};
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
    // SAFETY: `wide` is a NUL-terminated path that outlives the call;
    // SND_ASYNC copies what it needs before returning.
    unsafe {
        PlaySoundW(
            wide.as_ptr(),
            std::ptr::null_mut(),
            SND_FILENAME | SND_ASYNC | SND_NODEFAULT,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tones_are_short_quiet_and_start_and_end_softly() {
        for style in [NavSounds::Soft, NavSounds::Classic] {
            for cue in [Cue::Move, Cue::Confirm, Cue::Back, Cue::Tab] {
                let s = samples(style, cue, 1.0);
                assert!(!s.is_empty());
                assert!(s.len() < RATE as usize / 5, "under 200 ms");
                let peak = s.iter().map(|v| v.unsigned_abs()).max().unwrap();
                assert!(peak < i16::MAX as u16 / 2, "{style:?} {cue:?}: {peak}");
                assert!(s[0].unsigned_abs() < 200 && s[s.len() - 1].unsigned_abs() < 200);
            }
        }
        assert!(samples(NavSounds::Off, Cue::Move, 1.0).is_empty());
        let loud = samples(NavSounds::Soft, Cue::Tab, 1.0);
        let quiet = samples(NavSounds::Soft, Cue::Tab, 0.25);
        let peak = |s: &[i16]| s.iter().map(|v| v.unsigned_abs()).max().unwrap();
        assert!(peak(&quiet) * 3 < peak(&loud));
    }

    #[test]
    fn wav_files_have_a_correct_header() {
        let file = wav(&[0, 1000, -1000]);
        assert_eq!(&file[0..4], b"RIFF");
        assert_eq!(&file[8..16], b"WAVEfmt ");
        assert_eq!(u32::from_le_bytes(file[4..8].try_into().unwrap()), 36 + 6);
        assert_eq!(u32::from_le_bytes(file[24..28].try_into().unwrap()), RATE);
        assert_eq!(&file[36..40], b"data");
        assert_eq!(file.len(), 44 + 6);
    }

    #[test]
    fn held_moves_do_not_stack_tones() {
        let t0 = Instant::now();
        let ms = |n| t0 + Duration::from_millis(n);
        assert!(should_play(Cue::Move, t0, None, false));
        assert!(!should_play(Cue::Move, ms(30), Some(t0), false), "too soon");
        assert!(
            !should_play(Cue::Move, ms(200), Some(t0), true),
            "still sounding"
        );
        assert!(should_play(Cue::Move, ms(200), Some(t0), false));
        assert!(
            should_play(Cue::Confirm, ms(1), Some(t0), true),
            "opening always plays"
        );
    }
}
