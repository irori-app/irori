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

None yet.
