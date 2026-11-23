//! macOS: Carbon's `RegisterEventHotKey`, delivered to the application's
//! event target by the main run loop GPUI runs. It needs no Accessibility
//! permission, and since macOS 15 a shortcut must hold Control or Command,
//! which Control+Shift+O and Control+Option+K do. Every registration's
//! handler sees every press, so a press goes to the sender of its key's id.

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
    fn GetEventParameter(
        event: EventRef,
        name: u32,
        desired_type: u32,
        actual_type: *mut u32,
        buffer_size: usize,
        actual_size: *mut usize,
        data: *mut c_void,
    ) -> OsStatus;
}

/// `kEventClassKeyboard` ('keyb') and `kEventHotKeyPressed`.
const KEYBOARD: u32 = u32::from_be_bytes(*b"keyb");
const HOT_KEY_PRESSED: u32 = 5;
/// `controlKey`, `shiftKey` and `optionKey`.
const CONTROL: u32 = 0x1000;
const SHIFT: u32 = 0x0200;
const OPTION: u32 = 0x0800;
/// Irori's signature for its hot keys ('iror').
const SIGNATURE: u32 = u32::from_be_bytes(*b"iror");
/// `kEventParamDirectObject` ('----') and `typeEventHotKeyID` ('hkid').
const DIRECT_OBJECT: u32 = u32::from_be_bytes(*b"----");
const HOT_KEY_ID: u32 = u32::from_be_bytes(*b"hkid");
/// `eventNotHandledErr`: the next handler gets the press.
const NOT_HANDLED: OsStatus = -9874;

/// Where each key's presses go, by id; the handler is a plain C function.
static PRESSED: Mutex<Vec<(u32, UnboundedSender<()>)>> = Mutex::new(Vec::new());

extern "C" fn on_hot_key(_: EventHandlerCallRef, event: EventRef, _: *mut c_void) -> OsStatus {
    let mut id = EventHotKeyId {
        signature: 0,
        id: 0,
    };
    // SAFETY: reads the pressed key's id into a local of its exact size.
    let status = unsafe {
        GetEventParameter(
            event,
            DIRECT_OBJECT,
            HOT_KEY_ID,
            std::ptr::null_mut(),
            std::mem::size_of::<EventHotKeyId>(),
            std::ptr::null_mut(),
            (&mut id as *mut EventHotKeyId).cast(),
        )
    };
    if status != 0 || id.signature != SIGNATURE {
        return NOT_HANDLED;
    }
    match PRESSED.lock().iter().find(|(key, _)| *key == id.id) {
        Some((_, tx)) => {
            let _ = tx.unbounded_send(());
            0
        }
        None => NOT_HANDLED,
    }
}

/// The registered shortcut; dropping it gives it back.
pub(super) struct Registration {
    id: u32,
    hot_key: EventHotKeyRef,
    handler: EventHandlerRef,
}

/// Register `key`. Call on the main thread.
pub(super) fn register(key: super::Key, pressed: UnboundedSender<()>) -> Result<Registration> {
    {
        let mut senders = PRESSED.lock();
        senders.retain(|(id, _)| *id != key.id);
        senders.push((key.id, pressed));
    }
    let modifiers = CONTROL | if key.alt { OPTION } else { SHIFT };
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
            PRESSED.lock().retain(|(id, _)| *id != key.id);
            bail!("the event handler wasn't installed ({status})");
        }
        let id = EventHotKeyId {
            signature: SIGNATURE,
            id: key.id,
        };
        let status = RegisterEventHotKey(key.mac_key, modifiers, id, target, 0, &mut hot_key);
        if status != 0 {
            RemoveEventHandler(handler);
            PRESSED.lock().retain(|(id, _)| *id != key.id);
            // -9878, eventHotKeyExistsErr: another program has it.
            bail!("another program has the shortcut ({status})");
        }
    }
    Ok(Registration {
        id: key.id,
        hot_key,
        handler,
    })
}

impl Drop for Registration {
    fn drop(&mut self) {
        // SAFETY: both references came from the successful calls above and
        // are given back once, on the main thread.
        unsafe {
            UnregisterEventHotKey(self.hot_key);
            RemoveEventHandler(self.handler);
        }
        PRESSED.lock().retain(|(id, _)| *id != self.id);
    }
}
