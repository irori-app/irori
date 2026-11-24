//! What marks a clip among screenshots: a small play chip in the corner of
//! its poster with its length once known. The poster itself comes from the
//! art store, which makes it from the clip ([`crate::art`]).

use std::path::Path;

use gpui_kit::assets::IconName;
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, h_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{AnyElement, App, IntoElement, ParentElement as _, Styled as _};

/// Whether `path` is a clip rather than a picture.
pub fn is_clip(path: &Path) -> bool {
    irori_services::screenshots::is_video(path)
}

/// The clip's length as text (`1:23`), once its poster was made.
pub fn clip_length(path: &Path) -> Option<String> {
    irori_services::media::posters::known_length(path)
        .map(irori_services::media::posters::length_label)
}

/// The chip for the bottom right of a clip's poster: a play mark and the
/// length when known. Its parent must be `relative()`.
pub fn clip_mark(path: &Path, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let (bg, fg) = (theme.background.opacity(0.85), theme.foreground);
    let length = clip_length(path);
    h_flex()
        .absolute()
        .bottom_1()
        .right_1()
        .gap_0p5()
        .px_1()
        .py_0p5()
        .rounded_sm()
        .bg(bg)
        .text_color(fg)
        .text_xs()
        .child(Icon::new(IconName::Play).xsmall())
        .when_some(length, |d, l| d.child(l))
        .into_any_element()
}
