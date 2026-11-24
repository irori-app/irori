//! Irori's mark and wordmark, as the dock, taskbar, sidebar and tray show them.

use std::sync::{Arc, Mutex};

use gpui_kit::{
    App, Hsla, Image, ImageFormat, IntoElement, Rems, RenderOnce, Rgba, Styled as _, Window, img,
};
use once_cell::sync::Lazy;

const WORDMARK_ASPECT: f32 = 488.0 / 221.0;

fn mark_svg() -> String {
    include_str!("../../../../assets/brand/irori-mark.svg").replacen(
        "<svg ",
        r#"<svg width="192" height="192" "#,
        1,
    )
}

fn wordmark_svg(hex: &str) -> String {
    include_str!("../../../../assets/brand/irori-wordmark.svg")
        .replacen("<svg ", r#"<svg width="352" height="160" "#, 1)
        .replace("currentColor", hex)
}

fn hsla_to_rgb(color: Hsla) -> (u8, u8, u8) {
    let rgba: Rgba = color.into();
    let to_u8 = |c: f32| (c.clamp(0.0, 1.0) * 255.0).round() as u8;
    (to_u8(rgba.r), to_u8(rgba.g), to_u8(rgba.b))
}

/// `assets/brand/irori-mark.svg`, given a small intrinsic size: GPUI rasterizes
/// an SVG image at that size, and the 1024-unit original would be scaled down
/// on the GPU with visible aliasing.
static MARK: Lazy<Arc<Image>> =
    Lazy::new(|| Arc::new(Image::from_bytes(ImageFormat::Svg, mark_svg().into_bytes())));

/// Cache the last tinted wordmark SVG by packed RGB so theme switches re-tint
/// cleanly while repeated sidebar frames reuse the same image handle.
static WORDMARK_CACHE: Mutex<Option<(u32, Arc<Image>)>> = Mutex::new(None);

fn wordmark_image(color: Hsla) -> Arc<Image> {
    let (r, g, b) = hsla_to_rgb(color);
    let key = ((r as u32) << 16) | ((g as u32) << 8) | (b as u32);
    let mut cache = WORDMARK_CACHE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some((cached_key, img)) = cache.as_ref()
        && *cached_key == key
    {
        return img.clone();
    }
    let hex = format!("#{r:02x}{g:02x}{b:02x}");
    let image = Arc::new(Image::from_bytes(
        ImageFormat::Svg,
        wordmark_svg(&hex).into_bytes(),
    ));
    *cache = Some((key, image.clone()));
    image
}

/// The app icon: the RPG bonfire and sword mark.
#[derive(IntoElement)]
pub struct BrandMark {
    size: Rems,
}

impl BrandMark {
    pub fn new(size: Rems) -> Self {
        Self { size }
    }
}

impl RenderOnce for BrandMark {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        img(MARK.clone()).size(self.size).flex_shrink_0()
    }
}

/// The `irori` wordmark tinted to the current chrome text colour.
#[derive(IntoElement)]
pub struct BrandWordmark {
    height: Rems,
    color: Hsla,
}

impl BrandWordmark {
    pub fn new(height: Rems, color: Hsla) -> Self {
        Self { height, color }
    }
}

impl RenderOnce for BrandWordmark {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        img(wordmark_image(self.color))
            .h(self.height)
            .w(self.height * WORDMARK_ASPECT)
            .flex_shrink_0()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mark_svg_injects_intrinsic_size() {
        let svg = mark_svg();
        assert!(svg.starts_with(r#"<svg width="192" height="192" "#));
    }

    #[test]
    fn wordmark_svg_tints_current_color_and_sets_size() {
        let svg = wordmark_svg("#f3eff8");
        assert!(svg.starts_with(r#"<svg width="352" height="160" "#));
        assert!(!svg.contains("currentColor"));
        assert!(svg.contains(r##"fill="#f3eff8""##));
    }

    #[test]
    fn hsla_to_rgb_formats_extremes() {
        assert_eq!(
            hsla_to_rgb(Hsla {
                h: 0.0,
                s: 0.0,
                l: 1.0,
                a: 1.0
            }),
            (255, 255, 255)
        );
        assert_eq!(
            hsla_to_rgb(Hsla {
                h: 0.0,
                s: 0.0,
                l: 0.0,
                a: 1.0
            }),
            (0, 0, 0)
        );
    }
}
