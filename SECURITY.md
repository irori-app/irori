# Security Policy

## Supported Versions

Security fixes apply to `main` and the latest `v1.x` minor release. Older `v1.x` releases receive fixes through the next `v1.x` release rather than backported patch branches.

| Version | Supported |
| --- | --- |
| `main` | Yes |
| Latest `v1.x` release | Yes |
| Older `v1.x` releases | Upgrade to latest `v1.x` |

## Reporting a Vulnerability

Report security issues privately through GitHub Security Advisories:

https://github.com/irori-app/irori/security/advisories/new

Do not open public GitHub issues, discussions, or pull requests for suspected vulnerabilities, credential leaks, or unsafe script execution paths.

When reporting an issue, include:

- The affected Irori version or commit SHA and your operating system.
- Clear steps to reproduce the issue, including any sample script or configuration involved.
- What data, file path, or capability is exposed or affected.

We aim to acknowledge reports within 72 hours and coordinate a fix and release before any details are published.

## Credential Storage

Store credentials, refresh tokens, session cookies, and third-party API keys (IGDB, GG.deals, SteamGridDB, Nexus Mods, RetroAchievements, Sunshine, WebDAV) are managed by [`crates/secrets`](crates/secrets/src/lib.rs) and [`crates/sign-in`](crates/sign-in/src/lib.rs):

- Per-store login files and API key files (including `<config>/steamgriddb.json`, `<config>/igdb.json`, and `<config>/ggdeals.json`) are encrypted with AES-256-GCM (`aes_gcm::Aes256Gcm`) inside a versioned JSON envelope (`irori_sealed`, `nonce`, `data`).
- Legacy SteamGridDB keys stored in `settings.json` by earlier versions are migrated automatically into `irori-secrets` on first read and cleared from `settings.json`.
- The encryption key is kept in the OS keyring through `keyring-core` (`zbus-secret-service-keyring-store` on Linux, `apple-native-keyring-store` on macOS, and `windows-native-keyring-store` on Windows) and loaded on demand with a 30-second unlock wait.
- When no usable keyring is available in the environment, Irori writes files atomically using temporary files created with user-only (`0o600` on Unix) permissions.
- Interactive web logins run in the separate `irori-sign-in` helper process (`crates/sign-in`). OAuth codes, passwords, cookies, tokens, and authorization headers are kept out of logs and crash dumps.

## Memory Safety Guarantees

Store protocol parsers, cryptography wrappers, archive handlers, script interpreters, PE header readers, and credential vaults prohibit `unsafe` Rust code at compile time with `#![forbid(unsafe_code)]`:

- `crates/amazon` (`crates/amazon/src/lib.rs`)
- `crates/depot` (`crates/depot/src/lib.rs`)
- `crates/epic` (`crates/epic/src/lib.rs`)
- `crates/gog` (`crates/gog/src/lib.rs`)
- `crates/gog-galaxy` (`crates/gog-galaxy/src/lib.rs`)
- `crates/itch` (`crates/itch/src/lib.rs`)
- `crates/launchers` (`crates/launchers/src/lib.rs`)
- `crates/msixvc` (`crates/msixvc/src/lib.rs`)
- `crates/msstore` (`crates/msstore/src/lib.rs`)
- `crates/reshade` (`crates/reshade/src/lib.rs`)
- `crates/retroachievements` (`crates/retroachievements/src/lib.rs`)
- `crates/scripts` (`crates/scripts/src/lib.rs`)
- `crates/secrets` (`crates/secrets/src/lib.rs`)
- `crates/steam` (`crates/steam/src/lib.rs`)
- `crates/xbox` (`crates/xbox/src/lib.rs`)
- `crates/extras/src/shortcuts/pe_icon.rs`
- `crates/services/src/troubleshoot/pe.rs`

## Filesystem and Install Script Guardrails

### Recursive Deletion Guard

[`crates/core/src/fs_guard.rs`](crates/core/src/fs_guard.rs) (`check_deletable` and `ensure_deletable`) checks every recursive deletion target before removing a game, Wine prefix, or download directory. Deletions are refused for:

- Relative paths or paths containing `..` parent traversal segments.
- Paths shallower than two components.
- System directories (`/bin`, `/boot`, `/dev`, `/etc`, `/lib`, `/lib32`, `/lib64`, `/proc`, `/root`, `/run`, `/sbin`, `/snap`, `/sys`, `/usr`, `/var`).
- The user's home directory, its ancestors, or its direct children (such as `~/Games` or `~/Downloads`).
- Protected folders, including directories managed by other game launchers.

### Community Install Script Interpreter

[`crates/scripts`](crates/scripts/src/lib.rs) parses Lutris community install scripts into a typed execution plan rather than evaluating arbitrary code:

- Unknown commands, tasks, parameters, and variables fail parsing and name the offending step.
- Wine scripts that install store clients Irori already supports natively (Epic Games Launcher, EA app, Ubisoft Connect) are refused with `ScriptError::PreferNative`.
- `consent` enumerates all downloads, executables, verbatim shell commands, registry edits, and writes outside the game directory so the user reviews and approves them explicitly via `Approvals` before execution.
- File operations are confined to the game directory, cache, prefix, and approved paths. Failed or cancelled installations roll back changes made to the game directory.

## Network and Privacy Policy

Irori collects zero telemetry, embeds no analytics SDKs, and does not upload crash dumps automatically.

Network requests fall into three categories (enforced in CI by [`.github/scripts/check_security_domains.py`](.github/scripts/check_security_domains.py)):

1. Store backends and third-party launchers you sign in to or enable:
   - Steam (`store.steampowered.com`, `api.steampowered.com`, `login.steampowered.com`, `steamcommunity.com`, `steamstatic.com`, `steamdb.info`)
   - Epic Games (`epicgames.com`, `heroic.legendary.gl`)
   - GOG (`gog.com`, `gog-statics.com`)
   - Amazon Games (`amazon.com`, `gaming.amazon.com`)
   - itch.io (`itch.io`)
   - ZOOM Platform (`zoom-platform.com`)
   - Xbox / Microsoft Store (`xboxlive.com`, `xboxservices.com`, `xbox.com`, `microsoft.com`, `login.live.com`, `passport.net`, `gamepass.com`)
   - Third-party Windows launcher installers (`battle.net`, `origin-a.akamaihd.net`, `static3.cdn.ubi.com`, `rockstargames.com`)
2. Optional or user-triggered integrations (stay idle until enabled or explicitly triggered by the user):
   - Metadata and compatibility: IGDB / Twitch (`igdb.com`, `id.twitch.tv`), SteamGridDB (`steamgriddb.com`), VNDB (`vndb.org`), LaunchBox (`launchbox-app.com`), ProtonDB (`protondb.com`), PCGamingWiki (`pcgamingwiki.com`), HowLongToBeat (`howlongtobeat.com`), OpenCritic (`opencritic.com`), Metacritic (`metacritic.com`), RetroAchievements (`retroachievements.org`), libretro thumbnails and cores (`libretro.com`), and AreWeAntiCheatYet (`raw.githubusercontent.com`)
   - Deals and giveaways: CheapShark (`cheapshark.com`), GamerPower (`gamerpower.com`), and opt-in GG.deals (`gg.deals`)
   - Mods, graphics mods, and install scripts: Nexus Mods (`nexusmods.com`), Thunderstore (`thunderstore.io`), Lutris (`lutris.net`), ReShade (`reshade.me`), RenoDX / OptiScaler / Wine / Proton / emulator releases (`github.com`, `githubusercontent.com`, `dawn.wine`, `umu.openwinecomponents.org`), Flathub info links (`flathub.org`), Sunshine docs (`lizardbyte.dev`), and rclone downloads (`rclone.org`)
   - Voluntary log / crash report uploads: `dpaste.com` (only when you explicitly click upload, after home-path redaction)
3. Connectivity check in automatic network mode (`iroriNetworkMode = auto`):
   - Irori queries the operating system first (`ashpd::NetworkMonitor` on Linux and `GetNetworkConnectivityHint` on Windows) without sending external traffic.
   - Only when the operating system cannot report connectivity (such as macOS or minimal Linux sessions without a desktop portal) does Irori send a lightweight `HEAD` request to `www.gstatic.com/generate_204` to pause/resume downloads when going offline. Working offline (`iroriNetworkMode = offline`) disables all probes and outbound requests.

## Verifying Release Artifacts

Release artifacts published on GitHub include integrity checksums and build provenance:

1. Check downloaded archives and installers against the `SHA256SUMS` file attached to the release.
2. Verify SLSA build provenance generated by `actions/attest-build-provenance` in [`.github/workflows/release.yml`](.github/workflows/release.yml):

   ```bash
   gh attestation verify <downloaded-artifact> --repo irori-app/irori
   ```

Official release workflows also run Apple notarization (`xcrun notarytool`) for macOS builds and SignPath code signing for Windows builds when release signing credentials are configured.
