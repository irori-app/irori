//! Circular Hue/Saturation color wheel popover adapter for custom MangoHud palette slots.
//!
//! Delegates the wheel disc, brightness slider, hex copy chip, and quick swatches
//! to [`irori_ui::components::ColorWheel`], binding them to MangoHud's [`ColorSlot`]s.

use std::cell::Cell;
use std::rc::Rc;

use gpui_kit::{App, AppContext as _, Context, Entity, IntoElement, Render, SharedString, Window};
pub(crate) use irori_ui::components::color_wheel::wcag_contrast_ratio;
use irori_ui::components::{
    ColorWheel, ColorWheelLabels as SharedLabels, ColorWheelPick, ColorWheelReset, ColorWheelSwatch,
};

use crate::hud::panel::ColorSlot;

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

fn hud_swatches() -> Vec<ColorWheelSwatch> {
    QUICK_SWATCHES
        .into_iter()
        .map(|(id, hex, value)| ColorWheelSwatch::from_hex(id, hex, value))
        .collect()
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

impl ColorWheelLabels {
    fn into_shared(self) -> SharedLabels {
        SharedLabels {
            title: self.slot,
            brightness: self.brightness,
            quick_swatches: self.quick_swatches,
            reset: self.reset_slot,
            copy_hex: self.copy_hex,
            copied_hex: self.copied_hex,
            low_contrast_key: "settings.hud.color.low_contrast_short",
        }
    }
}

/// Shared circular Hue/Saturation wheel popover body reused across all 11 HUD color slots.
pub(crate) struct HudColorWheel {
    slot: Rc<Cell<ColorSlot>>,
    wheel: Entity<ColorWheel>,
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
        let current_slot = Rc::new(Cell::new(slot));
        let slot_for_cb = Rc::clone(&current_slot);
        let fallback = slot.fallback();
        let wheel = cx.new(|cx| {
            ColorWheel::new(
                initial_hex,
                fallback,
                ColorWheelReset::DefaultHex(fallback.to_owned()),
                pair_hex,
                labels.into_shared(),
                hud_swatches(),
                move |pick, released, window, cx| {
                    if let ColorWheelPick::Hex(hex) = pick {
                        on_pick(slot_for_cb.get(), hex, released, window, cx);
                    }
                },
                window,
                cx,
            )
        });
        Self {
            slot: current_slot,
            wheel,
        }
    }

    pub(crate) fn is_dragging(&self, cx: &App) -> bool {
        self.wheel.read(cx).is_dragging()
    }

    pub(crate) fn set_labels(
        &mut self,
        labels: ColorWheelLabels,
        update_slot_label: bool,
        cx: &mut Context<Self>,
    ) {
        self.wheel.update(cx, |w, cx| {
            w.set_labels(labels.into_shared(), update_slot_label, cx);
        });
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
        self.slot.set(slot);
        let fallback = slot.fallback();
        self.wheel.update(cx, |w, cx| {
            w.rebind(
                slot_label,
                hex,
                fallback,
                ColorWheelReset::DefaultHex(fallback.to_owned()),
                pair_hex,
                window,
                cx,
            );
        });
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
        if self.slot.get() == slot {
            self.wheel.update(cx, |w, cx| {
                w.sync_external_hex(hex, pair_hex, None, window, cx);
            });
        }
    }
}

impl Render for HudColorWheel {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        self.wheel.clone()
    }
}
