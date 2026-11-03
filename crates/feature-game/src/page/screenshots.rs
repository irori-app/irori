//! Screenshots: a strip of the store's screenshots across the page, the last
//! tile counting those that don't fit ("+13"), and a viewer that shows one
//! at a time over the page with a filmstrip of the rest. The viewer follows
//! the arrow keys, Home, End and Escape, which is also how a controller
//! reaches it.

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme as _, Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, Context, InteractiveElement as _, IntoElement, KeyDownEvent, ObjectFit,
    ParentElement, StatefulInteractiveElement as _, Styled, StyledImage as _, Window, div, rems,
};
use irori_ui::components::{ArtImage, art_image, art_img, art_loading};
use irori_ui::i18n::{t, t_args};
use irori_ui::style::KeyboardFocusable as _;

use super::GamePage;

/// Pixel width screenshots are decoded at in the strip and the filmstrip.
const THUMB_WIDTH: u32 = 640;
/// Largest pixel width screenshots are decoded at in the viewer.
const FULL_WIDTH: u32 = 1920;
/// Narrowest a strip tile gets before the strip shows one fewer, in rem.
const MIN_TILE_REM: f32 = 12.;
/// Gap between strip tiles (`gap_3`), in rem.
const GAP_REM: f32 = 0.75;
/// Most tiles the strip shows, so screenshots stay large on wide windows.
const MAX_TILES: usize = 5;
/// Height of a filmstrip thumbnail in the viewer, in rem.
const FILMSTRIP_REM: f32 = 4.5;

/// Tiles of the strip that fit `available` rem, at least one.
fn strip_tiles(available: f32) -> usize {
    let fit = ((available + GAP_REM) / (MIN_TILE_REM + GAP_REM)).floor();
    (fit.max(1.) as usize).min(MAX_TILES)
}

/// How many tiles to show of `count` screenshots when `fit` fit, and how
/// many the last tile stands for beyond itself ("+N"), if any.
fn strip_layout(count: usize, fit: usize) -> (usize, Option<usize>) {
    if count <= fit {
        (count, None)
    } else {
        (fit, Some(count - fit + 1))
    }
}

impl GamePage {
    pub(super) fn screenshots(&self) -> Vec<String> {
        self.extra
            .ready()
            .map(|e| e.screenshots.clone())
            .unwrap_or_default()
    }

    /// The screenshot strip, or nothing when the store has none.
    pub(super) fn render_screenshot_strip(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let shots = self.screenshots();
        if shots.is_empty() {
            return None;
        }
        // The body is inset by `px_8` on both sides.
        let fit = self
            .page_rem(window)
            .map_or(4, |page| strip_tiles(page - 4.));
        let (shown, more) = strip_layout(shots.len(), fit);
        let theme = cx.theme();
        let (scrim, on_scrim) = (theme.background.opacity(0.72), theme.foreground);
        let tiles: Vec<AnyElement> = shots
            .iter()
            .take(shown)
            .enumerate()
            .map(|(index, url)| {
                let content = match art_image(url, THUMB_WIDTH, false, cx) {
                    ArtImage::Ready(img) => img
                        .image_cache(&self.image_cache)
                        .size_full()
                        .object_fit(ObjectFit::Cover)
                        .into_any_element(),
                    ArtImage::Loading => art_loading(),
                    ArtImage::Missing => irori_ui::components::art_missing(cx),
                };
                let extra = more.filter(|_| index + 1 == shown);
                div()
                    .id(("screenshot", index))
                    .keyboard_focusable(cx)
                    .relative()
                    .flex_1()
                    .min_w_0()
                    .aspect_ratio(16. / 9.)
                    .rounded_md()
                    .overflow_hidden()
                    .bg(cx.theme().muted)
                    .cursor_pointer()
                    .child(content)
                    .when_some(extra, |d, n| {
                        d.child(
                            div()
                                .absolute()
                                .top_0()
                                .left_0()
                                .size_full()
                                .flex()
                                .items_center()
                                .justify_center()
                                .bg(scrim)
                                .text_color(on_scrim)
                                .text_2xl()
                                .font_bold()
                                .child(t_args(
                                    "game.screenshots.more",
                                    &[("count", &n.to_string())],
                                )),
                        )
                    })
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.open_screenshot(index, window, cx)
                    }))
                    .into_any_element()
            })
            .collect();
        Some(
            v_flex()
                .w_full()
                .gap_3()
                .child(super::caption(
                    t("game.info.tab_screenshots"),
                    self.ink.muted,
                ))
                .child(h_flex().w_full().gap_3().children(tiles))
                .into_any_element(),
        )
    }

    fn open_screenshot(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.viewer = Some(index);
        self.viewer_focus.focus(window, cx);
        self.viewer_strip.scroll_to_item(index);
        cx.notify();
    }

    fn close_viewer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.viewer = None;
        self.focus.focus(window, cx);
        cx.notify();
    }

    /// Move the viewer by `step` screenshots, wrapping around.
    fn step_viewer(&mut self, step: isize, cx: &mut Context<Self>) {
        let count = self.screenshots().len() as isize;
        if let (Some(index), true) = (self.viewer, count > 0) {
            self.show_in_viewer((index as isize + step).rem_euclid(count) as usize, cx);
        }
    }

    fn show_in_viewer(&mut self, index: usize, cx: &mut Context<Self>) {
        self.viewer = Some(index);
        self.viewer_strip.scroll_to_item(index);
        cx.notify();
    }

    /// The open screenshot over the whole page, or nothing.
    pub(super) fn render_viewer(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let index = self.viewer?;
        let shots = self.screenshots();
        let Some(url) = shots.get(index) else {
            self.viewer = None;
            return None;
        };
        let width = irori_ui::components::physical_width(rems(80.), window).min(FULL_WIDTH);
        let img = art_img(url, width, false, cx);
        let theme = cx.theme();
        Some(
            v_flex()
                .id("screenshot-viewer")
                .track_focus(&self.viewer_focus)
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .p_6()
                .gap_3()
                .bg(theme.background.opacity(0.95))
                .occlude()
                .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                    match event.keystroke.key.as_str() {
                        "escape" => this.close_viewer(window, cx),
                        "left" | "up" | "pageup" => this.step_viewer(-1, cx),
                        "right" | "down" | "pagedown" | "space" => this.step_viewer(1, cx),
                        "home" => this.show_in_viewer(0, cx),
                        "end" => {
                            let last = this.screenshots().len().saturating_sub(1);
                            this.show_in_viewer(last, cx)
                        }
                        _ => return,
                    }
                    cx.stop_propagation();
                }))
                .child(
                    h_flex()
                        .w_full()
                        .gap_2()
                        .child(
                            div()
                                .flex_1()
                                .text_sm()
                                .text_color(theme.muted_foreground)
                                .child(t_args(
                                    "game.screenshots.counter",
                                    &[
                                        ("index", &(index + 1).to_string()),
                                        ("count", &shots.len().to_string()),
                                    ],
                                )),
                        )
                        .child(
                            Button::new("screenshot-close")
                                .ghost()
                                .icon(IconName::X)
                                .accessibility_label(t("game.screenshots.close"))
                                .tooltip(t("game.screenshots.close"))
                                .on_click(
                                    cx.listener(|this, _, window, cx| {
                                        this.close_viewer(window, cx)
                                    }),
                                ),
                        ),
                )
                .child(
                    h_flex()
                        .flex_1()
                        .min_h_0()
                        .w_full()
                        .gap_3()
                        .child(
                            Button::new("screenshot-previous")
                                .ghost()
                                .large()
                                .icon(IconName::ChevronLeft)
                                .accessibility_label(t("game.screenshots.previous"))
                                .tooltip(t("game.screenshots.previous"))
                                .on_click(cx.listener(|this, _, _, cx| this.step_viewer(-1, cx))),
                        )
                        .child(div().flex_1().min_w_0().h_full().when_some(img, |d, img| {
                            d.child(
                                img.image_cache(&self.image_cache)
                                    .size_full()
                                    .object_fit(ObjectFit::Contain),
                            )
                        }))
                        .child(
                            Button::new("screenshot-next")
                                .ghost()
                                .large()
                                .icon(IconName::ChevronRight)
                                .accessibility_label(t("game.screenshots.next"))
                                .tooltip(t("game.screenshots.next"))
                                .on_click(cx.listener(|this, _, _, cx| this.step_viewer(1, cx))),
                        ),
                )
                .child(self.render_filmstrip(&shots, index, cx))
                .into_any_element(),
        )
    }
}

impl GamePage {
    /// Every screenshot small under the open one, the open one outlined;
    /// choosing one shows it.
    fn render_filmstrip(
        &self,
        shots: &[String],
        current: usize,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let thumbs: Vec<AnyElement> = shots
            .iter()
            .enumerate()
            .map(|(index, url)| {
                let image = art_img(url, THUMB_WIDTH, false, cx);
                let theme = cx.theme();
                div()
                    .id(("filmstrip", index))
                    .flex_shrink_0()
                    .h(rems(FILMSTRIP_REM))
                    .aspect_ratio(16. / 9.)
                    .rounded_md()
                    .overflow_hidden()
                    .border_2()
                    .border_color(if index == current {
                        theme.primary
                    } else {
                        theme.transparent
                    })
                    .bg(theme.muted)
                    .cursor_pointer()
                    .when(index != current, |d| d.opacity(0.6))
                    .when_some(image, |d, img| {
                        d.child(
                            img.image_cache(&self.image_cache)
                                .size_full()
                                .object_fit(ObjectFit::Cover),
                        )
                    })
                    .on_click(cx.listener(move |this, _, _, cx| this.show_in_viewer(index, cx)))
                    .into_any_element()
            })
            .collect();
        div()
            .id("screenshot-filmstrip")
            .w_full()
            .flex_shrink_0()
            .overflow_x_scroll()
            .track_scroll(&self.viewer_strip)
            .child(
                h_flex()
                    .gap_2()
                    .justify_center()
                    .min_w_full()
                    .children(thumbs),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::{MAX_TILES, strip_layout, strip_tiles};

    #[test]
    fn tiles_fill_the_width() {
        assert_eq!(strip_tiles(0.), 1);
        assert_eq!(strip_tiles(12.), 1);
        assert_eq!(strip_tiles(24.75), 2);
        assert_eq!(strip_tiles(52.), 4);
        assert_eq!(strip_tiles(500.), MAX_TILES);
    }

    #[test]
    fn the_last_tile_counts_the_rest() {
        assert_eq!(strip_layout(3, 4), (3, None));
        assert_eq!(strip_layout(4, 4), (4, None));
        // Three tiles, the fourth standing for itself and 13 more.
        assert_eq!(strip_layout(17, 4), (4, Some(14)));
    }
}
