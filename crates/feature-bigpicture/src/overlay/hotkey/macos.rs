//! macOS: Carbon's `RegisterEventHotKey`, delivered to the application's
//! event target by the main run loop GPUI runs. It needs no Accessibility
//! permission, and since macOS 15 a shortcut must hold Control or Command,
//! which Control+Shift+O does.

use std::ffi::c_void;

use anyhow::{Result, bail};
use futures::channel::mpsc::UnboundedSender;
use parking_lot::Mutex;

type OsStatus = i32;
type EventTargetRef = *mut c_void;
type EventHandlerRef = *mut c_void;
type EventHotKeyRef = *mut c_void;
type EventRef = *mut c_void;
type EventHandlerCallRef = *mut c_void;
type EventHandler = extern "C" fn(EventHandlerCallRef, EventRef, *mut c_void) -> OsStatus;

#[repr(C)]
struct EventTypeSpec {
    event_class: u32,
    event_kind: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct EventHotKeyId {
    signature: u32,
    id: u32,
}

// SAFETY: Carbon's hot key and event handler functions (HIToolbox), with
// the C layouts above; every pointer passed is valid for the call.
#[link(name = "Carbon", kind = "framework")]
unsafe extern "C" {
    fn GetApplicationEventTarget() -> EventTargetRef;
    fn InstallEventHandler(
        target: EventTargetRef,
        handler: EventHandler,
        num_types: usize,
        list: *const EventTypeSpec,
        user_data: *mut c_void,
        out_ref: *mut EventHandlerRef,
    ) -> OsStatus;
    fn RemoveEventHandler(handler: EventHandlerRef) -> OsStatus;
    fn RegisterEventHotKey(
        key_code: u32,
        modifiers: u32,
        id: EventHotKeyId,
        target: EventTargetRef,
        options: u32,
        out_ref: *mut EventHotKeyRef,
    ) -> OsStatus;
    fn UnregisterEventHotKey(hot_key: EventHotKeyRef) -> OsStatus;
}

/// `kEventClassKeyboard` ('keyb') and `kEventHotKeyPressed`.
const KEYBOARD: u32 = u32::from_be_bytes(*b"keyb");
const HOT_KEY_PRESSED: u32 = 5;
/// `controlKey` and `shiftKey`.
const MODIFIERS: u32 = 0x1000 | 0x0200;
/// `kVK_ANSI_O`: a key position, so other keyboard layouts press the key
/// where O is on a US keyboard.
const KEY_O: u32 = 0x1F;
/// Irori's signature for its hot keys ('iror').
const SIGNATURE: u32 = u32::from_be_bytes(*b"iror");

/// Where a press goes; the handler is a plain C function.
static PRESSED: Mutex<Option<UnboundedSender<()>>> = Mutex::new(None);

extern "C" fn on_hot_key(_: EventHandlerCallRef, _: EventRef, _: *mut c_void) -> OsStatus {
    if let Some(tx) = PRESSED.lock().as_ref() {
        let _ = tx.unbounded_send(());
    }
    0
}

/// The registered shortcut; dropping it gives it back.
pub(super) struct Registration {
    hot_key: EventHotKeyRef,
    handler: EventHandlerRef,
}

/// Register Control+Shift+O. Call on the main thread.
pub(super) fn register(pressed: UnboundedSender<()>) -> Result<Registration> {
    *PRESSED.lock() = Some(pressed);
    let spec = EventTypeSpec {
        event_class: KEYBOARD,
        event_kind: HOT_KEY_PRESSED,
    };
    let mut handler: EventHandlerRef = std::ptr::null_mut();
    let mut hot_key: EventHotKeyRef = std::ptr::null_mut();
    // SAFETY: on the main thread (GPUI's), with valid pointers to locals
    // that outlive the calls. The handler is a `'static` function and uses
    // no user data.
    unsafe {
        let target = GetApplicationEventTarget();
        let status = InstallEventHandler(
            target,
            on_hot_key,
            1,
            &spec,
            std::ptr::null_mut(),
            &mut handler,
        );
        if status != 0 {
            bail!("the event handler wasn't installed ({status})");
        }
        let id = EventHotKeyId {
            signature: SIGNATURE,
            id: 1,
        };
        let status = RegisterEventHotKey(KEY_O, MODIFIERS, id, target, 0, &mut hot_key);
        if status != 0 {
            RemoveEventHandler(handler);
            // -9878, eventHotKeyExistsErr: another program has it.
            bail!("another program has the shortcut ({status})");
        }
    }
    Ok(Registration { hot_key, handler })
}

impl Drop for Registration {
    fn drop(&mut self) {
        // SAFETY: both references came from the successful calls above and
        // are given back once, on the main thread.
        unsafe {
            UnregisterEventHotKey(self.hot_key);
            RemoveEventHandler(self.handler);
        }
        PRESSED.lock().take();
    }
}
