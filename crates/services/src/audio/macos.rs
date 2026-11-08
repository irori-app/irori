//! macOS: the default output's volume through `osascript`'s volume
//! settings (Standard Additions, which need no permission). macOS has no
//! per-program volume, so a game's own can't be read or changed.

use super::*;
use anyhow::{Context as _, bail};
use irori_core::process::{self, CommandSpec, RunOptions};

async fn osascript(statements: &[&str]) -> Result<String> {
    let mut spec = CommandSpec::new("/usr/bin/osascript");
    for statement in statements {
        spec = spec.arg("-e").arg(*statement);
    }
    let out = process::run(&spec, RunOptions::default()).await?;
    if !out.success() {
        bail!("osascript: {}", out.error_summary());
    }
    Ok(out.stdout)
}

async fn system() -> Result<Volume> {
    let text = osascript(&["get volume settings"]).await?;
    parse_volume_settings(&text).context("This sound output has no volume of its own")
}

pub(super) async fn audio(_game: Option<GameProcess>) -> Result<GameAudio> {
    Ok(GameAudio {
        system: Some(system().await?),
        game: None,
    })
}

pub(super) async fn change(
    _game: Option<GameProcess>,
    target: VolumeTarget,
    change: Change,
) -> Result<()> {
    if target == VolumeTarget::Game {
        bail!("macOS has no volume per program; change the system volume instead");
    }
    let now = system().await?;
    match change {
        Change::Step(step) => {
            let set = format!("set volume output volume {}", stepped(now.percent, step));
            if now.muted {
                osascript(&[&set, "set volume without output muted"]).await?;
            } else {
                osascript(&[&set]).await?;
            }
        }
        Change::ToggleMute => {
            let set = if now.muted {
                "set volume without output muted"
            } else {
                "set volume with output muted"
            };
            osascript(&[set]).await?;
        }
    }
    Ok(())
}

/// Core Audio's devices and the default output, through its documented C
/// API (`AudioObjectGetPropertyData` / `SetPropertyData`).
mod devices {
    use std::ffi::c_void;

    pub type AudioObjectId = u32;
    type OsStatus = i32;
    type CfStringRef = *const c_void;

    #[repr(C)]
    struct Address {
        selector: u32,
        scope: u32,
        element: u32,
    }

    const fn code(tag: &[u8; 4]) -> u32 {
        u32::from_be_bytes(*tag)
    }

    const SYSTEM_OBJECT: AudioObjectId = 1;
    const DEVICES: u32 = code(b"dev#");
    const DEFAULT_OUTPUT: u32 = code(b"dOut");
    const STREAMS: u32 = code(b"stm#");
    const NAME: u32 = code(b"lnam");
    const UID: u32 = code(b"uid ");
    const GLOBAL: u32 = code(b"glob");
    const OUTPUT: u32 = code(b"outp");
    const MAIN: u32 = 0;
    const UTF8: u32 = 0x0800_0100;

    #[link(name = "CoreAudio", kind = "framework")]
    unsafe extern "C" {
        fn AudioObjectGetPropertyDataSize(
            object: AudioObjectId,
            address: *const Address,
            qualifier_size: u32,
            qualifier: *const c_void,
            size: *mut u32,
        ) -> OsStatus;
        fn AudioObjectGetPropertyData(
            object: AudioObjectId,
            address: *const Address,
            qualifier_size: u32,
            qualifier: *const c_void,
            size: *mut u32,
            data: *mut c_void,
        ) -> OsStatus;
        fn AudioObjectSetPropertyData(
            object: AudioObjectId,
            address: *const Address,
            qualifier_size: u32,
            qualifier: *const c_void,
            size: u32,
            data: *const c_void,
        ) -> OsStatus;
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFStringGetLength(s: CfStringRef) -> isize;
        fn CFStringGetMaximumSizeForEncoding(length: isize, encoding: u32) -> isize;
        fn CFStringGetCString(s: CfStringRef, buffer: *mut u8, size: isize, encoding: u32) -> u8;
        fn CFRelease(object: *const c_void);
    }

    fn size_of(object: AudioObjectId, address: &Address) -> Option<u32> {
        let mut size = 0u32;
        // SAFETY: `address` and `size` are valid for the call; no qualifier.
        let status = unsafe {
            AudioObjectGetPropertyDataSize(object, address, 0, std::ptr::null(), &mut size)
        };
        (status == 0).then_some(size)
    }

    /// Every device's id.
    pub fn all() -> Vec<AudioObjectId> {
        let address = Address {
            selector: DEVICES,
            scope: GLOBAL,
            element: MAIN,
        };
        let Some(size) = size_of(SYSTEM_OBJECT, &address) else {
            return Vec::new();
        };
        let mut ids =
            vec![0 as AudioObjectId; size as usize / std::mem::size_of::<AudioObjectId>()];
        let mut size = (ids.len() * std::mem::size_of::<AudioObjectId>()) as u32;
        // SAFETY: `ids` holds `size` bytes, as Core Audio reported.
        let status = unsafe {
            AudioObjectGetPropertyData(
                SYSTEM_OBJECT,
                &address,
                0,
                std::ptr::null(),
                &mut size,
                ids.as_mut_ptr().cast(),
            )
        };
        if status != 0 {
            return Vec::new();
        }
        ids.truncate(size as usize / std::mem::size_of::<AudioObjectId>());
        ids
    }

    /// Whether `device` plays sound (has output streams).
    pub fn is_output(device: AudioObjectId) -> bool {
        let address = Address {
            selector: STREAMS,
            scope: OUTPUT,
            element: MAIN,
        };
        size_of(device, &address).is_some_and(|s| s > 0)
    }

    fn string(device: AudioObjectId, selector: u32) -> Option<String> {
        let address = Address {
            selector,
            scope: GLOBAL,
            element: MAIN,
        };
        let mut value: CfStringRef = std::ptr::null();
        let mut size = std::mem::size_of::<CfStringRef>() as u32;
        // SAFETY: `value` has room for the one CFStringRef these properties
        // hold; Core Audio returns it retained, released below.
        let status = unsafe {
            AudioObjectGetPropertyData(
                device,
                &address,
                0,
                std::ptr::null(),
                &mut size,
                (&raw mut value).cast(),
            )
        };
        if status != 0 || value.is_null() {
            return None;
        }
        // SAFETY: `value` is a live CFString; the buffer is as large as
        // CoreFoundation says the UTF-8 text may need, plus its NUL.
        let text = unsafe {
            let max = CFStringGetMaximumSizeForEncoding(CFStringGetLength(value), UTF8) + 1;
            let mut buffer = vec![0u8; max.max(1) as usize];
            let ok = CFStringGetCString(value, buffer.as_mut_ptr(), max, UTF8) != 0;
            CFRelease(value);
            ok.then(|| {
                let end = buffer.iter().position(|b| *b == 0).unwrap_or(buffer.len());
                String::from_utf8_lossy(&buffer[..end]).into_owned()
            })
        };
        text.filter(|t| !t.is_empty())
    }

    pub fn name(device: AudioObjectId) -> Option<String> {
        string(device, NAME)
    }

    pub fn uid(device: AudioObjectId) -> Option<String> {
        string(device, UID)
    }

    fn default_address() -> Address {
        Address {
            selector: DEFAULT_OUTPUT,
            scope: GLOBAL,
            element: MAIN,
        }
    }

    pub fn default_output() -> Option<AudioObjectId> {
        let mut device: AudioObjectId = 0;
        let mut size = std::mem::size_of::<AudioObjectId>() as u32;
        // SAFETY: `device` has room for the one id the property holds.
        let status = unsafe {
            AudioObjectGetPropertyData(
                SYSTEM_OBJECT,
                &default_address(),
                0,
                std::ptr::null(),
                &mut size,
                (&raw mut device).cast(),
            )
        };
        (status == 0 && device != 0).then_some(device)
    }

    pub fn set_default_output(device: AudioObjectId) -> bool {
        // SAFETY: passes one device id, the size the property takes.
        let status = unsafe {
            AudioObjectSetPropertyData(
                SYSTEM_OBJECT,
                &default_address(),
                0,
                std::ptr::null(),
                std::mem::size_of::<AudioObjectId>() as u32,
                (&raw const device).cast(),
            )
        };
        status == 0
    }
}

fn list_outputs() -> Vec<AudioOutput> {
    let current = devices::default_output();
    devices::all()
        .into_iter()
        .filter(|d| devices::is_output(*d))
        .filter_map(|d| {
            let id = devices::uid(d)?;
            Some(AudioOutput {
                name: devices::name(d).unwrap_or_else(|| id.clone()),
                id,
                current: Some(d) == current,
            })
        })
        .collect()
}

pub(super) async fn outputs() -> Result<Vec<AudioOutput>> {
    tokio::task::spawn_blocking(list_outputs)
        .await
        .context("The sound outputs couldn't be read")
}

pub(super) async fn set_output(_game: Option<GameProcess>, id: String) -> Result<()> {
    tokio::task::spawn_blocking(move || {
        let device = devices::all()
            .into_iter()
            .find(|d| devices::uid(*d).as_deref() == Some(id.as_str()))
            .context("That sound output isn't connected anymore")?;
        if !devices::set_default_output(device) {
            bail!("macOS didn't switch the sound output");
        }
        Ok(())
    })
    .await
    .context("The sound output couldn't be switched")?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_outputs_with_ids_and_at_most_one_current() {
        // Read only: CI machines may have no outputs at all.
        let outputs = list_outputs();
        assert!(
            outputs
                .iter()
                .all(|o| !o.id.is_empty() && !o.name.is_empty())
        );
        assert!(outputs.iter().filter(|o| o.current).count() <= 1);
    }
}
