//! Windows: the monitor of the foreground window (the game), saved as PNG.
//!
//! DXGI Desktop Duplication first: it reads the picture the compositor
//! shows, flip-model and most fullscreen games included, which GDI's copy
//! of the desktop may miss (black). On an HDR display it asks for the
//! picture in half floats (scRGB) and tone maps it to SDR at the display's
//! SDR white level ([`super::tone_map_scrgb`]); the 8-bit picture Windows
//! would otherwise hand over is clipped and washed out. Where duplication
//! isn't available (a remote session, another program holding it, a rotated
//! display, a driver that refuses), GDI's copy of the desktop is used.
//! Irori's process is per-monitor DPI aware (GPUI makes it so), so the
//! monitor's rectangle is in real pixels.

use super::*;
use anyhow::{Context as _, bail};
use windows::Win32::Foundation::HMODULE;
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_UNKNOWN;
use windows::Win32::Graphics::Direct3D11::{
    D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAP_READ,
    D3D11_MAPPED_SUBRESOURCE, D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING,
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_COLOR_SPACE_RGB_FULL_G2084_NONE_P2020, DXGI_FORMAT, DXGI_FORMAT_B8G8R8A8_UNORM,
    DXGI_FORMAT_R16G16B16A16_FLOAT, DXGI_MODE_ROTATION_IDENTITY, DXGI_MODE_ROTATION_UNSPECIFIED,
};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, DXGI_OUTDUPL_FRAME_INFO, IDXGIAdapter1, IDXGIFactory1, IDXGIOutput,
    IDXGIOutput1, IDXGIOutput6, IDXGIOutputDuplication, IDXGIResource,
};
use windows::core::Interface as _;
use windows_sys::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BitBlt, CAPTUREBLT, CreateCompatibleBitmap,
    CreateCompatibleDC, DIB_RGB_COLORS, DeleteDC, DeleteObject, GetDC, GetDIBits, GetMonitorInfoW,
    MONITOR_DEFAULTTOPRIMARY, MONITORINFOEXW, MonitorFromWindow, ReleaseDC, SRCCOPY, SelectObject,
};
use windows_sys::Win32::UI::WindowsAndMessaging::GetForegroundWindow;

/// The monitor a capture is of.
struct Monitor {
    /// The GDI device showing it (`\\.\DISPLAY1`), as DXGI names it too.
    device: String,
    left: i32,
    top: i32,
    width: i32,
    height: i32,
}

/// The monitor the foreground window (the game) is on.
fn foreground_monitor() -> Result<Monitor> {
    // SAFETY: plain user32 calls with a zeroed `MONITORINFOEXW` whose size
    // is stated, as `GetMonitorInfoW` requires for the larger structure.
    unsafe {
        let monitor = MonitorFromWindow(GetForegroundWindow(), MONITOR_DEFAULTTOPRIMARY);
        let mut info: MONITORINFOEXW = std::mem::zeroed();
        info.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
        if GetMonitorInfoW(monitor, (&raw mut info).cast()) == 0 {
            bail!("Windows didn't say where the screen is");
        }
        let r = info.monitorInfo.rcMonitor;
        let (width, height) = (r.right - r.left, r.bottom - r.top);
        if width <= 0 || height <= 0 {
            bail!("The screen has no size");
        }
        let end = info
            .szDevice
            .iter()
            .position(|c| *c == 0)
            .unwrap_or(info.szDevice.len());
        Ok(Monitor {
            device: String::from_utf16_lossy(&info.szDevice[..end]),
            left: r.left,
            top: r.top,
            width,
            height,
        })
    }
}

/// The DXGI output showing `device`, with the adapter driving it.
fn find_output(device: &str) -> Result<(IDXGIAdapter1, IDXGIOutput)> {
    // SAFETY: COM calls on interfaces this function owns; each result is
    // checked before use.
    unsafe {
        let factory: IDXGIFactory1 = CreateDXGIFactory1()?;
        let mut a = 0;
        while let Ok(adapter) = factory.EnumAdapters1(a) {
            let mut o = 0;
            while let Ok(output) = adapter.EnumOutputs(o) {
                let desc = output.GetDesc()?;
                let end = desc
                    .DeviceName
                    .iter()
                    .position(|c| *c == 0)
                    .unwrap_or(desc.DeviceName.len());
                if String::from_utf16_lossy(&desc.DeviceName[..end]) == device {
                    // The duplicated picture isn't turned with a rotated
                    // display; GDI's copy is.
                    if desc.Rotation != DXGI_MODE_ROTATION_IDENTITY
                        && desc.Rotation != DXGI_MODE_ROTATION_UNSPECIFIED
                    {
                        bail!("the display is rotated");
                    }
                    return Ok((adapter, output));
                }
                o += 1;
            }
            a += 1;
        }
    }
    bail!("no DXGI output shows {device}")
}

/// The monitor's picture through Desktop Duplication, as RGBA rows.
fn duplicate(monitor: &Monitor) -> Result<(u32, u32, Vec<u8>)> {
    let (adapter, output) = find_output(&monitor.device)?;
    // SAFETY: COM calls on interfaces owned here; the mapped texture is
    // read within its row pitch and height and unmapped before returning.
    unsafe {
        let mut device: Option<ID3D11Device> = None;
        let mut context: Option<ID3D11DeviceContext> = None;
        D3D11CreateDevice(
            &adapter,
            D3D_DRIVER_TYPE_UNKNOWN,
            HMODULE::default(),
            D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            None,
            D3D11_SDK_VERSION,
            Some(&mut device),
            None,
            Some(&mut context),
        )?;
        let device = device.context("Direct3D gave no device")?;
        let context = context.context("Direct3D gave no device context")?;

        let hdr = output
            .cast::<IDXGIOutput6>()
            .ok()
            .and_then(|o| o.GetDesc1().ok())
            .is_some_and(|d| d.ColorSpace == DXGI_COLOR_SPACE_RGB_FULL_G2084_NONE_P2020);
        let duplication: IDXGIOutputDuplication = match output.cast::<IDXGIOutput6>() {
            Ok(output6) if hdr => {
                let formats: [DXGI_FORMAT; 2] =
                    [DXGI_FORMAT_R16G16B16A16_FLOAT, DXGI_FORMAT_B8G8R8A8_UNORM];
                match output6.DuplicateOutput1(&device, 0, &formats) {
                    Ok(d) => d,
                    Err(_) => output.cast::<IDXGIOutput1>()?.DuplicateOutput(&device)?,
                }
            }
            _ => output.cast::<IDXGIOutput1>()?.DuplicateOutput(&device)?,
        };

        // A new duplication's first frame is the whole desktop as it is.
        let mut info = DXGI_OUTDUPL_FRAME_INFO::default();
        let mut resource: Option<IDXGIResource> = None;
        duplication.AcquireNextFrame(500, &mut info, &mut resource)?;
        let frame: ID3D11Texture2D = resource.context("no picture came")?.cast()?;
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        frame.GetDesc(&mut desc);
        let staging_desc = D3D11_TEXTURE2D_DESC {
            Usage: D3D11_USAGE_STAGING,
            BindFlags: 0,
            CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
            MiscFlags: 0,
            MipLevels: 1,
            ArraySize: 1,
            ..desc
        };
        let mut staging: Option<ID3D11Texture2D> = None;
        device.CreateTexture2D(&staging_desc, None, Some(&mut staging))?;
        let staging = staging.context("Direct3D gave no texture")?;
        context.CopyResource(&staging, &frame);
        drop(frame);
        duplication.ReleaseFrame()?;

        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        context.Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))?;
        let (width, height) = (desc.Width as usize, desc.Height as usize);
        let bytes_per_pixel = if desc.Format == DXGI_FORMAT_R16G16B16A16_FLOAT {
            8
        } else {
            4
        };
        let row = width * bytes_per_pixel;
        let pitch = mapped.RowPitch as usize;
        let mut rows = Vec::with_capacity(row * height);
        if !mapped.pData.is_null() && pitch >= row && height > 0 {
            // The last row may end at its pixels rather than its pitch.
            let data =
                std::slice::from_raw_parts(mapped.pData.cast::<u8>(), pitch * (height - 1) + row);
            for y in 0..height {
                rows.extend_from_slice(&data[y * pitch..y * pitch + row]);
            }
        }
        context.Unmap(&staging, 0);
        if rows.len() != row * height {
            bail!("the picture couldn't be read");
        }
        let pixels = if desc.Format == DXGI_FORMAT_R16G16B16A16_FLOAT {
            let white = crate::displays::sdr_white_scale(&monitor.device).unwrap_or(1.0);
            tone_map_scrgb(&rows, white)
        } else if desc.Format == DXGI_FORMAT_B8G8R8A8_UNORM {
            bgrx_to_rgba(&mut rows);
            rows
        } else {
            bail!("the picture came in a format Irori doesn't read");
        };
        Ok((desc.Width, desc.Height, pixels))
    }
}

/// The monitor's picture as GDI copies it from the desktop, as RGBA rows.
fn copy_desktop(monitor: &Monitor) -> Result<(u32, u32, Vec<u8>)> {
    let Monitor {
        left,
        top,
        width,
        height,
        ..
    } = *monitor;
    // SAFETY: plain GDI calls. Every handle made here is checked and given
    // back before returning; the buffer holds exactly the rows asked for.
    unsafe {
        let screen = GetDC(std::ptr::null_mut());
        if screen.is_null() {
            bail!("The screen can't be read");
        }
        let memory = CreateCompatibleDC(screen);
        let bitmap = CreateCompatibleBitmap(screen, width, height);
        let mut pixels = vec![0u8; width as usize * height as usize * 4];
        let mut copied = false;
        if !memory.is_null() && !bitmap.is_null() {
            let previous = SelectObject(memory, bitmap);
            if BitBlt(
                memory,
                0,
                0,
                width,
                height,
                screen,
                left,
                top,
                SRCCOPY | CAPTUREBLT,
            ) != 0
            {
                let mut header: BITMAPINFO = std::mem::zeroed();
                header.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
                header.bmiHeader.biWidth = width;
                // Negative: rows from the top, as PNG has them.
                header.bmiHeader.biHeight = -height;
                header.bmiHeader.biPlanes = 1;
                header.bmiHeader.biBitCount = 32;
                header.bmiHeader.biCompression = BI_RGB;
                copied = GetDIBits(
                    memory,
                    bitmap,
                    0,
                    height as u32,
                    pixels.as_mut_ptr().cast(),
                    &mut header,
                    DIB_RGB_COLORS,
                ) == height;
            }
            SelectObject(memory, previous);
        }
        if !bitmap.is_null() {
            DeleteObject(bitmap);
        }
        if !memory.is_null() {
            DeleteDC(memory);
        }
        ReleaseDC(std::ptr::null_mut(), screen);
        if !copied {
            bail!("The screen couldn't be copied");
        }
        bgrx_to_rgba(&mut pixels);
        Ok((width as u32, height as u32, pixels))
    }
}

/// The foreground window's monitor as RGBA rows, with its size.
fn grab() -> Result<(u32, u32, Vec<u8>)> {
    let monitor = foreground_monitor()?;
    match duplicate(&monitor) {
        Ok(picture) => Ok(picture),
        Err(e) => {
            tracing::debug!(
                "desktop duplication of {}: {e:#}; copying with GDI",
                monitor.device
            );
            copy_desktop(&monitor)
        }
    }
}

pub(super) async fn capture(target: &Path) -> Result<PathBuf> {
    let target = target.to_path_buf();
    tokio::task::spawn_blocking(move || {
        let (width, height, pixels) = grab()?;
        if let Some(dir) = target.parent() {
            std::fs::create_dir_all(dir)?;
        }
        image::save_buffer(
            &target,
            &pixels,
            width,
            height,
            image::ExtendedColorType::Rgba8,
        )
        .context("Could not save the screenshot")?;
        Ok(target)
    })
    .await
    .context("Could not save the screenshot")?
}
