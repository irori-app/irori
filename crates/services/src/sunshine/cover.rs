//! A game's art as the cover Sunshine shows in Moonlight: a 600 × 800 PNG,
//! cropped to fill, as Sunshine's documentation asks.

use anyhow::{Context as _, Result, bail};
use image::imageops::FilterType;

const WIDTH: u32 = 600;
const HEIGHT: u32 = 800;
/// Art larger than this is refused rather than decoded.
const MAX_BYTES: usize = 32 * 1024 * 1024;

/// The bytes of the art at `url`: a web address or a `file://` path.
pub(crate) async fn fetch(url: &str) -> Result<Vec<u8>> {
    if let Some(path) = url.strip_prefix("file://") {
        let bytes = tokio::fs::read(path)
            .await
            .with_context(|| format!("Could not read the art at {path}"))?;
        return Ok(bytes);
    }
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        bail!("Not an address art can be fetched from");
    }
    let response = irori_core::http::client()
        .get(url)
        .send()
        .await
        .and_then(|r| r.error_for_status())
        .map_err(|e| anyhow::anyhow!("Could not fetch the art: {}", e.without_url()))?;
    if response
        .content_length()
        .is_some_and(|len| len as usize > MAX_BYTES)
    {
        bail!("The art is too large");
    }
    let bytes = response
        .bytes()
        .await
        .map_err(|e| anyhow::anyhow!("Could not fetch the art: {}", e.without_url()))?;
    Ok(bytes.to_vec())
}

/// `bytes` (PNG, JPEG or WebP) as Sunshine's cover. Blocking: run it off the
/// async workers.
pub(crate) fn to_cover(bytes: &[u8]) -> Result<Vec<u8>> {
    if bytes.len() > MAX_BYTES {
        bail!("The art is too large");
    }
    let art = image::load_from_memory(bytes).context("The art isn't a picture Irori can read")?;
    let cover = art.resize_to_fill(WIDTH, HEIGHT, FilterType::Lanczos3);
    let mut png = Vec::new();
    cover
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .context("Could not make the cover")?;
    Ok(png)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn covers_are_600_by_800_pngs_whatever_the_art() {
        let wide = image::RgbImage::from_pixel(1920, 1080, image::Rgb([200, 40, 40]));
        let mut jpeg = Vec::new();
        image::DynamicImage::ImageRgb8(wide)
            .write_to(
                &mut std::io::Cursor::new(&mut jpeg),
                image::ImageFormat::Jpeg,
            )
            .unwrap();
        let png = to_cover(&jpeg).unwrap();
        assert_eq!(&png[1..4], b"PNG");
        let back = image::load_from_memory(&png).unwrap();
        assert_eq!((back.width(), back.height()), (WIDTH, HEIGHT));
        assert!(to_cover(b"not a picture").is_err());
    }
}
