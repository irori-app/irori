//! Windows' displays: the active display paths (`QueryDisplayConfig`), each
//! monitor's name and advanced colour (HDR) state, and the modes its GDI
//! device offers (`EnumDisplaySettingsW`).

use std::mem::size_of;

use windows_sys::Win32::Devices::Display::{
    DISPLAYCONFIG_DEVICE_INFO_GET_ADVANCED_COLOR_INFO, DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME,
    DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME, DISPLAYCONFIG_GET_ADVANCED_COLOR_INFO,
    DISPLAYCONFIG_MODE_INFO, DISPLAYCONFIG_PATH_INFO, DISPLAYCONFIG_SOURCE_DEVICE_NAME,
    DISPLAYCONFIG_TARGET_DEVICE_NAME, DisplayConfigGetDeviceInfo, GetDisplayConfigBufferSizes,
    QDC_ONLY_ACTIVE_PATHS, QueryDisplayConfig,
};
use windows_sys::Win32::Foundation::{ERROR_INSUFFICIENT_BUFFER, ERROR_SUCCESS};
use windows_sys::Win32::Graphics::Gdi::{
    DEVMODEW, DISPLAY_DEVICE_PRIMARY_DEVICE, DISPLAY_DEVICEW, ENUM_CURRENT_SETTINGS,
    EnumDisplayDevicesW, EnumDisplaySettingsW,
};

use super::{Display, DisplayMode, DisplayReport, parse};

pub(super) async fn probe() -> DisplayReport {
    let displays = tokio::task::spawn_blocking(read).await.unwrap_or_default();
    DisplayReport::new("DisplayConfig", displays, true)
}

fn read() -> Vec<Display> {
    let Some(paths) = active_paths() else {
        return Vec::new();
    };
    let primary = primary_device();
    paths
        .iter()
        .map(|path| {
            let gdi = source_name(path);
            let (capable, on) = match advanced_color(path) {
                Some((capable, on)) => (Some(capable), Some(on)),
                None => (None, None),
            };
            let rate = path.targetInfo.refreshRate;
            let current = gdi.as_deref().and_then(current_mode).map(|m| DisplayMode {
                // The path's rate is exact (59.94), the device's whole hertz.
                refresh_mhz: parse::rational(rate.Numerator, rate.Denominator).or(m.refresh_mhz),
                ..m
            });
            Display {
                connector: gdi.clone().unwrap_or_default(),
                name: target_name(path),
                primary: gdi.is_some() && gdi == primary,
                enabled: true,
                current,
                modes: gdi.as_deref().map(modes).unwrap_or_default(),
                hdr_on: on,
                hdr_capable: capable,
            }
        })
        .collect()
}

/// The active paths: one per display showing a picture.
fn active_paths() -> Option<Vec<DISPLAYCONFIG_PATH_INFO>> {
    // The configuration can change between asking for the sizes and the
    // paths; Windows then says the buffers are too small.
    for _ in 0..3 {
        let (mut path_count, mut mode_count) = (0u32, 0u32);
        // SAFETY: both pointers are to live `u32`s.
        let sized = unsafe {
            GetDisplayConfigBufferSizes(QDC_ONLY_ACTIVE_PATHS, &mut path_count, &mut mode_count)
        };
        if sized != ERROR_SUCCESS {
            return None;
        }
        let mut paths = vec![DISPLAYCONFIG_PATH_INFO::default(); path_count as usize];
        let mut modes = vec![DISPLAYCONFIG_MODE_INFO::default(); mode_count as usize];
        // SAFETY: the arrays hold as many elements as the counts passed with
        // them; the topology may be null for `QDC_ONLY_ACTIVE_PATHS`.
        let result = unsafe {
            QueryDisplayConfig(
                QDC_ONLY_ACTIVE_PATHS,
                &mut path_count,
                paths.as_mut_ptr(),
                &mut mode_count,
                modes.as_mut_ptr(),
                std::ptr::null_mut(),
            )
        };
        if result == ERROR_SUCCESS {
            paths.truncate(path_count as usize);
            return Some(paths);
        }
        if result != ERROR_INSUFFICIENT_BUFFER {
            return None;
        }
    }
    None
}

/// Text in a fixed UTF-16 buffer, up to its first NUL.
fn wide(buffer: &[u16]) -> Option<String> {
    let end = buffer.iter().position(|c| *c == 0).unwrap_or(buffer.len());
    let text = String::from_utf16_lossy(&buffer[..end]);
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_string())
}

/// The monitor's friendly name ("DELL U2720Q").
fn target_name(path: &DISPLAYCONFIG_PATH_INFO) -> Option<String> {
    let mut name = DISPLAYCONFIG_TARGET_DEVICE_NAME::default();
    name.header.r#type = DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME;
    name.header.size = size_of::<DISPLAYCONFIG_TARGET_DEVICE_NAME>() as u32;
    name.header.adapterId = path.targetInfo.adapterId;
    name.header.id = path.targetInfo.id;
    // SAFETY: the header begins a `DISPLAYCONFIG_TARGET_DEVICE_NAME` whose
    // size it states, as the request type requires.
    if unsafe { DisplayConfigGetDeviceInfo(&mut name.header) } != 0 {
        return None;
    }
    wide(&name.monitorFriendlyDeviceName)
}

/// The GDI device showing the path (`\\.\DISPLAY1`).
fn source_name(path: &DISPLAYCONFIG_PATH_INFO) -> Option<String> {
    let mut name = DISPLAYCONFIG_SOURCE_DEVICE_NAME::default();
    name.header.r#type = DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME;
    name.header.size = size_of::<DISPLAYCONFIG_SOURCE_DEVICE_NAME>() as u32;
    name.header.adapterId = path.sourceInfo.adapterId;
    name.header.id = path.sourceInfo.id;
    // SAFETY: the header begins a `DISPLAYCONFIG_SOURCE_DEVICE_NAME` whose
    // size it states.
    if unsafe { DisplayConfigGetDeviceInfo(&mut name.header) } != 0 {
        return None;
    }
    wide(&name.viewGdiDeviceName)
}

/// Whether the display can show HDR, and whether it is on. Windows before
/// 10 1709 doesn't answer.
fn advanced_color(path: &DISPLAYCONFIG_PATH_INFO) -> Option<(bool, bool)> {
    let mut info = DISPLAYCONFIG_GET_ADVANCED_COLOR_INFO::default();
    info.header.r#type = DISPLAYCONFIG_DEVICE_INFO_GET_ADVANCED_COLOR_INFO;
    info.header.size = size_of::<DISPLAYCONFIG_GET_ADVANCED_COLOR_INFO>() as u32;
    info.header.adapterId = path.targetInfo.adapterId;
    info.header.id = path.targetInfo.id;
    // SAFETY: the header begins a `DISPLAYCONFIG_GET_ADVANCED_COLOR_INFO`
    // whose size it states.
    if unsafe { DisplayConfigGetDeviceInfo(&mut info.header) } != 0 {
        return None;
    }
    // SAFETY: both members of the union are the same `u32` of bit fields.
    let bits = unsafe { info.Anonymous.value };
    Some(parse::advanced_color(bits))
}

/// `device` as a NUL-terminated UTF-16 string.
fn wide_z(device: &str) -> Vec<u16> {
    device.encode_utf16().chain(std::iter::once(0)).collect()
}

fn mode_of(dm: &DEVMODEW) -> DisplayMode {
    DisplayMode {
        width: dm.dmPelsWidth,
        height: dm.dmPelsHeight,
        // 0 and 1 stand for the hardware's default rate.
        refresh_mhz: (dm.dmDisplayFrequency > 1).then(|| dm.dmDisplayFrequency * 1000),
    }
}

/// One mode of `device`: `index`, or the current one for
/// `ENUM_CURRENT_SETTINGS`.
fn device_mode(device: &[u16], index: u32) -> Option<DEVMODEW> {
    let mut dm = DEVMODEW {
        dmSize: size_of::<DEVMODEW>() as u16,
        ..Default::default()
    };
    // SAFETY: `device` is NUL-terminated and `dm` states its size.
    let ok = unsafe { EnumDisplaySettingsW(device.as_ptr(), index, &mut dm) };
    (ok != 0).then_some(dm)
}

fn current_mode(device: &str) -> Option<DisplayMode> {
    device_mode(&wide_z(device), ENUM_CURRENT_SETTINGS).map(|dm| mode_of(&dm))
}

/// Every mode `device` offers in full colour.
fn modes(device: &str) -> Vec<DisplayMode> {
    let device = wide_z(device);
    // Drivers list a few hundred modes at most; the bound only guards
    // against one that never says it is done.
    (0..4096)
        .map_while(|i| device_mode(&device, i))
        .filter(|dm| dm.dmBitsPerPel >= 32)
        .map(|dm| mode_of(&dm))
        .collect()
}

/// The GDI device of the main display.
fn primary_device() -> Option<String> {
    (0..64).find_map(|i| {
        let mut dd = DISPLAY_DEVICEW {
            cb: size_of::<DISPLAY_DEVICEW>() as u32,
            ..Default::default()
        };
        // SAFETY: `dd` states its size; a null device lists the adapters.
        let ok = unsafe { EnumDisplayDevicesW(std::ptr::null(), i, &mut dd, 0) };
        if ok == 0 {
            // Past the last device: stop looking.
            return Some(None);
        }
        (dd.StateFlags & DISPLAY_DEVICE_PRIMARY_DEVICE != 0).then(|| wide(&dd.DeviceName))
    })?
}
