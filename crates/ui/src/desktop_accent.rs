//! The system's colours, for systems whose theme Irori doesn't follow whole.
//!
//! Linux desktops share one source, `accent-color` in the XDG settings portal,
//! so a single reader covers all that offer it (`docs/PLATFORMS.md` lists them)
//! and no desktop's files are involved. macOS ([`macos`]) and Windows
//! ([`windows`]) say more: an accent for each colour mode, how selected text is
//! highlighted, whether the user asked for more contrast or less transparency,
//! and on Windows the high-contrast colours. Every source signals its changes,
//! so nothing runs between them.

use futures::channel::mpsc;
use gpui_kit::Rgba;
use irori_services::Backend;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;

/// What the system says about colours, then each change of it, whole each time.
pub(crate) type Changes = mpsc::UnboundedReceiver<SystemColors>;

/// The system's colours at one moment.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct SystemColors {
    /// The accent in light themes. `None` means there is none.
    pub(crate) light: Option<Accent>,
    /// The accent in dark themes, which a system may keep a shade of its own for.
    pub(crate) dark: Option<Accent>,
    /// The user asked the system for more contrast.
    pub(crate) more_contrast: bool,
    /// Whether the system wants windows see-through, where it has a setting.
    pub(crate) transparency: Option<bool>,
    /// The system's high-contrast colours, which stand in for any theme.
    pub(crate) high_contrast: Option<ContrastColors>,
}

impl SystemColors {
    /// One accent for both colour modes, and nothing else, as the portal has it.
    #[cfg(any(target_os = "linux", test))]
    pub(crate) fn accent(color: Option<Rgba>) -> Self {
        let accent = color.map(|color| Accent {
            shades: vec![color],
            selection: None,
        });
        Self {
            light: accent.clone(),
            dark: accent,
            ..Self::default()
        }
    }
}

/// The accent for one colour mode.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Accent {
    /// The accent, then the system's own shades of it, each further from it
    /// towards the mode's text colour. Never empty.
    pub(crate) shades: Vec<Rgba>,
    /// What the system highlights selected text with.
    pub(crate) selection: Option<Rgba>,
}

/// The colours a high-contrast system theme gives every app (Windows' names).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ContrastColors {
    pub(crate) window: Rgba,
    pub(crate) window_text: Rgba,
    pub(crate) highlight: Rgba,
    pub(crate) highlight_text: Rgba,
    pub(crate) hotlight: Rgba,
    pub(crate) gray_text: Rgba,
    pub(crate) button_face: Rgba,
    pub(crate) button_text: Rgba,
}

/// Start listening. The channel closes when nothing can answer, which is how a
/// machine without a portal says so.
#[cfg(target_os = "linux")]
pub(crate) fn watch(backend: &Backend) -> Option<Changes> {
    let (tx, rx) = mpsc::unbounded();
    backend.spawn(async move {
        if let Err(e) = listen(tx).await {
            tracing::debug!("the desktop portal's accent colour: {e}");
        }
    });
    Some(rx)
}

/// Start listening. The first colours are read before this returns, so the
/// first window has them.
#[cfg(any(target_os = "macos", target_os = "windows"))]
pub(crate) fn watch(_backend: &Backend) -> Option<Changes> {
    let (tx, rx) = mpsc::unbounded();
    #[cfg(target_os = "macos")]
    macos::listen(tx);
    #[cfg(target_os = "windows")]
    if let Err(e) = windows::listen(tx) {
        tracing::debug!("Windows' colours: {e}");
    }
    Some(rx)
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
pub(crate) fn watch(_backend: &Backend) -> Option<Changes> {
    None
}

#[cfg(target_os = "linux")]
async fn listen(tx: mpsc::UnboundedSender<SystemColors>) -> ashpd::Result<()> {
    use ashpd::desktop::settings::Settings;
    use futures::StreamExt as _;

    let settings = Settings::new().await?;
    // Subscribing first means a change during the read can't be missed. A
    // desktop that offers no accent can still gain one with an update.
    let changes = settings.receive_accent_color_changed().await?;
    let accent = settings.accent_color().await.ok().and_then(from_portal);
    if tx.unbounded_send(SystemColors::accent(accent)).is_err() {
        return Ok(());
    }
    let mut changes = std::pin::pin!(changes);
    while let Some(color) = changes.next().await {
        if tx
            .unbounded_send(SystemColors::accent(from_portal(color)))
            .is_err()
        {
            break;
        }
    }
    Ok(())
}

/// The portal's colour. Its specification has values out of range mean "unset",
/// and has no other way to say so.
#[cfg(target_os = "linux")]
fn from_portal(color: ashpd::desktop::Color) -> Option<Rgba> {
    let channels = [color.red(), color.green(), color.blue()];
    channels
        .iter()
        .all(|channel| (0.0..=1.0).contains(channel))
        .then(|| {
            let [r, g, b] = channels.map(|channel| channel as f32);
            Rgba { r, g, b, a: 1.0 }
        })
}

/// An opaque colour from 8-bit channels, as Windows gives them.
// Here rather than in `windows` so its tests run on every system.
#[cfg_attr(not(any(target_os = "windows", test)), allow(dead_code))]
pub(crate) fn from_rgb8(r: u8, g: u8, b: u8) -> Rgba {
    let channel = |c: u8| f32::from(c) / 255.0;
    Rgba {
        r: channel(r),
        g: channel(g),
        b: channel(b),
        a: 1.0,
    }
}

/// An opaque colour from components that should be in 0..=1, as macOS gives
/// them. Extended-range sRGB can stray past either end; NaN can't be a colour.
// Here rather than in `macos` so its tests run on every system.
#[cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))]
pub(crate) fn from_unit(r: f64, g: f64, b: f64) -> Option<Rgba> {
    let channels = [r, g, b];
    if channels.iter().any(|c| c.is_nan()) {
        return None;
    }
    let [r, g, b] = channels.map(|c| c.clamp(0.0, 1.0) as f32);
    Some(Rgba { r, g, b, a: 1.0 })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_portals_accent_is_the_same_in_both_modes() {
        let blue = from_rgb8(0x35, 0x84, 0xe4);
        let colors = SystemColors::accent(Some(blue));
        assert_eq!(colors.light, colors.dark);
        let accent = colors.light.unwrap();
        assert_eq!(accent.shades, vec![blue]);
        assert_eq!(accent.selection, None);
        assert!(!colors.more_contrast);
        assert_eq!(colors.transparency, None);
        assert_eq!(colors.high_contrast, None);
    }

    #[test]
    fn no_accent_is_no_accent_in_either_mode() {
        assert_eq!(SystemColors::accent(None), SystemColors::default());
    }

    #[test]
    fn eight_bit_channels_cover_the_range() {
        assert_eq!(
            from_rgb8(0, 0x80, 0xff),
            Rgba {
                r: 0.0,
                g: 128.0 / 255.0,
                b: 1.0,
                a: 1.0
            }
        );
    }

    #[test]
    fn extended_range_components_are_held_in_range() {
        let wide = from_unit(-0.1, 0.5, 1.2).unwrap();
        assert_eq!((wide.r, wide.g, wide.b, wide.a), (0.0, 0.5, 1.0, 1.0));
        assert_eq!(from_unit(0.2, f64::NAN, 0.2), None);
    }

    #[cfg(target_os = "linux")]
    mod portal {
        use ashpd::desktop::Color;

        use super::super::*;

        #[test]
        fn a_colour_in_range_is_the_accent() {
            let accent = from_portal(Color::new(1.0, 0.5, 0.0)).unwrap();
            assert_eq!(
                (accent.r, accent.g, accent.b, accent.a),
                (1.0, 0.5, 0.0, 1.0)
            );
        }

        #[test]
        fn the_ends_of_the_range_count() {
            assert!(from_portal(Color::new(0.0, 0.0, 0.0)).is_some());
            assert!(from_portal(Color::new(1.0, 1.0, 1.0)).is_some());
        }

        #[test]
        fn any_channel_out_of_range_means_unset() {
            assert_eq!(from_portal(Color::new(-1.0, -1.0, -1.0)), None);
            assert_eq!(from_portal(Color::new(0.2, 1.5, 0.2)), None);
            assert_eq!(from_portal(Color::new(0.2, 0.2, f64::NAN)), None);
        }
    }
}
