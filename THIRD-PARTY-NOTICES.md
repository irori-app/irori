# Third-Party Notices

This document records the upstream projects, vendored crates, bundled assets, and behavioral references used in Irori. The license of Irori itself is defined in [`LICENSE`](LICENSE).

## Protocol Ports

Several Rust crates in this workspace implement clean protocol support ported from open-source store clients:

| Workspace Crate | Upstream Reference | Upstream License | Notes |
| --- | --- | --- | --- |
| `crates/epic` | [derrod/legendary](https://github.com/derrod/legendary) | GPL-3.0 | Epic Games Store manifest, chunk download, library, cloud save, and overlay protocols |
| `crates/gog` | [Heroic-Games-Launcher/heroic-gogdl](https://github.com/Heroic-Games-Launcher/heroic-gogdl) | GPL-3.0 | GOG auth, content metadata, depot manifests, installers, and cloud saves |
| `crates/amazon` | [imLinguin/nile](https://github.com/imLinguin/nile) | GPL-3.0 | Amazon Games authentication, signed manifests, and SDK handling |
| `crates/msixvc` | [domsvoboda/nexus-launcher](https://github.com/domsvoboda/nexus-launcher) (Xodus) | MIT | MSIXVC package parsing, AES-XTS decryption, and streaming installation |
| `crates/msstore` | [domsvoboda/nexus-launcher](https://github.com/domsvoboda/nexus-launcher) (Xodus) | MIT | Microsoft Store and Xbox Live device auth, tokens, licensing, and catalog APIs |
| `crates/core` (`src/fs_part.rs`) | [agalwood/Motrix](https://github.com/agalwood/Motrix) (`packages/finalize-fs`) | MIT | Resilient `.irori-part` finalization with Windows lock retry backoff and download filename sanitization |

## Vendored and Patched Crates

These crates live inside the workspace with their upstream licenses preserved intact:

| Workspace Path | Component | License | License Files |
| --- | --- | --- | --- |
| `crates/comet` | Fork of [imLinguin/comet](https://github.com/imLinguin/comet) 0.4.1 (protocol definitions from [Yepoleb/gog_protocols](https://github.com/Yepoleb/gog_protocols)) | Apache-2.0 (`comet`), CC0 (`proto`) | [`crates/comet/LICENSE-APACHE`](crates/comet/LICENSE-APACHE), [`crates/comet/proto/LICENSE`](crates/comet/proto/LICENSE) |
| `crates/gpui-pre-windows` | Patched `gpui-pre-windows` 0.3.9 | Apache-2.0 | [`crates/gpui-pre-windows/LICENSE-APACHE`](crates/gpui-pre-windows/LICENSE-APACHE) |
| `crates/gilrs-core` | Patched `gilrs-core` 0.6.8 | MIT | [`crates/gilrs-core/LICENSE-MIT`](crates/gilrs-core/LICENSE-MIT) |

## Bundled Fonts, Themes, and Brand Assets

| Repository Path | Asset | License / Notice | Reference File |
| --- | --- | --- | --- |
| `assets/themes/` | Bundled `gpui-kit` theme presets | Apache-2.0 | [`assets/themes/LICENSE-APACHE-gpui-kit-themes`](assets/themes/LICENSE-APACHE-gpui-kit-themes) |
| `assets/fonts/` | Manrope font family | SIL OFL 1.1 | [`assets/fonts/OFL-Manrope.txt`](assets/fonts/OFL-Manrope.txt) |
| `assets/brand/stores/` | Store, launcher, and emulator SVG marks (Simple Icons 16.38.1) | CC0 1.0 (marks remain trademarks of their owners) | [`assets/brand/stores/NOTICE.md`](assets/brand/stores/NOTICE.md) |
| `assets/brand/` | Irori logo marks and banners | Original project artwork | [`assets/brand/README.md`](assets/brand/README.md) |

## Behavioral References

Irori also studies expected launcher, compatibility, and install-script behaviors from other open-source Linux gaming projects without copying their source code:

- [Bottles](https://github.com/bottlesdevs/Bottles) (GPL-3.0)
- [Heroic Games Launcher](https://github.com/Heroic-Games-Launcher/HeroicGamesLauncher) (GPL-3.0)
- [Heronite](https://github.com/RafaelGoulartB/Heronite) (GPL-3.0)
- [Lutris](https://github.com/lutris/lutris) (GPL-3.0)

## Trademark Notice

Steam, Epic Games, GOG, Amazon Games, Prime Gaming, itch.io, Xbox, Microsoft Store, ZOOM Platform, Battle.net, EA, Ubisoft, Rockstar Games, RetroArch, Dolphin, Lutris, and all associated logos and marks belong to their respective owners. Names and marks appear in Irori solely to identify compatible stores, runtimes, and games owned by the user and do not imply affiliation, sponsorship, or endorsement.
