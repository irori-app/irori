//! Windows: `ExitWindowsEx` and `SetSuspendState`.

use super::*;
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, LUID};
use windows_sys::Win32::Security::{
    AdjustTokenPrivileges, LUID_AND_ATTRIBUTES, LookupPrivilegeValueW, SE_PRIVILEGE_ENABLED,
    SE_SHUTDOWN_NAME, TOKEN_ADJUST_PRIVILEGES, TOKEN_PRIVILEGES, TOKEN_QUERY,
};
use windows_sys::Win32::System::Power::{
    GetPwrCapabilities, IsPwrSuspendAllowed, SYSTEM_POWER_CAPABILITIES, SetSuspendState,
};
use windows_sys::Win32::System::Shutdown::{
    EWX_LOGOFF, EWX_POWEROFF, EWX_REBOOT, ExitWindowsEx, SHTDN_REASON_FLAG_PLANNED,
    SHTDN_REASON_MAJOR_OTHER,
};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

/// Whether this PC sleeps the classic way (S3): Modern Standby PCs
/// (`AoAc`) don't, and `SetSuspendState` does nothing useful there.
fn can_sleep() -> bool {
    // SAFETY: plain calls without arguments, and a zeroed plain-data struct
    // that GetPwrCapabilities fills in.
    unsafe {
        if !IsPwrSuspendAllowed() {
            return false;
        }
        let mut caps: SYSTEM_POWER_CAPABILITIES = std::mem::zeroed();
        !GetPwrCapabilities(&mut caps) || !caps.AoAc
    }
}

pub(super) async fn permissions() -> Vec<(PowerAction, Permission)> {
    let sleep = tokio::task::spawn_blocking(can_sleep)
        .await
        .unwrap_or(false);
    vec![
        (PowerAction::PowerOff, Permission::Yes),
        (PowerAction::Restart, Permission::Yes),
        (
            PowerAction::Sleep,
            if sleep {
                Permission::Yes
            } else {
                Permission::Unavailable
            },
        ),
        (PowerAction::LogOut, Permission::Yes),
    ]
}

/// Turn on the shutdown privilege, which every interactive user holds but
/// no process has enabled until it asks.
fn enable_shutdown_privilege() -> Result<()> {
    // SAFETY: the token handle is checked and closed; every struct passed is
    // plain data owned here for the calls' duration.
    unsafe {
        let mut token: HANDLE = std::ptr::null_mut();
        if OpenProcessToken(
            GetCurrentProcess(),
            TOKEN_ADJUST_PRIVILEGES | TOKEN_QUERY,
            &mut token,
        ) == 0
        {
            bail!(
                "Windows didn't let Irori shut down: {}",
                std::io::Error::last_os_error()
            );
        }
        let mut luid = LUID {
            LowPart: 0,
            HighPart: 0,
        };
        let looked_up = LookupPrivilegeValueW(std::ptr::null(), SE_SHUTDOWN_NAME, &mut luid);
        let privileges = TOKEN_PRIVILEGES {
            PrivilegeCount: 1,
            Privileges: [LUID_AND_ATTRIBUTES {
                Luid: luid,
                Attributes: SE_PRIVILEGE_ENABLED,
            }],
        };
        let adjusted = looked_up != 0
            && AdjustTokenPrivileges(
                token,
                0,
                &privileges,
                0,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            ) != 0;
        let error = std::io::Error::last_os_error();
        CloseHandle(token);
        // AdjustTokenPrivileges succeeds without granting a privilege the
        // user doesn't hold (ERROR_NOT_ALL_ASSIGNED, 1300).
        if !adjusted || error.raw_os_error() == Some(1300) {
            bail!("Windows didn't let Irori shut down: {error}");
        }
    }
    Ok(())
}

fn perform_blocking(action: PowerAction) -> Result<()> {
    let flags = match action {
        PowerAction::PowerOff => EWX_POWEROFF,
        PowerAction::Restart => EWX_REBOOT,
        PowerAction::LogOut => EWX_LOGOFF,
        PowerAction::Sleep => {
            // SAFETY: a plain call; false, false, false is sleep (not
            // hibernate), not forced, with wake-up events kept.
            if unsafe { SetSuspendState(false, false, false) } {
                return Ok(());
            }
            bail!(
                "Windows didn't go to sleep: {}",
                std::io::Error::last_os_error()
            );
        }
    };
    if action != PowerAction::LogOut {
        enable_shutdown_privilege()?;
    }
    // SAFETY: a plain call with flags and a reason code.
    let ok = unsafe { ExitWindowsEx(flags, SHTDN_REASON_MAJOR_OTHER | SHTDN_REASON_FLAG_PLANNED) };
    if ok == 0 {
        bail!("Windows refused: {}", std::io::Error::last_os_error());
    }
    Ok(())
}

pub(super) async fn perform(action: PowerAction) -> Result<()> {
    tokio::task::spawn_blocking(move || perform_blocking(action)).await?
}
