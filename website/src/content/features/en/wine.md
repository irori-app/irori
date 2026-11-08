---
title: Wine, Proton and Game Porting Toolkit, managed for you
seoTitle: 'Wine and Proton manager for Linux, CrossOver and GPTK on macOS'
description: 'Irori downloads GE-Proton, Proton-CachyOS, Wine-GE and Kron4ek on Linux, and Game Porting Toolkit, Wine-Crossover and Wine-Staging on macOS, with a prefix per game.'
tag: Wine and Proton
summary: Managed Wine and Proton builds on Linux, Wine, CrossOver and Game Porting Toolkit on macOS, a prefix per game.
icon: wine
screen: wine
order: 4
updated: 2026-11-08
related: [playing, emulators, downloads]
highlights:
  - title: Linux builds, downloaded for you
    body: GE-Proton, Proton-CachyOS, Wine-GE and Kron4ek, plus the builds already on your system.
  - title: macOS, three ways
    body: Game Porting Toolkit, Wine-Crossover and Wine-Staging-macOS, or your CrossOver and Whisky installs.
  - title: DXVK and VKD3D-Proton
    body: Installed and picked per Wine build, with DXVK-NVAPI and umu alongside, and a prefix per game.
  - title: Windows programs in a game’s prefix
    body: Open an installer, a patch or a registry file with Irori, and pick the game whose prefix runs it.
  - title: Roll back an update
    body: An updated Wine build keeps the one it replaced, so you can go back.
  - title: No Wine on Windows
    body: Windows games run directly, on x64 and arm64 Windows, with no Wine download at all.
---

## The right runtime on every system

Irori keeps the system running the launcher apart from each game’s own platform. A Windows game
needs Wine on Linux and macOS and runs directly on Windows. Native macOS, Linux and browser games
skip Wine entirely, and a game built for another native system is refused before launch instead
of failing mysteriously.

| System | Native games | Windows games | Builds Irori downloads |
| --- | --- | --- | --- |
| Linux x86-64 | Linux, browser | Wine, Proton or umu | GE-Proton, Proton-CachyOS, Wine-GE, Kron4ek |
| Linux arm64 | Linux, browser; x86 programs through FEX, Box64 or qemu-user | Proton with FEX built in | GE-Proton, Proton-CachyOS |
| macOS on Intel | macOS, browser | Wine, CrossOver | Wine-Crossover, Wine-Staging-macOS |
| macOS on Apple silicon | macOS, browser | Wine, CrossOver, Whisky, Game Porting Toolkit | Game Porting Toolkit, Wine-Crossover, Wine-Staging-macOS |
| Windows x64 and arm64 | Windows, browser | Runs natively | None needed |

## Linux

The Wine manager downloads GE-Proton, Proton-CachyOS, Wine-GE and Kron4ek builds, and finds the
ones already installed by your system, Steam, Lutris and Bottles. DXVK, VKD3D-Proton,
DXVK-NVAPI, gamescope, MangoHud, GameMode, umu and the Steam runtime are handled for you. For
Steam games, a game’s settings choose the Proton build Steam runs it with.

On arm64, Windows games run with the arm64 builds of GE-Proton and Proton-CachyOS, which carry
FEX inside Wine. An x86 program, a native Linux game or an x86 Wine build, needs an emulator
registered with the system, such as FEX, Box64 or qemu-user; without one, Irori says so and
links to FEX instead of starting a program that can only fail.

## macOS

Irori downloads Game Porting Toolkit, Wine-Crossover and Wine-Staging-macOS, and discovers Wine
app bundles, Homebrew installs, CrossOver and CrossOver Preview, and Whisky’s Wine. Game Porting
Toolkit builds need Apple silicon and macOS 14 or later; Intel Wine on Apple silicon needs
Rosetta 2, which Irori checks before it sets up a prefix.

To use the CrossOver app, create a bottle in CrossOver first and name it in the game’s Wine
settings. Irori runs the game in that bottle and never deletes a shared CrossOver bottle.
Retina mode makes Wine, CrossOver and Game Porting Toolkit render at your display’s full
resolution.

## Windows files and links in a game’s prefix

On Linux and macOS, a Windows program, installer, batch or registry file (`.exe`, `.msi`, `.bat`,
`.cmd`, `.reg`) opened with Irori asks which installed Windows game’s prefix should run it, most
recently played first, then runs it there with that game’s Wine build and settings. Irori shows
up under Open With; on Linux a button in Settings makes it the default for these files, and on
a Mac Finder’s Get Info does.

A link that a Windows program registered in its prefix, such as a game client’s own `someapp://`
invite, works the same way: hand it to Irori and pick the game whose prefix opens it. Nothing
runs until you pick a game, and Irori never takes over a link type for itself. Steam, third-party
launcher and emulated games aren’t offered, because their prefixes belong to someone else.

## Windows

Windows games run as they are, on x64 and arm64. Added apps, before- and after-launch scripts
and programs started alongside a game may also be PowerShell (`.ps1`) or Windows Script Host
(`.vbs`, `.js`, `.wsf`) scripts. A Windows game’s arguments are split by Windows’ own rules
everywhere, so paths with backslashes survive under Wine too.

## Updates you can undo

Wine builds, DXVK, VKD3D-Proton, DXVK-NVAPI, umu and the anticheat runtimes are checked once a
session and daily, on the same Updates page as your games. When a Wine build is updated in place,
the build it replaced is kept, so you can go back if a game stops working.

A runtime being available doesn’t guarantee a game works: DRM, anticheat, graphics APIs and the
chosen runtime still matter. Irori shows ProtonDB, Steam Deck and anticheat information on each
game’s page to help you decide, and its launch troubleshooter explains the most common failures.
