//! Light or dark pop-up menus on Windows, following Irori's theme.
//!
//! The tray's menu is a Win32 pop-up menu, which Windows draws light unless
//! the process asks otherwise. The only switch is uxtheme's undocumented
//! `SetPreferredAppMode` (ordinal 135, Windows 10 1903 and later; in 1809 the
//! same ordinal took a different argument, so older builds are left alone),
//! followed by `FlushMenuThemes` (ordinal 136) so a menu built before the
//! change is drawn again in the new mode. Irori forces the mode of its own
//! theme rather than the system's: a light Irori on a dark Windows keeps a
//! light menu, as its window is. High contrast stays Windows' own: uxtheme
//! ignores the preferred mode there.

/// `PreferredAppMode` values uxtheme takes.
#[cfg_attr(not(any(target_os = "windows", test)), allow(dead_code))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
enum AppMode {
    ForceDark = 2,
    ForceLight = 3,
}

/// The first build whose ordinal 135 is `SetPreferredAppMode`.
#[cfg_attr(not(any(target_os = "windows", test)), allow(dead_code))]
const FIRST_BUILD: u32 = 18362;

/// The mode to ask for, or `None` on a build that can't take it (or whose
/// number is unknown).
#[cfg_attr(not(any(target_os = "windows", test)), allow(dead_code))]
fn app_mode(dark: bool, build: Option<u32>) -> Option<AppMode> {
    build.filter(|b| *b >= FIRST_BUILD)?;
    Some(if dark {
        AppMode::ForceDark
    } else {
        AppMode::ForceLight
    })
}

/// Draw Irori's pop-up menus (the tray's) dark or light. Elsewhere menus
/// follow the system's appearance on their own.
#[cfg(target_os = "windows")]
pub fn follow(dark: bool) {
    use std::sync::OnceLock;
    use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
    use windows_sys::core::{PCSTR, w};

    type SetPreferredAppMode = unsafe extern "system" fn(i32) -> i32;
    type FlushMenuThemes = unsafe extern "system" fn();

    static BUILD: OnceLock<Option<u32>> = OnceLock::new();
    let build = *BUILD.get_or_init(windows_build);
    let Some(mode) = app_mode(dark, build) else {
        return;
    };
    // SAFETY: uxtheme.dll ships with every Windows Irori runs on and stays
    // loaded for the process; the ordinals are looked up by number (a
    // `PCSTR` below 0x10000 is an ordinal), and on the builds `app_mode`
    // allows they have exactly these signatures.
    unsafe {
        let uxtheme = LoadLibraryW(w!("uxtheme.dll"));
        if uxtheme.is_null() {
            return;
        }
        let Some(set) = GetProcAddress(uxtheme, 135 as PCSTR) else {
            return;
        };
        let set: SetPreferredAppMode = std::mem::transmute(set);
        set(mode as i32);
        if let Some(flush) = GetProcAddress(uxtheme, 136 as PCSTR) {
            let flush: FlushMenuThemes = std::mem::transmute(flush);
            flush();
        }
    }
}

#[cfg(not(target_os = "windows"))]
pub fn follow(_dark: bool) {}

/// The Windows build number, which `GetVersionEx` hides from programs
/// without a compatibility manifest; the registry tells it as it is.
#[cfg(target_os = "windows")]
fn windows_build() -> Option<u32> {
    windows_registry::LOCAL_MACHINE
        .open(r"SOFTWARE\Microsoft\Windows NT\CurrentVersion")
        .ok()?
        .get_string("CurrentBuildNumber")
        .ok()?
        .trim()
        .parse()
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asks_for_the_themes_mode_on_builds_that_take_it() {
        assert_eq!(app_mode(true, Some(19045)), Some(AppMode::ForceDark));
        assert_eq!(app_mode(false, Some(22631)), Some(AppMode::ForceLight));
        assert_eq!(app_mode(true, Some(FIRST_BUILD)), Some(AppMode::ForceDark));
    }

    #[test]
    fn leaves_older_and_unknown_builds_alone() {
        // 1809's ordinal 135 takes a bool, not a mode.
        assert_eq!(app_mode(true, Some(17763)), None);
        assert_eq!(app_mode(false, None), None);
    }
}
