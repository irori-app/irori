---
title: Move from Heroic Games Launcher
seoTitle: 'Switch from Heroic Games Launcher to Irori in one click'
description: 'Import your Heroic accounts, installed games, per-game settings, favourites and categories into Irori in one click. Heroic stays as it is.'
summary: One click brings your logins, installs, settings, favourites and categories along.
platforms: [linux, macos, windows]
order: 8
updated: 2026-11-03
related: [move-from-lutris, epic-games-on-linux, gog-on-linux]
---

Irori matches what Heroic Games Launcher can do, and reads Heroic’s files directly, so you can
switch without signing in again or downloading a single game twice.

## What comes along

- Your **Epic Games, GOG, Amazon Games and ZOOM Platform** logins.
- Your **installed games**, where they already are on disk, and the games you added to Heroic
  yourself.
- **Per-game settings**: Irori uses Heroic’s setting names, so they mean the same thing.
- Your **favourites** and **categories**.

Heroic itself is left unchanged: Irori only copies from its folder, including the Flatpak’s, and
never writes there. Where Irori already has something, Irori’s copy wins.

## Import from the first start

When Irori finds Heroic on its first start, onboarding offers **Bring over your Heroic setup**.
Choose **Import from Heroic**; a summary shows which stores came across and how many game
settings, favourites and categories were imported.

## Or from a terminal

```bash
irori import-heroic
```

The command imports the same things without opening the window.

## After the move

- Your games play from where Heroic installed them. Check one or two to see that their Wine
  builds came across, and pick a build in the Wine manager where one is missing.
- Links made for Heroic keep working: Irori understands `heroic://launch/...` links.
