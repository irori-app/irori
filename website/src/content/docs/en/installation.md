---
title: Installation
seoTitle: 'Install Irori on Linux, macOS and Windows'
description: 'Every way to install Irori: apt, dnf and pacman, Flatpak, AppImage, Homebrew, the disk image, the Windows installer, portable zip and Scoop, or from source.'
summary: Packages for every system, how they update, and building from source.
order: 2
updated: 2026-11-08
related: [getting-started, files-and-folders, troubleshooting]
---

Every release is on the [releases page](https://github.com/irori-app/irori/releases), and the
[download page](/download/) has the commands below with copy buttons.

## Linux

Linux builds are for x86-64 and arm64; the Arch packages are x86-64 only, as Arch is. The
packages put the sign-in window in a separate, recommended package, because it brings WebKitGTK;
Irori runs without it and signs in through your browser instead.

### Debian, Ubuntu, Linux Mint, Pop!_OS

```bash
sudo apt install -y curl
curl -fsSL https://packages.irori.app/irori.gpg | sudo tee /usr/share/keyrings/irori.gpg > /dev/null
curl -fsSL https://packages.irori.app/irori.list | sudo tee /etc/apt/sources.list.d/irori.list
sudo apt update && sudo apt install irori
```

### Fedora, openSUSE

```bash
sudo curl -fsSLo /etc/yum.repos.d/irori.repo https://packages.irori.app/irori.repo
sudo dnf install irori
```

### Arch, Manjaro, EndeavourOS, CachyOS

```bash
curl -fsSLo /tmp/irori.asc https://packages.irori.app/irori.asc
sudo pacman-key --add /tmp/irori.asc && sudo pacman-key --lsign-key 6DD7B72703FE3BD5C719F4CE6FE681AB798DF656
printf '\n[irori]\nServer = https://packages.irori.app/arch/$arch\n' | sudo tee -a /etc/pacman.conf
sudo pacman -Syu irori
```

### Flatpak and Steam Deck

Download `Irori-…-linux.flatpak` (`Irori-…-linux-arm64.flatpak` on arm64) from the
[latest release](https://github.com/irori-app/irori/releases/latest), then (on a Steam Deck, in
Desktop Mode):

```bash
flatpak remote-add --if-not-exists --user flathub https://dl.flathub.org/repo/flathub.flatpakrepo
flatpak install --user ./Irori-*-linux.flatpak
```

Irori isn’t on Flathub yet, so install each new version the same way.

### AppImage

One file for any distribution with glibc 2.35 or newer (Ubuntu 22.04, Debian 12, Fedora,
openSUSE, Arch and their relatives). Download `Irori-…-x86_64.AppImage` (`Irori-…-aarch64.AppImage`
on arm64) from the
[latest release](https://github.com/irori-app/irori/releases/latest), then:

```bash
chmod +x Irori-*-x86_64.AppImage
./Irori-*-x86_64.AppImage
```

It uses your system’s graphics libraries and, when it’s installed, its WebKitGTK for the
sign-in window; without WebKitGTK, Irori signs in through your browser. AppImageUpdate,
Gear Lever and AppImageLauncher update it in place, downloading only what changed. A
tarball is on the releases page too.

## macOS

macOS 12 or later, on Apple silicon and Intel.

```bash
brew install --cask irori-app/tap/irori
```

Or download the `.dmg` for your Mac from the
[latest release](https://github.com/irori-app/irori/releases/latest) and drag Irori to
Applications. The app is signed with Irori’s Developer ID and notarized by Apple.

## Windows

Windows 10 and 11, x64 or arm64. Download `Irori-…-windows-setup.exe` (`…-windows-arm64-setup.exe`
on arm64) from the
[latest release](https://github.com/irori-app/irori/releases/latest). It installs for your user
under `%LOCALAPPDATA%\Programs\Irori`, registers the `irori://` links and a Start-menu shortcut,
and never asks for administrator rights. Or with Scoop:

```powershell
scoop bucket add irori https://github.com/irori-app/scoop-bucket
scoop install irori
```

### Portable

`Irori-…-windows-portable.zip` runs from any folder, a USB drive too. Unpack it and start
`irori.exe` in the `Irori-portable` folder. While the `portable` file sits beside
`irori.exe`, Irori keeps its settings, logins, Wine builds and caches in that folder and
adds nothing to Windows: no Start menu entry, no `irori://` links, no registry keys.
Logins are encrypted with a key in your Windows account, so on another computer you sign in
to your stores again. Delete the `portable` file to switch that copy to the usual folders.

Windows builds aren’t signed yet, so SmartScreen may warn the first time: choose **More info**,
then **Run anyway**.

## Keeping Irori up to date

Irori looks for its own new versions only where nothing else updates it:

| Installed with | Irori |
| --- | --- |
| apt, dnf, pacman, AUR, Nix, Flatpak | Never checks: your system updates it |
| Built from source | Never checks |
| Homebrew | Checks, and offers `brew upgrade --cask irori` |
| Scoop | Checks, and offers `scoop update irori` |
| winget | Checks, and offers `winget upgrade Irori.Irori` |
| Windows setup, zip or portable zip, disk image, tarball, AppImage | Checks, and links to the release page |

The check asks GitHub at most once a day, only while online, and can be turned off in
**Settings › General › Updates**. Irori never downloads or installs itself. Flathub, winget, Fedora
COPR and the AUR are planned: releases are ready to publish to each once its account is set up.

## Building from source

Irori needs Rust 1.95 or newer. On Linux it needs the usual GPUI libraries (Vulkan, Wayland,
X11/xcb, xkbcommon, fontconfig, freetype); the sign-in window also needs WebKitGTK 4.1
(`webkit2gtk4.1-devel` on Fedora, `libwebkit2gtk-4.1-dev` on Debian and Ubuntu).

```bash
cargo build --release -p irori-sign-in && cargo run --release -p irori
```

To install a build for your user on Linux, with its desktop entry, icon and `irori://` handler,
run `packaging/install.sh`.
