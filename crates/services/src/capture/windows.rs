//! Windows: the monitor of the foreground window (the game), copied from
//! the desktop with GDI and saved as PNG. Irori's process is per-monitor DPI
//! aware (GPUI makes it so), so the monitor's rectangle is in real pixels.

use super::*;
use anyhow::{Context as _, bail};
use windows_sys::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BitBlt, CAPTUREBLT, CreateCompatibleBitmap,
    CreateCompatibleDC, DIB_RGB_COLORS, DeleteDC, DeleteObject, GetDC, GetDIBits, GetMonitorInfoW,
    MONITOR_DEFAULTTOPRIMARY, MONITORINFO, MonitorFromWindow, ReleaseDC, SRCCOPY, SelectObject,
};
use windows_sys::Win32::UI::WindowsAndMessaging::GetForegroundWindow;

/// The foreground window's monitor as RGBA rows, with its size.
fn grab() -> Result<(u32, u32, Vec<u8>)> {
    // SAFETY: plain GDI calls. Every handle made here is checked and given
    // back before returning; the buffer holds exactly the rows asked for.
    unsafe {
        let monitor = MonitorFromWindow(GetForegroundWindow(), MONITOR_DEFAULTTOPRIMARY);
        let mut info: MONITORINFO = std::mem::zeroed();
        info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        if GetMonitorInfoW(monitor, &mut info) == 0 {
            bail!("Windows didn't say where the screen is");
        }
        let r = info.rcMonitor;
        let (width, height) = (r.right - r.left, r.bottom - r.top);
        if width <= 0 || height <= 0 {
            bail!("The screen has no size");
        }
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
                r.left,
                r.top,
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
