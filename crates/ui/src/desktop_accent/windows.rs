//! Windows' colours, from the same settings Windows' own apps read: the accent
//! and its shades and the high-contrast colours from `UISettings`, high
//! contrast itself from `AccessibilitySettings`, and "Transparency effects"
//! (`EnableTransparency`) as `UISettings::AdvancedEffectsEnabled`.
//!
//! Each raises an event when it changes, on a thread of Windows' own, so
//! nothing of Irori's runs or waits between changes.

use futures::channel::mpsc::UnboundedSender;
use windows::Foundation::TypedEventHandler;
use windows::UI::Color;
use windows::UI::ViewManagement::{AccessibilitySettings, UIColorType, UIElementType, UISettings};
use windows::core::IInspectable;

use super::{Accent, ContrastColors, SystemColors, from_rgb8};

/// Send the colours now and after every change, for as long as Irori runs.
pub(super) fn listen(tx: UnboundedSender<SystemColors>) -> windows::core::Result<()> {
    let ui = UISettings::new()?;
    let access = AccessibilitySettings::new()?;
    let send = {
        let (ui, access, tx) = (ui.clone(), access.clone(), tx.clone());
        move || {
            let _ = tx.unbounded_send(read(&ui, &access));
        }
    };
    // Subscribing first means a change during the read can't be missed.
    // Accent and high-contrast colours both raise `ColorValuesChanged`.
    let on_colors = send.clone();
    ui.ColorValuesChanged(&TypedEventHandler::<UISettings, IInspectable>::new(
        move |_, _| {
            on_colors();
            Ok(())
        },
    ))?;
    let on_effects = send.clone();
    ui.AdvancedEffectsEnabledChanged(&TypedEventHandler::<UISettings, IInspectable>::new(
        move |_, _| {
            on_effects();
            Ok(())
        },
    ))?;
    access.HighContrastChanged(
        &TypedEventHandler::<AccessibilitySettings, IInspectable>::new(move |_, _| {
            send();
            Ok(())
        }),
    )?;
    let _ = tx.unbounded_send(read(&ui, &access));
    // The handlers last as long as these objects do, and Irori listens until
    // it exits.
    std::mem::forget((ui, access));
    Ok(())
}

fn read(ui: &UISettings, access: &AccessibilitySettings) -> SystemColors {
    let accent = |shades: [UIColorType; 4]| {
        let shades: Vec<_> = shades
            .into_iter()
            .map_while(|kind| ui.GetColorValue(kind).ok().map(rgba))
            .collect();
        (!shades.is_empty()).then_some(Accent {
            shades,
            selection: None,
        })
    };
    let high_contrast = access
        .HighContrast()
        .unwrap_or(false)
        .then(|| contrast_colors(ui))
        .flatten();
    // High contrast turns transparency effects off throughout Windows.
    let transparency = ui
        .AdvancedEffectsEnabled()
        .ok()
        .map(|on| on && high_contrast.is_none());
    SystemColors {
        light: accent([
            UIColorType::Accent,
            UIColorType::AccentDark1,
            UIColorType::AccentDark2,
            UIColorType::AccentDark3,
        ]),
        dark: accent([
            UIColorType::Accent,
            UIColorType::AccentLight1,
            UIColorType::AccentLight2,
            UIColorType::AccentLight3,
        ]),
        more_contrast: high_contrast.is_some(),
        transparency,
        high_contrast,
    }
}

fn contrast_colors(ui: &UISettings) -> Option<ContrastColors> {
    let color = |element| ui.UIElementColor(element).ok().map(rgba);
    Some(ContrastColors {
        window: color(UIElementType::Window)?,
        window_text: color(UIElementType::WindowText)?,
        highlight: color(UIElementType::Highlight)?,
        highlight_text: color(UIElementType::HighlightText)?,
        hotlight: color(UIElementType::Hotlight)?,
        gray_text: color(UIElementType::GrayText)?,
        button_face: color(UIElementType::ButtonFace)?,
        button_text: color(UIElementType::ButtonText)?,
    })
}

fn rgba(color: Color) -> gpui_kit::Rgba {
    from_rgb8(color.R, color.G, color.B)
}
