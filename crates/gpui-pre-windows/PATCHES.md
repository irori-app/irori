# gpui-pre-windows, patched

GPUI's Windows backend, copied unchanged from crates.io's `gpui-pre-windows` 0.3.9
(a snapshot of Zed's `gpui_windows` at `93d745668805029476e6d7fbb19652116fed51de`,
Apache-2.0, `LICENSE-APACHE`) and used through `[patch.crates-io]` in the root
`Cargo.toml`. The commit that added it holds the crate as published, so `git log`
on this folder shows only Irori's changes. It stays out of the workspace, so Irori's
fmt and clippy settings don't apply to it.

When gpui-pre moves to a new version, copy that version's crate here and apply the
patches below again, or drop this folder once upstream has the fix.

## Patches

### Free a window the system destroyed, and drop it without errors

When the user closes a window (its close button, Alt+F4, the taskbar), Windows
destroys it before GPUI drops `WindowsWindow`. `Drop` then hid the window and queued
`RevokeDragDrop` and `DestroyWindow` on a handle that no longer exists:

- each call failed and logged `0x80040102` (`DRAGDROP_E_INVALIDHWND`) or
  `0x80070578` (`ERROR_INVALID_WINDOW_HANDLE`);
- OLE kept the drop target, whose handler holds an `Rc` to the window's inner state,
  so the renderer, swap chain and the rest of the window were never freed. Irori
  closes its window to the tray and opens a new one, so every close leaked one.

Now:

- `WM_DESTROY` revokes drag and drop while the handle is still valid and marks the
  window destroyed (`WindowsWindowInner::release_destroyed_handle`).
- `Drop` neither hides nor destroys a destroyed window. A flag, not `IsWindow`,
  because Windows may already have given the handle to another window.
- A window the app closes itself is hidden and destroyed as before; its
  `DestroyWindow` sends `WM_DESTROY`, which revokes, so each call still happens once.
