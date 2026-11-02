//! The desktop's accent colour, for desktops Irori doesn't follow whole.
//!
//! Linux desktops share one source, `accent-color` in the XDG settings portal,
//! so a single reader covers all that offer it (`docs/PLATFORMS.md` lists them)
//! and no desktop's files are involved. The portal signals changes over D-Bus,
//! so nothing runs between them. Other systems have no source yet.

use futures::channel::mpsc;
use gpui_kit::Rgba;
use irori_services::Backend;

/// The accent colour, then each change of it. `None` means there is none.
pub(crate) type Changes = mpsc::UnboundedReceiver<Option<Rgba>>;

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

#[cfg(not(target_os = "linux"))]
pub(crate) fn watch(_backend: &Backend) -> Option<Changes> {
    None
}

#[cfg(target_os = "linux")]
async fn listen(tx: mpsc::UnboundedSender<Option<Rgba>>) -> ashpd::Result<()> {
    use ashpd::desktop::settings::Settings;
    use futures::StreamExt as _;

    let settings = Settings::new().await?;
    // Subscribing first means a change during the read can't be missed. A
    // desktop that offers no accent can still gain one with an update.
    let changes = settings.receive_accent_color_changed().await?;
    let accent = settings.accent_color().await.ok().and_then(from_portal);
    if tx.unbounded_send(accent).is_err() {
        return Ok(());
    }
    let mut changes = std::pin::pin!(changes);
    while let Some(color) = changes.next().await {
        if tx.unbounded_send(from_portal(color)).is_err() {
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

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use ashpd::desktop::Color;

    use super::*;

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
