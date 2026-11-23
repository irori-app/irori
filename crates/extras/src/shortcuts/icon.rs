//! Game icons for shortcuts: picks the best source art, converts it to a
//! square PNG and stores it under Irori's data directory. With
//! `iroriShortcutIconFromExe`, a Windows program's own icon
//! ([`super::pe_icon`]) comes first, stored apart from the art's, so turning
//! the setting off or on takes effect at the next shortcut.

use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context as _, Result, anyhow};
use image::{DynamicImage, GenericImageView as _, ImageFormat, RgbaImage, imageops};
use irori_core::{GameInfo, Paths, Runner};
use serde::Deserialize;

use crate::util;

/// Edge length of generated icons, in pixels.
const ICON_SIZE: u32 = 256;

/// `<data>/icons`, owned by Irori.
pub fn icons_dir() -> PathBuf {
    Paths::get().data.join("icons")
}

/// Where the icon of a game lives.
pub fn icon_path(game: &GameInfo) -> PathBuf {
    icons_dir().join(format!(
        "{}-{}.png",
        game.runner().id(),
        irori_core::paths::sanitize(&game.app_name)
    ))
}

/// Where the icon taken from a game's program lives.
fn exe_icon_path(game: &GameInfo) -> PathBuf {
    icons_dir().join(format!(
        "{}-{}-exe.png",
        game.runner().id(),
        irori_core::paths::sanitize(&game.app_name)
    ))
}

/// The Windows program of an installed game, when it is one.
pub fn windows_program(game: &GameInfo) -> Option<PathBuf> {
    let install = game.install.as_ref()?;
    let exe = Path::new(&install.executable);
    if !exe
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("exe"))
    {
        return None;
    }
    Some(if exe.is_absolute() {
        exe.to_path_buf()
    } else {
        Path::new(&install.install_path).join(exe)
    })
}

/// The icon of the game's Windows program as a square PNG, stored once.
async fn ensure_exe_icon(game: &GameInfo) -> Option<PathBuf> {
    let target = exe_icon_path(game);
    if tokio::fs::try_exists(&target).await.unwrap_or(false) {
        return Some(target);
    }
    let exe = windows_program(game)?;
    let png = tokio::task::spawn_blocking(move || {
        let ico = super::pe_icon::icon_of(&exe)?;
        square_png(&ico, ICON_SIZE).ok()
    })
    .await
    .ok()??;
    util::write_atomic(&target, &png).await.ok()?;
    Some(target)
}

/// Make sure the game's icon exists and return its path. Sources, in order:
/// the Windows program's own icon (with `from_exe`), GOG's bundled icons in
/// the install folder, the store's icon art, GOG's product API icon, then
/// the square and cover art. Returns `Ok(None)` when no source produced an
/// image.
pub async fn ensure_icon(game: &GameInfo, from_exe: bool) -> Result<Option<PathBuf>> {
    if from_exe && let Some(icon) = ensure_exe_icon(game).await {
        return Ok(Some(icon));
    }
    let target = icon_path(game);
    if tokio::fs::try_exists(&target).await.unwrap_or(false) {
        return Ok(Some(target));
    }
    for source in icon_sources(game).await {
        let bytes = match util::load_image_source(&source).await {
            Ok(b) => b,
            Err(e) => {
                tracing::debug!("icon source for {} skipped: {e:#}", game.app_name);
                continue;
            }
        };
        let png = match tokio::task::spawn_blocking(move || square_png(&bytes, ICON_SIZE)).await {
            Ok(Ok(png)) => png,
            Ok(Err(e)) => {
                tracing::debug!("icon source for {} unusable: {e:#}", game.app_name);
                continue;
            }
            Err(e) => return Err(anyhow!("icon conversion task failed: {e}")),
        };
        util::write_atomic(&target, &png)
            .await
            .context("saving the game icon")?;
        return Ok(Some(target));
    }
    Ok(None)
}

/// Delete the stored icons of a game (with their `.ico` and `.icns`
/// copies), if any.
pub async fn remove_icon(game: &GameInfo) {
    for png in [icon_path(game), exe_icon_path(game)] {
        for path in [png.with_extension("ico"), png.with_extension("icns"), png] {
            let _ = tokio::fs::remove_file(path).await;
        }
    }
}

async fn icon_sources(game: &GameInfo) -> Vec<String> {
    let mut out = Vec::new();
    let is_gog = game.runner() == Runner::Gog;
    if is_gog && let Some(dir) = game.install_path() {
        for candidate in [
            dir.join("support/icon.png"),
            dir.join(format!("goggame-{}.ico", game.app_name)),
        ] {
            if tokio::fs::try_exists(&candidate).await.unwrap_or(false) {
                out.push(candidate.to_string_lossy().into_owned());
            }
        }
    }
    out.extend(game.art_icon.iter().filter(|s| !s.is_empty()).cloned());
    if is_gog && let Some(url) = gog_api_icon(&game.app_name).await {
        out.push(url);
    }
    out.extend(
        [&game.art_square, &game.art_cover, &game.art_logo]
            .into_iter()
            .flatten()
            .filter(|s| !s.is_empty())
            .cloned(),
    );
    out
}

#[derive(Deserialize)]
struct GogProduct {
    #[serde(default)]
    images: GogImages,
}

#[derive(Deserialize, Default)]
struct GogImages {
    icon: Option<String>,
}

async fn gog_api_icon(app_name: &str) -> Option<String> {
    let url = format!(
        "https://api.gog.com/products/{}?locale=en-US",
        util::encode_component(app_name)
    );
    let product: GogProduct = util::get_json(&url, Duration::from_secs(10)).await.ok()?;
    let icon = product.images.icon.filter(|s| !s.is_empty())?;
    Some(if icon.starts_with("//") {
        format!("https:{icon}")
    } else {
        icon
    })
}

/// Decode any supported image and fit it, centred, into a transparent
/// `size`×`size` PNG.
pub(crate) fn square_png(bytes: &[u8], size: u32) -> Result<Vec<u8>> {
    let img = image::load_from_memory(bytes).context("decoding image")?;
    let (w, h) = img.dimensions();
    if w == 0 || h == 0 {
        return Err(anyhow!("empty image"));
    }
    let scaled = img.resize(size, size, imageops::FilterType::Lanczos3);
    let mut canvas = RgbaImage::new(size, size);
    let x = (size - scaled.width()) / 2;
    let y = (size - scaled.height()) / 2;
    imageops::overlay(&mut canvas, &scaled.to_rgba8(), x.into(), y.into());
    encode_png(&DynamicImage::ImageRgba8(canvas))
}

/// An Apple icon file (`.icns`) of a square PNG icon, with the 128 and 256
/// pixel sizes Finder and the Dock use, each stored as PNG.
pub fn icns(png: &[u8]) -> Result<Vec<u8>> {
    let img = image::load_from_memory(png).context("decoding the icon")?;
    let mut body = Vec::new();
    for (kind, side) in [(b"ic07", 128), (b"ic08", 256)] {
        let data = encode_png(&img.resize_exact(side, side, imageops::FilterType::Lanczos3))?;
        body.extend_from_slice(kind);
        body.extend_from_slice(&(data.len() as u32 + 8).to_be_bytes());
        body.extend_from_slice(&data);
    }
    let mut out = Vec::with_capacity(body.len() + 8);
    out.extend_from_slice(b"icns");
    out.extend_from_slice(&(body.len() as u32 + 8).to_be_bytes());
    out.extend(body);
    Ok(out)
}

/// A Windows icon file (`.ico`) holding one PNG picture of up to 256
/// pixels, which Windows reads since Vista.
pub fn ico(png: &[u8]) -> Result<Vec<u8>> {
    let img = image::load_from_memory(png).context("decoding the icon")?;
    let img = if img.width() > 256 || img.height() > 256 {
        img.resize(256, 256, imageops::FilterType::Lanczos3)
    } else {
        img
    };
    let data = encode_png(&img)?;
    // 256 is written as 0.
    let side = |n: u32| if n >= 256 { 0 } else { n as u8 };
    let mut out = vec![
        0,
        0,
        1,
        0,
        1,
        0,
        side(img.width()),
        side(img.height()),
        0,
        0,
    ];
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&32u16.to_le_bytes());
    out.extend_from_slice(&(data.len() as u32).to_le_bytes());
    out.extend_from_slice(&22u32.to_le_bytes());
    out.extend(data);
    Ok(out)
}

pub(crate) fn encode_png(img: &DynamicImage) -> Result<Vec<u8>> {
    let mut out = Cursor::new(Vec::new());
    img.write_to(&mut out, ImageFormat::Png)
        .context("encoding PNG")?;
    Ok(out.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_programs_icon_becomes_a_square_png() {
        let mut png = Cursor::new(Vec::new());
        DynamicImage::ImageRgba8(RgbaImage::from_pixel(48, 48, image::Rgba([0, 0, 255, 255])))
            .write_to(&mut png, ImageFormat::Png)
            .unwrap();
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("Game.EXE");
        std::fs::write(
            &exe,
            super::super::pe_icon::tests::program_with_icon(png.get_ref()),
        )
        .unwrap();
        let ico = super::super::pe_icon::icon_of(&exe).unwrap();
        let out = square_png(&ico, 64).unwrap();
        let img = image::load_from_memory(&out).unwrap();
        assert_eq!(img.dimensions(), (64, 64));
        assert_eq!(img.to_rgba8().get_pixel(32, 32)[2], 255);

        let game = GameInfo {
            install: Some(irori_core::InstalledInfo {
                install_path: dir.path().display().to_string(),
                executable: "Game.EXE".into(),
                ..Default::default()
            }),
            ..Default::default()
        };
        assert_eq!(windows_program(&game), Some(exe));
        let native = GameInfo {
            install: Some(irori_core::InstalledInfo {
                executable: "game.sh".into(),
                ..Default::default()
            }),
            ..Default::default()
        };
        assert_eq!(windows_program(&native), None);
    }

    #[test]
    fn icon_files_for_macos_and_windows() {
        let src =
            DynamicImage::ImageRgba8(RgbaImage::from_pixel(256, 256, image::Rgba([9, 9, 9, 255])));
        let png = encode_png(&src).unwrap();
        let mac = icns(&png).unwrap();
        assert_eq!(&mac[..4], b"icns");
        assert_eq!(
            u32::from_be_bytes(mac[4..8].try_into().unwrap()) as usize,
            mac.len()
        );
        assert_eq!(&mac[8..12], b"ic07");
        let first = u32::from_be_bytes(mac[12..16].try_into().unwrap()) as usize;
        assert_eq!(&mac[8 + first..12 + first], b"ic08");
        assert_eq!(
            image::load_from_memory(&mac[16..8 + first])
                .unwrap()
                .dimensions(),
            (128, 128)
        );

        let win = ico(&png).unwrap();
        let back = image::load_from_memory_with_format(&win, ImageFormat::Ico).unwrap();
        assert_eq!(back.dimensions(), (256, 256));
    }

    #[test]
    fn converts_to_square_png() {
        let src =
            DynamicImage::ImageRgba8(RgbaImage::from_pixel(40, 80, image::Rgba([255, 0, 0, 255])));
        let mut jpg = Cursor::new(Vec::new());
        src.to_rgb8().write_to(&mut jpg, ImageFormat::Jpeg).unwrap();
        let png = square_png(jpg.get_ref(), 64).unwrap();
        let out = image::load_from_memory_with_format(&png, ImageFormat::Png).unwrap();
        assert_eq!(out.dimensions(), (64, 64));
        assert_eq!(out.to_rgba8().get_pixel(0, 0)[3], 0);
        assert_eq!(out.to_rgba8().get_pixel(32, 32)[3], 255);
    }
}
