//! Keep the screen awake and the computer from sleeping while a game runs,
//! through the desktop portal's Inhibit interface. KDE, GNOME and other
//! desktops implement it, Flatpak included; where nothing answers, games run
//! without it.

use std::time::Duration;

/// How long to wait for the portal before giving up.
const PORTAL_TIMEOUT: Duration = Duration::from_secs(3);

/// An active inhibition; [`Inhibitor::release`] ends it (so does Irori
/// exiting, which closes its session bus connection).
pub struct Inhibitor {
    #[cfg(target_os = "linux")]
    request: ashpd::desktop::Request<()>,
    #[cfg(target_os = "macos")]
    child: tokio::process::Child,
    #[cfg(windows)]
    _release: std::sync::mpsc::Sender<()>,
}

impl Inhibitor {
    /// Ask the desktop not to blank, lock or suspend while `reason` holds.
    pub async fn acquire(reason: &str) -> Option<Self> {
        Self::acquire_with(reason, true).await
    }

    /// Ask the computer not to sleep while `reason` holds, leaving the
    /// display to blank and lock as the desktop is set to (downloads with
    /// the screen off).
    pub async fn acquire_system(reason: &str) -> Option<Self> {
        Self::acquire_with(reason, false).await
    }

    async fn acquire_with(reason: &str, display: bool) -> Option<Self> {
        match tokio::time::timeout(PORTAL_TIMEOUT, Self::request(reason, display)).await {
            Ok(Ok(inhibitor)) => Some(inhibitor),
            Ok(Err(e)) => {
                tracing::info!("cannot keep the screen awake: {e:#}");
                None
            }
            Err(_) => {
                tracing::info!("cannot keep the screen awake: the desktop portal did not answer");
                None
            }
        }
    }

    #[cfg(target_os = "linux")]
    async fn request(reason: &str, display: bool) -> anyhow::Result<Self> {
        use ashpd::desktop::inhibit::{InhibitFlags, InhibitOptions, InhibitProxy};
        let proxy = InhibitProxy::new().await?;
        let flags = if display {
            InhibitFlags::Idle | InhibitFlags::Suspend
        } else {
            InhibitFlags::Suspend.into()
        };
        let request = proxy
            .inhibit(None, flags, InhibitOptions::default().set_reason(reason))
            .await?;
        Ok(Self { request })
    }

    #[cfg(target_os = "macos")]
    async fn request(_reason: &str, display: bool) -> anyhow::Result<Self> {
        // `-d` keeps the display awake too, `-i` the system.
        let flags: &[&str] = if display { &["-d", "-i"] } else { &["-i"] };
        let child = tokio::process::Command::new("/usr/bin/caffeinate")
            .args(flags)
            .args(["-w", &std::process::id().to_string()])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true)
            .spawn()?;
        Ok(Self { child })
    }

    #[cfg(windows)]
    async fn request(_reason: &str, display: bool) -> anyhow::Result<Self> {
        use windows_sys::Win32::System::Power::{
            ES_CONTINUOUS, ES_DISPLAY_REQUIRED, ES_SYSTEM_REQUIRED, SetThreadExecutionState,
        };
        let (release, released) = std::sync::mpsc::channel();
        let (ready, started) = tokio::sync::oneshot::channel();
        std::thread::Builder::new()
            .name("irori-awake".into())
            .spawn(move || {
                // This API is thread-scoped: hold the request and release it on the
                // same dedicated thread, never on a migrating async worker.
                let flags = if display {
                    ES_CONTINUOUS | ES_DISPLAY_REQUIRED | ES_SYSTEM_REQUIRED
                } else {
                    ES_CONTINUOUS | ES_SYSTEM_REQUIRED
                };
                // SAFETY: a plain flag word; released below on this thread.
                let ok = unsafe { SetThreadExecutionState(flags) } != 0;
                let _ = ready.send(ok);
                if ok {
                    let _ = released.recv();
                    unsafe {
                        SetThreadExecutionState(ES_CONTINUOUS);
                    }
                }
            })?;
        anyhow::ensure!(started.await?, "Windows refused the power request");
        Ok(Self { _release: release })
    }

    /// [`Inhibitor::acquire`] with an owned reason, for spawning.
    pub async fn acquire_owned(reason: String) -> Option<Self> {
        Self::acquire(&reason).await
    }

    /// End the inhibition.
    pub async fn release(self) {
        #[cfg(target_os = "macos")]
        {
            let mut child = self.child;
            let _ = child.kill().await;
            let _ = child.wait().await;
        }
        #[cfg(target_os = "linux")]
        if let Err(e) = self.request.close().await {
            tracing::debug!("ending the screen inhibition: {e:#}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Talks to the real desktop. Run with `cargo test -p irori-services --
    /// --ignored keeps_the_screen_awake` while watching the session bus
    /// (`dbus-monitor --session`): on KDE the portal turns it into
    /// `PolicyAgent.AddInhibition`, and release into `ReleaseInhibition`.
    #[tokio::test]
    #[ignore = "needs a desktop session with a portal"]
    async fn keeps_the_screen_awake() {
        let inhibitor = Inhibitor::acquire("Irori test")
            .await
            .expect("portal answered");
        inhibitor.release().await;
    }
}
