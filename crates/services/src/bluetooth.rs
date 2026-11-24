//! Turning a Bluetooth controller off from the in-game overlay.
//!
//! Only Linux offers it to a program without extra rights: BlueZ's
//! `org.bluez.Device1.Disconnect` on the system bus, which BlueZ's D-Bus
//! policy allows the desktop's own user. A controller let go that way
//! switches itself off (or goes back to pairing mode) the way it does when
//! the system sleeps. Windows and macOS keep disconnecting to their own
//! settings, so Irori doesn't offer it there ([`DISCONNECTS`]).
//!
//! A controller's Bluetooth address comes from the kernel's input device
//! (`/sys/class/input/eventN/device`): its bus is Bluetooth (`id/bustype`
//! 0005) and `uniq` holds the address ([`address_from_sysfs`]).

use anyhow::Result;

use crate::Backend;

/// Whether this system lets Irori disconnect a Bluetooth controller.
pub const DISCONNECTS: bool = cfg!(target_os = "linux");

/// The kernel's bus number for Bluetooth (`BUS_BLUETOOTH`).
const BUS_BLUETOOTH: u16 = 0x0005;

/// The Bluetooth address of an input device from its sysfs `id/bustype`
/// and `uniq` files: `uniq` written as BlueZ writes addresses (upper case,
/// colons), and only for a device on the Bluetooth bus.
pub fn address_from_sysfs(bustype: &str, uniq: &str) -> Option<String> {
    let bus = u16::from_str_radix(bustype.trim(), 16).ok()?;
    if bus != BUS_BLUETOOTH {
        return None;
    }
    let parts: Vec<&str> = uniq.trim().split(':').collect();
    let valid = parts.len() == 6
        && parts
            .iter()
            .all(|p| p.len() == 2 && p.chars().all(|c| c.is_ascii_hexdigit()));
    valid.then(|| parts.join(":").to_ascii_uppercase())
}

/// The Bluetooth address of the controller read from `event_device`
/// (`/dev/input/eventN`); `None` for one on USB, or when the system says
/// nothing. Reads sysfs: call it off the UI thread.
pub fn address_of(event_device: &std::path::Path) -> Option<String> {
    if !DISCONNECTS {
        return None;
    }
    let name = event_device.file_name()?;
    let device = std::path::Path::new("/sys/class/input")
        .join(name)
        .join("device");
    let bustype = std::fs::read_to_string(device.join("id/bustype")).ok()?;
    let uniq = std::fs::read_to_string(device.join("uniq")).ok()?;
    address_from_sysfs(&bustype, &uniq)
}

impl Backend {
    /// Disconnect the Bluetooth device at `address` (as [`address_of`]
    /// gives it), which turns a controller off.
    pub async fn disconnect_bluetooth(&self, address: &str) -> Result<()> {
        platform::disconnect(address).await
    }
}

#[cfg(target_os = "linux")]
mod platform {
    use std::collections::HashMap;

    use anyhow::{Context as _, Result, bail};
    use ashpd::zbus::zvariant::{OwnedObjectPath, OwnedValue};
    use ashpd::zbus::{Connection, Proxy};

    type Objects = HashMap<OwnedObjectPath, HashMap<String, HashMap<String, OwnedValue>>>;

    pub(super) async fn disconnect(address: &str) -> Result<()> {
        let system = Connection::system()
            .await
            .context("The system bus can't be reached")?;
        let manager = Proxy::new(
            &system,
            "org.bluez",
            "/",
            "org.freedesktop.DBus.ObjectManager",
        )
        .await
        .context("Bluetooth (BlueZ) isn't running")?;
        let objects: Objects = manager
            .call("GetManagedObjects", &())
            .await
            .context("Bluetooth (BlueZ) didn't answer")?;
        let path = objects.into_iter().find_map(|(path, interfaces)| {
            let device = interfaces.get("org.bluez.Device1")?;
            let found: String = device.get("Address")?.try_clone().ok()?.try_into().ok()?;
            found.eq_ignore_ascii_case(address).then_some(path)
        });
        let Some(path) = path else {
            bail!("This controller isn't connected over Bluetooth anymore");
        };
        let device = Proxy::new(&system, "org.bluez", path, "org.bluez.Device1").await?;
        device
            .call::<_, _, ()>("Disconnect", &())
            .await
            .context("Bluetooth didn't let the controller go")?;
        Ok(())
    }
}

#[cfg(not(target_os = "linux"))]
mod platform {
    use anyhow::{Result, bail};

    pub(super) async fn disconnect(_address: &str) -> Result<()> {
        bail!("Turn the controller off in the system's Bluetooth settings")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_address_of_bluetooth_controllers_only() {
        assert_eq!(
            address_from_sysfs("0005\n", "aa:bb:cc:dd:ee:0f\n").as_deref(),
            Some("AA:BB:CC:DD:EE:0F")
        );
        // USB (0003) has a serial number or nothing in `uniq`.
        assert_eq!(address_from_sysfs("0003", "aa:bb:cc:dd:ee:ff"), None);
        assert_eq!(address_from_sysfs("0005", ""), None);
        assert_eq!(address_from_sysfs("0005", "not-an-address"), None);
        assert_eq!(address_from_sysfs("0005", "aa:bb:cc:dd:ee"), None);
        assert_eq!(address_from_sysfs("zz", "aa:bb:cc:dd:ee:ff"), None);
    }
}
