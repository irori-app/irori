---
name: irori-runtime-launch
description: Modify Wine, Proton, umu, CrossOver, GPTK, bubblewrap sandboxing, emulators, and Lutris install scripts in crates/runtime, crates/engines, and crates/scripts with offline cross-OS tests.
---

# Runtime, Launch Environment, Sandboxing, and Emulators

Use this skill whenever modifying Wine/Proton/umu launch composition, prefix management, bubblewrap containment, emulator manifests and launch commands, or Lutris install script execution in `crates/runtime`, `crates/engines`, or `crates/scripts`.

## Core rules

- Design runtime and emulator flows for Click-and-Play (`AGENTS.md`): auto-detect and select safe defaults (best already-installed Proton, Wine, or umu runtime, default prefix path, emulator cores/BIOS, and safe known game fixes) instead of asking users to edit environment variables or paths manually. Keep heavy downloads and third-party scripts (`Approvals`) behind explicit 1-click user actions, return actionable plain-language errors when a component is missing, and keep `preview_launch` transparent for power users.
- Keep launch environment logic pure and testable across operating systems. Gather host facts once into `LaunchFacts` (`crates/runtime/src/lib.rs`), then compute environment maps through pure helpers (`compose_env`, `wine_env_for`, `generic_env_for` in `crates/runtime/src/env.rs`) taking explicit `HostEnv` and `irori_core::HostPlatform` inputs so tests never read real process `std::env` variables.
- Never allow imported Linux settings to activate Linux-only graphics or runtime injection on macOS or Windows (`wine_env_for` gates by `HostPlatform`). Keep `#[cfg(target_os = "...")]` code isolated in OS modules or guarded with clean fallbacks so Linux, macOS, and Windows CI builds stay warning-free.
- Never weaken sandbox containment silently. In `crates/runtime/src/sandbox.rs`, construct `bwrap_args` purely from `HostLayout` and `SandboxPlan`, keep `env -i` cleansing when `host.env_program` is present, respect `SandboxKind` (`Native`, `Wine`, `Proton`, `PressureVessel`), and fail with `SandboxUnavailable` rather than silently running unsandboxed when `iroriSandbox` is enabled and bubblewrap cannot run.
- Keep recursive deletes and prefix removals gated by `fs_guard::ensure_deletable` (`crates/core/src/fs_guard.rs`), and check `prefix_is_shared` (`crates/runtime/src/lib.rs`) before warning or deleting a shared Wine prefix.
- Keep `crates/scripts` (`#![forbid(unsafe_code)]`) strictly confined to the install destination and Wine prefix with explicit `Approvals` consent (`crates/scripts/src/consent.rs`, `crates/scripts/src/exec.rs`) and rollback on failure; never let scripts depend on stores or UI crates.
- Keep `crates/engines` emulator discovery, RetroArch core mapping, and BIOS linking deterministic and offline-testable via `tempfile` directories. Never touch real `$HOME`, real prefixes, or real Steam/Lutris installs in unit tests; gate live smoke tests behind `#[ignore]` and explicit env vars (`IRORI_SANDBOX_SMOKE=1`, `IRORI_RUNTIME_SMOKE=1`).
- Keep runtime monitoring event-driven (`pidfd_open` with `poll` on Linux, async wakeups, zero idle polling) and keep `preview_launch` sharing the exact `compose_env` path as `prepare_launch` so the UI preview never drifts from real execution.

## Verification

```bash
cargo clippy --locked -p irori-runtime -p irori-engines -p irori-scripts --all-targets --profile ci -- -D warnings
cargo test --locked -p irori-runtime -p irori-engines -p irori-scripts --profile ci
```

## Few-shot examples

<example>
<input>
Task: Add a Linux-only Wine/Proton environment variable in `crates/runtime/src/env.rs` and verify that it applies on Linux without leaking onto macOS or Windows hosts.
</input>
<output>
1. Add the variable inside `wine_env_for(platform, input)` in `crates/runtime/src/env.rs` under the `HostPlatform::Linux` branch (keeping `HostPlatform::Mac` and `HostPlatform::Windows` clean).
2. Add an offline unit test constructing `HostEnv::default()` and `WineEnvInput` with `tempfile` or synthetic `Path::new("/tmp/...")` values:
   ```rust
   #[test]
   fn linux_only_env_does_not_leak_to_macos_or_windows() {
       let settings = GameSettings::default();
       let wine = test_proton_installation();
       let host = HostEnv::default();
       let input = WineEnvInput {
           settings: &settings,
           wine: &wine,
           prefix: Path::new("/tmp/prefix"),
           game_id: "demo",
           steam_client_path: Path::new("/tmp/steam"),
           proton_log_dir: Path::new("/tmp/logs"),
           eac_runtime: Path::new("/tmp/eac"),
           battleye_runtime: Path::new("/tmp/be"),
           host: &host,
       };

       let linux_env = wine_env_for(irori_core::HostPlatform::Linux, &input);
       let mac_env = wine_env_for(irori_core::HostPlatform::Mac, &input);
       let win_env = wine_env_for(irori_core::HostPlatform::Windows, &input);
       assert!(linux_env.contains_key("PROTON_ENABLE_WAYLAND"));
       assert!(!mac_env.contains_key("PROTON_ENABLE_WAYLAND"));
       assert!(win_env.is_empty());
   }
   ```
3. Run `cargo test --locked -p irori-runtime --profile ci`.
</output>
</example>

<example>
<input>
Task: Add or adjust a bubblewrap bind mount or flag in `crates/runtime/src/sandbox.rs` and verify the exact `bwrap` argv without launching bubblewrap on the host.
</input>
<output>
1. Update `bwrap_args(bwrap, host, plan)` in `crates/runtime/src/sandbox.rs` using `HostLayout` and `SandboxPlan` inputs (never reading real host paths directly inside `bwrap_args`).
2. Add an offline unit test constructing synthetic `HostLayout` (with `RootEntry::Dir`, `env_program`, `disable_userns`, `overlay`) and `SandboxPlan` (`SandboxKind::Native`, `SandboxKind::Wine`, or `SandboxKind::PressureVessel`), then assert on the returned `Vec<String>` argv (for example verifying `--unshare-net` when `settings.no_network` is true and verifying `--disable-userns` is omitted for `SandboxKind::PressureVessel`).
3. Run `cargo clippy --locked -p irori-runtime --all-targets --profile ci -- -D warnings` and `cargo test --locked -p irori-runtime --profile ci -- sandbox`.
</output>
</example>
