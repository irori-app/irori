//! Circular Hue/Saturation color wheel popover for custom MangoHud palette slots.
//!
//! Renders a cached `256x256` BGRA wheel texture with GPU rounded-corner
//! clipping, a Brightness (Value) gradient slider, a hex readout, and quick
//! hardware/HUD color swatches. All displayed text is cached in component state
//! so `Render::render` allocates zero `String`s.

use std::cell::Cell;
use std::rc::Rc;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::slider::{Slider, SliderEvent, SliderState};
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Selectable, Sizable as _, StyledExt as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, AppContext as _, Bounds, ClipboardItem, Context, Corners, Entity, Hsla,
    InteractiveElement as _, IntoElement, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, ParentElement as _, Pixels, Point, Render, RenderImage, RenderOnce, SharedString,
    StatefulInteractiveElement as _, Styled as _, Subscription, Task, Window, canvas, div,
    linear_color_stop, linear_gradient, px, rgb, transparent_black,
};
use irori_core::hud::sanitize_hex6;
use irori_ui::i18n::t_args;

use crate::hud::panel::ColorSlot;

/// Backing pixel resolution of the cached hue/saturation wheel texture.
const WHEEL_TEX_SIZE: u32 = 256;
/// On-screen diameter of the circular hue/saturation wheel in logical pixels.
const WHEEL_SIZE_PX: f32 = 168.0;
/// Diameter of the draggable color thumb drawn on top of the wheel.
const THUMB_SIZE_PX: f32 = 18.0;
/// Maximum dim overlay opacity on the wheel so hues remain visible even at `value = 0`.
const MAX_WHEEL_DIM_OPACITY: f32 = 0.68;

/// Curated one-click color swatches useful for hardware labels and dark HUDs.
const QUICK_SWATCHES: [(&str, &str, u32); 10] = [
    ("hud-quick-amd", "ED1C24", 0xED1C24),
    ("hud-quick-nvidia", "76B900", 0x76B900),
    ("hud-quick-intel", "0095FF", 0x0095FF),
    ("hud-quick-frametime", "00FF00", 0x00FF00),
    ("hud-quick-cyan", "06B6D4", 0x06B6D4),
    ("hud-quick-purple", "A491D3", 0xA491D3),
    ("hud-quick-amber", "F59E0B", 0xF59E0B),
    ("hud-quick-white", "FFFFFF", 0xFFFFFF),
    ("hud-quick-gray", "A1A1AA", 0xA1A1AA),
    ("hud-quick-black", "000000", 0x000000),
];

type ColorPickCallback = Rc<dyn Fn(ColorSlot, String, bool, &mut Window, &mut App)>;

/// Convert HSV (`h` in `0..360`, `s` and `v` in `0..=1`) to 8-bit sRGB channels.
pub(crate) fn hsv_to_rgb(h: f32, s: f32, v: f32) -> (u8, u8, u8) {
    let h = h.rem_euclid(360.0);
    let s = s.clamp(0.0, 1.0);
    let v = v.clamp(0.0, 1.0);
    let c = v * s;
    let sector = h / 60.0;
    let x = c * (1.0 - (sector % 2.0 - 1.0).abs());
    let m = v - c;
    let (r1, g1, b1) = match sector as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    (
        ((r1 + m) * 255.0).round().clamp(0.0, 255.0) as u8,
        ((g1 + m) * 255.0).round().clamp(0.0, 255.0) as u8,
        ((b1 + m) * 255.0).round().clamp(0.0, 255.0) as u8,
    )
}

/// Convert 8-bit sRGB channels to HSV (`h` in `0..360`, `s` and `v` in `0..=1`).
pub(crate) fn rgb_to_hsv(r: u8, g: u8, b: u8) -> (f32, f32, f32) {
    let rf = f32::from(r) / 255.0;
    let gf = f32::from(g) / 255.0;
    let bf = f32::from(b) / 255.0;
    let max = rf.max(gf).max(bf);
    let min = rf.min(gf).min(bf);
    let delta = max - min;
    let h = if delta <= f32::EPSILON {
        0.0
    } else if (max - rf).abs() <= f32::EPSILON {
        60.0 * (((gf - bf) / delta).rem_euclid(6.0))
    } else if (max - gf).abs() <= f32::EPSILON {
        60.0 * (((bf - rf) / delta) + 2.0)
    } else {
        60.0 * (((rf - gf) / delta) + 4.0)
    };
    let s = if max <= f32::EPSILON {
        0.0
    } else {
        delta / max
    };
    (h.rem_euclid(360.0), s.clamp(0.0, 1.0), max.clamp(0.0, 1.0))
}

/// Parse a 6-digit hex color string into HSV, falling back to `fallback`.
pub(crate) fn hex6_to_hsv(hex: &str, fallback: &str) -> (f32, f32, f32) {
    let clean = sanitize_hex6(hex, fallback);
    let val = u32::from_str_radix(&clean, 16).unwrap_or(0xFFFFFF);
    let r = ((val >> 16) & 0xFF) as u8;
    let g = ((val >> 8) & 0xFF) as u8;
    let b = (val & 0xFF) as u8;
    rgb_to_hsv(r, g, b)
}

/// Encode HSV as an uppercase 6-digit `RRGGBB` string without `#`.
pub(crate) fn hsv_to_hex6(h: f32, s: f32, v: f32) -> String {
    let (r, g, b) = hsv_to_rgb(h, s, v);
    format!("{r:02X}{g:02X}{b:02X}")
}

fn rgb_u32(r: u8, g: u8, b: u8) -> u32 {
    (u32::from(r) << 16) | (u32::from(g) << 8) | u32::from(b)
}

/// Compute the WCAG 2.1 contrast ratio (`1.0..=21.0`) between two 6-digit hex colors.
pub(crate) fn wcag_contrast_ratio(hex_a: &str, hex_b: &str) -> f32 {
    fn channel_lum(c: u8) -> f32 {
        let s = f32::from(c) / 255.0;
        if s <= 0.03928 {
            s / 12.92
        } else {
            ((s + 0.055) / 1.055).powf(2.4)
        }
    }
    fn rel_lum(hex: &str) -> f32 {
        let clean = sanitize_hex6(hex, "FFFFFF");
        let val = u32::from_str_radix(&clean, 16).unwrap_or(0xFFFFFF);
        let r = ((val >> 16) & 0xFF) as u8;
        let g = ((val >> 8) & 0xFF) as u8;
        let b = (val & 0xFF) as u8;
        0.2126 * channel_lum(r) + 0.7152 * channel_lum(g) + 0.0722 * channel_lum(b)
    }
    let la = rel_lum(hex_a);
    let lb = rel_lum(hex_b);
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

/// Map a pointer offset `(dx, dy)` relative to the wheel center to `(hue, saturation)`.
///
/// Matches the clockwise wheel layout with red (`0°`) at the bottom (6 o'clock),
/// yellow (`60°`) bottom-left, green (`120°`) upper-left, cyan (`180°`) top,
/// blue (`240°`) upper-right, magenta (`300°`) lower-right, and white at center.
pub(crate) fn wheel_point_to_hs(dx: f32, dy: f32, radius: f32) -> (f32, f32) {
    if radius <= f32::EPSILON {
        return (0.0, 0.0);
    }
    let dist = (dx * dx + dy * dy).sqrt();
    let sat = (dist / radius).clamp(0.0, 1.0);
    let hue = if dist <= f32::EPSILON {
        0.0
    } else {
        (-dx).atan2(dy).to_degrees().rem_euclid(360.0)
    };
    (hue, sat)
}

/// Inverse of [`wheel_point_to_hs`]: returns `(dx, dy)` offset from wheel center.
pub(crate) fn hs_to_wheel_offset(hue_deg: f32, sat: f32, radius: f32) -> (f32, f32) {
    let theta = hue_deg.to_radians();
    let r = sat.clamp(0.0, 1.0) * radius;
    (-r * theta.sin(), r * theta.cos())
}

/// Build and cache the `256x256` BGRA Hue/Saturation wheel texture once per process.
fn wheel_image() -> Arc<RenderImage> {
    static WHEEL: OnceLock<Arc<RenderImage>> = OnceLock::new();
    Arc::clone(WHEEL.get_or_init(build_wheel_image))
}

fn build_wheel_image() -> Arc<RenderImage> {
    let mut buffer = image::RgbaImage::new(WHEEL_TEX_SIZE, WHEEL_TEX_SIZE);
    let half = WHEEL_TEX_SIZE as f32 * 0.5;
    let radius = half - 1.0;
    for (x, y, pixel) in buffer.enumerate_pixels_mut() {
        let dx = (x as f32 + 0.5) - half;
        let dy = (y as f32 + 0.5) - half;
        let (hue, sat) = wheel_point_to_hs(dx, dy, radius);
        let (r, g, b) = hsv_to_rgb(hue, sat, 1.0);
        // GPUI's `RenderImage` expects BGRA channel order; corners outside the
        // disc stay saturated at the rim so GPU rounded-corner clipping produces
        // a clean anti-aliased edge with no dark fringe.
        *pixel = image::Rgba([b, g, r, 255]);
    }
    Arc::new(RenderImage::new(vec![image::Frame::new(buffer)]))
}

/// Clickable `18x18` color square used as the `Popover` trigger for each custom slot.
#[derive(IntoElement)]
pub(crate) struct ColorSwatchTrigger {
    id: &'static str,
    color: Hsla,
    tooltip: SharedString,
    selected: bool,
}

impl ColorSwatchTrigger {
    pub(crate) fn new(id: &'static str, color: Hsla, tooltip: SharedString) -> Self {
        Self {
            id,
            color,
            tooltip,
            selected: false,
        }
    }
}

impl Selectable for ColorSwatchTrigger {
    fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    fn is_selected(&self) -> bool {
        self.selected
    }
}

impl RenderOnce for ColorSwatchTrigger {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let border = if self.selected {
            theme.ring
        } else {
            theme.border
        };
        let hover_border = theme.foreground;
        let tip = self.tooltip;

        div()
            .id(self.id)
            .w(px(22.))
            .h(px(22.))
            .flex_shrink_0()
            .rounded(px(5.))
            .bg(self.color)
            .when(self.selected, |d| d.border_2())
            .when(!self.selected, |d| d.border_1())
            .border_color(border)
            .hover(move |s| s.border_color(hover_border))
            .cursor_pointer()
            .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
    }
}

/// Cached display labels rendered inside [`HudColorWheel`].
pub(crate) struct ColorWheelLabels {
    pub(crate) slot: SharedString,
    pub(crate) brightness: SharedString,
    pub(crate) quick_swatches: SharedString,
    pub(crate) reset_slot: SharedString,
    pub(crate) copy_hex: SharedString,
    pub(crate) copied_hex: SharedString,
}

/// Shared circular Hue/Saturation wheel popover body reused across all 11 color slots.
pub(crate) struct HudColorWheel {
    slot: ColorSlot,
    slot_label: SharedString,
    hue: f32,
    sat: f32,
    value: f32,
    dragging: bool,
    syncing_slider: bool,
    wheel_bounds: Rc<Cell<Bounds<Pixels>>>,
    value_slider: Entity<SliderState>,
    on_pick: ColorPickCallback,
    hex_raw: String,
    hex_label: SharedString,
    value_pct_label: SharedString,
    pair_hex: Option<String>,
    contrast_note: Option<SharedString>,
    copied: bool,
    copied_task: Option<Task<()>>,
    brightness_label: SharedString,
    quick_swatches_label: SharedString,
    reset_slot_label: SharedString,
    copy_hex_label: SharedString,
    copied_hex_label: SharedString,
    _subscriptions: Vec<Subscription>,
}

impl HudColorWheel {
    pub(crate) fn new(
        slot: ColorSlot,
        initial_hex: &str,
        pair_hex: Option<&str>,
        labels: ColorWheelLabels,
        on_pick: impl Fn(ColorSlot, String, bool, &mut Window, &mut App) + 'static,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let (hue, sat, value) = hex6_to_hsv(initial_hex, slot.fallback());
        let value_slider = cx.new(|_| {
            SliderState::new()
                .min(0.0)
                .max(1.0)
                .step(0.01)
                .default_value(value)
        });
        let sub = cx.subscribe_in(
            &value_slider,
            window,
            |this, _, event: &SliderEvent, window, cx| {
                if this.syncing_slider {
                    return;
                }
                let (v, released) = match event {
                    SliderEvent::Change(val) => (val.start(), false),
                    SliderEvent::Release(val) => (val.start(), true),
                };
                this.value = v.clamp(0.0, 1.0);
                this.refresh_cached_labels();
                this.emit_pick(released, window, cx);
                cx.notify();
            },
        );
        let mut this = Self {
            slot,
            slot_label: labels.slot,
            hue,
            sat,
            value,
            dragging: false,
            syncing_slider: false,
            wheel_bounds: Rc::new(Cell::new(Bounds::default())),
            value_slider,
            on_pick: Rc::new(on_pick),
            hex_raw: String::new(),
            hex_label: SharedString::default(),
            value_pct_label: SharedString::default(),
            pair_hex: pair_hex.map(str::to_string),
            contrast_note: None,
            copied: false,
            copied_task: None,
            brightness_label: labels.brightness,
            quick_swatches_label: labels.quick_swatches,
            reset_slot_label: labels.reset_slot,
            copy_hex_label: labels.copy_hex,
            copied_hex_label: labels.copied_hex,
            _subscriptions: vec![sub],
        };
        this.refresh_cached_labels();
        this
    }

    pub(crate) fn is_dragging(&self) -> bool {
        self.dragging
    }

    pub(crate) fn set_labels(
        &mut self,
        labels: ColorWheelLabels,
        update_slot_label: bool,
        cx: &mut Context<Self>,
    ) {
        self.brightness_label = labels.brightness;
        self.quick_swatches_label = labels.quick_swatches;
        self.reset_slot_label = labels.reset_slot;
        self.copy_hex_label = labels.copy_hex;
        self.copied_hex_label = labels.copied_hex;
        if update_slot_label {
            self.slot_label = labels.slot;
        }
        self.refresh_cached_labels();
        cx.notify();
    }

    /// Rebind the shared wheel to `slot` and load its current hex value.
    pub(crate) fn set_slot(
        &mut self,
        slot: ColorSlot,
        slot_label: SharedString,
        hex: &str,
        pair_hex: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.slot = slot;
        self.slot_label = slot_label;
        self.pair_hex = pair_hex.map(str::to_string);
        self.dragging = false;
        self.copied = false;
        self.copied_task = None;
        self.apply_external_hex(hex, window, cx);
    }

    /// Update the wheel state when the user edits the adjacent hex input while
    /// this slot's popover is open.
    pub(crate) fn set_hex_if_slot(
        &mut self,
        slot: ColorSlot,
        hex: &str,
        pair_hex: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.slot == slot && !self.dragging {
            self.pair_hex = pair_hex.map(str::to_string);
            self.apply_external_hex(hex, window, cx);
        }
    }

    fn apply_external_hex(&mut self, hex: &str, window: &mut Window, cx: &mut Context<Self>) {
        let (h, s, v) = hex6_to_hsv(hex, self.slot.fallback());
        self.hue = h;
        self.sat = s;
        self.value = v;
        self.sync_value_slider(window, cx);
        self.refresh_cached_labels();
        cx.notify();
    }

    fn sync_value_slider(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.syncing_slider = true;
        let value = self.value;
        self.value_slider
            .update(cx, |s, cx| s.set_value(value, window, cx));
        self.syncing_slider = false;
    }

    fn refresh_cached_labels(&mut self) {
        let hex = hsv_to_hex6(self.hue, self.sat, self.value);
        let pct = (self.value * 100.0).round() as u8;
        self.contrast_note = self.pair_hex.as_deref().and_then(|other| {
            let ratio = wcag_contrast_ratio(&hex, other);
            (ratio < 4.5).then(|| {
                let ratio_str = format!("{ratio:.1}");
                t_args(
                    "settings.hud.color.low_contrast_short",
                    &[("ratio", ratio_str.as_str())],
                )
            })
        });
        self.hex_label = SharedString::from(format!("#{hex}"));
        self.hex_raw = hex;
        self.value_pct_label = SharedString::from(format!("{pct}%"));
    }

    fn copy_hex(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(self.hex_label.to_string()));
        self.copied = true;
        cx.notify();
        self.copied_task = Some(cx.spawn_in(window, async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(900))
                .await;
            let _ = this.update(cx, |this, cx| {
                this.copied = false;
                this.copied_task = None;
                cx.notify();
            });
        }));
    }

    fn pick_from_point(
        &mut self,
        position: Point<Pixels>,
        released: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let bounds = self.wheel_bounds.get();
        let width = f32::from(bounds.size.width);
        let height = f32::from(bounds.size.height);
        let radius = width.min(height) * 0.5;
        if radius <= f32::EPSILON {
            return;
        }
        let cx_px = f32::from(bounds.origin.x) + width * 0.5;
        let cy_px = f32::from(bounds.origin.y) + height * 0.5;
        let dx = f32::from(position.x) - cx_px;
        let dy = f32::from(position.y) - cy_px;
        let (hue, sat) = wheel_point_to_hs(dx, dy, radius);
        self.hue = hue;
        self.sat = sat;
        self.refresh_cached_labels();
        self.emit_pick(released, window, cx);
        cx.notify();
    }

    fn pick_quick_swatch(&mut self, hex: &str, window: &mut Window, cx: &mut Context<Self>) {
        let (h, s, v) = hex6_to_hsv(hex, self.slot.fallback());
        self.hue = h;
        self.sat = s;
        self.value = v;
        self.sync_value_slider(window, cx);
        self.refresh_cached_labels();
        self.emit_pick(true, window, cx);
        cx.notify();
    }

    fn emit_pick(&self, released: bool, window: &mut Window, cx: &mut Context<Self>) {
        let hex = hsv_to_hex6(self.hue, self.sat, self.value);
        let cb = self.on_pick.clone();
        let slot = self.slot;
        cb(slot, hex, released, window, cx);
    }
}

impl Render for HudColorWheel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (border, muted_fg, warning, mono_font, hover_border) = {
            let theme = cx.theme();
            (
                theme.border,
                theme.muted_foreground,
                theme.warning,
                theme.mono_font_family.clone(),
                theme.foreground,
            )
        };

        let (cr, cg, cb) = hsv_to_rgb(self.hue, self.sat, self.value);
        let current_color: Hsla = rgb(rgb_u32(cr, cg, cb)).into();
        let (fr, fg, fb) = hsv_to_rgb(self.hue, self.sat, 1.0);
        let full_bright_color: Hsla = rgb(rgb_u32(fr, fg, fb)).into();

        let radius = WHEEL_SIZE_PX * 0.5;
        let (dx, dy) = hs_to_wheel_offset(self.hue, self.sat, radius);
        let thumb_left = (radius + dx - THUMB_SIZE_PX * 0.5)
            .clamp(-THUMB_SIZE_PX * 0.25, WHEEL_SIZE_PX - THUMB_SIZE_PX * 0.75);
        let thumb_top = (radius + dy - THUMB_SIZE_PX * 0.5)
            .clamp(-THUMB_SIZE_PX * 0.25, WHEEL_SIZE_PX - THUMB_SIZE_PX * 0.75);
        let dim_opacity = ((1.0 - self.value) * MAX_WHEEL_DIM_OPACITY).clamp(0.0, 1.0);
        let wheel_bounds = self.wheel_bounds.clone();

        let wheel = div()
            .id("hud-color-wheel-disc")
            .relative()
            .size(px(WHEEL_SIZE_PX))
            .cursor_pointer()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, window, cx| {
                    this.dragging = true;
                    this.pick_from_point(event.position, false, window, cx);
                    cx.stop_propagation();
                }),
            )
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, window, cx| {
                if !this.dragging {
                    return;
                }
                if event.pressed_button == Some(MouseButton::Left) {
                    this.pick_from_point(event.position, false, window, cx);
                } else {
                    this.dragging = false;
                    this.emit_pick(true, window, cx);
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, event: &MouseUpEvent, window, cx| {
                    if this.dragging {
                        this.dragging = false;
                        this.pick_from_point(event.position, true, window, cx);
                    }
                }),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    if this.dragging {
                        this.dragging = false;
                        this.emit_pick(true, window, cx);
                    }
                }),
            )
            .child(
                canvas(
                    |_, _, _| {},
                    move |bounds, _, window, _| {
                        wheel_bounds.set(bounds);
                        let _ = window.paint_image(
                            bounds,
                            bounds,
                            Corners::all(px(radius)),
                            wheel_image(),
                            0,
                            false,
                        );
                    },
                )
                .size_full(),
            )
            .when(dim_opacity > 0.005, |d| {
                d.child(
                    div()
                        .absolute()
                        .inset_0()
                        .rounded_full()
                        .bg(Hsla::black().opacity(dim_opacity)),
                )
            })
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .rounded_full()
                    .border_2()
                    .border_color(Hsla::white().opacity(0.85)),
            )
            .child(
                div()
                    .absolute()
                    .left(px(thumb_left))
                    .top(px(thumb_top))
                    .size(px(THUMB_SIZE_PX))
                    .rounded_full()
                    .bg(current_color)
                    .border_2()
                    .border_color(Hsla::white())
                    .shadow_sm(),
            );

        let brightness_track = div()
            .relative()
            .w_full()
            .h_6()
            .flex()
            .items_center()
            .child(
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .top(px(9.))
                    .h_1p5()
                    .rounded_full()
                    .bg(linear_gradient(
                        90.0,
                        linear_color_stop(Hsla::black(), 0.0),
                        linear_color_stop(full_bright_color, 1.0),
                    ))
                    .border_1()
                    .border_color(border),
            )
            .child(
                div().absolute().inset_0().flex().items_center().child(
                    Slider::new(&self.value_slider)
                        .bg(transparent_black())
                        .accessibility_label(self.brightness_label.clone()),
                ),
            );

        let brightness_section = v_flex()
            .w_full()
            .gap_1()
            .child(
                h_flex()
                    .w_full()
                    .justify_between()
                    .items_center()
                    .child(
                        div()
                            .text_xs()
                            .font_semibold()
                            .child(self.brightness_label.clone()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(muted_fg)
                            .child(self.value_pct_label.clone()),
                    ),
            )
            .child(brightness_track);

        let is_slot_default = self.hex_raw.eq_ignore_ascii_case(self.slot.fallback());
        let hex_chip_label = if self.copied {
            self.copied_hex_label.clone()
        } else {
            self.hex_label.clone()
        };
        let preview_row = h_flex()
            .w_full()
            .items_center()
            .justify_between()
            .gap_2()
            .child(
                h_flex()
                    .items_center()
                    .gap_2()
                    .min_w_0()
                    .child(
                        div()
                            .w(px(20.))
                            .h(px(20.))
                            .flex_shrink_0()
                            .rounded(px(4.))
                            .bg(current_color)
                            .border_1()
                            .border_color(border),
                    )
                    .child(
                        div()
                            .text_xs()
                            .font_semibold()
                            .truncate()
                            .child(self.slot_label.clone()),
                    ),
            )
            .child(
                h_flex()
                    .items_center()
                    .gap_1()
                    .flex_shrink_0()
                    .child(
                        Button::new("hud-wheel-copy-hex")
                            .outline()
                            .xsmall()
                            .tooltip(self.copy_hex_label.clone())
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.copy_hex(window, cx);
                            }))
                            .child(div().font_family(mono_font).text_xs().child(hex_chip_label)),
                    )
                    .child(
                        Button::new("hud-wheel-reset-slot")
                            .ghost()
                            .xsmall()
                            .icon(IconName::RotateCcw)
                            .disabled(is_slot_default)
                            .accessibility_label(self.reset_slot_label.clone())
                            .tooltip(self.reset_slot_label.clone())
                            .on_click(cx.listener(|this, _, window, cx| {
                                let fallback = this.slot.fallback();
                                this.pick_quick_swatch(fallback, window, cx);
                            })),
                    ),
            );

        let mut quick_row = h_flex().w_full().flex_wrap().gap_1p5();
        for (id, hex, value_u32) in QUICK_SWATCHES {
            let swatch_color: Hsla = rgb(value_u32).into();
            quick_row = quick_row.child(
                div()
                    .id(id)
                    .w(px(18.))
                    .h(px(18.))
                    .rounded(px(4.))
                    .bg(swatch_color)
                    .border_1()
                    .border_color(border)
                    .hover(move |s| s.border_color(hover_border))
                    .cursor_pointer()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, window, cx| {
                            this.pick_quick_swatch(hex, window, cx);
                            cx.stop_propagation();
                        }),
                    ),
            );
        }

        let quick_section = v_flex()
            .w_full()
            .gap_1p5()
            .child(
                div()
                    .text_xs()
                    .text_color(muted_fg)
                    .child(self.quick_swatches_label.clone()),
            )
            .child(quick_row);

        v_flex()
            .w(px(236.))
            .gap_3()
            .items_center()
            .child(preview_row)
            .child(wheel)
            .child(brightness_section)
            .when_some(self.contrast_note.clone(), |v, note| {
                v.child(div().w_full().text_xs().text_color(warning).child(note))
            })
            .child(quick_section)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rgb_hsv_round_trip_preserves_primary_and_accent_colors() {
        for (r, g, b) in [
            (0x00, 0x00, 0x00),
            (0xFF, 0xFF, 0xFF),
            (0xED, 0x1C, 0x24),
            (0x76, 0xB9, 0x00),
            (0x00, 0x95, 0xFF),
            (0x2E, 0x97, 0x62),
            (0xAD, 0x64, 0xC1),
            (0x02, 0x02, 0x02),
        ] {
            let (h, s, v) = rgb_to_hsv(r, g, b);
            let (r2, g2, b2) = hsv_to_rgb(h, s, v);
            assert!(
                r.abs_diff(r2) <= 1 && g.abs_diff(g2) <= 1 && b.abs_diff(b2) <= 1,
                "round-trip drift for ({r},{g},{b}) -> ({r2},{g2},{b2})"
            );
        }
    }

    #[test]
    fn hex6_hsv_round_trip_and_fallback() {
        let (h, s, v) = hex6_to_hsv("#2e9762", "FFFFFF");
        assert_eq!(hsv_to_hex6(h, s, v), "2E9762");

        let (h_bad, s_bad, v_bad) = hex6_to_hsv("not-hex", "0095FF");
        assert_eq!(hsv_to_hex6(h_bad, s_bad, v_bad), "0095FF");
    }

    #[test]
    fn wheel_coordinates_match_cardinal_hues_and_clamp_outside_disc() {
        let r = 84.0;
        let (h_bottom, s_bottom) = wheel_point_to_hs(0.0, r, r);
        assert!(h_bottom.abs() < 0.01);
        assert!((s_bottom - 1.0).abs() < 0.01);

        let (h_top, s_top) = wheel_point_to_hs(0.0, -r, r);
        assert!((h_top - 180.0).abs() < 0.01);
        assert!((s_top - 1.0).abs() < 0.01);

        // Points beyond the disc radius clamp saturation to 1.0.
        let (h_far, s_far) = wheel_point_to_hs(r * 3.0, 0.0, r);
        assert!((h_far - 270.0).abs() < 0.01);
        assert!((s_far - 1.0).abs() < 0.001);

        // Inverse maps (hue, sat) back to the original wheel offset.
        for (hue, sat) in [(0.0, 1.0), (60.0, 0.75), (120.0, 0.5), (240.0, 0.9)] {
            let (dx, dy) = hs_to_wheel_offset(hue, sat, r);
            let (h2, s2) = wheel_point_to_hs(dx, dy, r);
            assert!((h2 - hue).abs() < 0.1, "{hue} -> {h2}");
            assert!((s2 - sat).abs() < 0.01, "{sat} -> {s2}");
        }
    }

    #[test]
    fn wheel_image_has_expected_dimensions() {
        let img = build_wheel_image();
        assert_eq!(img.frame_count(), 1);
        let size = img.size(0);
        assert_eq!(size.width.0, WHEEL_TEX_SIZE as i32);
        assert_eq!(size.height.0, WHEEL_TEX_SIZE as i32);
    }

    #[test]
    fn wcag_contrast_ratio_identifies_high_and_low_contrast_pairs() {
        assert!((wcag_contrast_ratio("FFFFFF", "000000") - 21.0).abs() < 0.05);
        assert!((wcag_contrast_ratio("2E9762", "2E9762") - 1.0).abs() < 0.01);
        assert!(wcag_contrast_ratio("222222", "020202") < 2.0);
        assert!(wcag_contrast_ratio("FFFFFF", "020202") > 20.0);
    }
}
