---
title: Move from Lutris
seoTitle: 'Switch from Lutris to Irori: games, categories and playtime'
description: 'Import your Lutris games, favourites, categories and playtime into Irori, read-only. Windows, Linux and emulated games come along, and Lutris scripts still work.'
summary: Games, categories and playtime, read without touching Lutris; install scripts still work.
platforms: [linux]
order: 9
updated: 2026-11-03
related: [move-from-heroic, battlenet-ea-ubisoft-on-linux, itch-io]
---

Irori reads your Lutris library without changing anything in it, so you can try Irori and keep
Lutris as it is.

## What comes along

- **Windows and Linux games** become added games in Irori, with their Wine prefix, Wine build,
  arguments and environment variables.
- **Emulated games** join Irori’s ROM library, with the emulator they used.
- **Steam and store games** are matched to the same games in Irori’s stores by their ids.
- Your **favourites**, **categories** and **playtime**.

Lutris’s database is opened read-only, and nothing is written in Lutris’s folders. Every game that
isn’t imported comes back with the reason, for example a Wine build Irori couldn’t match by name,
so you know what to look at.

## Import from the first start, or from a terminal

When Irori finds Lutris, onboarding asks **Coming from Lutris?** Choose **Import from Lutris**,
then **Show details** to see what came across. From a terminal:

```bash
irori import-lutris
```

## Lutris install scripts still work

Irori runs lutris.net install scripts too. Choose **Add game › Install with a Lutris script…**,
use the command palette, or open an `irori://lutris/<slug>` link (Lutris’s own `lutris:<slug>`
works as an argument too). Before anything runs, Irori shows every download, every write outside
the game folder, the programs it will start and any shell command exactly as the script wrote it,
and asks you to approve them. A script with a command Irori doesn’t understand is refused instead
of half-run.

Scripts that would install the Epic Games Launcher, the EA app or Ubisoft Connect in Wine are
declined in favour of Irori’s own support, and GOG, Amazon and ZOOM scripts point you to the
store in Irori.
