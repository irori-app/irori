# Release QA Checklist

Attach a copy of this checklist to every release note to record what automated checks ran and which platforms were verified on physical hardware.

## Automated Build and Supply-Chain Checks

Verify these checks against the release tag before publishing the GitHub draft release:

- [ ] `cargo fmt --all --check`
- [ ] `cargo clippy --locked --workspace --all-targets --profile ci -- -D warnings`
- [ ] `cargo test --locked --workspace --no-fail-fast --profile ci`
- [ ] `cargo deny check`
- [ ] `python3 -m unittest discover -s packaging -p "test_*.py"`
- [ ] `appstreamcli validate --pedantic --explain packaging/linux/app.irori.Irori.metainfo.xml`
- [ ] `flatpak-builder-lint appstream packaging/linux/app.irori.Irori.metainfo.xml` and `flatpak-builder-lint manifest packaging/flatpak/app.irori.Irori.yml`
- [ ] Checksums and build provenance verified for downloaded release artifacts:
  - `sha256sum -c SHA256SUMS`
  - `gh attestation verify <artifact> --repo irori-app/irori`

## Real Hardware Test Matrix

Mark each target as tested on physical hardware or explicitly mark `Not tested on physical hardware`. Automated CI builds never replace physical testing.

| Platform | Target / Package | Status | Tester & Notes |
| --- | --- | --- | --- |
| Linux (Wayland) | Native tarball / `.deb` / `.rpm` / Arch | Not tested yet | Record distro, kernel, desktop, and GPU |
| Linux (Flatpak) | `.flatpak` bundle (`x86_64`) | Not tested yet | Verify host tools (`flatpak-spawn`), tray, and game launch |
| Steam Deck | `.flatpak` in Game Mode | Not tested yet | Verify Big Picture auto-open and controller input |
| macOS (Apple Silicon) | Signed and notarized `.dmg` | Not tested yet | Verify Gatekeeper, Wine/CrossOver/GPTK launch |
| macOS (Intel) | Signed and notarized `.dmg` | Not tested yet | Or mark `Not tested on physical hardware` |
| Windows 11 (`x64`) | Installer, zip, and portable zip | Not tested yet | Verify install, `irori://` handler, and clean uninstall |
| Windows 11 (`arm64`) | `windows-arm64-setup.exe` | Not tested on physical hardware | CI build/install/uninstall only unless physically tested |

## Manual Smoke Checks

Run these checks on each physically tested platform:

1. Cold start from a clean profile and confirm the window renders cleanly and tray integration works.
2. Sign in to at least one store (or load Steam local games) and confirm covers, metadata, and search/filters populate.
3. Launch and stop a game, verifying playtime tracking and clean process teardown.
4. Open the Wine / Emulator / Updates managers and verify build discovery or download flow.
5. Switch into Big Picture mode (`Ctrl+Shift+B`) and verify controller/keyboard navigation.
6. Verify Downloads Engine & Web Download Manager behavior:
   - **GOG & Epic chunk resume:** pause mid-install, quit and reopen Irori, confirm resume skips already-verified chunks and completes cleanly without corrupting files.
   - **Segmented HTTP direct links:** add an HTTP/HTTPS archive via `+ Add download` (or `Ctrl+V` on Downloads), pause/resume mid-transfer, verify speed cap applies live without restarting, and confirm optional auto-extraction + one-click `Play` registration.
   - **Opt-in BitTorrent (`aria2c` zero-seed):** drop a `.torrent` file or paste a `magnet:` URI, confirm explicit opt-in consent before starting, select a subset of files, verify download stops immediately at 100% with zero seeding and `aria2c` exits cleanly on idle/app quit.
   - **Windows ARM64 (Prism):** confirm x64 `aria2c.exe` auto-install and execution work smoothly under Prism emulation on Windows 11 ARM64.
   - **macOS quarantine:** verify extracted apps/binaries downloaded via web links have `com.apple.quarantine` handled cleanly and launch as expected under Gatekeeper rules.
   - **Linux Flatpak host `aria2c`:** verify `aria2c` discovery and loopback JSON-RPC execution via `flatpak-spawn --host` when installed on the host, and confirm clean fallback instructions when absent.
7. Uninstall cleanly (including verifying registry/handler/data expectations for the package format).
