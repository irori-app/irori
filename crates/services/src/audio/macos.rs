//! macOS: the default output's volume and the microphone's level through
//! `osascript`'s volume
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

/// The level a microphone muted by Irori had, for unmuting one whose device
/// has no mute switch of its own (its level went to 0 instead).
static MUTED_LEVEL: parking_lot::Mutex<Option<u32>> = parking_lot::Mutex::new(None);

/// The default microphone: its level from `osascript`, muted when its
/// device says so (Core Audio), else when its level is 0.
async fn microphone(settings: &str) -> Option<Volume> {
    let percent = parse_input_volume(settings)?;
    let muted = tokio::task::spawn_blocking(devices::input_muted)
        .await
        .ok()
        .flatten()
        .unwrap_or(percent == 0);
    Some(Volume { percent, muted })
}

pub(super) async fn audio(_game: Option<GameProcess>) -> Result<GameAudio> {
    let text = osascript(&["get volume settings"]).await?;
    let system =
        parse_volume_settings(&text).context("This sound output has no volume of its own")?;
    Ok(GameAudio {
        system: Some(system),
        game: None,
        microphone: microphone(&text).await,
    })
}

async fn change_microphone(change: Change) -> Result<()> {
    let text = osascript(&["get volume settings"]).await?;
    let now = microphone(&text)
        .await
        .context("There is no microphone to change")?;
    let set_level = |level: u32| format!("set volume input volume {level}");
    match change {
        Change::Step(step) => {
            osascript(&[&set_level(stepped(now.percent, step))]).await?;
            if now.muted {
                let _ = tokio::task::spawn_blocking(|| devices::set_input_muted(false)).await;
            }
        }
        Change::ToggleMute => {
            let switched =
                tokio::task::spawn_blocking(move || devices::set_input_muted(!now.muted))
                    .await
                    .unwrap_or(false);
            if !switched {
                // A device without a mute switch: its level stands in.
                if now.muted {
                    let level = MUTED_LEVEL.lock().take().unwrap_or(50);
                    osascript(&[&set_level(level)]).await?;
                } else {
                    *MUTED_LEVEL.lock() = Some(now.percent);
                    osascript(&[&set_level(0)]).await?;
                }
            }
        }
    }
    Ok(())
}

pub(super) async fn change(
    _game: Option<GameProcess>,
    target: VolumeTarget,
    change: Change,
) -> Result<()> {
    match target {
        VolumeTarget::Game => {
            bail!("macOS has no volume per program; change the system volume instead")
        }
        VolumeTarget::Microphone => return change_microphone(change).await,
        VolumeTarget::System => {}
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
    const INPUT: u32 = code(b"inpt");
    const DEFAULT_INPUT: u32 = code(b"dIn ");
    const MUTE: u32 = code(b"mute");
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

    /// Whether sound goes out of a device or into it.
    #[derive(Clone, Copy)]
    pub enum Flow {
        Output,
        Input,
    }

    impl Flow {
        fn scope(self) -> u32 {
            match self {
                Flow::Output => OUTPUT,
                Flow::Input => INPUT,
            }
        }

        fn default_selector(self) -> u32 {
            match self {
                Flow::Output => DEFAULT_OUTPUT,
                Flow::Input => DEFAULT_INPUT,
            }
        }
    }

    /// Whether `device` plays (`Output`) or records (`Input`) sound: it has
    /// streams that way.
    pub fn has_streams(device: AudioObjectId, flow: Flow) -> bool {
        let address = Address {
            selector: STREAMS,
            scope: flow.scope(),
            element: MAIN,
        };
        size_of(device, &address).is_some_and(|s| s > 0)
    }

    fn mute_address() -> Address {
        Address {
            selector: MUTE,
            scope: INPUT,
            element: MAIN,
        }
    }

    /// Whether the default microphone is muted; `None` when it has no mute
    /// switch.
    pub fn input_muted() -> Option<bool> {
        let device = default_device(Flow::Input)?;
        let mut muted: u32 = 0;
        let mut size = std::mem::size_of::<u32>() as u32;
        // SAFETY: `muted` has room for the one UInt32 the property holds.
        let status = unsafe {
            AudioObjectGetPropertyData(
                device,
                &mute_address(),
                0,
                std::ptr::null(),
                &mut size,
                (&raw mut muted).cast(),
            )
        };
        (status == 0).then_some(muted != 0)
    }

    /// Mute or unmute the default microphone; false when it has no mute
    /// switch (or there is no microphone).
    pub fn set_input_muted(muted: bool) -> bool {
        let Some(device) = default_device(Flow::Input) else {
            return false;
        };
        let value = u32::from(muted);
        // SAFETY: passes one UInt32, the size the property takes.
        let status = unsafe {
            AudioObjectSetPropertyData(
                device,
                &mute_address(),
                0,
                std::ptr::null(),
                std::mem::size_of::<u32>() as u32,
                (&raw const value).cast(),
            )
        };
        status == 0
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

    fn default_address(flow: Flow) -> Address {
        Address {
            selector: flow.default_selector(),
            scope: GLOBAL,
            element: MAIN,
        }
    }

    pub fn default_device(flow: Flow) -> Option<AudioObjectId> {
        let mut device: AudioObjectId = 0;
        let mut size = std::mem::size_of::<AudioObjectId>() as u32;
        // SAFETY: `device` has room for the one id the property holds.
        let status = unsafe {
            AudioObjectGetPropertyData(
                SYSTEM_OBJECT,
                &default_address(flow),
                0,
                std::ptr::null(),
                &mut size,
                (&raw mut device).cast(),
            )
        };
        (status == 0 && device != 0).then_some(device)
    }

    pub fn set_default_device(flow: Flow, device: AudioObjectId) -> bool {
        // SAFETY: passes one device id, the size the property takes.
        let status = unsafe {
            AudioObjectSetPropertyData(
                SYSTEM_OBJECT,
                &default_address(flow),
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
    list(devices::Flow::Output)
}

fn list(flow: devices::Flow) -> Vec<AudioOutput> {
    let current = devices::default_device(flow);
    devices::all()
        .into_iter()
        .filter(|d| devices::has_streams(*d, flow))
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
        if !devices::set_default_device(devices::Flow::Output, device) {
            bail!("macOS didn't switch the sound output");
        }
        Ok(())
    })
    .await
    .context("The sound output couldn't be switched")?
}

pub(super) async fn inputs() -> Result<Vec<AudioOutput>> {
    tokio::task::spawn_blocking(|| list(devices::Flow::Input))
        .await
        .context("The microphones couldn't be read")
}

pub(super) async fn set_input(id: String) -> Result<()> {
    tokio::task::spawn_blocking(move || {
        let device = devices::all()
            .into_iter()
            .find(|d| devices::uid(*d).as_deref() == Some(id.as_str()))
            .context("That microphone isn't connected anymore")?;
        if !devices::set_default_device(devices::Flow::Input, device) {
            bail!("macOS didn't switch the microphone");
        }
        Ok(())
    })
    .await
    .context("The microphone couldn't be switched")?
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
