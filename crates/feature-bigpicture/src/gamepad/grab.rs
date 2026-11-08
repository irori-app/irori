//! Holding the controllers from the game while the in-game overlay is open.
//!
//! On Linux each controller's event device is opened again and grabbed
//! (`EVIOCGRAB`): while grabbed, only that file sees the device's events, so
//! neither the game nor gilrs gets the overlay's presses. A thread of its
//! own reads the grabbed devices ([`super::evdev::RawPad`]) and sends what
//! they press down the reader's channel, so the overlay is driven exactly as
//! by gilrs. Dropping the [`Grab`] asks the thread to stop; it lets go once
//! no button is held any more (at most [`RELEASE_WAIT`] later), so the game
//! never sees the release of a press it didn't see. The kernel lets go of a
//! grab when its file closes, so a crash of Irori can't keep the
//! controller from the game.
//!
//! The thread blocks in `poll` until an event, a repeat or the stop, so an
//! idle controller costs no CPU, and it exists only while the overlay is
//! open. A controller read through hidraw (Steam Input, SDL's HIDAPI
//! drivers inside some games) is not held by this: the overlay then works,
//! but the game sees the presses too.

use std::path::PathBuf;
use std::sync::Arc;

use futures::channel::mpsc::UnboundedSender;

use super::input::ControllerKind;
use super::reader::{Message, SharedConfig};

/// The longest the grab is kept after the overlay closed, waiting for the
/// buttons to be let go. Only the Linux grab waits.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) const RELEASE_WAIT: std::time::Duration = std::time::Duration::from_millis(800);

/// The controllers held from the game; dropping it lets go of them.
pub(crate) struct Grab {
    #[cfg(target_os = "linux")]
    stop: Option<std::os::fd::OwnedFd>,
}

#[cfg(target_os = "linux")]
impl Grab {
    /// Grab `devices` (event device and family) on a thread of their own.
    /// `None` when there is nothing to grab or the thread couldn't start.
    pub(crate) fn start(
        devices: Vec<(PathBuf, ControllerKind)>,
        shared: Arc<SharedConfig>,
        tx: UnboundedSender<Message>,
    ) -> Option<Self> {
        if devices.is_empty() {
            return None;
        }
        let (read, write) = linux::pipe()?;
        let spawned = std::thread::Builder::new()
            .name("irori-overlay-grab".into())
            .spawn(move || linux::run(devices, read, shared, tx));
        match spawned {
            Ok(_) => Some(Self { stop: Some(write) }),
            Err(e) => {
                tracing::warn!("holding the controller from the game: {e}");
                None
            }
        }
    }
}

#[cfg(not(target_os = "linux"))]
impl Grab {
    /// Windows and macOS offer no grab a program may use (see
    /// `overlay::support`): the game sees the overlay's presses too.
    pub(crate) fn start(
        _devices: Vec<(PathBuf, ControllerKind)>,
        _shared: Arc<SharedConfig>,
        _tx: UnboundedSender<Message>,
    ) -> Option<Self> {
        None
    }
}

impl Drop for Grab {
    fn drop(&mut self) {
        // Closing the pipe's write end wakes the thread, which lets go.
        #[cfg(target_os = "linux")]
        drop(self.stop.take());
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use std::ffi::CString;
    use std::os::fd::{AsRawFd as _, FromRawFd as _, OwnedFd};
    use std::os::unix::ffi::OsStrExt as _;
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use futures::channel::mpsc::UnboundedSender;

    use super::super::evdev::{ABS_X, ABS_Y, RawPad};
    use super::super::input::{ControllerKind, GamepadInput, GuideTap, Repeater};
    use super::super::reader::{Message, SharedConfig};
    use super::RELEASE_WAIT;

    /// `EVIOCGRAB`, `_IOW('E', 0x90, int)`, in the generic ioctl encoding
    /// of x86-64 and arm64, the architectures Irori ships for.
    const EVIOCGRAB: u64 = 0x4004_4590;
    const GRAB: libc::c_int = 1;
    const UNGRAB: libc::c_int = 0;

    /// `EVIOCGABS(axis)`, `_IOR('E', 0x40 + axis, struct input_absinfo)`.
    fn eviocgabs(axis: u16) -> u64 {
        let size = std::mem::size_of::<libc::input_absinfo>() as u64;
        (2 << 30) | (size << 16) | (u64::from(b'E') << 8) | (0x40 + u64::from(axis))
    }

    /// A close-on-exec pipe: (read end, write end).
    pub(super) fn pipe() -> Option<(OwnedFd, OwnedFd)> {
        let mut fds = [0 as libc::c_int; 2];
        // SAFETY: `fds` has room for the two descriptors pipe2 writes.
        if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) } != 0 {
            tracing::warn!(
                "holding the controller from the game: {}",
                std::io::Error::last_os_error()
            );
            return None;
        }
        // SAFETY: pipe2 succeeded, so both are open descriptors nothing else owns.
        Some(unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) })
    }

    /// Open and grab one event device, retrying while an earlier grab of it
    /// (the overlay closed a moment ago) still lets go.
    fn open_grabbed(path: &Path) -> Option<OwnedFd> {
        let c_path = CString::new(path.as_os_str().as_bytes()).ok()?;
        // SAFETY: `c_path` is a NUL-terminated path; the flags are plain.
        let raw = unsafe {
            libc::open(
                c_path.as_ptr(),
                libc::O_RDONLY | libc::O_NONBLOCK | libc::O_CLOEXEC,
            )
        };
        if raw < 0 {
            tracing::warn!(
                "opening {} to hold it from the game: {}",
                path.display(),
                std::io::Error::last_os_error()
            );
            return None;
        }
        // SAFETY: `raw` was just opened and is owned by nothing else.
        let fd = unsafe { OwnedFd::from_raw_fd(raw) };
        let deadline = Instant::now() + RELEASE_WAIT + Duration::from_millis(200);
        loop {
            // SAFETY: EVIOCGRAB takes an int by value; `fd` is an open event device.
            let result = unsafe { libc::ioctl(fd.as_raw_fd(), EVIOCGRAB as _, GRAB) };
            if result == 0 {
                return Some(fd);
            }
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() != Some(libc::EBUSY) || Instant::now() >= deadline {
                tracing::warn!("holding {} from the game: {error}", path.display());
                return None;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// The range of an absolute axis, if the device has it.
    fn axis_range(fd: &OwnedFd, axis: u16) -> Option<(i32, i32)> {
        // SAFETY: input_absinfo is plain integers; all zeroes is a valid value.
        let mut info: libc::input_absinfo = unsafe { std::mem::zeroed() };
        // SAFETY: EVIOCGABS writes one input_absinfo into `info`, which outlives the call.
        let result = unsafe {
            libc::ioctl(
                fd.as_raw_fd(),
                eviocgabs(axis) as _,
                &mut info as *mut libc::input_absinfo,
            )
        };
        (result == 0).then_some((info.minimum, info.maximum))
    }

    struct Held {
        fd: OwnedFd,
        pad: RawPad,
        kind: ControllerKind,
    }

    pub(super) fn run(
        devices: Vec<(PathBuf, ControllerKind)>,
        stop: OwnedFd,
        shared: Arc<SharedConfig>,
        tx: UnboundedSender<Message>,
    ) {
        let mut held: Vec<Held> = devices
            .into_iter()
            .filter_map(|(path, kind)| {
                let fd = open_grabbed(&path)?;
                let mut pad = RawPad::new(kind);
                for axis in [ABS_X, ABS_Y] {
                    if let Some((min, max)) = axis_range(&fd, axis) {
                        pad.set_range(axis, min, max);
                    }
                }
                Some(Held { fd, pad, kind })
            })
            .collect();
        if held.is_empty() {
            return;
        }
        let (initial, interval) = shared.timing();
        let mut repeater = Repeater::new(initial, interval);
        let mut guide = GuideTap::default();
        let mut stopping: Option<Instant> = None;
        // SAFETY: input_event is plain integers; all zeroes is a valid value.
        let mut events: [libc::input_event; 64] = unsafe { std::mem::zeroed() };

        loop {
            if let Some(since) = stopping
                && (held.iter().all(|h| !h.pad.any_held()) || since.elapsed() >= RELEASE_WAIT)
            {
                break;
            }
            let mut fds: Vec<libc::pollfd> = std::iter::once(&stop)
                .chain(held.iter().map(|h| &h.fd))
                .map(|fd| libc::pollfd {
                    fd: fd.as_raw_fd(),
                    events: libc::POLLIN,
                    revents: 0,
                })
                .collect();
            if stopping.is_some() {
                // The stop pipe stays readable once closed; don't spin on it.
                fds[0].fd = -1;
            }
            let now = Instant::now();
            let mut deadline = repeater.deadline();
            if let Some(since) = stopping {
                let end = since + RELEASE_WAIT;
                deadline = Some(deadline.map_or(end, |d| d.min(end)));
            }
            let timeout = deadline.map_or(-1, |d| {
                let ms = d.saturating_duration_since(now).as_millis();
                libc::c_int::try_from(ms).unwrap_or(libc::c_int::MAX)
            });
            // SAFETY: `fds` holds `fds.len()` initialised pollfd entries.
            let ready = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, timeout) };
            if ready < 0 {
                let error = std::io::Error::last_os_error();
                if error.kind() == std::io::ErrorKind::Interrupted {
                    continue;
                }
                tracing::warn!("reading the held controller: {error}");
                break;
            }
            if stopping.is_none() && fds[0].revents != 0 {
                stopping = Some(Instant::now());
                repeater.reset();
            }
            let mut gone = Vec::new();
            for (ix, entry) in fds.iter().enumerate().skip(1) {
                if entry.revents == 0 {
                    continue;
                }
                let device = &mut held[ix - 1];
                if entry.revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) != 0 {
                    gone.push(ix - 1);
                    continue;
                }
                // SAFETY: `events` is a writable buffer of exactly that many bytes.
                let read = unsafe {
                    libc::read(
                        device.fd.as_raw_fd(),
                        events.as_mut_ptr().cast(),
                        std::mem::size_of_val(&events),
                    )
                };
                if read <= 0 {
                    continue;
                }
                let count = read as usize / std::mem::size_of::<libc::input_event>();
                for event in &events[..count] {
                    for (input, pressed) in device.pad.event(event.type_, event.code, event.value) {
                        if stopping.is_some() {
                            continue;
                        }
                        let send = |input| {
                            let _ = tx.unbounded_send(Message::Input {
                                input,
                                kind: device.kind,
                            });
                        };
                        match (input, pressed) {
                            (GamepadInput::Guide, true) => guide.press(ix, Instant::now()),
                            (GamepadInput::Guide, false) => {
                                if guide.release(ix, Instant::now()) {
                                    send(GamepadInput::Guide);
                                }
                            }
                            (input, true) if input.is_directional() => {
                                send(repeater.press(input, Instant::now()));
                            }
                            (input, false) if input.is_directional() => repeater.release(input),
                            (input, true) => send(input),
                            (_, false) => {}
                        }
                    }
                }
            }
            for ix in gone.into_iter().rev() {
                held.remove(ix);
            }
            if held.is_empty() {
                break;
            }
            if stopping.is_none()
                && let Some(input) = repeater.poll(Instant::now())
            {
                let kind = held.first().map(|h| h.kind).unwrap_or_default();
                let _ = tx.unbounded_send(Message::Input { input, kind });
            }
            if tx.is_closed() {
                break;
            }
        }
        for device in &held {
            // SAFETY: EVIOCGRAB with 0 lets go of the grab on this open device.
            unsafe {
                libc::ioctl(device.fd.as_raw_fd(), EVIOCGRAB as _, UNGRAB);
            }
        }
    }
}
