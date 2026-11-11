---
title: Irori vs Heroic Games Launcher
seoTitle: 'Irori vs Heroic Games Launcher: an honest comparison'
description: 'Irori and Heroic Games Launcher compared: the same stores, built-in protocols instead of legendary, gogdl and nile, plus Steam, emulators and a fifth of the memory.'
rival: Heroic
summary: The same stores and features, without Electron or helper tools, plus Steam, itch.io and emulators.
order: 1
updated: 2026-11-12
related: [playnite, lutris]
rows:
  - feature: Systems
    irori: Linux, macOS, Windows
    rival: Linux, macOS 14 or newer, Windows
  - feature: Built with
    irori: Rust and GPUI, one native process
    rival: Electron
  - feature: Epic Games, GOG, Amazon Games
    irori: Built-in Rust protocols
    rival: Through legendary, gogdl and nile
  - feature: ZOOM Platform
    irori: Yes, opt-in
    rival: Yes
  - feature: itch.io
    irori: Yes, opt-in
    rival: No
  - feature: Steam library
    irori: Installed games without a login; every owned game from the running client or a login
    rival: No
  - feature: Battle.net, EA app, Ubisoft Connect, Rockstar
    irori: Set up in their own prefix; games listed and started
    rival: No
  - feature: Microsoft Store and PC Game Pass
    irori: Installed and played by Irori on Linux; the Xbox app’s games on Windows
    rival: No
  - feature: Several accounts per store
    irori: Epic Games, GOG, Amazon Games, ZOOM, itch.io
    rival: No
  - feature: Cloud saves
    irori: Epic Games and GOG, asks on conflict
    rival: Epic Games and GOG
  - feature: Wine and Proton
    irori: Linux builds, plus GPTK, Wine-Crossover and Wine-Staging on macOS
    rival: Linux builds, plus GPTK, Wine-Crossover and Wine-Staging on macOS
  - feature: Emulators and ROM library
    irori: Yes, installed and updated by Irori
    rival: No
  - feature: Big Picture for controllers
    irori: Yes
    rival: Controller navigation
  - feature: Languages
    irori: '19'
    rival: About 40
  - feature: Memory, window open, empty profile
    irori: About 88 MiB, 1 process
    rival: About 478 MiB, 7 processes
  - feature: Licence
    irori: MIT
    rival: GPL-3.0
---

Heroic Games Launcher is the launcher most Linux players reach for first, and for good reason:
it is mature, well translated and widely used. Irori was built to match what Heroic 2.22.4 does,
then go further, without copying its code.

## Where they are the same

Both launchers install, update and play Epic Games, GOG and Amazon Games libraries, sync Epic and
GOG cloud saves, run Windows games through Wine or Proton on Linux and through Game Porting
Toolkit, Wine-Crossover or Wine-Staging on macOS, and run on Linux, macOS and Windows. Both add
games you bring yourself and both support ZOOM Platform.

Irori even reads Heroic’s settings with Heroic’s own names, which is why
[moving from Heroic](/guides/move-from-heroic/) is one click.

## Where Irori differs

- **No helper tools.** Heroic drives legendary, gogdl and nile, separate programs, for its stores.
  Irori’s store support is a Rust port of the parts of those tools it needs, built into the
  launcher.
- **No Electron.** Irori is one native process drawn on your graphics card. On the same Linux PC,
  with the window open and an empty profile, Irori 0.2.0 used about 88 MiB of memory in one
  process and Heroic 2.22.4 about 478 MiB across seven.
- **More in one library.** Your Steam games (installed ones without a login, the rest from the
  running Steam client or a login), itch.io, the games from Battle.net, the EA app, Ubisoft
  Connect and Rockstar, your Microsoft Store and PC Game Pass games (installed and played by Irori
  on Linux, through the Xbox app on Windows), and a ROM library with emulators Irori
  installs and updates. A game you own on several stores is one entry.
- **Several accounts per store.** Epic Games, GOG, Amazon Games, ZOOM Platform and itch.io each
  take more than one account, in one library.
- **Big Picture.** A full controller interface that opens by itself in the Steam Deck’s Game Mode.
- **MIT licence**, where Heroic is GPL-3.0.

## Where Heroic is ahead

- **Languages.** Heroic is translated into about 40 languages; Irori into 19.
- **Track record.** Heroic has years of releases and a large community. Irori is new: its first
  release was in October 2026.

## Which to choose

If you rely on a language Irori doesn’t have yet, Heroic is the better fit today. If you want
Steam, itch.io and emulators in the same library, several accounts per store, or a lighter
launcher, try Irori: it imports your Heroic setup without changing Heroic, so trying costs
nothing.
