//! The system's text size, which scales Irori's interface size by default:
//! GNOME's "Large text" and text scaling (`text-scaling-factor` through the
//! XDG settings portal, else `GDK_DPI_SCALE`) on Linux, and "Text size"
//! (`UISettings::TextScaleFactor`) on Windows. macOS has no system text size
//! for apps. The display's own scale needs nothing here: GPUI draws at it.
//!
//! Every source signals its changes, so nothing runs between them.

use futures::channel::mpsc;
use irori_services::Backend;

/// The factor now, then each change of it.
pub(crate) type Changes = mpsc::UnboundedReceiver<f32>;

/// The smallest and largest factor taken; anything outside is a mistake
/// rather than a wish (GNOME's own range is 0.5 to 3).
// Only Linux and Windows have a text size to read.
#[cfg_attr(
    not(any(target_os = "linux", target_os = "windows", test)),
    allow(dead_code)
)]
const RANGE: std::ops::RangeInclusive<f64> = 0.5..=3.0;

/// A factor the system gave, or 1 for one that is missing, not a number or
/// out of [`RANGE`].
// Only Linux and Windows have a text size to read.
#[cfg_attr(
    not(any(target_os = "linux", target_os = "windows", test)),
    allow(dead_code)
)]
pub(crate) fn factor(value: Option<f64>) -> f32 {
    value
        .filter(|v| v.is_finite() && RANGE.contains(v))
        .map_or(1.0, |v| v as f32)
}

/// `GDK_DPI_SCALE`, which GTK applications take as their text scale.
#[cfg_attr(not(any(target_os = "linux", test)), allow(dead_code))]
fn from_env(value: Option<&str>) -> Option<f64> {
    value?.trim().parse().ok()
}

/// Start listening, on systems that have a text size.
#[cfg(target_os = "linux")]
pub(crate) fn watch(backend: &Backend) -> Option<Changes> {
    let (tx, rx) = mpsc::unbounded();
    backend.spawn(async move {
        if let Err(e) = listen(&tx).await {
            tracing::debug!("the desktop portal's text scaling: {e}");
            let env = std::env::var("GDK_DPI_SCALE").ok();
            let _ = tx.unbounded_send(factor(from_env(env.as_deref())));
        }
    });
    Some(rx)
}

/// Start listening. The first factor is sent before this returns, so the
/// first window has it.
#[cfg(target_os = "windows")]
pub(crate) fn watch(_backend: &Backend) -> Option<Changes> {
    let (tx, rx) = mpsc::unbounded();
    if let Err(e) = win::listen(tx) {
        tracing::debug!("Windows' text size: {e}");
    }
    Some(rx)
}

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
pub(crate) fn watch(_backend: &Backend) -> Option<Changes> {
    None
}

#[cfg(target_os = "linux")]
async fn listen(tx: &mpsc::UnboundedSender<f32>) -> ashpd::Result<()> {
    use ashpd::desktop::settings::Settings;
    use futures::StreamExt as _;

    const NAMESPACE: &str = "org.gnome.desktop.interface";
    const KEY: &str = "text-scaling-factor";
    let settings = Settings::new().await?;
    // Subscribing first means a change during the read can't be missed.
    let changes = settings
        .receive_setting_changed_with_args::<f64>(NAMESPACE, KEY)
        .await?;
    // Desktops other than GNOME's have no such key; GTK's variable may say.
    let first = match settings.read::<f64>(NAMESPACE, KEY).await {
        Ok(v) => Some(v),
        Err(_) => from_env(std::env::var("GDK_DPI_SCALE").ok().as_deref()),
    };
    if tx.unbounded_send(factor(first)).is_err() {
        return Ok(());
    }
    let mut changes = std::pin::pin!(changes);
    while let Some(value) = changes.next().await {
        if tx.unbounded_send(factor(value.ok())).is_err() {
            break;
        }
    }
    Ok(())
}

#[cfg(target_os = "windows")]
mod win {
    use futures::channel::mpsc::UnboundedSender;
    use windows::Foundation::TypedEventHandler;
    use windows::UI::ViewManagement::UISettings;
    use windows::core::IInspectable;

    use super::factor;

    /// Send the factor now and after every change, for as long as Irori runs.
    pub(super) fn listen(tx: UnboundedSender<f32>) -> windows::core::Result<()> {
        let ui = UISettings::new()?;
        let send = {
            let (ui, tx) = (ui.clone(), tx.clone());
            move || {
                let _ = tx.unbounded_send(factor(ui.TextScaleFactor().ok()));
            }
        };
        // Subscribing first means a change during the read can't be missed.
        let on_change = send.clone();
        ui.TextScaleFactorChanged(&TypedEventHandler::<UISettings, IInspectable>::new(
            move |_, _| {
                on_change();
                Ok(())
            },
        ))?;
        send();
        // The handler lasts as long as the object does, and Irori listens
        // until it exits.
        std::mem::forget(ui);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn factors_outside_the_range_mean_none() {
        assert_eq!(factor(Some(1.25)), 1.25);
        assert_eq!(factor(Some(0.5)), 0.5);
        assert_eq!(factor(Some(3.0)), 3.0);
        assert_eq!(factor(None), 1.0);
        assert_eq!(factor(Some(0.0)), 1.0);
        assert_eq!(factor(Some(12.0)), 1.0);
        assert_eq!(factor(Some(f64::NAN)), 1.0);
    }

    #[test]
    fn reads_gtks_variable() {
        assert_eq!(from_env(Some("1.5")), Some(1.5));
        assert_eq!(from_env(Some(" 0.8 ")), Some(0.8));
        assert_eq!(from_env(Some("big")), None);
        assert_eq!(from_env(None), None);
    }
}
