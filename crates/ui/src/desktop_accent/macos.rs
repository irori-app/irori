//! macOS's colours: the accent (`controlAccentColor`) and the highlight colour
//! (`selectedTextBackgroundColor`) from System Settings → Appearance, and the
//! display options in Accessibility → Display ("Increase contrast", "Reduce
//! transparency").
//!
//! Both colours are dynamic: macOS resolves them for the appearance drawing
//! them, so each is read once in a light and once in a dark appearance, the
//! high-contrast ones while "Increase contrast" is on. Multicolour gives the
//! system's blue, as it does to Apple's apps; Graphite gives grey.
//!
//! AppKit posts a notification when either changes, on the main thread, so
//! nothing of Irori's runs between changes.

use std::cell::Cell;
use std::ptr::NonNull;

use block2::{RcBlock, StackBlock};
use futures::channel::mpsc::UnboundedSender;
use gpui_kit::Rgba;
use objc2_app_kit::{
    NSAppearance, NSAppearanceName, NSAppearanceNameAccessibilityHighContrastAqua,
    NSAppearanceNameAccessibilityHighContrastDarkAqua, NSAppearanceNameAqua,
    NSAppearanceNameDarkAqua, NSColor, NSColorSpace, NSSystemColorsDidChangeNotification,
    NSWorkspace, NSWorkspaceAccessibilityDisplayOptionsDidChangeNotification,
};
use objc2_foundation::{NSNotification, NSNotificationCenter};

use super::{Accent, SystemColors, from_unit};

/// Send the colours now and after every change, for as long as Irori runs.
pub(super) fn listen(tx: UnboundedSender<SystemColors>) {
    let send = {
        let tx = tx.clone();
        RcBlock::new(move |_: NonNull<NSNotification>| {
            let _ = tx.unbounded_send(read());
        })
    };
    let workspace = NSWorkspace::sharedWorkspace();
    // Subscribing first means a change during the read can't be missed.
    // SAFETY: the notification names are AppKit's own constants, valid for
    // the life of the process. Neither observer filters on an object, and
    // with no queue the block runs where AppKit posts, the main thread; it
    // only sends on a channel, which any thread may do.
    let observers = unsafe {
        [
            NSNotificationCenter::defaultCenter().addObserverForName_object_queue_usingBlock(
                Some(NSSystemColorsDidChangeNotification),
                None,
                None,
                &send,
            ),
            workspace
                .notificationCenter()
                .addObserverForName_object_queue_usingBlock(
                    Some(NSWorkspaceAccessibilityDisplayOptionsDidChangeNotification),
                    None,
                    None,
                    &send,
                ),
        ]
    };
    let _ = tx.unbounded_send(read());
    // The observers last as long as their tokens do, and Irori listens until
    // it exits.
    std::mem::forget(observers);
}

fn read() -> SystemColors {
    let workspace = NSWorkspace::sharedWorkspace();
    let more_contrast = workspace.accessibilityDisplayShouldIncreaseContrast();
    let reduce_transparency = workspace.accessibilityDisplayShouldReduceTransparency();
    // SAFETY: AppKit's own constants, valid for the life of the process.
    let (light, dark) = unsafe {
        if more_contrast {
            (
                NSAppearanceNameAccessibilityHighContrastAqua,
                NSAppearanceNameAccessibilityHighContrastDarkAqua,
            )
        } else {
            (NSAppearanceNameAqua, NSAppearanceNameDarkAqua)
        }
    };
    SystemColors {
        light: accent_in(light),
        dark: accent_in(dark),
        more_contrast,
        transparency: Some(!reduce_transparency),
        high_contrast: None,
    }
}

/// The accent and highlight colours as `appearance` draws them.
fn accent_in(appearance: &NSAppearanceName) -> Option<Accent> {
    let appearance = NSAppearance::appearanceNamed(appearance)?;
    let read = Cell::new(None);
    let block = StackBlock::new(|| {
        read.set(Some((
            srgb(&NSColor::controlAccentColor()),
            srgb(&NSColor::selectedTextBackgroundColor()),
        )));
    });
    appearance.performAsCurrentDrawingAppearance(&block);
    let (accent, selection) = read.take()?;
    Some(Accent {
        shades: vec![accent?],
        selection,
    })
}

/// `color` in sRGB, the space Irori's themes are in.
fn srgb(color: &NSColor) -> Option<Rgba> {
    let color = color.colorUsingColorSpace(&NSColorSpace::sRGBColorSpace())?;
    from_unit(
        color.redComponent(),
        color.greenComponent(),
        color.blueComponent(),
    )
}
