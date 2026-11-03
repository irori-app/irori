---
title: Irori vs Lutris
seoTitle: 'Irori vs Lutris: comparing Linux game launchers'
description: 'Irori and Lutris compared: both run Windows games, emulators and lutris.net scripts on Linux; Irori adds macOS, Windows, cloud saves and built-in store protocols.'
rival: Lutris
summary: Both run Wine games, emulators and lutris.net scripts; Irori adds macOS, Windows and cloud saves.
order: 3
updated: 2026-11-03
related: [heroic, playnite]
rows:
  - feature: Systems
    irori: Linux, macOS, Windows
    rival: Linux
  - feature: Built with
    irori: Rust and GPUI
    rival: Python and GTK
  - feature: Epic Games
    irori: Built-in protocol, no launcher needed
    rival: Through the Epic Games Launcher in Wine
  - feature: GOG, Amazon, ZOOM, itch.io
    irori: Built in
    rival: Built in as services
  - feature: Humble Bundle and Game Jolt
    irori: No
    rival: Yes
  - feature: Cloud saves
    irori: Epic Games and GOG
    rival: GOG only
  - feature: lutris.net install scripts
    irori: Yes, with every step shown and approved first
    rival: Yes
  - feature: Emulators
    irori: Installed and updated by Irori, BIOS checked
    rival: Yes, as runners
  - feature: Controller interface
    irori: Big Picture
    rival: No
  - feature: Licence
    irori: MIT
    rival: GPL-3.0
---

Lutris has been the Swiss army knife of Linux gaming for years: a huge catalogue of community
install scripts, many runners, and services for most stores. Irori shares much of that ground,
and reads Lutris libraries directly.

## Where they overlap

Both run Windows games through Wine on Linux, play emulated games, install GOG games natively,
and run lutris.net install scripts. Irori’s ROM library and emulator management play the part
Lutris’s runners do, and its Wine manager finds the Wine builds Lutris installed.

## Where Irori differs

- **macOS and Windows.** Lutris is a Linux program. Irori runs on all three.
- **Epic Games without its launcher.** Lutris installs the Epic Games Launcher in Wine to reach
  Epic games; Irori speaks Epic’s protocol itself, and syncs Epic cloud saves too. Lutris’s cloud
  save sync covers GOG only.
- **Scripts with consent.** Before a lutris.net script runs, Irori shows every download, every
  write outside the game folder, every program and every shell command exactly as written, and
  asks you to approve them. A script with anything Irori doesn’t understand is refused.
- **Big Picture** for controllers and the Steam Deck.

## Where Lutris is ahead

- **Services.** Lutris connects to more services, including Humble Bundle and Game Jolt.
- **Runners.** Lutris supports more runners and engines than Irori’s built-in emulator list.
- **Maturity.** Lutris has a long history and a large script community.

## Which to choose

If you live in Lutris’s script catalogue or need a runner Irori doesn’t have, keep Lutris. If you
want Epic Games without its launcher, cloud saves, a controller interface or the same launcher on
a Mac, try Irori: [it imports your Lutris library](/guides/move-from-lutris/) read-only, so
Lutris stays exactly as it was.
