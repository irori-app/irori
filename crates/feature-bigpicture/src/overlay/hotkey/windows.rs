//! Windows: `RegisterHotKey` on a thread of its own, which blocks in
//! `GetMessageW` until the shortcut is pressed or the registration is
//! dropped, so it costs no CPU meanwhile.

use anyhow::{Context as _, Result};
use futures::channel::mpsc::UnboundedSender;
use windows_sys::Win32::System::Threading::GetCurrentThreadId;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, MOD_SHIFT, RegisterHotKey, UnregisterHotKey,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetMessageW, MSG, PM_NOREMOVE, PeekMessageW, PostThreadMessageW, WM_HOTKEY, WM_QUIT, WM_USER,
};

use super::Key;

/// The registered shortcut; dropping it gives it back.
pub(super) struct Registration {
    thread: u32,
}

/// Register `key` on a new thread.
pub(super) fn register(key: Key, pressed: UnboundedSender<()>) -> Result<Registration> {
    let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<u32, String>>();
    std::thread::Builder::new()
        .name(format!("irori-shortcut-{}", key.id))
        .spawn(move || run(key, pressed, ready_tx))
        .context("the shortcut's thread didn't start")?;
    let thread = ready_rx
        .recv()
        .context("the shortcut's thread stopped")?
        .map_err(anyhow::Error::msg)?;
    Ok(Registration { thread })
}

fn run(
    key: Key,
    pressed: UnboundedSender<()>,
    ready: std::sync::mpsc::Sender<Result<u32, String>>,
) {
    // Each key has a thread and so a message queue of its own: the id only
    // names it there.
    let id = key.id as i32;
    // SAFETY: plain user32 calls on this thread with a zeroed `MSG` it owns.
    // A null window ties the hot key and its messages to this thread.
    unsafe {
        let mut msg: MSG = std::mem::zeroed();
        // Make the thread's message queue, so `PostThreadMessageW` reaches
        // it as soon as `register` returns.
        PeekMessageW(
            &mut msg,
            std::ptr::null_mut(),
            WM_USER,
            WM_USER,
            PM_NOREMOVE,
        );
        let second = if key.alt { MOD_ALT } else { MOD_SHIFT };
        let modifiers = MOD_CONTROL | second | MOD_NOREPEAT;
        if RegisterHotKey(std::ptr::null_mut(), id, modifiers, key.windows_key) == 0 {
            let error = std::io::Error::last_os_error();
            let _ = ready.send(Err(format!("another program has the shortcut ({error})")));
            return;
        }
        let _ = ready.send(Ok(GetCurrentThreadId()));
        while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
            if msg.message == WM_HOTKEY {
                let _ = pressed.unbounded_send(());
            }
        }
        UnregisterHotKey(std::ptr::null_mut(), id);
    }
}

impl Drop for Registration {
    fn drop(&mut self) {
        // SAFETY: posts WM_QUIT to the shortcut's thread, which then gives
        // the hot key back and ends.
        unsafe { PostThreadMessageW(self.thread, WM_QUIT, 0, 0) };
    }
}
