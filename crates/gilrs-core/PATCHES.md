# gilrs-core, patched

The platform layer of gilrs, copied unchanged from crates.io's `gilrs-core` 0.6.8
(gilrs at `07e286e24b046cf39e5c367daa2770b805a64692`, dual Apache-2.0/MIT, used here
under MIT, `LICENSE-MIT`; its `Cargo.lock` and changelog left out) and used through
`[patch.crates-io]` in the root `Cargo.toml`. The commit that added it holds the crate
as published, so `git log` on this folder shows only Irori's changes. It stays out of
the workspace, so Irori's fmt and clippy settings don't apply to it.

When gilrs moves to a new version, copy that version's `gilrs-core` here and apply the
patches below again, or drop this folder once upstream has the fixes.

## Patches

All of them are in the Windows.Gaming.Input backend
(`src/platform/windows_wgi/gamepad.rs`) and marked `Irori patch`.

### Never abort the program

Irori's release build has `panic = "abort"`, so a panic anywhere in gilrs ends Irori.

- The reading thread unwrapped a controller's first reading; a controller being
  disconnected fails it. It now tries that controller again on the next round.
- The WinRT added/removed handlers expected their channel to be open, and starting
  the backend unwrapped the handler registrations and the thread spawn. Sends that
  fail are ignored and start-up failures are returned as errors, which Irori logs
  ("controller support unavailable").
- The reading thread can see a new controller in `RawGameControllers` before its
  `RawGameControllerAdded` event arrives. `handle_event` then made a gamepad for it
  and passed on, say, a button press with a new id, and `gilrs` (which creates a
  gamepad's state only on `Connected`) indexed past its list and panicked. The
  backend now reports `Connected` first and holds the event back for the next call;
  a `Disconnected` for a controller never seen is dropped.

### Read controllers whose `NonRoamableId` fails

`NonRoamableId` fails on some Windows builds. Upstream then failed to start when such
a controller was connected, or dropped all of its input (logging an error every
minute). Controllers are now keyed by `controller_key`: the `NonRoamableId`, else the
controller object's address. WinRT hands out one object per connected controller,
and SDL's own WGI backend compares them by address too.

### Keep the reading list bounded

The reading thread kept every controller's last readings forever. It now forgets
those of controllers that are no longer connected.

## Not fixed here

Once Steam or an SDL program switches a DualSense, DualShock 4 (over Bluetooth) or
Switch Pro controller to its full HID report, Windows.Gaming.Input still lists it but
reports no input. Only reading the HID reports directly (SDL's HIDAPI drivers) fixes
that; it is not something this backend can do.
