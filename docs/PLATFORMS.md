# Desktop platform support

Irori separates the operating system running the launcher from the platform of
each installed game. A Windows build needs Wine on Linux/macOS and runs directly
on Windows. Native macOS, Linux, and browser builds bypass Wine. A build for a
different native OS is rejected before launch.

| Host | Native game builds | Windows game builds | Runtime manager |
| --- | --- | --- | --- |
| Linux x86-64 | Linux, browser | Wine / Proton / umu | GE-Proton, CachyOS, Wine-GE, Kron4ek |
| Linux arm64 | Linux, browser | Wine / Proton / umu | The arm64 builds of GE-Proton and CachyOS |
| macOS Intel | macOS, browser | macOS Wine / CrossOver | Wine-Crossover, Wine-Staging-macOS |
| macOS Apple silicon | macOS, browser | macOS Wine / CrossOver / Whisky / GPTK | Game-Porting-Toolkit, Wine-Crossover, Wine-Staging-macOS |
| Windows x64 | Windows, browser | Native execution | No Wine download required |
| Windows arm64 | Windows, browser | Native execution (x64 games through Windows' own emulation) | No Wine download required |

The GPTK tab is shown on macOS, but its builds are compatible only with Apple
silicon on macOS 14 or later. Intel Wine on Apple silicon requires Rosetta 2;
Irori checks it before initializing a prefix and reports a missing prerequisite.
Runtime availability does not guarantee game compatibility. DRM, anticheat,
graphics APIs, CPU instructions, and the selected runtime still matter.

## macOS compatibility layers

The manager downloads the same three macOS source families used by Heroic:

- [Game Porting Toolkit](https://github.com/Gcenx/game-porting-toolkit/releases).
- [Wine-Crossover](https://github.com/Heroic-Games-Launcher/wine-crossover/releases).
- [Wine-Staging-macOS](https://github.com/Gcenx/macOS_Wine_builds/releases).

Wine-Crossover is a downloadable Wine distribution. It is distinct from the
commercial CrossOver application and does not create CrossOver bottles.

Discovery includes Irori/Heroic downloads, Wine app bundles, Homebrew paths,
CrossOver and CrossOver Preview in `/Applications` and `~/Applications`, and
Whisky's Wine installation. Custom paths may point to a runtime executable.
CrossOver uses the bottle named in a game's Wine settings ("CrossOver bottle"),
located under `~/Library/Application Support/CrossOver/Bottles` unless General ›
Folders › "CrossOver bottles folder" points elsewhere (for bottles moved to another
drive in CrossOver's preferences). Create the bottle in CrossOver first. Irori uses
`CX_BOTTLE` and `CX_BOTTLE_PATH`, including for setup and wineserver commands, and
does not initialize an external bottle. Uninstall refuses to delete a shared CrossOver bottle; keep the prefix and manage it in CrossOver. Whisky uses its runtime with an Irori
prefix; existing Whisky bottles are not imported automatically.

macOS settings include Msync, Metal HUD, Rosetta AVX advertisement and Retina mode.
Retina mode sets `HKCU\Software\Wine\Mac Driver\RetinaMode` in the prefix before
launch, only when it differs from what `user.reg` already holds, so Wine, CrossOver
and Game Porting Toolkit render at the display's full resolution. Linux
wrappers, Fsync, GameMode, gamescope, MangoHud, Linux DXVK/VKD3D/NVAPI installation,
umu, and Linux anticheat runtimes are not automatically applied on macOS or
Windows, even when old settings enable them. Explicit custom environment variables
and wrappers remain user overrides.

Native `.app` launches read `CFBundleExecutable` from XML or binary Info.plist
and spawn that executable with the game's arguments and environment. This keeps
the process attached for exit status, logs, cancellation, and playtime.

## Build and package

Use Rust 1.95 or newer and Python 3.11+ for packaging; CI uses current stable Rust. Build on the target operating
system. Cross-compiling the complete GPUI/WebView stack is not the supported
release workflow.

```sh
cargo build --locked --release -p irori -p irori-sign-in
python3 packaging/package.py --platform macos --binary-dir target/release --output dist
```

Choose `linux`, `macos`, or `windows` for `--platform`; use `python` on Windows.
The output directory must not already contain the staging `Irori`/`Irori.app`.
Both binaries are required, so a package never silently omits store sign-in.

- Linux needs the GPUI libraries listed in CI, plus GTK 3 and WebKitGTK 4.1 for
  sign-in. `packaging/install.sh` retains the per-user Linux installation flow.
  `--appimage` also writes `Irori-X.Y.Z-x86_64.AppImage` (`-aarch64` on arm64) and its `.zsync` (see
  [AppImage](#appimage)). It needs [appimagetool](https://github.com/AppImage/appimagetool)
  (`IRORI_APPIMAGETOOL`, else on `PATH`) and `zsyncmake`; `IRORI_APPIMAGE_RUNTIME`
  names the AppImage runtime to use instead of the one appimagetool downloads.
- macOS needs Xcode Command Line Tools. The package includes an `.app` with the
  helper and `irori://` URL declaration. XZ support links statically; packaging rejects
  non-system dynamic libraries so Homebrew paths cannot leak into distributed binaries.
  Move it into Applications before making
  shortcuts. `package.py` ad-hoc signs local builds. Set `IRORI_SIGN_IDENTITY` to
  a Developer ID for distribution signing, then notarize and staple with Apple's
  tools. An ad-hoc signature is not notarization.
- Windows needs the MSVC Rust toolchain, Visual Studio C++ Build Tools/Windows
  SDK, and WebView2 Runtime for sign-in. Unzip and run the included `install.ps1`
  to install under LocalAppData and register a Start-menu shortcut and URL
  handler. Packaging includes the Visual C++ runtime DLLs for the binaries' architecture (x64 or arm64) from the installed
  Visual Studio redistributable directory, so Irori does not depend on build tools
  or an existing VC runtime installation on the user's machine. Set
  `IRORI_WINDOWS_CRT_DIR` to an explicit `x64` or `arm64` `Microsoft.VC*.CRT` directory if
  discovery cannot find it. Keep the build toolchain current and rebuild packages
  to ship runtime security updates; these app-local DLLs are updated with Irori.
  See Microsoft's [deployment guidance](https://learn.microsoft.com/en-us/cpp/windows/deployment-in-visual-cpp).
  No administrator access is required for Irori itself. Game
  redistributables may have their own UAC requirements. Public builds need
  Authenticode signing before distribution. `--portable` also writes
  `Irori-X.Y.Z-windows-portable.zip` (see [Portable Windows build](#portable-windows-build)).

Package names follow the binaries' architecture, read from them: x86-64 packages keep
their plain names (`Irori-X.Y.Z-linux.tar.gz`, `Irori-X.Y.Z-windows.zip`), and arm64 ones
add `-arm64` (`Irori-X.Y.Z-linux-arm64.tar.gz`, `Irori-X.Y.Z-windows-arm64-setup.exe`),
or `-aarch64` for the AppImage. `.deb` and `.rpm` names carry their own (`arm64`,
`aarch64`). Each architecture is built natively; there is no cross-compiling.

CI builds and tests Linux x86-64 and arm64, Windows x64 and arm64, macOS Intel and Apple silicon and uploads
unsigned validation packages using the small `ci` Cargo profile. It does not publish a release.
`release.yml` builds the release packages (tarball, AppImage, `.deb`, `.rpm`, Flatpak,
macOS disk images, the Windows installer, zip and portable zip), tests that they install and start, and
drafts a GitHub release for a `v*` tag; [RELEASING.md](RELEASING.md) covers signing
and the package channels.

## Saved logins

Store logins are JSON files under `<config>/stores/<store>/`, encrypted with AES-256-GCM.
The key is one random value in the system keyring: Secret Service on Linux (GNOME Keyring,
KWallet), Keychain on macOS, Credential Manager on Windows. Irori stores tokens, never
store passwords.

Without a keyring (no Secret Service running, a session without D-Bus), logins stay usable
as plain files only the user can read, and are encrypted the next time they are read with
a keyring available. If the keyring is locked or its key is gone, Irori leaves the files
alone, says so, and the stores look logged out until it is unlocked or the user logs in
again. Sandboxed packages need access to `org.freedesktop.secrets` to use the keyring.

On macOS an ad-hoc signed build asks for Keychain access after each update, because the
signature changes. A build signed with a Developer ID asks once.

## Paths and desktop behavior

| Data | Linux | macOS | Windows |
| --- | --- | --- | --- |
| Settings/credentials | XDG config / `irori` | `~/Library/Application Support/irori` | RoamingAppData / `irori` |
| Runtimes/data | XDG data / `irori` | `~/Library/Application Support/irori` | LocalAppData / `irori` |
| Cache | XDG cache / `irori` | `~/Library/Caches/irori` | LocalAppData / `irori` (dirs cache location) |
| Steam default | `~/.steam/steam` | `~/Library/Application Support/Steam` | ProgramFiles(x86) / `Steam` |
| Screenshots read | Spectacle's `imageSaveLocation`, Pictures/`Screenshots`, Pictures, replay clips (Videos/`Irori`/<game>) | `com.apple.screencapture` location (else Desktop), Pictures/`Screenshots` | Videos/`Captures` (Game Bar), Pictures/`Screenshots` |
| Start with the system (`iroriStartWithSystem`) | XDG config / `autostart/irori.desktop` | `~/Library/LaunchAgents/app.irori.Irori.autostart.plist` (opens the app bundle) | `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`, value `Irori` |

`IRORI_HOME` relocates settings, data and cache under one root and is useful for
isolated testing. On Windows a file named `portable` beside `irori.exe` does the same
with `irori.exe`'s folder as the root (`config`, `data` and `cache` beside it); see
[Portable Windows build](#portable-windows-build). `IRORI_HOME` wins over the file.
The start-with-system entry is one per user and belongs to the usual Irori: a run
with `IRORI_HOME` or a portable folder leaves an entry it didn't write as it is at
startup, writes its own only when the setting is turned on there, and removes only
an entry naming its folder. The Heroic import reads Heroic's native config folder or
the Flatpak's (`~/.var/app/com.heroicgameslauncher.hgl/config/heroic`); when both
exist, the one whose settings, installs or logins changed last. A configured Steam path takes precedence when it holds a Steam
library (`steamapps`); otherwise, on Linux, Irori looks at `~/.steam/steam`,
`~/.local/share/Steam`, `~/.steam/debian-installation`, Flatpak Steam
(`~/.var/app/com.valvesoftware.Steam`, `.steam/steam`, `.local/share/Steam` or
`data/Steam`) and Snap Steam (`~/snap/steam/common/.local/share/Steam`), in that order.
The first that lists its libraries (`steamapps/libraryfolders.vdf`) wins, else the
first with a library, so the empty `steamapps` a removed native Steam leaves behind
doesn't hide a Flatpak Steam. Adding games to Steam looks for `userdata` the same way.

The Wine manager installs the Wine and Proton builds it downloads into `<data>/tools/wine`
and `<data>/tools/proton`, or into `wine` and `proton` folders inside the folder set in
`iroriCompatToolsPath`; "Install Proton downloads into Steam" still sends Proton to Steam's
`compatibilitytools.d`. Builds stay where they were installed when the folder changes,
and Irori reads both places. A Wine or Proton update is dropped as soon as its build is
on the computer, wherever it came from (the Wine manager, ProtonUp-Qt, Steam).

On Linux, games start with room for 10 GiB of shader cache (`iroriShaderCacheSize`, per
game or as a default; 0 keeps the drivers' limits): `__GL_SHADER_DISK_CACHE_SIZE` for
NVIDIA and `MESA_SHADER_CACHE_MAX_SIZE` for Mesa. A value already set in Irori's own
environment or in the game's variables wins.

The Graphics settings of a game are Linux only; on macOS and Windows they show why.
Variables a game's own settings set always win over them; the host environment's
don't, since these are chosen per game.

- **Graphics card** (`iroriGpu`, a PCI address): cards come from `/sys/class/drm`,
  named from `pci.ids`. A Mesa card gets `DRI_PRIME=pci-…` and
  `MESA_VK_DEVICE_SELECT=<vendor>:<device>!` (left out when two cards are the same
  model), which also hides the other cards from DXVK; with an NVIDIA card present,
  `__VK_LAYER_NV_optimus=non_NVIDIA_only`. An NVIDIA card that doesn't drive the desktop
  gets PRIME render offload (`__NV_PRIME_RENDER_OFFLOAD`, `__VK_LAYER_NV_optimus=NVIDIA_only`,
  `__GLX_VENDOR_LIBRARY_NAME=nvidia`). A chosen card replaces "Use the discrete GPU".
- **NVIDIA Smooth Motion** (`iroriSmoothMotion`): `NVPRESENT_ENABLE_SMOOTH_MOTION=1`.
  Needs a GeForce RTX 40 series card or newer and a driver with the `VK_LAYER_NV_present`
  layer (575 and later).
- **NVIDIA Reflex in Vulkan games** (`iroriNvidiaReflex`): `DXVK_NVAPI_VKREFLEX=1` for
  Wine and Proton games while DXVK-NVAPI is on. DirectX 11 and 12 games get Reflex from
  DXVK and VKD3D-Proton without it.
- **Lossless Scaling frame generation** (`iroriLsfg`): needs lsfg-vk 2
  (`VkLayer_LSFGVK_frame_generation.json` in a Vulkan implicit layer folder) and
  Lossless Scaling in a Steam library (`lsfg-vk.dll` from its "lsfg-vk" branch, or
  `Lossless.dll`). Irori passes the game's choice as an environment profile
  (`LSFGVK_ENV=1`, `LSFGVK_MULTIPLIER`, `LSFGVK_FLOW_SCALE`, `LSFGVK_PERFORMANCE_MODE`,
  `LSFGVK_DLL_PATH`) and never writes lsfg-vk's configuration file.

The Steam library scans that Steam folder and every library in its
`steamapps/libraryfolders.vdf` for `appmanifest_*.acf` files, skipping Proton, the
Steam Linux Runtime and other tools, and watches those `steamapps` folders so installs
and removals made in Steam show at once. Art comes from Steam's own cache
(`appcache/librarycache/<id>`, where newer apps keep each image in a hashed subfolder),
else from the store (`IStoreBrowseService/GetItems`), else from the fixed CDN paths that
only older apps have.

Logging in to Steam is optional (Accounts › Steam). Irori draws Steam's own login: a QR
code for the Steam Mobile app, or the account name and password followed by Steam Guard.
The password goes to Steam only; Irori keeps the account's refresh token (about 200
days) in `<config>/stores/steam/session.json`, stored like every login (below), and renews
the access token itself. With an account, the library adds every game it owns and the games
its Steam Family shares (`library.json` in Steam's cache folder keeps them for offline
starts); Install hands those to Steam (`steam://install/<id>`), as Verify files and
Uninstall do (`steam://validate`, `steam://uninstall`).

Without a login, Accounts › Steam (and a suggestion in the library, the command palette)
can read the whole library from the running Steam client instead. Irori explains first, then
creates an empty `.cef-enable-remote-debugging` in Steam's folder (`~/.steam/steam`, Flatpak
Steam's own folder, `C:\Program Files (x86)\Steam`; on macOS
`~/Library/Application Support/Steam/Steam.AppBundle/Steam/Contents/MacOS`). Steam reads the
file when it starts and then serves Chromium's remote debugging on `127.0.0.1` (port 8080,
or whatever `--remote-debugging-port=` its `steamwebhelper` was given); Irori asks before
restarting Steam, and never restarts it in Game Mode. While the port is open any program on
the computer can control the Steam client, so by default Irori reads once and removes the
file again (the port closes at Steam's next start); "Keep it on" leaves it and re-reads on
library refreshes. A file another program created (Decky Loader) is left alone. The games
read are kept in `client-games.json` in Steam's cache folder and never leave the computer.
Millennium can open the port itself on another port, or keep it closed; Irori reads the port
from the web helper's command line and says so when it finds none.

Playing opens `steam://rungameid/<id>` through `xdg-open`, `open` or the Windows URL
handler, so Steam applies its own Proton version, launch options and overlay. A game's
launch options in Irori (its menu, `launcherArgs`) are passed with
`steam -applaunch <id> <args>` (`flatpak run com.valvesoftware.Steam` for Flatpak Steam),
or with `steam://run/<id>//<args>/` when Steam's program isn't found, where Steam asks
the user to confirm them. The opener exits at once, so the session follows the game
itself: on Linux Steam's per-game reaper (`reaper SteamLaunch AppId=<id>`), on every
system the processes running from the game's folder, and the manifest's running,
updating and validating flags while Steam prepares the launch. Found processes are
checked again directly every 3 seconds instead of scanning every process. Stop ends
the reaper and the game's processes, never the Steam client. Playtime is the larger of
the minutes Steam keeps in `userdata/<user>/config/localconfig.vdf` (the logged-in
account's own folder when there is one) and the account's own total.

On Linux a Steam game's settings (Wine & Proton) choose the compatibility tool Steam runs
it with, next to its ProtonDB rating: Steam's default, Valve's Proton builds installed in
any Steam library, or the tools in `compatibilitytools.d` (Steam's own folder, plus
`/usr/share/steam`, `/usr/local/share/steam` and `STEAM_EXTRA_COMPAT_TOOLS_PATHS` for a
native Steam; Flatpak Steam sees only its own). Proton builds from the Wine manager are
among them when "Install Proton downloads into Steam" is on. The choice is the game's
entry of `CompatToolMapping` in `<steam>/config/config.vdf`; Irori changes that entry
only, keeps the previous file as `config.vdf.irori-backup` and replaces the file
atomically. Steam rewrites the file when it exits, so Irori writes only while Steam is
closed and otherwise asks to close it first (`steam -shutdown`); in Steam's Game Mode it
can't, and the choice stays with the game's properties in Steam. The choice opens from
the game menu ("Steam Play…"), the command palette and the game page's Settings button.
A Steam game's settings list only what Irori still does around Steam's launch: the
launch arguments, scripts, programs started alongside, the replay buffer, Sunshine, save
backups and art.

itch.io (turned on in Settings › General › Integrations, or from its account card)
signs in with an API key from `https://itch.io/user/settings/api-keys`, kept owner-only
in the store's config folder. Installs download the upload that fits the platform into
`<install folder>/.itch-download/<game id>/` (resumable), then unpack zip and tar
archives (gzip, xz, bzip2) into the game folder; other single files such as AppImages
or portable `.exe` files are copied as they are, and disk images, 7-Zip and RAR files
are refused. Updates unpack next to the game and swap folders, so a failed update leaves
the old files in place. The program to start comes from the game's `.itch.toml` when it
has one, else from a search of the folder.

Epic Games, GOG, Amazon Games, ZOOM Platform and itch.io take several accounts each.
The first account keeps the store's usual folders (`stores/<store>` under the config
and cache roots), exactly as a single login always has. Each further account gets
`stores/<store>/accounts/<n>/` for its login and installs and `accounts/<store>/<n>/`
under the cache root; `stores/<store>/accounts.json` remembers which account each
folder holds. A game installed with one account belongs to it: updates, repairs, cloud
saves and launches use that account. For a game several accounts own, the install
sheet asks which account installs it (the first one listed by default), and the
library's account switcher shows one account's games while its store is shown. GOG's Galaxy service listens on one port, so while a game of one
GOG account runs, a game of another GOG account started alongside it gets no Galaxy
achievements. Galaxy's in-game overlay isn't available in Irori.

On Linux a Steam game's menu offers Protontricks once Steam has made its Proton prefix
(`steamapps/compatdata/<id>/pfx`). Irori runs `protontricks` from `PATH`, or the Flathub
app `com.github.Matoking.protontricks`, with `STEAM_DIR` set to the Steam folder it reads.
protontricks only knows Steam app IDs, so games Irori runs itself keep using winetricks.

Each configuration root has one running Irori instance, protected by an OS file
lock. Subsequent invocations forward arguments over authenticated loopback IPC.
The tray uses the desktop's native integration. macOS uses Command shortcuts and a native app menu; Windows/Linux retain Control shortcuts. Games inhibit sleep through the
Linux portal, macOS caffeinate, or Windows thread execution state. Desktop/menu
shortcuts use `.desktop`, Finder app bundles, or Windows shell links respectively.
Windows cancellation uses `taskkill /T /F`; games that deliberately escape their
launcher process tree still require game-specific testing. External EA/Ubisoft
launchers may reuse an existing process, which limits playtime tracking.
Xbox app and Microsoft Store games are Windows-only: they start through the shell
(`shell:AppsFolder\<family>!<app id>`), and their sessions and the Stop button
follow the processes running from the package folder. On Linux and macOS the Stores
page explains this instead of listing them.

The Xbox card's optional Xbox (Microsoft) account works on every system: it signs in
on Microsoft's page in the sign-in window (or the browser, pasting the address of the
blank page it ends on) and adds the account's playtime, last played times,
achievements and store art. PC games the account played that aren't installed are
listed as not installed everywhere, so their playtime and achievements count in the
stats on Linux and macOS too; only Windows installs and plays them, from their
Microsoft Store page. Packagers can point `IRORI_XBOX_CLIENT_ID` at their own
Microsoft app, or set it empty to leave the account out.

### Playnite import (Windows)

Settings → Advanced → "Import from Playnite" (shown when Playnite is installed) and
`irori import-playnite` read Playnite's library: the Playnite its `playnite://` link
opens, installed or portable, with the library folder its `config.json` names.
Playnite keeps its library open for itself while it runs, so close it first; Irori
only reads it. Installed games become added games that Playnite starts
(`playnite://playnite/start/<id>`); their sessions and the Stop button follow the
processes running from the game's folder (an emulated game's ROM folder), as the Xbox
app's games do. Games of Steam, Epic Games, GOG, Amazon Games, itch.io, the Xbox app,
Battle.net, the EA app, Ubisoft Connect and Rockstar are matched to Irori's own
listing instead, so their playtime, favourites and categories join the game Irori
already has. Removing an imported game from Irori leaves its files: Playnite
installed them.

### Windows files

A Windows program, installer, batch or registry file (`.exe`, `.msi`, `.bat`, `.cmd`,
`.reg`) opened with Irori asks which installed Windows game's Wine prefix opens it
(the most recently played first), then runs it there from its own folder with that
game's Wine build and settings. Registry files go to `regedit`; everything else goes
to Wine as it is, whose `start` hands batch files to `cmd` and packages to `msiexec`.
Steam games, third-party launcher games and ROMs are not offered: their prefixes
belong to someone else.

| | Linux | macOS | Windows |
| --- | --- | --- | --- |
| Offered under "Open With" | The packaged `irori.desktop` lists the file types | `Irori.app` declares them (`LSHandlerRank` Alternate) | Not offered: Windows runs them itself |
| Opened with a double click | On request (Settings → General → "Open Windows programs with Irori"): `irori-windows-files.desktop` in the user's applications folder and `xdg-mime default` for each type, replacing the current choice (often Wine's) | Finder's Get Info → Open with → Irori → Change All | — |

The file arrives as a path or a `file://` URL (`irori /path/setup.exe`); a second
instance makes a relative path absolute before forwarding it.

On Windows itself, an added app, a before- or after-launch script and a program started
alongside a game may be a script Windows can't start as a program
(`irori_core::process::program_command`): `.ps1` runs through
`powershell.exe -NoProfile -ExecutionPolicy Bypass -File`, and `.vbs`, `.vbe`, `.js`,
`.jse` and `.wsf` through `wscript.exe //NoLogo`. Batch files (`.bat`, `.cmd`) start as
they are.

#### Other programs' links

A link of a scheme that a Windows program registers in its prefix (a game client's
`ddnet:[::1]:8303`, `someapp://join/…`) can reach that program the same way: handed to
Irori, it asks which game's prefix opens it (the same games as for Windows files), then
runs Wine's `start <link>` there, which passes it to the program the prefix registered
for the scheme. Proton runs `start /wait <link>` with `PROTON_VERB=runinprefix`, and
under umu `UMU_CONTAINER_NSENTER=1` sends it into the Steam runtime container of a game
already running in that prefix. The link is one argument of that command and never goes
through a shell.

Irori forwards only links it doesn't open itself
(`irori_runtime::windows_uri::is_forwarded_uri`): never `irori:`, `heroic:`, `lutris:`,
`nxm:` or `file:`, never web pages (`http:`, `https:`, which belong in the user's own
browser, not a Wine one) or a browser's internal schemes (`about:`, `blob:`, `data:`,
`javascript:`), and never a link holding spaces, quotes or control characters, or one
longer than 4,096 characters. A single letter before the colon is a drive (`C:\…`), not
a scheme. Since any web page can open a link, the picker opens only when no other
dialog is showing, and the link starts only when the user picks a game and presses
Open. Windows hands links to their programs itself, so Irori there forwards none.

Irori registers no scheme besides its own: which schemes a prefix's programs take is
unknown, and claiming one would take it from a native client. The user points a scheme
at Irori themselves:

| | How |
| --- | --- |
| Linux | `xdg-mime default irori.desktop x-scheme-handler/<scheme>` (the packaged entry's `Exec=irori %u` passes the link); undo it by choosing another default program |
| macOS | Only from a terminal or script (`/Applications/Irori.app/Contents/MacOS/irori 'ddnet:[::1]:8303'`): macOS hands an app only the schemes its bundle declares, and `Irori.app` declares `irori` alone |
| Any | `irori <link>` from a terminal, a script or another program; a running Irori gets it from the second instance |

### Launch troubleshooting

When a game closes right after starting, or on "Troubleshoot launch" (game menu,
command palette, Ctrl+Shift+T on its page), Irori lists what it found about the last
run, each cause with the log lines showing it, and offers fixes. Games through Wine
or Proton run with `WINEDEBUG=-all,err+module,err+mscoree` unless the game's
variables or Irori's environment set `WINEDEBUG` or verbose logs are on, so a
missing DLL is named in the log even under Proton (whose own default is `-all`).

| | Linux | macOS | Windows |
| --- | --- | --- | --- |
| Missing DLLs (log and the program's imports) | Installs `vcrun2022`, `d3dcompiler_47`, `dotnet48`… with winetricks (umu's when umu runs the game; protontricks for Steam's own prefixes), then starts again | Same, with winetricks | Not offered: no Wine |
| Anticheat entry | Explains a denied or broken anticheat; offers the EAC/BattlEye runtimes | Explains it | — |
| `vm.max_map_count` | Below 1048576: explains it and copies the `sysctl` commands. Irori never runs `sudo` | — | — |

A fix tried this session is listed after the others.

### Screenshots

The stats page's Album reads screenshots from Steam's
`userdata/<account>/760/remote/<appid>/screenshots` folders (the app id names the
game) and from the screenshot folders in the table above, where a file counts for
the play session it was taken in. Irori never moves, renames or removes them.
gamescope's own screenshots land in `/tmp` and are not read; in Steam's Game Mode
Steam saves them instead. Videos (mp4, mkv, webm, mov) in those folders and in the
replay buffer's clip folders show with a play badge over their game's art and open in
the system's player.

### Replay buffer

Linux only. With Settings › Performance › Replay buffer turned on (for every game or
for one), a game Irori starts runs with
[gpu-screen-recorder](https://git.dec05eba.com/gpu-screen-recorder/about/) beside it in
replay mode, keeping the last 15 seconds to 10 minutes in memory. Irori never bundles
it: it runs `gpu-screen-recorder` from `PATH`, else the Flatpak
`com.dec05eba.gpu_screen_recorder` (user or system install, given access to the clip
folder with `--filesystem`), and the settings link to Flathub when neither is there.
Distribution packages exist for Arch, Fedora (Copr), Nobara, openSUSE, Debian/Ubuntu
and others.

The recorder is started as
`gpu-screen-recorder -w screen -c mp4 -f 60 -r <seconds> -bm cbr -q <kbit/s> -v no -a default_output -o <clips>/<game>`
(`-w portal -restore-portal-session yes` when the desktop should be asked once what to
record; `default_output|default_input` mixes in the microphone; no `-a` without sound).
Quality presets are constant bitrates (10, 20, 35 and 60 Mbit/s) because a constant
bitrate keeps the buffer the same size whatever is on screen, as the recorder advises
for replays; the settings show the resulting memory, about bitrate × length (35 Mbit/s
for 60 seconds is about 250 MB). Whole-screen capture on AMD and Intel uses the
recorder's `gsr-kms-server`, which the distribution packages and a system-wide Flatpak
install set up; a user Flatpak install asks for a password or needs portal capture.

A clip is saved by sending the recorder SIGUSR1. The recorder process is found by its
`-o` folder, so the Flatpak's process inside its sandbox is found like a native one.
It prints the saved `Replay_<date>_<time>.mp4`, which Irori renames to
`<game> <date> <time>.mp4` and confirms with a notice (a desktop notification while the
game has focus). The session's end sends SIGINT, which stops the recorder without
saving. Ways to save:

- a controller: hold Guide and press X (Square on PlayStation, X on Nintendo layouts),
  read by Irori's controller thread also while the game has the window; Guide's release
  then does not toggle Big Picture;
- the tray menu's "Save clip" and the command palette, while the buffer records;
- a key from the XDG GlobalShortcuts portal (KDE Plasma 6, GNOME 48, Hyprland). Irori
  registers as `app.irori.Irori` through the host app registry, suggests Alt+F10 and lets
  the desktop assign and change the keys. Where the portal is missing the settings say
  that keys pressed in a game cannot reach Irori and point to the other ways.

Clips land in `Videos/Irori/<game>/` (or the chosen folder), one folder per game named
after its title, which is how the Album finds the game; a folder whose game left the
library is matched to sessions by time like screenshots.

Cost. The capture and the encoding happen in the recorder's own process, on the GPU's
video encoder; Irori only waits on the recorder's output pipe and polls nothing. Measured
on Fedora/Nobara (KDE Plasma 6, Wayland, Radeon RX 7900) with Irori's backend running
headless for 20 seconds before and after starting a buffer, with a stand-in that behaves
like the recorder's replay mode:

| | Buffer off | Buffer on |
| --- | --- | --- |
| Irori CPU time in 20 s | 0 ticks (< 10 ms) | 0 ticks (< 10 ms) |
| Irori resident memory | 13.8 MB | 17.3 MB (+3.4 MB: the pipe reader and the portal connection of the clip shortcut) |

Saving a clip lists the processes once (to find the recorder) and renames one file;
stopping took 111 ms. The recorder's own memory is the buffer above (bitrate × length)
plus its encoder. It was not installed on the machine these numbers come from, so its
CPU and GPU cost while a game runs still has to be measured with the real program.

### In-game overlay (beta)

Off by default on every system (Settings › General › Controller › "In-game
overlay", `iroriOverlay.enabled`). With it on, the controller's Guide button over a
game Irori started (and on Windows and macOS also Ctrl+Shift+O, ⌃⇧O on a Mac) opens a
panel above the game instead of switching to Big Picture.
LB/RB switch its tabs, A/B and the D-pad or left stick move, and Guide or B close it;
after a click on it the keyboard works too (arrows, Enter, Escape, Page Up/Down).

- **Home**: the system's volume (the default output) and the game's (its streams
  together), changed with Left/Right and muted with A; a screenshot; Back to Irori
  (the main window, reopened from the tray if it was closed); quitting the game, which
  asks first with Cancel focused and then stops it as the game page's Stop does.
- **Screenshots**: what was taken during this session (the screenshot index, with the
  running session counted), shown large with A.
- **Achievements**: unlocked out of total, what this session unlocked and what is next
  (furthest progress first, then the most common). Opening the overlay asks the store
  afresh, at most once a minute per game.
- **Journal**: today's note for the game, written or edited in the usual editor. Typing
  needs the keyboard, so click the overlay first.

Unlocks the session made are announced one after another in the overlay's corner, with
their picture, when it opens (each once per run of Irori). Nothing is announced while
the overlay is closed: that would need polling the stores during play.

Shortcuts that work without opening it, also while the game has the window: Guide + RB
(R1) takes a screenshot, Guide + Up/Down change the system's volume by 5%, and Guide + X
still saves a clip of the replay buffer.

The surface on Linux. On Wayland desktops with the wlr layer-shell protocol (KDE Plasma, sway,
Hyprland, COSMIC, niri, labwc) the overlay is a layer surface on the overlay layer,
centred on the output, with on-demand keyboard interactivity: it never takes the
keyboard from the game on its own. GNOME has no layer-shell; there the overlay opens as
an ordinary window that takes focus, which GNOME may keep behind a fullscreen game
(windowed or borderless games work). On X11 it is an override-redirect notification
window that stays on top but never gets the keyboard. A game run in nested gamescope on
a desktop is covered like any window; inside gamescope's own session (Steam's Game Mode)
Steam decides what is shown and the overlay is not visible, so use Steam's overlay
there. The surface exists only while the overlay is open: closed, it costs no thread,
timer or CPU.

The controller on Linux. While the overlay is open each controller's event device is opened again
and grabbed (`EVIOCGRAB`), so the game sees none of its presses; a thread reads the
grabbed devices meanwhile and stops when the overlay closes. The grab is let go once no
button is held (at most 0.8 s later), so the game never sees the release of a press it
didn't see, and the kernel lets go by itself if Irori exits. Games that read the
controller through hidraw instead of evdev (Steam Input, SDL's HIDAPI drivers in some
games) still see the presses; the overlay works regardless.

On Linux the sound goes through `pactl` (PulseAudio's, which PipeWire's `pipewire-pulse` answers
too), found on the host inside the Flatpak. A game's streams are those whose process
(`application.process.id`, or `pipewire.sec.pid`) is in the game's process group or
descends from the process Irori started, which covers Wine's and Proton's processes.
Games another program owns (Steam, third-party launchers) have no process group of
Irori's, so only the system volume applies to them.

On Linux screenshots come from the desktop portal's Screenshot interface without its dialog
(KDE Plasma asks once whether Irori may), else from `grim` on wlroots desktops without
the portal. They are saved in the desktop's screenshot folder (Spectacle's choice, else
`Pictures/Screenshots`) as `<game> <date> <time>.png` and announced with a notice. The
overlay closes before its own screenshot is taken. gamescope's session has neither: use
Steam's screenshot there.

Per system (`irori_bigpicture::overlay::support` holds the same table):

| | Linux | macOS | Windows |
| --- | --- | --- | --- |
| Surface | layer-shell surface, else a pop-up window | non-activating panel above fullscreen apps | topmost tool window, shown without activating |
| Over a fullscreen game | yes (GNOME may keep it behind) | yes, in the game's fullscreen Space | borderless and windowed: yes; exclusive fullscreen: no, a notice says so |
| Opens with | Guide | Guide, ⌃⇧O | Guide (not on Xbox pads), Ctrl+Shift+O |
| Controller held from the game | yes (`EVIOCGRAB`) | no: the game sees the presses too | no: the game sees the presses too |
| System volume | default output (`pactl`) | default output (`osascript`) | default output (Core Audio) |
| Game volume | its streams | not available | its audio sessions (Core Audio) |
| Screenshot | portal, else `grim` | `screencapture`, Screen Recording permission | GDI copy of the game's monitor |

On macOS the overlay is GPUI's pop-up window: a non-activating `NSPanel` at the pop-up
menu level that joins every Space and may sit over fullscreen apps, so it shows over a
game in its own fullscreen Space without taking it out of it; the game keeps the keyboard
until the panel is clicked. ⌃⇧O is registered with Carbon's `RegisterEventHotKey`, which
needs no Accessibility permission, and only while a game Irori started runs. Holding the
controller from the game would mean seizing its HID device, which also takes it from
Irori's own reader, so the game sees the presses made in the overlay (B both closes it
and presses B in the game). macOS has no per-program volume, so the overlay's game volume
says "Not available on macOS" and only the system volume changes. Screenshots run
`screencapture -x -m` (the main display); the first one asks for the Screen Recording
permission, and until it is granted a screenshot fails with a notice saying where to
allow it (without it macOS would return only the desktop). On macOS 26 and later a
controller's Home button may also open the system's own game overlay.

On Windows the overlay is GPUI's pop-up window: a topmost tool window shown without
activating, which the desktop composes over windowed and borderless fullscreen games;
clicking it gives it the keyboard. A Direct3D game in exclusive fullscreen owns the
display and can't be covered (a window coming up could even take it out of fullscreen):
when Windows reports one (`SHQueryUserNotificationState`), the overlay doesn't open and a
notice asks to switch the game to borderless. Windows keeps the Xbox controller's Guide
button for the Xbox Game Bar and neither XInput nor Windows.Gaming.Input reports it to
other programs, so with Xbox pads use Ctrl+Shift+O (`RegisterHotKey`, registered only
while a game Irori started runs); PlayStation and other pads read as raw controllers may
report their Guide button. There is no supported way to hold an XInput or
Windows.Gaming.Input controller from a game, so the game sees the presses too. Volumes
are Core Audio's: the system volume is the default output's endpoint volume, and the
game's are its audio sessions (as in the volume mixer) on every active output, matched to
the process Irori started and its descendants started since the session began (Windows
reuses process ids and has no process groups). Screenshots copy the monitor of the
foreground window with GDI into `Videos\Captures`, next to the Game Bar's; that is what
the desktop composes, so a game in exclusive fullscreen or with HDR may come out black.

### Mods

A game's mods sit in `<data>/mods/<runner>/<app>/` and go into the game only
while it runs from Irori:

| | Linux | macOS | Windows |
| --- | --- | --- | --- |
| How mods are applied | Overlay with `fuse-overlayfs` when installed (the game folder is never written), else links | Hard links, else symbolic links, else copies | Hard links, else copies (symbolic links need administrator rights or developer mode) |
| `nxm://` links (Nexus Mods "Mod manager download") | On request: `irori-nxm.desktop` in the user's applications folder and `xdg-mime default` | Not available (link handlers come from the app bundle); Premium members install from the mod's page | On request: `HKCU\Software\Classes\nxm` |
| rar archives | `unrar`, `7z` or `bsdtar` on `PATH` | Same | Same, or 7-Zip in Program Files |

Kernel overlayfs is not used: an unprivileged user may mount it only in a user
and mount namespace of its own, which only the processes inside see, and
Steam's runtime container, Proton and Wine's server would all have to start
inside it as a remapped root. Links are recorded in `deployment.json` before
any file is touched, and a game file a mod replaces is renamed beside itself
(`<name>.irori-original`), so a crash at any point is undone exactly when
Irori next starts.

### ReShade

A Windows game Irori installed can get ReShade from its settings (Tools →
ReShade). Irori reads the game program's imports to pick the DLL, as
ReShade's installer does (`dxgi.dll` for Direct3D 10/11/12, `d3d9.dll`,
`opengl32.dll`), follows ReShade's `Compatibility.ini` (refusing games it
marks banned) and asks first when AreWeAntiCheatYet lists anticheat for the
game. Downloads come from reshade.me (the DLLs inside the setup program),
crosire/reshade-shaders (`slim`) and, under Wine, Mozilla's copy of
`d3dcompiler_47.dll`; they are cached in `<cache>/reshade`. Every file written
is recorded with its hash in `<data>/reshade/<runner>/<app>.json`.

| | Linux | macOS | Windows |
| --- | --- | --- | --- |
| Loading the DLL | `WINEDLLOVERRIDES` gets `<dll>=n,b` (and `d3dcompiler_47=n,b`) at launch, merged with the user's | Same, with Wine, CrossOver or Game Porting Toolkit | Windows loads it from the game folder; nothing to set |
| `d3dcompiler_47.dll` | Installed beside ReShade unless the game ships one | Same | Not needed (part of Windows) |

Turning ReShade off renames its DLL to `<dll>.irori-disabled`. Removing it
deletes only the files Irori wrote that are unchanged, so `ReShade.ini`
(which ReShade rewrites), presets and edited shaders stay. Vulkan games (which
need ReShade's layer registered), Direct3D 8 and DirectDraw games are not
supported, and neither are games other launchers manage (Steam, third-party
launchers).

### Storage

Settings → Storage (`irori://settings/storage`) groups what Irori measures by the disk
holding it: on Linux and macOS the deepest mount point a folder's real path (links
followed) starts with, so a games folder linked to another disk counts there; on Windows
the drive letter. Steam's and the third-party launchers' games are measured and shown
too, though Irori never removes them. Sizes are the files' lengths, measured on a
background thread and cached in `<cache>/storage.json` by folder and modified time for
up to a week; Rescan measures everything again.

Cleanups delete only what the rebuilt report offers at that moment, each item through
`fs_guard::ensure_deletable`:

| Cleanup | What goes | Never touched |
| --- | --- | --- |
| Download leftovers | `.irori-part` files in installed games with no download queued, folders in the games folder holding only an unfinished install (none while any download is queued), launcher installers in the Launchers cache | Folders without partial files, anything outside the games folder |
| Unused Wine prefixes | Prefixes (`system.reg`, or `pfx/system.reg` for Proton) in the prefix folder that no installed game, no launcher setup and the defaults' prefix use | Custom prefixes elsewhere, the prefix folder itself; none is chosen until the user ticks it |
| Old Wine and Proton builds | Builds Irori downloaded that no installed game, the defaults or a launcher uses, removed through the Wine manager's own removal; the newest of each kind stays | System Wine, Steam's Proton, Lutris and Bottles builds, CrossOver |
| Art thumbnails | `<cache>/images/thumbs` | Downloaded originals |
| Logs | Everything under the logs folder except the current `irori.log` | On Windows a log a running game holds open is skipped and goes next time |

### Interface font

The font list is the system's: fontconfig on Linux, Core Text on macOS and DirectWrite on
Windows, read once a session, so a font installed while Irori runs shows up after a
restart. Manrope, Irori's own, is always first. A picked font that is later uninstalled
falls back to Manrope.

### Interface size

Settings → Appearance → Interface size (`iroriUi.fontSize`, 75 to 150 %) scales text,
spacing and controls together, as do <kbd>Ctrl</kbd> and <kbd>+</kbd>, <kbd>−</kbd> or
<kbd>0</kbd> (<kbd>Cmd</kbd> on macOS, where the View menu has them too) and the command
palette. The display's own scale needs nothing: GPUI draws at it. On top of the size the
user picks, Irori follows the system's text size, live as it changes:

| | Source |
| --- | --- |
| Linux | `org.gnome.desktop.interface` `text-scaling-factor` through the XDG settings portal (GNOME's Large text and text scaling); without it, `GDK_DPI_SCALE` |
| macOS | None: macOS has no text size for apps |
| Windows | Settings → Accessibility → Text size (`UISettings::TextScaleFactor`) |

Factors outside 0.5 to 3 are ignored. Big Picture's own size multiplies on top.

### New versions of Irori

Irori looks for its own new versions only where nothing else updates it. It works out
how it was installed when it starts:

| Install | Recognised by | Irori |
| --- | --- | --- |
| Flatpak | `FLATPAK_ID` or `/.flatpak-info` | Never checks |
| apt, dnf, pacman, AUR | the program lives under `/usr` (not `/usr/local`) | Never checks |
| Nix | the program lives under `/nix/store` | Never checks |
| Built from source | the program lives in a Cargo `target` folder | Never checks |
| Any build with `IRORI_UPDATE_EXPLANATION` | the variable, at build or run time | Never checks; Settings › About shows its text |
| Homebrew | `Caskroom/irori` under `/opt/homebrew` or `/usr/local` | Checks; offers `brew upgrade --cask irori` |
| Scoop | `scoop/apps/irori` in the program's path | Checks; offers `scoop update irori` |
| winget | `install.ini` beside the program says `Channel=winget` | Checks; offers `winget upgrade Irori.Irori` |
| Windows setup, zip or portable zip, disk image, macOS zip, tarball, AppImage | none of the above | Checks; links to the release page |

The check asks GitHub's `releases/latest` for `irori-app/irori` (which leaves out drafts
and pre-releases) at most once a day, only while online and only in the app, never in the
commands without a window. Only `vX.Y.Z` tags count. `state/app-update.json` keeps the
time of the last answer, the newest version, the version the user dismissed and the
version that ran last. A newer release shows a notice that stays until closed; closing
it, or using its button, dismisses that version. Settings › About shows the state and
"Check now", and Settings › General › Updates turns the check off
(`iroriCheckForAppUpdates`). The first start of a new version says so, with a link to its
release notes. Irori never downloads or installs itself.

Packagers who build Irori themselves set `IRORI_UPDATE_EXPLANATION` (for example
"Update Irori with your distribution's package manager.") when building.

### Portable Windows build

`Irori-X.Y.Z-windows-portable.zip` unpacks to an `Irori-portable` folder that runs from
anywhere, a USB drive included. It holds the same programs and Visual C++ runtime as the
other zip, without `install.ps1`, plus a `portable` file. While that file is beside
`irori.exe` (`irori_core::paths::portable_root`), Irori:

- keeps settings, logins, state, logs, Wine builds and caches in `config`, `data` and
  `cache` next to `irori.exe`, never in AppData;
- registers no `irori://` handler and adds no Start menu shortcuts (Settings hides the
  option; "Add shortcut" puts one on the desktop only), so it writes nothing to the
  registry. Games and their own installers may still change Windows as they always do.

Logins stay encrypted with the key in this Windows account's Credential Manager, so on
another computer or account the stores ask to sign in again. Deleting the `portable`
file makes that copy use the usual AppData folders and leaves the folder's data where it
is. The marker is honoured on Windows only: a Unix `bin` folder may hold an unrelated
program called `portable`.

### AppImage

`Irori-X.Y.Z-x86_64.AppImage` (and `Irori-X.Y.Z-aarch64.AppImage` for arm64) holds `irori`, `irori-sign-in`, the desktop entry, the icon
and the AppStream metainfo, and nothing else: no libraries. It is built on Ubuntu 22.04
from the same binaries as the tarball and the `.deb`, so it runs where they run (glibc
2.35 or newer) and uses the system's graphics, Vulkan, Wayland, X11 and font libraries.
Bundling none keeps the download small (about 32 MB for Irori 0.3.0, against 34 MB for
the tarball) and keeps `LD_LIBRARY_PATH` out of the Wine, Proton and native games Irori
starts. It is compressed with zstd in 128 KiB blocks, appimagetool's default: its xz
setting came out larger (34 MB), and larger blocks start slower.

- **Sign-in.** `irori-sign-in` uses the system's WebKitGTK 4.1 and GTK 3. Where they are
  missing the window can't start, and Irori opens the store's page in the browser instead
  and reads the code from the clipboard, as with any install without the window.
  WebKitGTK isn't bundled: it needs its own helper processes, GPU drivers and the
  system's TLS certificates, and a partial copy breaks in ways the fallback doesn't.
  The store window (Stores → Open store) is the same program and falls back to the
  browser the same way.
- **Updates.** The AppImage carries update information
  (`gh-releases-zsync|irori-app|irori|latest|Irori-*-x86_64.AppImage.zsync`, or
  `-aarch64` on arm64, so an AppImage never turns into the other architecture's):
  AppImageUpdate, Gear Lever and AppImageLauncher download only the changed blocks
  from the latest release's `.zsync`. Irori itself treats it as a manual install
  (below) and links to the release page.
- **Desktop.** Shortcuts and the `irori://` handler point at the AppImage file
  (`$APPIMAGE`), not its temporary mount. It needs FUSE to mount itself; without FUSE
  run it with `--appimage-extract-and-run`. `<name>.AppImage.home` and `.config` folders
  beside it (the runtime's `--appimage-portable-home` and `--appimage-portable-config`)
  move Irori's data with it.

### Flatpak

`app.irori.Irori` sees the home folder and other drives, but its sandbox hides the host's
`/usr`, the host's processes and other apps' folders under `~/.var/app`. Programs the
user installed on the host therefore run there through `flatpak-spawn --host`
(`irori_core::host`, allowed by `--talk-name=org.freedesktop.Flatpak`):

- **Steam.** The manifest shares `~/.var/app/com.valvesoftware.Steam`, so a Flatpak
  Steam's library, playtime and `userdata` are read like a native one's. `steam://`
  links open with the host's `xdg-open`, and launches with arguments run the host's
  `steam` or `flatpak run com.valvesoftware.Steam`. The session follows the game on the
  host: `ps` lists the reaper and the game's processes, and Stop sends them `kill`.
- **Ludusavi.** The program chosen in settings is checked on the host, then `ludusavi`
  on the host's `PATH`, `~/.cargo/bin` and `~/.local/bin`, then the Flathub app
  (`flatpak info com.github.mtkennerly.ludusavi`). Each command runs on the host with
  `--watch-bus`, so cancelling it stops Ludusavi. The Ludusavi Flatpak gets the game's
  Wine prefix and the backup folder with `flatpak run --filesystem=…` (`:create` for the
  backup folder), because Irori's own prefixes live under
  `~/.var/app/app.irori.Irori`, which its sandbox hides; a path holding `:` can't be
  passed that way.

### Backups and sync

Settings › Backup and sync backs up on its own, daily or weekly (weekly unless changed),
and before a new version of Irori first runs. Backups are the same JSON files as one saved
by hand, named `irori-backup-<date>-<time>.json` (`…-before-<version>.json` before an
update), in `<data>/backups` or a folder the user picks; the newest few are kept (10
unless changed, and three made before updates), files saved by hand are never removed.
With "Downloaded game art" on, the art cache is copied into a `.art` folder beside the
backup. A chosen folder that isn't there (an unplugged drive, a share that isn't
mounted) fails the backup instead of being created. `state/backups.json` keeps the last
result and the version that ran last. Store sign-ins never go in, nor do the backup and
sync settings, which belong to each computer.

Sync shares favourites, hidden games, ratings, play statuses, categories, custom titles
and web art, yearly goals, playtime and the play history between computers. Each
computer writes one file, `Irori sync/<device id>.json`, into a folder another program
keeps in sync (Syncthing, Dropbox, Nextcloud, OneDrive, a network share), or into a
WebDAV folder directly. Install folders, per-game and app settings, art from local files,
recently played and store sign-ins stay on each computer. The WebDAV login is kept like
the store logins (see [Saved logins](#saved-logins)) in `<config>/sync/webdav.json`;
`state/sync.json` keeps the device id and what was last synced. How values merge is
described in `crates/services/src/sync/merge.rs`.

### Steam Deck and Steam Input

When Steam's controller interface starts Irori (`XDG_CURRENT_DESKTOP=gamescope`
in the Deck's Game Mode, or `SteamGamepadUI=1` from Steam Big Picture), Irori
opens in Big Picture and never starts hidden in the tray; Settings → General →
"Open Big Picture in Steam's Game Mode" turns this off. Controllers listed in
`SDL_GAMECONTROLLER_IGNORE_DEVICES` (or left out of
`SDL_GAMECONTROLLER_IGNORE_DEVICES_EXCEPT`) are not read, because Steam Input
drives them through its virtual controller, and arrow, Enter and Escape key
presses that echo a controller press within 150 ms are dropped. Steam
shortcuts carry an "Irori" collection tag and may point at games that aren't
installed; `--launch` then shows the install options instead of failing.

## Streaming with Sunshine

Irori finds Sunshine by its configuration folder: `~/.config/sunshine` on Linux and
macOS (`$XDG_CONFIG_HOME/sunshine` when set), the Flatpak's
`~/.var/app/dev.lizardbyte.app.Sunshine/config/sunshine`, and
`%ProgramFiles%\Sunshine\config` on Windows; Apollo, Sunshine's fork, keeps
`%ProgramFiles%\Apollo\config` on Windows and Sunshine's folder elsewhere. Irori tells the
two apart by their web API. Its web UI listens on `sunshine.conf`'s
`port` plus one (47990 by default); `iroriSunshine.port` overrides it. Irori talks to it on
`127.0.0.1` only.

Sunshine follows the whole process group (on Windows, the job object) of the command it
starts and ends it when the stream quits. `irori --launch … --wait` hands the game to the
running Irori and waits; when no Irori runs it starts one with `--minimized` in a process
group of its own and, on Windows, with `CREATE_BREAKAWAY_FROM_JOB`, so ending the stream
doesn't end Irori. Where a job forbids leaving it, that Irori ends with the stream. An
AppImage gives Sunshine `$APPIMAGE`, not its temporary mount.

## Sandboxed games (Linux)

A game, an added app or a Lutris script install can run in a
[bubblewrap](https://github.com/containers/bubblewrap) (`bwrap`) sandbox, for
programs the user doesn't trust. The per-game settings (Launch → Sandbox) have
two switches, `iroriSandbox.noNetwork` ("Block the network") and
`iroriSandbox.noHome` ("Hide the home folder"); the Lutris installer window
offers the same two, which then apply to the script's programs and to the game
it installs. The game page shows a "Sandboxed" card under Play ("Running
sandboxed" while it plays) that opens to list what the game can and can't
reach.

Inside, the game sees an empty root holding only:

| What | How |
| --- | --- |
| `/usr`, `/etc`, `/opt`, `/lib*`, `/bin`, `/sbin`, `/nix` | read-only (symlinks kept as symlinks) |
| Install folder, Wine prefix, `target_exe`'s folder | read-write |
| Wine or Proton build, umu-run, EAC/BattlEye runtimes, Steam's `legacycompat` | read-only |
| umu's Steam runtime (`~/.local/share/umu`) | read-only with a throwaway writable layer (`--overlay-src`/`--tmp-overlay`, bubblewrap 0.10+; older bubblewrap binds it writable) |
| GPU, controllers, sound | `/dev/dri`, `/dev/nvidia*`, `/dev/input`, `/dev/hidraw*`, `/dev/snd`; never `/dev/uinput` |
| Display, audio | the Wayland, PipeWire and PulseAudio sockets in `$XDG_RUNTIME_DIR`, the socket of `$DISPLAY`'s X server (`/tmp/.X11-unix/X<n>`, no other display's), the X cookie, gamescope's socket when gamescope is on |
| Environment | empty (`env -i`, `--clearenv`), then Irori's display, sound, locale, XDG, graphics driver and Wine/Proton/Steam runtime/umu variables, then every variable of the launch (the game's settings, its store's); nothing else of the session, so its tokens and API keys stay out, `/proc/1/environ` included |
| `/proc`, `/tmp`, `/run`, `/var/tmp`, `/dev/shm` | private (new PID namespace, fresh tmpfs) |
| `/sys`, `/run/udev`, fontconfig's cache, DNS configuration | read-only |

"Hide the home folder" puts `<data>/sandbox/<runner>/<app>` where the home
folder was (same path, so `$HOME` and the paths in Irori's variables stay
valid): native games keep their saves and settings there, the game card opens
it, and other drives (`/mnt`, `/media`, `/run/media`, any other top-level
folder) stay hidden. Without it the home folder and those drives are shown
writable, as outside the sandbox. "Block the network" gives the game a network
namespace with only loopback: no internet, no LAN, nothing listening on the
computer (GOG Galaxy's comet on port 9977 included); X11 clients then reach the
display through its socket in `/tmp/.X11-unix` instead of the abstract socket.
IPC stays shared, as X11 clients use shared memory with the server. In every mode
the session D-Bus is left out (it can start programs outside the sandbox) and
`DBUS_SESSION_BUS_ADDRESS` not passed, `--new-session` stops terminal input
injection, and `--die-with-parent` ends the sandbox with Irori's stop. A plain
Wine game waits for its prefix's wineserver inside the sandbox, as everything
in the sandbox ends with its first process; Proton's `waitforexitandrun` and
umu wait on their own. Under X11 the game still shares the display server with
every other window; the game card says so.

Wrappers: gamescope and gamemoderun run outside the sandbox (they need the
compositor and D-Bus; gamemoderun registers the sandbox, which lives as long as
the game), the user's wrappers, MangoHud and umu-run inside it.

**umu and pressure-vessel.** umu runs Proton in pressure-vessel, which is
itself bubblewrap. It nests inside Irori's sandbox: the outer sandbox has its
own PID namespace and a fresh `/proc`, so pressure-vessel can mount its own,
and it keeps user namespaces (games run without umu get
`--unshare-user --disable-userns` on bubblewrap 0.8+, so they can't create
more namespaces). pressure-vessel's own options are not the layer to restrict
a game with: it shares the home folder, D-Bus and the runtime directory by
design, isn't a security boundary, and doesn't apply to direct Proton, plain
Wine or native games. With the network blocked umu can't update the Steam
runtime (`UMU_RUNTIME_UPDATE=0` inside); Irori's unsandboxed launches keep it
current, and a sandboxed first launch needs the runtime already downloaded.

**When the sandbox can't start, the game doesn't.** Irori checks bubblewrap
before every sandboxed start (`bwrap --unshare-pid --unshare-net … true`) and
refuses with `SandboxUnavailable`, which the UI turns into a dialog with the
fix: bubblewrap missing (with the `apt`/`dnf`/`pacman`/`zypper` command),
unprivileged user namespaces refused (Ubuntu 24.04's AppArmor restriction, with
bubblewrap's own message), or Irori running as a Flatpak, whose seccomp
filter lets no program inside it create namespaces; there the native package
is the way to sandbox games. Emulated games (the ROM library) don't offer the
sandbox yet: their emulators may be Flatpaks, which can't start inside it.

`crates/runtime/src/sandbox.rs` builds the argument list as a pure function of
the host's layout and the game's needs; its tests cover each mode and runner
kind, and three opt-in tests start a real native program, Wine and umu in it:

```bash
IRORI_SANDBOX_SMOKE=1 IRORI_SANDBOX_PROTON="<Proton folder>" cargo test -p irori-runtime sandbox -- --ignored
```

## Window translucency

Settings → Appearance → Translucency lets the desktop show through the title bar
and sidebar (Subtle), or the pages too (Strong). The blur is the system's, so
Irori draws exactly what it draws when opaque; only some colours carry alpha.

| Host | Backdrop | Automatic |
| --- | --- | --- |
| macOS | Native vibrancy (blur only, untinted) | On |
| Windows 11 22H2+ | Mica | On |
| Windows 10 / 11 21H2 | Acrylic (lags while dragging) | Off |
| Linux, compositor with `ext-background-effect-v1` blur | Compositor blur | On |
| Linux, KWin before 6.7 on Wayland | KWin blur (`org_kde_kwin_blur`) | On |
| Linux, KWin on X11 | KWin blur (`_KDE_NET_WM_BLUR_BEHIND_REGION`) | On |
| Linux, Hyprland without the protocol | Hyprland's own blur | On |
| Linux, anything else | See-through, no blur | Off |
| gamescope | None | Off |

On Linux, Irori asks the compositor what it can do instead of going by the
desktop's name (`irori_ui::glass`, logged at start as `desktop blur: …`). KWin
6.7 removed `org_kde_kwin_blur`, the only blur protocol GPUI speaks, so Irori
binds `ext-background-effect-v1` itself on GPUI's Wayland connection. A
compositor that has the protocol with its blur switched off counts as having no
blur, and a change takes effect at the next start. The blur region is the visible
window, without the margin client-side decorations keep for their shadow. KWin
6.7 measures it from the window geometry rather than the surface, so Irori
offsets it there; if KWin starts following the protocol, the blur will sit 20
pixels off and `irori_ui::glass` needs to stop.

| Desktop | True blur from Irori's request |
| --- | --- |
| KDE Plasma 6.7+ | Yes, unless the Blur desktop effect is off |
| KDE Plasma before 6.7, Wayland or X11 | Yes |
| GNOME 51+ | Yes |
| GNOME before 51 | No. Blur my Shell can blur applications by window class |
| Hyprland | Yes |
| niri 26.04+ | Yes |
| COSMIC 1.3+ | Yes |
| Sway | No |
| SwayFX | No request: set `blur enable`, which is off by default |
| X11 without KWin | No. picom and the like can add their own by window class |

Irori has been run on KDE Plasma 6.7 (Wayland, and X11 windows under its
Xwayland). The other rows come from each project's own documentation and source.

Automatic also does as the system asks: it stays opaque while macOS's Accessibility →
Display → “Reduce transparency” is on, while Windows' Personalization → Colors →
“Transparency effects” is off (`EnableTransparency`, read as `UISettings`'
`AdvancedEffectsEnabled`), and in Windows high contrast. Both signal their changes, so the
window follows at once. Subtle and Strong are the user asking Irori, and stay.

Where nothing blurs, Automatic stays opaque. Subtle and Strong still work
because a compositor's own rule may add blur (match the app id or window class
`irori`), but the surfaces keep more body, as sharp detail behind text is hard
to read.

gpui-kit paints the dim behind dialogs and sheets across the whole surface, which
on Linux includes that margin, whatever the translucency level. So Irori keeps the
theme's `overlay` colour transparent there and the shell paints the dim inside the
visible window (`irori_ui::theme::modal_dim`). A dialog stacked on another no
longer dims the one below it on Linux.

The window is opaque in Big Picture and fullscreen, where nothing behind it is
worth showing, which also spares the compositor the blur. Views take their
surface colours from `irori_ui::glass` (`chrome`, `page`, `solid_pages`) rather
than adding alpha themselves.

## Omarchy

On [Omarchy](https://omarchy.org), the default theme is the desktop's. Irori reads
`~/.local/state/omarchy/current/theme/colors.toml` (`~/.config/omarchy/current/theme`
on older versions), derives its surfaces from that palette, and follows
`omarchy-theme-set` while it runs, so third-party themes work without Irori knowing
them. Text and icons are held to Irori's contrast minimums, which terminal palettes
don't always meet.

Settings → Appearance → Theme shows this as “Omarchy (default)”. Picking any other
theme stops following Omarchy, and “Follow system” takes its light or dark mode from
the Omarchy theme. Forcing the mode the Omarchy theme isn't in shows Irori's own theme.

## COSMIC

In a COSMIC session (`XDG_CURRENT_DESKTOP` names it, whichever distribution this is),
the default theme is the desktop's. Irori reads the `com.system76.CosmicTheme.Dark`
and `.Light` files, one per field, from `~/.config/cosmic`, falling back field by field
to the system's defaults in `XDG_DATA_DIRS`, and only from the newest `vN` folder, as
the schema moves with COSMIC. `com.system76.CosmicTheme.Mode` says which is showing.
Irori uses COSMIC's surfaces, accent, status colours and corner radius, and follows
changes while it runs. Its files can stay behind on a machine that no longer runs
COSMIC, which is why the session decides rather than the files.

COSMIC's compositor offers blur whatever the user chose, so translucency set to
Automatic follows Settings → Desktop → Appearance → Frosted glass → Windows
(`frosted_windows`) instead, on and off while Irori runs. The title bar and sidebar
then take the opacity of COSMIC's own windows, from `transparent_background`, which
both Frost thickness and Glass opacity move, as far as Irori's text stays readable.
The blur itself is the compositor's. Subtle and Strong stay the user's own choice.

Settings → Appearance → Theme shows this as “COSMIC (default)”. Picking any other theme
stops following COSMIC, and “Follow system” takes its mode from COSMIC. Unlike Omarchy,
forcing the other mode shows COSMIC's theme for that mode. If the files can't be read,
as with a schema Irori doesn't know yet, Irori's own theme stays.

## KDE Plasma

In a Plasma session (`XDG_CURRENT_DESKTOP` names `KDE`, whichever distribution this is,
so also a Steam Deck's desktop mode, but not its Game Mode), the default theme is the
desktop's colour scheme. `kdeglobals` also exists wherever a KDE app has run, which is
why the session decides rather than the files.

Irori looks up each colour on its own, the first place holding it winning:

1. `kdeglobals` in the user's config folder, then in its `kdedefaults` folder, then in
   `XDG_CONFIG_DIRS`. Plasma 6.8 writes the whole scheme here at first login and when
   one is applied.
2. The scheme `[General] ColorScheme` names, as `color-schemes/<name>.colors` in the
   user's data folder and then `XDG_DATA_DIRS`. No name means `BreezeLight`: KConfig
   drops a value that equals its default, so that line is often absent. After a Global
   Theme change the name is only in `kdedefaults`.

Checked in a real Plasma 6.8 session on Wayland, with System Settings' Colors page:
schemes, custom accents, the wallpaper's accent and a return to the scheme's own.

Pages use `[Colors:View]`, and the sidebar, title bar and cards `[Colors:Window]`, as
KDE's own apps do. Popovers use `[Colors:Button]`. The accent is `[General] AccentColor`
when there is one, else the view's `DecorationFocus`. Both hold the colour the user
picked: Plasma changes `[Colors:Selection]` from it to keep selected text readable, so
that is only the last resort. Status colours are the view's
`ForegroundNegative`, `ForegroundPositive` and `ForegroundNeutral`. Whether the theme
is dark goes by the view's background, as schemes don't say. Corners are Irori's own,
near Breeze's: schemes hold no radius.

Irori watches `kdeglobals` and follows every change while it runs: another scheme, an
accent colour or another Global Theme, whose scheme name is only in `kdedefaults`.
Translucency is unaffected: it depends on the compositor, as in the table above.

Settings → Appearance → Theme shows this as “KDE Plasma (default)”. Picking any other
theme stops following Plasma, and “Follow system” takes its mode from the scheme.
Forcing the mode the scheme isn't in shows Irori's own theme. If no background, text or
accent colour can be found, Irori's own theme stays.

The Flatpak has a config folder of its own, so it reads `~/.config/kdeglobals` and
`~/.local/share/color-schemes` through its access to the home folder. That is enough,
as `kdeglobals` holds the colours from first login on. It can't see the host's
`/usr/share/color-schemes`, so a `kdeglobals` that only names a system scheme gets
Irori's own theme there.

## Wallpaper colours (pywal, wallust, matugen)

Settings → Appearance → “Use colors from pywal, wallust or matugen”
(`iroriUi.wallpaperColors`, off by default) makes the default theme of the colours
these tools take from the wallpaper, on every system, and wins over a followed
desktop's theme while it is on. It is off by default because the files stay behind
whether or not the user wants Irori themed. “Colors file”
(`iroriUi.wallpaperColorsFile`) names the file; a leading `~` is the home folder.

- **pywal**: left empty, Irori reads `colors.json` in `$PYWAL_CACHE_DIR`, else
  `~/.cache/wal`. Its `special` background and foreground make the surfaces and text,
  `color4` the accent and `color8` the dimmer text. The sixteen colours are hues of the
  wallpaper rather than a red that means danger, so warnings and errors keep Irori's
  colours. Whether the theme is dark goes by the background (`wal -l` makes it light).
- **wallust** writes the same file when its templates include pywal's `colors.json`:
  `wal = { template = 'wal/', target = '~/.cache/wal/', pywal = true }` in
  `wallust.toml`. Its own cache is named after each wallpaper and doesn't say which is
  current, so Irori doesn't read it.
- **matugen** writes only what its templates say, so pick the file one of them
  renders. Irori reads Material You roles (`surface`, `on_surface`,
  `on_surface_variant`, `primary`, `secondary`, `tertiary`, `error` and the
  `surface_container*` roles), either as flat keys or in `matugen … --json hex`'s own
  shapes (`colors.primary.dark`, and matugen 2's `colors.dark.primary`), so
  `matugen image … --json hex > ~/.cache/matugen/irori.json` works too. A file with both
  modes gives Irori a light and a dark theme, and its `mode` decides which shows. A
  template, in `~/.config/matugen/config.toml`:

  ```toml
  [templates.irori]
  input_path = '~/.config/matugen/templates/irori.json'
  output_path = '~/.cache/matugen/irori.json'
  ```

  with `~/.config/matugen/templates/irori.json`:

  ```json
  {
    "primary": "{{colors.primary.default.hex}}",
    "secondary": "{{colors.secondary.default.hex}}",
    "tertiary": "{{colors.tertiary.default.hex}}",
    "error": "{{colors.error.default.hex}}",
    "surface": "{{colors.surface.default.hex}}",
    "on_surface": "{{colors.on_surface.default.hex}}",
    "on_surface_variant": "{{colors.on_surface_variant.default.hex}}",
    "surface_container_low": "{{colors.surface_container_low.default.hex}}",
    "surface_container": "{{colors.surface_container.default.hex}}",
    "surface_container_high": "{{colors.surface_container_high.default.hex}}",
    "surface_container_highest": "{{colors.surface_container_highest.default.hex}}"
  }
  ```

  Only `surface`, `on_surface` and `primary` are needed; Irori derives what is missing.

The tools write by renaming, so Irori watches the file's folder (or the one above,
until the tool first makes it), waits for writes to settle and re-themes only when the
colours changed. Nothing polls. Text and icons are held to Irori's contrast minimums.
Settings → Appearance → Theme shows this as “Wallpaper colors (default)”; picking any
other theme stops following them, and “Follow system” takes its mode from them.

## System accent colour

Wherever Irori doesn't follow a desktop's whole theme, macOS and Windows included, the
default theme stays Irori's own, in the system's accent colour. Settings → Appearance →
“Use the system accent color” (`iroriUi.systemAccent`, on by default) turns this off.

An accent the user picks in Settings → Appearance → Accent color (`iroriUi.accentColor`)
wins over the system's and a followed desktop's, and tints whichever theme shows, a picked
one included, held legible the same way. Clearing it hands the accent back. Windows high
contrast keeps its own colours.

### Linux

On a Linux desktop whose whole theme Irori doesn't follow, the default theme stays
Irori's own, in the desktop's accent colour. Irori asks the XDG settings portal for
`accent-color` in `org.freedesktop.appearance`, a single source, so no desktop's files
are read. Support comes from the desktop's portal backend:

| Desktop | Backend | `accent-color` |
| --- | --- | --- |
| GNOME 47+ | `xdg-desktop-portal-gnome` 47 | Yes. GNOME 46 has no accent setting |
| KDE Plasma | `xdg-desktop-portal-kde` 5.27.8+ | Yes, though Irori follows its whole theme instead |
| COSMIC | `xdg-desktop-portal-cosmic` | Yes, though Irori follows its whole theme instead |
| Cinnamon, MATE, Xfce | `xdg-desktop-portal-xapp` 1.1.1+ | Yes |
| Anything falling back to `xdg-desktop-portal-gtk` | | No |
| Hyprland, Sway and other wlroots compositors | `-hyprland`, `-wlr` | No: these have no settings portal, and rely on the fallback above |

Without an answer Irori's own orange stays, and a missing portal costs nothing. The
specification has values out of range mean “unset”, and they count as no accent.

A desktop's theme, where Irori follows one, already has its accent, so the portal isn't
asked then. If such a theme can't be read, as with a COSMIC schema Irori doesn't know
yet, the portal's accent still applies. Settings → Appearance → Theme keeps calling this
“Irori (default)”. Picking any theme, Irori Night and Irori Day included, shows it as it is.

Only what Irori paints in its own accent changes: buttons, links, focus rings, the caret,
selections, progress and the hover tint. A desktop's accent is made for its own surfaces
and can be anything (a Plasma selection colour may be near black), so Irori holds it to
3:1 on its surfaces where it fills or marks things and 4.5:1 where it writes, and an
accent that already reads stays exactly as the desktop has it.

Irori listens for the portal's `SettingChanged` signal, so changes show while it runs and
nothing polls. The first window waits up to 100 ms for the answer, so a portal that is
still starting can't hold Irori up; its answer then applies once it comes.

### macOS

Irori reads `NSColor.controlAccentColor` and the highlight colour,
`selectedTextBackgroundColor`, from System Settings → Appearance. Both are dynamic, so
each is resolved once in a light and once in a dark appearance (the high-contrast
appearances while “Increase contrast” is on), and Irori's light and dark themes each get
their own. Multicolour gives the system blue, as it does to Apple's apps, and Graphite
grey. The highlight colour becomes the selection colour where Irori's text reads on it;
selections are painted under the text, as in AppKit.

AppKit posts `NSSystemColorsDidChangeNotification` when either colour changes and
`NSWorkspaceAccessibilityDisplayOptionsDidChangeNotification` when a display option does.
Irori observes both on the main thread, so nothing runs between changes.

### Windows

Irori reads the accent from `UISettings.GetColorValue(Accent)` together with Windows' own
shades of it, `AccentLight1–3` for dark themes and `AccentDark1–3` for light ones, and uses
the first of them that reads as text on Irori's surfaces: Windows makes its shades for
this, so the result looks like Windows' own apps (light blue buttons on dark, deep blue on
light). If none reads, the closest is held legible like any accent.

`UISettings.ColorValuesChanged`, `UISettings.AdvancedEffectsEnabledChanged` and
`AccessibilitySettings.HighContrastChanged` signal changes on a thread of Windows' own, so
nothing of Irori's waits between them. “Show accent color on title bars” isn't followed:
Irori draws its own title bar in the theme's chrome colour.

### Accessibility

What the system asks for accessibility applies to every theme, including one the user
picked, since it is about being able to see Irori at all:

| Setting | Effect |
| --- | --- |
| macOS “Increase contrast” | Borders and dividers held to 3:1 on every surface; accents resolved in the high-contrast appearance |
| macOS “Reduce transparency” | Automatic translucency stays opaque |
| Windows high contrast (any contrast theme) | The theme is made of the system's colours (`Window`, `WindowText`, `Highlight`, `HighlightText`, `Hotlight`, `GrayText`, `ButtonFace`, `ButtonText`), with borders in the text colour; the mode follows the contrast theme |
| Windows “Transparency effects” off | Automatic translucency stays opaque |
| Any of these transparency settings, or COSMIC's frosted windows off | The library's background picture stays off |

Status colours in a high-contrast theme are Windows' console colours, held legible on the
contrast theme's window colour like any status colour.

## Verification and remaining machine tests

The regression suite checks the host/build matrix, unsupported runtime families,
Linux environment preservation, macOS environment isolation and CrossOver bottle
selection, release asset architecture selection, macOS bundle layouts and native
launches, argument quoting, instance forwarding, and native shortcuts. Platform
specific process tests run on their native CI hosts.

`postman/simulations/runtime-releases.context.md` explains the local HTTP test for
the three macOS release sources, including GitHub rate-limit errors. No real store
accounts or purchases are used by automated tests.

Before calling a platform release ready, run these checks on actual machines:

1. Sign in to each store using the packaged helper and browser fallback.
2. Install, update, repair, launch and uninstall a native game and a Windows game
   from each applicable store; test prerequisites needing elevation on Windows.
3. On macOS, launch one game with each installed Wine family, including a named
   CrossOver bottle; validate Metal rendering, sound, controllers, cloud saves,
   Rosetta, and Intel/Apple silicon behavior separately.
4. Verify cancellation kills child processes, tray quit/reopen, URL links while
   already running, shortcuts, and sleep inhibition on each desktop.
5. Validate signed/notarized release packages on clean machines without the
   development tools installed. Exercise anticheat/DRM only where the vendor
   supports the chosen runtime.

Automated checks do not establish that every game works. See the task's
verification report for which checks were actually run and their results.

## Reference inspection

Heroic Games Launcher was cloned and inspected at commit
[`509a34feea8541b73c928e289f2cf262d432db4a`](https://github.com/Heroic-Games-Launcher/HeroicGamesLauncher/tree/509a34feea8541b73c928e289f2cf262d432db4a).
The reference covered `src/backend/launcher.ts`,
`src/backend/utils/compatibility_layers.ts`, and
`src/backend/wine/manager/downloader/{constants,main,utilities}.ts`.
Irori uses its own Rust abstractions; the clone is reference material, not a
runtime dependency or a vendored copy.

Windows argument handling follows Microsoft's
[CRT parsing rules](https://learn.microsoft.com/en-us/cpp/c-language/parsing-c-command-line-arguments).
The parser/encoder has exhaustive short-input roundtrip tests covering quotes,
slashes, whitespace, Unicode and metacharacters.

On Windows, Epic Launcher synchronization accepts its native `C:\ProgramData\Epic\EpicGamesLauncher\Data` folder (or `Manifests` beneath it). On Wine hosts it continues to accept a Wine prefix.

macOS base-prefix creation temporarily suppresses Wine’s interactive Mono/Gecko downloader. Its built-in checksum validation is unchanged; the overrides apply only to `wineboot`, never to games or stored prefix settings. Games needing .NET/HTML dependencies must install the appropriate component through Wine tools/Winetricks. A successful prefix smoke test does not verify Mono or a .NET game.

Opt-in real archive/prefix smoke test (downloads roughly 200–240 MB per source; Rosetta required on Apple silicon): set a dedicated absolute `IRORI_HOME`, set `IRORI_RUNTIME_SMOKE` to `Wine-Crossover`, `Wine-Staging-macOS`, or `Game-Porting-Toolkit`, then run `cargo test -p irori-runtime macos_runtime_archive_smoke -- --ignored --nocapture`. Run it separately from the ordinary suite.
