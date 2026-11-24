//! The Windows side of hiding a graphics card: the display adapters
//! (Configuration Manager), which of them show a display
//! (`EnumDisplayDevicesW`), and the elevated helper that disables one.
//!
//! The helper is Irori itself, started through `ShellExecuteExW` with the
//! `runas` verb, so Windows shows its own permission prompt. It talks to the
//! Irori that started it through two named events in this session's
//! namespace: it sets `…-ready` once the card is disabled, and enables it
//! again when `…-release` is set or the starting Irori's process ends.
//! `CM_Disable_DevNode` without `CM_DISABLE_PERSIST` lasts until the next
//! restart at most.

use std::sync::atomic::{AtomicU32, Ordering};

use anyhow::{Context as _, Result, anyhow, bail};
use windows_sys::Win32::Devices::DeviceAndDriverInstallation::{
    CM_DISABLE_UI_NOT_OK, CM_DRP_DEVICEDESC, CM_Disable_DevNode, CM_Enable_DevNode,
    CM_GETIDLIST_FILTER_CLASS, CM_GETIDLIST_FILTER_PRESENT, CM_Get_DevNode_Registry_PropertyW,
    CM_Get_DevNode_Status, CM_Get_Device_ID_List_SizeW, CM_Get_Device_ID_ListW,
    CM_LOCATE_DEVNODE_NORMAL, CM_Locate_DevNodeW, CM_PROB_DISABLED, CR_SUCCESS,
};
use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_CANCELLED, GetLastError, HANDLE, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows_sys::Win32::Graphics::Gdi::{
    DISPLAY_DEVICE_ATTACHED_TO_DESKTOP, DISPLAY_DEVICE_MIRRORING_DRIVER, DISPLAY_DEVICEW,
    EnumDisplayDevicesW,
};
use windows_sys::Win32::System::Threading::{
    CreateEventW, EVENT_MODIFY_STATE, GetExitCodeProcess, INFINITE, OpenEventW, OpenProcess,
    PROCESS_SYNCHRONIZE, SYNCHRONIZATION_SYNCHRONIZE, SetEvent, WaitForMultipleObjects,
    WaitForSingleObject,
};
use windows_sys::Win32::UI::Shell::{SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, ShellExecuteExW};
use windows_sys::Win32::UI::WindowsAndMessaging::SW_HIDE;

use super::{Adapter, GpuDevices, HELPER_FLAG, Held, shows_display, valid_instance_id};

/// The display adapter class (`GUID_DEVCLASS_DISPLAY`), as Configuration
/// Manager's class filter takes it.
const DISPLAY_CLASS: &str = "{4d36e968-e325-11ce-bfc1-08002be10318}";
/// How long the user has to answer the permission prompt and the helper to
/// disable the card.
const READY_WITHIN_MS: u32 = 120_000;
/// How long the helper has to enable the card again.
const RELEASE_WITHIN_MS: u32 = 30_000;

/// The helper's exit codes, read back by [`exit_reason`].
mod exit {
    pub const OK: i32 = 0;
    pub const BAD_ARGUMENTS: i32 = 2;
    pub const ALREADY_DISABLED: i32 = 3;
    pub const DISABLE_FAILED: i32 = 4;
    pub const ENABLE_FAILED: i32 = 5;
    pub const NOT_FOUND: i32 = 6;
    pub const NO_CHANNEL: i32 = 7;
}

fn exit_reason(code: u32) -> &'static str {
    match code as i32 {
        exit::ALREADY_DISABLED => "it was already disabled, so it was left alone",
        exit::DISABLE_FAILED => "Windows wouldn't disable it (it may be in use)",
        exit::ENABLE_FAILED => "Windows wouldn't enable it again",
        exit::NOT_FOUND => "Windows doesn't list it any more",
        exit::NO_CHANNEL => "the helper couldn't reach Irori",
        _ => "the helper stopped unexpectedly",
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn from_wide(buf: &[u16]) -> String {
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..end])
}

/// A handle closed when dropped.
struct Owned(HANDLE);

impl Drop for Owned {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: the handle came from a Windows call that returned it
            // open, and nothing else closes it.
            unsafe { CloseHandle(self.0) };
        }
    }
}

// SAFETY: kernel handles may be used from any thread.
unsafe impl Send for Owned {}

/// The device node of `instance_id`.
fn locate(instance_id: &str) -> Option<u32> {
    let id = wide(instance_id);
    let mut devinst = 0u32;
    // SAFETY: `id` is NUL-terminated and outlives the call.
    let cr = unsafe { CM_Locate_DevNodeW(&mut devinst, id.as_ptr(), CM_LOCATE_DEVNODE_NORMAL) };
    (cr == CR_SUCCESS).then_some(devinst)
}

/// Whether the device node is disabled (problem code 22).
fn is_disabled(devinst: u32) -> bool {
    let (mut status, mut problem) = (0u32, 0u32);
    // SAFETY: both outputs point at live locals.
    let cr = unsafe { CM_Get_DevNode_Status(&mut status, &mut problem, devinst, 0) };
    cr == CR_SUCCESS && problem == CM_PROB_DISABLED
}

fn description(devinst: u32) -> Option<String> {
    let mut buf = [0u16; 256];
    let mut len = (buf.len() * 2) as u32;
    // SAFETY: `buf` holds `len` bytes.
    let cr = unsafe {
        CM_Get_DevNode_Registry_PropertyW(
            devinst,
            CM_DRP_DEVICEDESC,
            std::ptr::null_mut(),
            buf.as_mut_ptr().cast(),
            &mut len,
            0,
        )
    };
    (cr == CR_SUCCESS)
        .then(|| from_wide(&buf))
        .filter(|s| !s.is_empty())
}

/// The present display adapters' instance ids.
fn adapter_ids() -> Result<Vec<String>> {
    let filter = wide(DISPLAY_CLASS);
    let flags = CM_GETIDLIST_FILTER_CLASS | CM_GETIDLIST_FILTER_PRESENT;
    let mut len = 0u32;
    // SAFETY: `filter` is NUL-terminated; `len` is a live local.
    let cr = unsafe { CM_Get_Device_ID_List_SizeW(&mut len, filter.as_ptr(), flags) };
    if cr != CR_SUCCESS {
        bail!("Windows didn't list the graphics cards (CONFIGRET {cr})");
    }
    let mut buf = vec![0u16; len as usize + 1];
    // SAFETY: `buf` holds `len` characters.
    let cr = unsafe { CM_Get_Device_ID_ListW(filter.as_ptr(), buf.as_mut_ptr(), len, flags) };
    if cr != CR_SUCCESS {
        bail!("Windows didn't list the graphics cards (CONFIGRET {cr})");
    }
    // A list of NUL-terminated strings, ending with an empty one.
    Ok(buf
        .split(|&c| c == 0)
        .map(String::from_utf16_lossy)
        .filter(|s| !s.is_empty())
        .collect())
}

/// The hardware ids of the adapters showing a display of the desktop, or
/// `None` when one of them didn't say which it is.
fn display_hardware_ids() -> Option<Vec<String>> {
    let mut ids = Vec::new();
    for index in 0.. {
        let mut device = DISPLAY_DEVICEW {
            cb: size_of::<DISPLAY_DEVICEW>() as u32,
            ..Default::default()
        };
        // SAFETY: `device` is sized in `cb`; a null device lists adapters.
        if unsafe { EnumDisplayDevicesW(std::ptr::null(), index, &mut device, 0) } == 0 {
            break;
        }
        if device.StateFlags & DISPLAY_DEVICE_MIRRORING_DRIVER != 0
            || device.StateFlags & DISPLAY_DEVICE_ATTACHED_TO_DESKTOP == 0
        {
            continue;
        }
        let id = from_wide(&device.DeviceID);
        if id.trim().is_empty() {
            return None;
        }
        ids.push(id);
    }
    (!ids.is_empty()).then_some(ids)
}

pub(super) struct SystemDevices;

impl GpuDevices for SystemDevices {
    fn adapters(&self) -> Result<Vec<Adapter>> {
        let shown = display_hardware_ids();
        Ok(adapter_ids()?
            .into_iter()
            .filter_map(|instance_id| {
                let devinst = locate(&instance_id)?;
                Some(Adapter {
                    name: description(devinst).unwrap_or_else(|| instance_id.clone()),
                    disabled: is_disabled(devinst),
                    drives_display: shown.as_ref().map(|ids| shows_display(&instance_id, ids)),
                    instance_id,
                })
            })
            .collect())
    }

    fn hide(&self, instance_id: &str) -> Result<Box<dyn Held>> {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let channel = format!(
            r"Local\irori-gpu-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        );
        let ready = create_event(&format!("{channel}-ready"))?;
        let release = create_event(&format!("{channel}-release"))?;
        let process = run_elevated(&[
            "hide",
            instance_id,
            &channel,
            &std::process::id().to_string(),
        ])?;
        let handles = [ready.0, process.0];
        // SAFETY: both handles are open for the call.
        let woke = unsafe { WaitForMultipleObjects(2, handles.as_ptr(), 0, READY_WITHIN_MS) };
        if woke == WAIT_OBJECT_0 {
            return Ok(Box::new(Hold {
                process,
                release,
                _ready: ready,
            }));
        }
        if woke == WAIT_OBJECT_0 + 1 {
            bail!("{}", exit_reason(exit_code(&process).unwrap_or(1)));
        }
        // Nothing heard: tell it to put everything back, whatever it did.
        // SAFETY: `release` is open.
        unsafe { SetEvent(release.0) };
        if woke == WAIT_TIMEOUT {
            bail!("the helper didn't disable the card in time");
        }
        bail!("waiting for the helper failed")
    }

    fn enable(&self, instance_id: &str) -> Result<()> {
        let process = run_elevated(&["enable", instance_id])?;
        // SAFETY: `process` is open.
        unsafe { WaitForSingleObject(process.0, RELEASE_WITHIN_MS) };
        match exit_code(&process) {
            Some(0) => Ok(()),
            Some(code) => bail!("{}", exit_reason(code)),
            None => bail!("the helper didn't finish in time"),
        }
    }
}

struct Hold {
    process: Owned,
    release: Owned,
    _ready: Owned,
}

impl Held for Hold {
    fn release(self: Box<Self>) -> Result<()> {
        // SAFETY: both handles are open.
        unsafe {
            SetEvent(self.release.0);
            WaitForSingleObject(self.process.0, RELEASE_WITHIN_MS);
        }
        match exit_code(&self.process) {
            Some(0) => Ok(()),
            Some(code) => bail!("{}", exit_reason(code)),
            None => bail!("the helper didn't enable the card in time"),
        }
    }
}

fn create_event(name: &str) -> Result<Owned> {
    let name = wide(name);
    // SAFETY: `name` is NUL-terminated; default security: this user only.
    let handle = unsafe { CreateEventW(std::ptr::null(), 1, 0, name.as_ptr()) };
    if handle.is_null() {
        bail!(
            "Could not prepare the helper ({})",
            std::io::Error::last_os_error()
        );
    }
    Ok(Owned(handle))
}

/// The process's exit code, `None` while it runs.
fn exit_code(process: &Owned) -> Option<u32> {
    const STILL_ACTIVE: u32 = 259;
    let mut code = 0u32;
    // SAFETY: `process` is open; `code` is a live local.
    let ok = unsafe { GetExitCodeProcess(process.0, &mut code) } != 0;
    (ok && code != STILL_ACTIVE).then_some(code)
}

/// Start `irori --gpu-helper <args>` as administrator: Windows asks the user.
fn run_elevated(args: &[&str]) -> Result<Owned> {
    for arg in args {
        // Every argument is a word, an instance id, an event name or a
        // number: none needs quoting beyond the quotes below.
        if arg.contains('"') {
            bail!("unexpected argument for the helper");
        }
    }
    let exe = std::env::current_exe().context("Could not find Irori's program")?;
    let parameters: String = std::iter::once(HELPER_FLAG.to_string())
        .chain(args.iter().map(|a| format!("\"{a}\"")))
        .collect::<Vec<_>>()
        .join(" ");
    let (verb, file, parameters) = (
        wide("runas"),
        wide(&exe.to_string_lossy()),
        wide(&parameters),
    );
    let mut info = SHELLEXECUTEINFOW {
        cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS,
        lpVerb: verb.as_ptr(),
        lpFile: file.as_ptr(),
        lpParameters: parameters.as_ptr(),
        nShow: SW_HIDE,
        ..Default::default()
    };
    // SAFETY: every string is NUL-terminated and outlives the call.
    if unsafe { ShellExecuteExW(&mut info) } == 0 {
        // SAFETY: no call in between.
        let error = unsafe { GetLastError() };
        if error == ERROR_CANCELLED {
            bail!("Windows' permission prompt was declined");
        }
        return Err(anyhow!(std::io::Error::from_raw_os_error(error as i32)))
            .context("Could not start the helper");
    }
    if info.hProcess.is_null() {
        bail!("Could not start the helper");
    }
    Ok(Owned(info.hProcess))
}

/// `irori --gpu-helper hide <instance id> <channel> <pid>` or
/// `irori --gpu-helper enable <instance id>`, run as administrator. Returns
/// the exit code.
pub fn helper_main(args: impl Iterator<Item = String>) -> i32 {
    let args: Vec<String> = args.collect();
    match args.iter().map(String::as_str).collect::<Vec<_>>()[..] {
        ["enable", id] if valid_instance_id(id) => match locate(id) {
            None => exit::NOT_FOUND,
            Some(devinst) => enable(devinst),
        },
        ["hide", id, channel, pid]
            if valid_instance_id(id) && channel.starts_with(r"Local\irori-gpu-") =>
        {
            let Ok(pid) = pid.parse::<u32>() else {
                return exit::BAD_ARGUMENTS;
            };
            hide_until_released(id, channel, pid)
        }
        _ => exit::BAD_ARGUMENTS,
    }
}

fn enable(devinst: u32) -> i32 {
    // A card busy finishing its removal can refuse once.
    for _ in 0..3 {
        // SAFETY: plain call on a located device node.
        if unsafe { CM_Enable_DevNode(devinst, 0) } == CR_SUCCESS {
            return exit::OK;
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
    exit::ENABLE_FAILED
}

fn hide_until_released(id: &str, channel: &str, pid: u32) -> i32 {
    let open_event = |suffix: &str| {
        let name = wide(&format!("{channel}-{suffix}"));
        // SAFETY: `name` is NUL-terminated.
        Owned(unsafe {
            OpenEventW(
                EVENT_MODIFY_STATE | SYNCHRONIZATION_SYNCHRONIZE,
                0,
                name.as_ptr(),
            )
        })
    };
    let (ready, release) = (open_event("ready"), open_event("release"));
    // SAFETY: plain call; a null handle is checked below.
    let parent = Owned(unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) });
    if ready.0.is_null() || release.0.is_null() || parent.0.is_null() {
        return exit::NO_CHANNEL;
    }
    let Some(devinst) = locate(id) else {
        return exit::NOT_FOUND;
    };
    // Checked again here: the user may have disabled it since Irori looked.
    if is_disabled(devinst) {
        return exit::ALREADY_DISABLED;
    }
    // SAFETY: plain call on a located device node.
    if unsafe { CM_Disable_DevNode(devinst, CM_DISABLE_UI_NOT_OK) } != CR_SUCCESS {
        return exit::DISABLE_FAILED;
    }
    // SAFETY: `ready` is open.
    unsafe { SetEvent(ready.0) };
    let handles = [release.0, parent.0];
    // Until Irori says the session is over, or Irori is gone.
    // SAFETY: both handles are open.
    unsafe { WaitForMultipleObjects(2, handles.as_ptr(), 0, INFINITE) };
    enable(devinst)
}
