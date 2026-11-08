//! Windows: Core Audio. The system volume is the default output's endpoint
//! volume (`IAudioEndpointVolume`); a game's is that of its audio sessions
//! (`ISimpleAudioVolume`, what the volume mixer shows per program) on every
//! active output. COM objects stay on the blocking thread that made them.

use super::*;
use anyhow::{Context as _, bail};
use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
use windows::Win32::Media::Audio::{
    AudioSessionStateExpired, DEVICE_STATE_ACTIVE, IAudioSessionControl2, IAudioSessionManager2,
    IMMDeviceEnumerator, ISimpleAudioVolume, MMDeviceEnumerator, eConsole, eRender,
};
use windows::Win32::System::Com::{
    CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize,
};
use windows::core::Interface as _;

/// COM on this thread, for as long as it lives.
struct Com(bool);

impl Com {
    fn init() -> Com {
        // SAFETY: initializes COM for the calling (blocking) thread; balanced
        // by `CoUninitialize` in `drop` when it succeeded. A thread already in
        // another apartment fails with RPC_E_CHANGED_MODE and keeps working.
        let hr = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        Com(hr.is_ok())
    }
}

impl Drop for Com {
    fn drop(&mut self) {
        if self.0 {
            // SAFETY: balances the successful `CoInitializeEx` above, on the
            // same thread, after every COM object of this call was dropped.
            unsafe { CoUninitialize() };
        }
    }
}

fn enumerator() -> Result<IMMDeviceEnumerator> {
    // SAFETY: a plain COM activation with COM initialized on this thread.
    unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) }
        .context("Windows' audio service didn't answer")
}

fn endpoint(devices: &IMMDeviceEnumerator) -> Result<IAudioEndpointVolume> {
    // SAFETY: COM calls on live interfaces, on the thread that made them.
    unsafe {
        let device = devices
            .GetDefaultAudioEndpoint(eRender, eConsole)
            .context("There is no sound output")?;
        device
            .Activate::<IAudioEndpointVolume>(CLSCTX_ALL, None)
            .context("The sound output has no volume")
    }
}

fn system(devices: &IMMDeviceEnumerator) -> Result<Volume> {
    let endpoint = endpoint(devices)?;
    // SAFETY: COM calls on a live interface, on the thread that made it.
    let (level, muted) = unsafe {
        (
            endpoint.GetMasterVolumeLevelScalar()?,
            endpoint.GetMute()?.as_bool(),
        )
    };
    Ok(Volume {
        percent: level_percent(level),
        muted,
    })
}

/// Every audio session on an active output, with its process.
fn sessions(devices: &IMMDeviceEnumerator) -> Result<Vec<(u32, ISimpleAudioVolume)>> {
    let mut out = Vec::new();
    // SAFETY: COM calls on live interfaces, on the thread that made them.
    unsafe {
        let outputs = devices.EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE)?;
        for i in 0..outputs.GetCount()? {
            let Ok(device) = outputs.Item(i) else {
                continue;
            };
            let Ok(manager) = device.Activate::<IAudioSessionManager2>(CLSCTX_ALL, None) else {
                continue;
            };
            let list = manager.GetSessionEnumerator()?;
            for j in 0..list.GetCount()? {
                let Ok(control) = list.GetSession(j) else {
                    continue;
                };
                if control
                    .GetState()
                    .is_ok_and(|s| s == AudioSessionStateExpired)
                {
                    continue;
                }
                let Ok(control2) = control.cast::<IAudioSessionControl2>() else {
                    continue;
                };
                // The system sounds session names process 0.
                let pid = control2.GetProcessId().unwrap_or(0);
                if let (true, Ok(volume)) = (pid != 0, control.cast::<ISimpleAudioVolume>()) {
                    out.push((pid, volume));
                }
            }
        }
    }
    Ok(out)
}

/// The game's processes (see `session_pids`).
fn game_pids(game: &GameProcess) -> HashSet<u32> {
    use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System};
    let mut sys = System::new();
    sys.refresh_processes_specifics(ProcessesToUpdate::All, true, ProcessRefreshKind::nothing());
    let table: Vec<Proc> = sys
        .processes()
        .values()
        .map(|p| Proc {
            pid: p.pid().as_u32(),
            parent: p.parent().map(|p| p.as_u32()),
            start: p.start_time(),
        })
        .collect();
    let mut pids = session_pids(&table, game.pid, game.since);
    if let Some(handoff) = &game.handoff {
        pids.extend(handoff.pids());
    }
    pids
}

/// The game's sessions and their volumes.
fn game_sessions(
    devices: &IMMDeviceEnumerator,
    game: &GameProcess,
) -> Result<Vec<(ISimpleAudioVolume, Volume)>> {
    let pids = game_pids(game);
    let mut out = Vec::new();
    for (pid, volume) in sessions(devices)? {
        if !pids.contains(&pid) {
            continue;
        }
        // SAFETY: COM calls on a live interface, on the thread that made it.
        let now = unsafe {
            Volume {
                percent: level_percent(volume.GetMasterVolume()?),
                muted: volume.GetMute()?.as_bool(),
            }
        };
        out.push((volume, now));
    }
    Ok(out)
}

fn read(game: Option<&GameProcess>) -> Result<GameAudio> {
    let _com = Com::init();
    let devices = enumerator()?;
    let system = system(&devices);
    let sessions = match game {
        Some(game) => game_sessions(&devices, game).unwrap_or_default(),
        None => Vec::new(),
    };
    if let (Err(e), true) = (&system, sessions.is_empty()) {
        bail!("{e:#}");
    }
    Ok(GameAudio {
        system: system.ok(),
        game: combine(sessions.iter().map(|(_, v)| *v)),
    })
}

fn apply(game: Option<&GameProcess>, target: VolumeTarget, change: Change) -> Result<()> {
    let _com = Com::init();
    let devices = enumerator()?;
    let quiet = std::ptr::null();
    match target {
        VolumeTarget::System => {
            let endpoint = endpoint(&devices)?;
            let now = system(&devices)?;
            // SAFETY: COM calls on a live interface, on the thread that made
            // it; a null event context is allowed.
            unsafe {
                match change {
                    Change::Step(step) => {
                        let level = stepped(now.percent, step) as f32 / 100.0;
                        endpoint.SetMasterVolumeLevelScalar(level, quiet)?;
                        if now.muted {
                            endpoint.SetMute(false, quiet)?;
                        }
                    }
                    Change::ToggleMute => endpoint.SetMute(!now.muted, quiet)?,
                }
            }
        }
        VolumeTarget::Game => {
            let game = game.context("The game isn't running")?;
            let sessions = game_sessions(&devices, game)?;
            let Some(now) = combine(sessions.iter().map(|(_, v)| *v)) else {
                bail!("The game isn't playing any sound");
            };
            for (volume, current) in &sessions {
                // SAFETY: as above.
                unsafe {
                    match change {
                        Change::Step(step) => {
                            let level = stepped(now.percent, step) as f32 / 100.0;
                            volume.SetMasterVolume(level, quiet)?;
                            if current.muted {
                                volume.SetMute(false, quiet)?;
                            }
                        }
                        Change::ToggleMute => volume.SetMute(!now.muted, quiet)?,
                    }
                }
            }
        }
    }
    Ok(())
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> Result<T> + Send + 'static) -> Result<T> {
    tokio::task::spawn_blocking(f)
        .await
        .context("The volume couldn't be read")?
}

pub(super) async fn audio(game: Option<GameProcess>) -> Result<GameAudio> {
    blocking(move || read(game.as_ref())).await
}

pub(super) async fn change(
    game: Option<GameProcess>,
    target: VolumeTarget,
    change: Change,
) -> Result<()> {
    blocking(move || apply(game.as_ref(), target, change)).await
}

/// Windows' policy configuration object, which sets the default output
/// (what Windows' own sound settings call). Microsoft doesn't document it,
/// but it has kept this layout since Windows 7 and every output switcher
/// uses it. Only `SetDefaultEndpoint` is called; the methods before it are
/// declared so it sits in its place in the table.
#[windows::core::interface("f8679f50-850a-41cf-9c72-430f290290c8")]
unsafe trait IPolicyConfig: windows::core::IUnknown {
    fn GetMixFormat(&self) -> windows::core::HRESULT;
    fn GetDeviceFormat(&self) -> windows::core::HRESULT;
    fn ResetDeviceFormat(&self) -> windows::core::HRESULT;
    fn SetDeviceFormat(&self) -> windows::core::HRESULT;
    fn GetProcessingPeriod(&self) -> windows::core::HRESULT;
    fn SetProcessingPeriod(&self) -> windows::core::HRESULT;
    fn GetShareMode(&self) -> windows::core::HRESULT;
    fn SetShareMode(&self) -> windows::core::HRESULT;
    fn GetPropertyValue(&self) -> windows::core::HRESULT;
    fn SetPropertyValue(&self) -> windows::core::HRESULT;
    fn SetDefaultEndpoint(
        &self,
        device: windows::core::PCWSTR,
        role: windows::Win32::Media::Audio::ERole,
    ) -> windows::core::HRESULT;
}

/// `CPolicyConfigClient`, the class that answers [`IPolicyConfig`].
const POLICY_CONFIG: windows::core::GUID =
    windows::core::GUID::from_u128(0x870af99c_171d_4f9e_af0d_e63df40c2bc9);

/// A device's id as text, freeing the string COM gave.
fn device_id(device: &windows::Win32::Media::Audio::IMMDevice) -> Option<String> {
    // SAFETY: COM calls on a live interface; the id string COM allocated is
    // read once and freed with the allocator it came from.
    unsafe {
        let id = device.GetId().ok()?;
        let text = id.to_string().ok();
        windows::Win32::System::Com::CoTaskMemFree(Some(id.0.cast()));
        text
    }
}

fn list_outputs() -> Result<Vec<AudioOutput>> {
    use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
    use windows::Win32::System::Com::STGM_READ;
    let _com = Com::init();
    let devices = enumerator()?;
    // SAFETY: COM calls on live interfaces, on the thread that made them.
    unsafe {
        let current = devices
            .GetDefaultAudioEndpoint(eRender, eConsole)
            .ok()
            .and_then(|d| device_id(&d));
        let outputs = devices.EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE)?;
        let mut out = Vec::new();
        for i in 0..outputs.GetCount()? {
            let Ok(device) = outputs.Item(i) else {
                continue;
            };
            let Some(id) = device_id(&device) else {
                continue;
            };
            let name = device
                .OpenPropertyStore(STGM_READ)
                .and_then(|store| store.GetValue(&PKEY_Device_FriendlyName))
                .map(|value| value.to_string())
                .ok()
                .filter(|n| !n.trim().is_empty())
                .unwrap_or_else(|| id.clone());
            out.push(AudioOutput {
                current: current.as_deref() == Some(id.as_str()),
                id,
                name,
            });
        }
        Ok(out)
    }
}

fn switch_output(id: &str) -> Result<()> {
    use windows::Win32::Media::Audio::{eCommunications, eMultimedia};
    let _com = Com::init();
    let wide: Vec<u16> = id.encode_utf16().chain(std::iter::once(0)).collect();
    // SAFETY: COM activation and calls on the thread that made them; `wide`
    // is a NUL-terminated UTF-16 id that outlives the calls.
    unsafe {
        let policy: IPolicyConfig = CoCreateInstance(&POLICY_CONFIG, None, CLSCTX_ALL)
            .context("Windows' sound settings didn't answer")?;
        for role in [eConsole, eMultimedia, eCommunications] {
            policy
                .SetDefaultEndpoint(windows::core::PCWSTR(wide.as_ptr()), role)
                .ok()
                .context("Windows didn't switch the sound output")?;
        }
    }
    Ok(())
}

pub(super) async fn outputs() -> Result<Vec<AudioOutput>> {
    blocking(list_outputs).await
}

pub(super) async fn set_output(_game: Option<GameProcess>, id: String) -> Result<()> {
    // A program's sound follows the default output unless the user gave it
    // one in Windows' settings, so the game moves along.
    blocking(move || switch_output(&id)).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_outputs_with_ids_and_at_most_one_current() {
        // Read only: CI machines may have no outputs, or no audio service.
        let Ok(outputs) = list_outputs() else {
            return;
        };
        assert!(
            outputs
                .iter()
                .all(|o| !o.id.is_empty() && !o.name.is_empty())
        );
        assert!(outputs.iter().filter(|o| o.current).count() <= 1);
    }
}
