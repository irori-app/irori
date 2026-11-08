---
title: One click to play, tuned per game
seoTitle: 'Per-game settings, gamescope, ReShade, mods and cloud saves'
description: 'Play from the library, the palette, the tray, shortcuts or links. Per-game settings, gamescope, ReShade, mods, a launch troubleshooter and streaming to Moonlight.'
tag: Play
summary: One click to play from anywhere, with per-game settings that inherit your defaults and show what you changed.
icon: gamepad
screen: game
order: 3
updated: 2026-11-08
related: [wine, stores, stats]
highlights:
  - title: Play from anywhere
    body: Library tiles, the game page, Ctrl K, the tray menu, desktop and Steam shortcuts, and irori:// links.
  - title: Settings that inherit
    body: Every game starts from your defaults and shows exactly what you overrode.
  - title: gamescope with one switch
    body: Run any game in gamescope, with upscaling if you want it.
  - title: When a game won’t start
    body: Irori reads the last run, names the likely cause and offers a fix you can apply in one click.
  - title: ReShade and mods
    body: ReShade per game, and a mod manager for Thunderstore, Nexus Mods and FOMOD installers.
  - title: Saves that follow you
    body: Cloud saves sync before and after you play, and Ludusavi backs up any game, even while it runs.
  - title: Stream to Moonlight
    body: Your installed games appear in Sunshine or Apollo, with their covers, and stop when the stream does.
  - title: A sandbox for games you don’t trust
    body: On Linux, block the network or hide your home folder from a game.
  - title: Discord Rich Presence
    body: Show what you play on Discord, with the game’s art, or keep a game to yourself.
---

## One click, from anywhere

A game starts from its tile in the library, its own page, the command palette
(<kbd>Ctrl</kbd> <kbd>K</kbd>), the tray menu’s recent games, a desktop or Steam shortcut, or an
`irori://launch/<runner>/<app>` link. Heroic’s `heroic://` links work too. Because any web page
can open a link, `irori://launch` asks before it starts a game.

Before a game starts, Irori checks it is ready, syncs its cloud saves when that is on, prepares
Wine when the game needs it, and runs your before-launch script. While it runs, your computer
won’t go to sleep, and when it ends the session is added to your playtime. When a game starts
through a small launcher that exits once the game is up, Irori follows the game instead, so the
session and the Stop button still reach it.

## Settings that inherit

Each game’s settings start from your global defaults. Change one, and the game page shows that
it is overridden; reset it, and the game follows the default again. Settings are searchable, so
you don’t have to remember where an option lives.

## Tune without the wiki

- Run any game in **gamescope** with one switch, upscaling optional.
- Pick the **DXVK** and **VKD3D-Proton** versions a Wine build uses.
- Turn on **descriptor heap** and Proton’s **DLSS, FSR and XeSS** upgrades without looking up
  environment variables.
- **MangoHud**, **GameMode**, umu and the Steam runtime on Linux; **Msync**, the **Metal HUD**,
  Rosetta AVX advertisement and **Retina mode** on macOS.
- The game’s log can open by itself when it starts.

## When a game won’t start

When a game closes right after starting, or when you choose Troubleshoot launch (the game menu,
the command palette, <kbd>Ctrl</kbd> <kbd>Shift</kbd> <kbd>T</kbd> on its page), Irori lists
what it found about the last run, each cause with the log lines that show it:

- A **missing runtime DLL** under Wine or Proton: Irori installs `vcrun2022`, `d3dcompiler_47`,
  `dotnet48` or the other runtime into the game’s prefix, then starts the game again.
- **Anticheat** that refuses Wine is explained; one that works with the EAC or BattlEye runtimes
  is offered them on Linux.
- A **`vm.max_map_count`** too low on Linux is explained, with the two commands to copy. Irori
  never runs `sudo` itself.

## ReShade and mods

A Windows game Irori installed can get **ReShade** from its settings, on Linux, macOS and
Windows. Irori picks the right DLL from the game’s program, follows ReShade’s own compatibility
list, asks first when the game has anticheat, and downloads ReShade and its standard shaders from
their official sources. Turning it off or removing it leaves your presets and settings. Vulkan,
Direct3D 8 and DirectDraw games aren’t supported, nor are games Steam or another launcher
manages.

The **mod manager** keeps each game’s mods in their own folder, in a load order you set, and
installs them from archives, FOMOD installers, Thunderstore (BepInEx and MelonLoader games) and
Nexus Mods. Mods go into the game only while it runs from Irori, and Irori never deletes the
game’s own files; on Linux, with fuse-overlayfs installed, the game folder isn’t written at all.

## Saves, shortcuts and the rest

Cloud saves for Epic Games and GOG sync before and after you play. When saves changed both on
your computer and in the cloud since the last sync, Irori stops and asks which ones to keep
instead of guessing, and when the download before a game fails it asks whether to try again or
play anyway. With [Ludusavi](https://github.com/mtkennerly/ludusavi) installed, any game’s saves
can be backed up on request, after each session or every few minutes while you play.

“Add to Steam” puts any game in Steam, installed or not, under an “Irori” collection; Steam
starts Irori, which offers to install a game that isn’t installed yet. Irori also makes desktop
shortcuts and browses winetricks verbs for a game’s prefix. Discord Rich Presence, turned on in
Settings, shows the game you play with its art and a link to its store page, and any game can be
left out.

## Streaming with Sunshine

With Add games to Sunshine on (Settings › General › Integrations), Irori keeps your installed
games, or only your favourites, among the applications of [Sunshine](https://app.lizardbyte.dev/Sunshine/)
or its fork Apollo, each with its cover, plus an optional “Irori Big Picture” to pick a game
from the couch. The list follows your library, and only Irori’s own entries are ever changed.
Quitting in Moonlight stops the game in Irori, so playtime and cloud saves end as usual. A game
in gamescope can take the size, frame rate and HDR of the device you stream to, and any game can
be left out of Sunshine.

## A sandbox for games you don’t trust

On Linux, a game’s settings can run it in a [bubblewrap](https://github.com/containers/bubblewrap)
sandbox that blocks the network, hides your home folder, or both. The game starts from a clean
environment, so your session’s tokens and keys stay out, and the game page shows what it can and
can’t reach. When the sandbox can’t start, the game doesn’t start either, and Irori explains the
fix. The sandbox isn’t available inside Irori’s Flatpak or for emulated games.
