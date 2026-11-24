//! Screenshots Irori takes for the player: Guide + RB on a controller and
//! the in-game overlay's Screenshot.
//!
//! - **Linux** (`linux.rs`): the desktop portal's Screenshot interface
//!   without its dialog (GNOME, KDE Plasma and the wlroots portal answer
//!   it; Plasma asks once whether Irori may), else `grim` on wlroots
//!   desktops without a portal. Inside gamescope neither works: Steam's own
//!   screenshot is the one to use there.
//! - **Windows** (`windows.rs`): the monitor the foreground window (the
//!   game) is on, through DXGI Desktop Duplication (HDR pictures tone mapped
//!   to SDR, [`tone_map_scrgb`]), else copied from the desktop with GDI.
//!   Both read what the desktop composes; a game in exclusive fullscreen may
//!   still come out black with GDI.
//! - **macOS** (`macos.rs`): `screencapture` of the main display, once
//!   Irori has the Screen Recording permission (asked for the first time a
//!   screenshot is taken; without it the picture would hold only the
//!   desktop).
//!
//! The picture is saved in the desktop's screenshot folder as
//! `<game> <date> <time>.png`, where the screenshot index finds it for the
//! session it was taken in.

use std::path::{Path, PathBuf};

use anyhow::Result;
use irori_core::{GameId, Notice, NoticeLevel};

use crate::Backend;

#[cfg(target_os = "linux")]
#[path = "linux.rs"]
mod platform;
#[cfg(target_os = "macos")]
#[path = "macos.rs"]
mod platform;
#[cfg(windows)]
#[path = "windows.rs"]
mod platform;
#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
mod platform {
    use super::*;

    pub(super) async fn capture(_target: &Path) -> Result<PathBuf> {
        anyhow::bail!("Screenshots aren't available on this system")
    }
}

/// Turn GDI's rows of blue, green, red and unused bytes into RGBA, opaque.
// Used on Windows; tested everywhere.
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn bgrx_to_rgba(pixels: &mut [u8]) {
    for px in pixels.as_chunks_mut::<4>().0 {
        px.swap(0, 2);
        px[3] = 255;
    }
}

/// An IEEE 754 half-precision float's bits as an `f32`.
// Used on Windows; tested everywhere.
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn half_to_f32(bits: u16) -> f32 {
    let sign = if bits & 0x8000 != 0 { -1.0 } else { 1.0 };
    let exponent = i32::from((bits >> 10) & 0x1f);
    let mantissa = f32::from(bits & 0x3ff);
    sign * match exponent {
        0 => mantissa * 2f32.powi(-24),
        31 if mantissa == 0.0 => f32::INFINITY,
        31 => f32::NAN,
        _ => (1.0 + mantissa / 1024.0) * 2f32.powi(exponent - 15),
    }
}

/// Where highlights start to roll off, as a share of SDR white: below it a
/// picture stays as the display showed SDR content.
const KNEE: f32 = 0.8;

/// One linear channel (SDR white at 1.0) of a picture whose brightest
/// channel is `peak`, brought into `0..=1`. A picture no brighter than SDR
/// white stays as it is. Otherwise everything up to [`KNEE`] stays and the
/// rest rolls off so that `peak` lands on white, with the same slope at the
/// knee, so highlights keep their shape instead of clipping flat.
fn roll_off(value: f32, peak: f32) -> f32 {
    if value.is_nan() {
        return 0.0;
    }
    if peak <= 1.0 || value <= KNEE {
        return value.clamp(0.0, 1.0);
    }
    let room = 1.0 - KNEE;
    // `x * (1 + a) / (1 + a * x)` runs from 0 to 1 over `0..=1` and starts
    // at slope `1 + a`, the slope the straight part above the knee has.
    let a = (peak - KNEE) / room - 1.0;
    let x = ((value - KNEE) / (peak - KNEE)).min(1.0);
    KNEE + room * x * (1.0 + a) / (1.0 + a * x)
}

/// A linear channel in `0..=1` encoded with sRGB's curve, as a byte.
fn srgb_byte(linear: f32) -> u8 {
    let c = linear.clamp(0.0, 1.0);
    let encoded = if c <= 0.003_130_8 {
        12.92 * c
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    };
    (encoded * 255.0).round() as u8
}

/// An HDR desktop picture as Desktop Duplication hands it over (scRGB:
/// linear half floats of red, green, blue and alpha, 1.0 = 80 nits) as
/// opaque sRGB bytes. `white` is the display's SDR white in scRGB units
/// (Windows' SDR content brightness: 2.5 for 200 nits), so the desktop and
/// SDR games come out as they look, and HDR highlights roll off toward
/// white instead of clipping.
// Used on Windows; tested everywhere.
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn tone_map_scrgb(rows: &[u8], white: f32) -> Vec<u8> {
    let white = if white.is_finite() && white > 0.0 {
        white
    } else {
        1.0
    };
    let pixels = rows.as_chunks::<8>().0;
    let channel = |px: &[u8; 8], c: usize| {
        half_to_f32(u16::from_le_bytes([px[c * 2], px[c * 2 + 1]])) / white
    };
    let peak = pixels
        .iter()
        .flat_map(|px| (0..3).map(move |c| channel(px, c)))
        .filter(|v| v.is_finite())
        .fold(0.0f32, f32::max);
    let mut out = Vec::with_capacity(pixels.len() * 4);
    for px in pixels {
        for c in 0..3 {
            out.push(srgb_byte(roll_off(channel(px, c), peak)));
        }
        out.push(255);
    }
    out
}

/// `file://` URI as the portal returns it, as a path: percent escapes
/// decoded, `None` for anything that isn't a local file.
pub fn file_uri_path(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    // `file://localhost/…` names this computer too.
    let rest = rest.strip_prefix("localhost").unwrap_or(rest);
    if !rest.starts_with('/') {
        return None;
    }
    let bytes = rest.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && let Some(hex) = rest.get(i + 1..i + 3)
            && let Ok(byte) = u8::from_str_radix(hex, 16)
        {
            out.push(byte);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt as _;
        Some(PathBuf::from(std::ffi::OsString::from_vec(out)))
    }
    #[cfg(not(unix))]
    {
        Some(PathBuf::from(String::from_utf8(out).ok()?))
    }
}

/// The name Irori gives a screenshot of `title` taken at `at`.
pub fn screenshot_name(title: &str, at: chrono::NaiveDateTime) -> String {
    crate::replay::clip_name(title, at, "png")
}

/// A moment marked in game: the journal note waiting for words and the
/// screenshot it is about.
#[derive(Debug, Clone)]
pub struct MarkedMoment {
    pub note: String,
    pub title: String,
    /// The screenshot, or why none was taken (the note is kept anyway).
    pub capture: Result<PathBuf, String>,
}

/// The note a moment marked at `at` starts as: at its time of day, about
/// `capture`, waiting for words.
pub fn moment_note(at: chrono::NaiveDateTime, capture: Option<&Path>) -> crate::JournalEntry {
    use chrono::Timelike as _;
    crate::JournalEntry {
        time: chrono::NaiveTime::from_hms_opt(at.hour(), at.minute(), 0),
        capture: capture.map(|p| p.display().to_string()),
        marked: true,
        ..Default::default()
    }
}

impl Backend {
    /// The title screenshots of `id` are named after.
    fn capture_title(&self, id: Option<&GameId>) -> String {
        id.and_then(|id| self.game(id))
            .map(|g| g.title.clone())
            .unwrap_or_else(|| "Irori".to_string())
    }

    /// Take a screenshot into the screenshots folder, named after `title`
    /// and `at`.
    async fn capture_now(title: &str, at: chrono::NaiveDateTime) -> anyhow::Result<PathBuf> {
        let dir = crate::screenshots::save_folder();
        let target = crate::replay::free_path(&dir, &screenshot_name(title, at));
        platform::capture(&target).await
    }

    /// Mark this moment of the running game `id`: a screenshot and a
    /// journal note at this time about it, waiting for words. A screenshot
    /// that can't be taken leaves the note without one.
    pub async fn mark_moment(&self, id: &GameId) -> MarkedMoment {
        let title = self.capture_title(Some(id));
        let at = chrono::Local::now().naive_local();
        let capture = Self::capture_now(&title, at).await.map_err(|e| {
            tracing::warn!("taking the screenshot of a marked moment: {e:#}");
            format!("{e:#}")
        });
        let entry = moment_note(at, capture.as_ref().ok().map(PathBuf::as_path));
        let note = self.userdata.set_journal_entry(id, at.date(), entry);
        MarkedMoment {
            note,
            title,
            capture,
        }
    }

    /// Take a screenshot of what is on screen for the running game `id`
    /// (or the screen, without one) and announce where it went.
    pub fn take_screenshot(&self, id: Option<GameId>) {
        let b = self.clone();
        self.spawn(async move {
            let title = b.capture_title(id.as_ref());
            let at = chrono::Local::now().naive_local();
            match Self::capture_now(&title, at).await {
                Ok(path) => {
                    tracing::info!("screenshot saved to {}", path.display());
                    let mut notice =
                        Notice::new(NoticeLevel::Success, format!("Screenshot of {title} saved"))
                            .message(path.display().to_string())
                            .keys("shell.notice.screenshot_saved", None, &[("title", &title)]);
                    if let Some(id) = id {
                        notice = notice.game(id);
                    }
                    b.bus.notice(notice);
                }
                Err(e) => {
                    tracing::warn!("taking a screenshot: {e:#}");
                    b.bus.notice(
                        Notice::new(NoticeLevel::Error, "The screenshot wasn't taken")
                            .message(format!("{e:#}"))
                            .keys("shell.notice.screenshot_failed", None, &[]),
                    );
                }
            }
        });
    }
}

/// Move what the portal wrote to `target`; it stays where it is when it
/// can't be moved (another disk and no copy either). Only the Linux portal
/// writes elsewhere, so on other systems only the tests call it.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn settle(from: &Path, target: &Path) -> PathBuf {
    if from == target {
        return target.to_path_buf();
    }
    if let Some(dir) = target.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if std::fs::rename(from, target).is_ok() {
        return target.to_path_buf();
    }
    match std::fs::copy(from, target) {
        Ok(_) => {
            let _ = std::fs::remove_file(from);
            target.to_path_buf()
        }
        Err(e) => {
            tracing::debug!("keeping the screenshot at {}: {e}", from.display());
            from.to_path_buf()
        }
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn a_marked_moment_is_a_timed_note_about_its_capture() {
        let at = chrono::NaiveDate::from_ymd_opt(2026, 11, 2)
            .unwrap()
            .and_hms_opt(21, 40, 31)
            .unwrap();
        let note = moment_note(at, Some(Path::new("/p/Hades.png")));
        assert_eq!(note.time, chrono::NaiveTime::from_hms_opt(21, 40, 0));
        assert_eq!(note.capture.as_deref(), Some("/p/Hades.png"));
        assert!(note.is_waiting() && !note.is_empty());
        // Without a screenshot the moment is still kept.
        assert!(!moment_note(at, None).is_empty());
    }

    use super::*;

    #[test]
    fn reads_the_portals_file_uris() {
        assert_eq!(
            file_uri_path("file:///home/me/Pictures/Screenshot%20from%202026.png"),
            Some(PathBuf::from("/home/me/Pictures/Screenshot from 2026.png"))
        );
        assert_eq!(
            file_uri_path("file://localhost/tmp/a.png"),
            Some(PathBuf::from("/tmp/a.png"))
        );
        assert_eq!(
            file_uri_path("file:///tmp/%E2%9C%93%zz.png"),
            Some(PathBuf::from("/tmp/✓%zz.png"))
        );
        assert_eq!(file_uri_path("https://example.com/a.png"), None);
        assert_eq!(file_uri_path("file://server/share/a.png"), None);
    }

    #[test]
    fn names_screenshots_after_the_game() {
        let at = chrono::NaiveDate::from_ymd_opt(2026, 3, 4)
            .unwrap()
            .and_hms_opt(5, 6, 7)
            .unwrap();
        assert_eq!(
            screenshot_name("Hades: II", at),
            format!(
                "{} 2026-03-04 05-06-07.png",
                irori_core::paths::sanitize("Hades: II")
            )
        );
    }

    #[test]
    fn reads_half_floats() {
        assert_eq!(half_to_f32(0x3c00), 1.0);
        assert_eq!(half_to_f32(0xc000), -2.0);
        assert_eq!(half_to_f32(0x3800), 0.5);
        assert_eq!(half_to_f32(0x0001), 2f32.powi(-24));
        assert_eq!(half_to_f32(0x7c00), f32::INFINITY);
        assert!(half_to_f32(0x7e00).is_nan());
    }

    fn scrgb(pixels: &[[f32; 3]]) -> Vec<u8> {
        // Encode with the halves the tests need: exact powers of two.
        let half = |v: f32| -> [u8; 2] {
            let bits: u16 = match v {
                0.0 => 0,
                0.5 => 0x3800,
                1.0 => 0x3c00,
                2.0 => 0x4000,
                2.5 => 0x4100,
                8.0 => 0x4800,
                _ => panic!("no half for {v}"),
            };
            bits.to_le_bytes()
        };
        pixels
            .iter()
            .flat_map(|[r, g, b]| [half(*r), half(*g), half(*b), half(1.0)].concat())
            .collect()
    }

    #[test]
    fn sdr_pictures_on_an_hdr_display_keep_their_look() {
        // SDR white at 200 nits (2.5 in scRGB) comes out white, its half
        // as half (in sRGB), black as black.
        let rows = scrgb(&[[2.5, 2.5, 2.5], [0.0, 0.0, 0.0], [1.0, 1.0, 1.0]]);
        let out = tone_map_scrgb(&rows, 2.5);
        assert_eq!(&out[0..4], &[255, 255, 255, 255]);
        assert_eq!(&out[4..8], &[0, 0, 0, 255]);
        // 0.4 of white, linear, is 170 in sRGB.
        assert_eq!(out[8], 170);
    }

    #[test]
    fn hdr_highlights_roll_off_instead_of_clipping() {
        let rows = scrgb(&[[8.0, 2.0, 0.5], [2.0, 2.0, 2.0], [0.5, 0.5, 0.5]]);
        let out = tone_map_scrgb(&rows, 1.0);
        // The brightest channel lands on white; brighter ones stay apart.
        assert_eq!(out[0], 255);
        assert!(out[1] < 255 && out[1] > out[2]);
        assert_eq!(out[4], out[1]);
        // Below the knee nothing changes: 0.5 linear is 188 in sRGB.
        assert_eq!(out[8], 188);
        // A white level that makes no sense is read as 80 nits.
        assert_eq!(tone_map_scrgb(&rows, f32::NAN), out);
        assert_eq!(tone_map_scrgb(&rows, 0.0), out);
    }

    #[test]
    fn turns_gdi_pixels_into_opaque_rgba() {
        let mut pixels = [1, 2, 3, 0, 10, 20, 30, 7];
        bgrx_to_rgba(&mut pixels);
        assert_eq!(pixels, [3, 2, 1, 255, 30, 20, 10, 255]);
    }

    #[test]
    fn moves_the_picture_to_its_name() {
        let dir = tempfile::tempdir().unwrap();
        let from = dir.path().join("Screenshot.png");
        std::fs::write(&from, b"png").unwrap();
        let target = dir.path().join("shots").join("Game.png");
        assert_eq!(settle(&from, &target), target);
        assert!(!from.exists());
        assert_eq!(std::fs::read(&target).unwrap(), b"png");
    }
}
