//! A game's program icon as library art: the program a game starts, when it
//! is a Windows program (`.exe`), and its own icon ([`crate::shortcuts::pe_icon`])
//! as a PNG the art store can thumbnail. Used for games without cover art,
//! so a niche game the stores have no box for still has a face of its own.
//!
//! Native Linux programs carry no icon (ELF has none) and macOS bundles keep
//! theirs as `.icns`, which isn't read here, so only Windows programs count.

use std::io::Cursor;
use std::path::{Path, PathBuf};

use image::{DynamicImage, ImageFormat, ImageReader, Limits, imageops};
use irori_core::GameInfo;

/// Icons drawn larger than this side are read no further: no program icon
/// is bigger than 256 pixels, so anything larger is malformed.
const MAX_SIDE: u32 = 1024;
/// Small icons are enlarged by whole multiples to at least this side, so
/// a 32 pixel icon stays crisp instead of being blurred by the scaler.
const MIN_SIDE: u32 = 128;

/// The Windows program an installed game starts, when it is one: the
/// install's executable (relative to its folder when not absolute).
pub fn windows_program(game: &GameInfo) -> Option<PathBuf> {
    let install = game.install.as_ref()?;
    if install.executable.is_empty() {
        return None;
    }
    let exe = Path::new(&install.executable);
    if !exe
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("exe"))
    {
        return None;
    }
    Some(if exe.is_absolute() {
        exe.to_path_buf()
    } else if install.install_path.is_empty() {
        return None;
    } else {
        Path::new(&install.install_path).join(exe)
    })
}

/// The icon of the Windows program at `path` as PNG bytes, enlarged to at
/// least [`MIN_SIDE`]; `None` when it has none or it can't be read. Reads
/// only the program's headers, resource tree and one picture, each bounded.
pub fn png_of(path: &Path) -> Option<Vec<u8>> {
    let ico = crate::shortcuts::pe_icon::icon_of(path)?;
    png_of_ico(&ico)
}

/// An `.ico` file's picture (PNG or bitmap) as PNG bytes.
fn png_of_ico(ico: &[u8]) -> Option<Vec<u8>> {
    let mut reader = ImageReader::with_format(Cursor::new(ico), ImageFormat::Ico);
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_SIDE);
    limits.max_image_height = Some(MAX_SIDE);
    limits.max_alloc = Some(64 * 1024 * 1024);
    reader.limits(limits);
    let img = reader.decode().ok()?;
    let side = img.width().max(img.height());
    if side == 0 {
        return None;
    }
    let factor = MIN_SIDE.div_ceil(side).max(1);
    let img = if factor > 1 {
        DynamicImage::ImageRgba8(imageops::resize(
            &img.to_rgba8(),
            img.width() * factor,
            img.height() * factor,
            imageops::FilterType::Nearest,
        ))
    } else {
        img
    };
    let mut out = Cursor::new(Vec::new());
    img.write_to(&mut out, ImageFormat::Png).ok()?;
    Some(out.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{GenericImageView as _, RgbaImage};

    fn png(side: u32) -> Vec<u8> {
        let mut out = Cursor::new(Vec::new());
        DynamicImage::ImageRgba8(RgbaImage::from_pixel(
            side,
            side,
            image::Rgba([200, 10, 10, 255]),
        ))
        .write_to(&mut out, ImageFormat::Png)
        .unwrap();
        out.into_inner()
    }

    #[test]
    fn a_programs_icon_becomes_a_png_enlarged_crisply() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("Game.EXE");
        std::fs::write(
            &exe,
            crate::shortcuts::pe_icon::tests::program_with_icon(&png(48)),
        )
        .unwrap();
        let out = png_of(&exe).unwrap();
        let img = image::load_from_memory_with_format(&out, ImageFormat::Png).unwrap();
        // 48 px, the larger picture, three times.
        assert_eq!(img.dimensions(), (144, 144));
        assert_eq!(img.to_rgba8().get_pixel(70, 70).0, [200, 10, 10, 255]);
    }

    #[test]
    fn large_icons_stay_as_they_are() {
        let ico = {
            let data = png(256);
            let mut ico = vec![0, 0, 1, 0, 1, 0, 0, 0, 0, 0, 1, 0, 32, 0];
            ico.extend_from_slice(&(data.len() as u32).to_le_bytes());
            ico.extend_from_slice(&22u32.to_le_bytes());
            ico.extend(data);
            ico
        };
        let out = png_of_ico(&ico).unwrap();
        let img = image::load_from_memory(&out).unwrap();
        assert_eq!(img.dimensions(), (256, 256));
    }

    #[test]
    fn bitmap_icons_become_pngs_too() {
        // A 2 × 2, 32-bit DIB (height doubled for the AND mask), bottom-up
        // BGRA rows, then the mask's two rows padded to four bytes.
        let mut dib = Vec::new();
        for v in [40u32, 2, 4] {
            dib.extend_from_slice(&v.to_le_bytes());
        }
        dib.extend_from_slice(&1u16.to_le_bytes());
        dib.extend_from_slice(&32u16.to_le_bytes());
        dib.extend_from_slice(&[0; 24]);
        for _ in 0..4 {
            dib.extend_from_slice(&[10, 20, 30, 255]);
        }
        dib.extend_from_slice(&[0; 8]);
        let mut ico = vec![0, 0, 1, 0, 1, 0, 2, 2, 0, 0, 1, 0, 32, 0];
        ico.extend_from_slice(&(dib.len() as u32).to_le_bytes());
        ico.extend_from_slice(&22u32.to_le_bytes());
        ico.extend(dib);
        let out = png_of_ico(&ico).unwrap();
        let img = image::load_from_memory_with_format(&out, ImageFormat::Png).unwrap();
        assert_eq!(img.dimensions(), (128, 128));
        assert_eq!(img.to_rgba8().get_pixel(5, 120).0, [30, 20, 10, 255]);
    }

    #[test]
    fn programs_without_an_icon_give_none() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("game.exe");
        std::fs::write(&exe, b"MZ not really").unwrap();
        assert!(png_of(&exe).is_none());
        assert!(png_of(&dir.path().join("missing.exe")).is_none());
        assert!(png_of_ico(b"\0\0\x01\0\x01\0garbage").is_none());
    }

    #[test]
    fn only_windows_programs_count() {
        let game = |path: &str, exe: &str| GameInfo {
            install: Some(irori_core::InstalledInfo {
                install_path: path.into(),
                executable: exe.into(),
                ..Default::default()
            }),
            ..Default::default()
        };
        assert_eq!(
            windows_program(&game("/games/x", "bin/Game.Exe")),
            Some(Path::new("/games/x").join("bin/Game.Exe"))
        );
        let dir = tempfile::tempdir().unwrap();
        let elsewhere = dir.path().join("run.exe");
        assert_eq!(
            windows_program(&game("/games/x", &elsewhere.display().to_string())),
            Some(elsewhere)
        );
        assert_eq!(windows_program(&game("/games/x", "start.sh")), None);
        assert_eq!(windows_program(&game("", "game.exe")), None);
        assert_eq!(windows_program(&game("/games/x", "")), None);
        assert_eq!(windows_program(&GameInfo::default()), None);
    }
}
