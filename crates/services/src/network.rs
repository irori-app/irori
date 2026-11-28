//! System network state: whether the computer is online (used before falling
//! back to a connectivity probe) and whether the connection is metered (for
//! automatic updates). Linux asks the desktop portal (`NetworkMonitor`) and
//! Windows reads `GetNetworkConnectivityHint`; macOS has no answer without
//! Network.framework, so there both return `None`.

/// `Some(true)` on a metered connection, `None` when the system can't tell.
pub(crate) async fn is_metered() -> Option<bool> {
    let answer = tokio::time::timeout(std::time::Duration::from_secs(3), ask_metered());
    answer.await.ok().flatten()
}

/// `Some(true)` when the OS reports internet availability, `Some(false)` when
/// offline, or `None` when the OS cannot report connectivity so callers may
/// fall back to an explicit probe.
pub(crate) async fn is_online() -> Option<bool> {
    let answer = tokio::time::timeout(std::time::Duration::from_secs(3), ask_online());
    answer.await.ok().flatten()
}

#[cfg(target_os = "linux")]
async fn ask_metered() -> Option<bool> {
    use ashpd::desktop::network_monitor::NetworkMonitor;
    let monitor = NetworkMonitor::new().await.ok()?;
    monitor.is_metered().await.ok()
}

#[cfg(target_os = "linux")]
async fn ask_online() -> Option<bool> {
    use ashpd::desktop::network_monitor::NetworkMonitor;
    let monitor = NetworkMonitor::new().await.ok()?;
    monitor.is_available().await.ok()
}

#[cfg(windows)]
fn connectivity_hint()
-> Option<windows_sys::Win32::Networking::WinSock::NL_NETWORK_CONNECTIVITY_HINT> {
    use windows_sys::Win32::Networking::WinSock::NL_NETWORK_CONNECTIVITY_HINT;
    use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
    type GetHint = unsafe extern "system" fn(*mut NL_NETWORK_CONNECTIVITY_HINT) -> u32;

    // Looked up at run time: the call arrived with Windows 10 2004, and an
    // import of it would stop Irori from starting on older systems.
    let library: Vec<u16> = "iphlpapi.dll\0".encode_utf16().collect();
    // SAFETY: `library` is a NUL-terminated wide string that outlives the call.
    let module = unsafe { LoadLibraryW(library.as_ptr()) };
    if module.is_null() {
        return None;
    }
    // SAFETY: `module` is a loaded library, which is never freed, and the
    // name is NUL-terminated.
    let symbol = unsafe { GetProcAddress(module, c"GetNetworkConnectivityHint".as_ptr().cast()) }?;
    // SAFETY: the export has this signature, from its Windows SDK declaration.
    let get_hint: GetHint = unsafe { std::mem::transmute(symbol) };
    // SAFETY: an all-zero hint is a valid value of this plain C struct.
    let mut hint: NL_NETWORK_CONNECTIVITY_HINT = unsafe { std::mem::zeroed() };
    // SAFETY: `hint` is a live, writable struct of the type the call fills in.
    if unsafe { get_hint(&mut hint) } != 0 {
        return None;
    }
    Some(hint)
}

#[cfg(windows)]
async fn ask_metered() -> Option<bool> {
    use windows_sys::Win32::Networking::WinSock::{
        NetworkConnectivityCostHintFixed, NetworkConnectivityCostHintUnrestricted,
        NetworkConnectivityCostHintVariable,
    };
    let cost = connectivity_hint()?.ConnectivityCost;
    if cost == NetworkConnectivityCostHintFixed || cost == NetworkConnectivityCostHintVariable {
        Some(true)
    } else if cost == NetworkConnectivityCostHintUnrestricted {
        Some(false)
    } else {
        None
    }
}

#[cfg(windows)]
async fn ask_online() -> Option<bool> {
    use windows_sys::Win32::Networking::WinSock::{
        NetworkConnectivityLevelHintConstrainedInternetAccess,
        NetworkConnectivityLevelHintInternetAccess, NetworkConnectivityLevelHintLocalAccess,
        NetworkConnectivityLevelHintNone,
    };
    let level = connectivity_hint()?.ConnectivityLevel;
    if level == NetworkConnectivityLevelHintInternetAccess
        || level == NetworkConnectivityLevelHintConstrainedInternetAccess
    {
        Some(true)
    } else if level == NetworkConnectivityLevelHintNone
        || level == NetworkConnectivityLevelHintLocalAccess
    {
        Some(false)
    } else {
        None
    }
}

#[cfg(not(any(target_os = "linux", windows)))]
async fn ask_metered() -> Option<bool> {
    None
}

#[cfg(not(any(target_os = "linux", windows)))]
async fn ask_online() -> Option<bool> {
    None
}
