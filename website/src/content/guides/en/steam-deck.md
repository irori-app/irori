---
title: Irori on the Steam Deck
seoTitle: 'Epic, GOG and Amazon games on Steam Deck with Irori'
description: 'Install Irori on the Steam Deck in Desktop Mode, add it to Steam and play your Epic, GOG, Amazon and itch.io games in Game Mode with Big Picture.'
summary: Install the Flatpak in Desktop Mode, add it to Steam, and play every store from Game Mode.
platforms: [steamdeck, linux]
order: 5
updated: 2026-11-03
related: [epic-games-on-linux, gog-on-linux, amazon-games-on-linux]
---

The Steam Deck runs Irori in both modes: the full launcher in Desktop Mode, and Big Picture with
the Deck’s controls in Game Mode.

## 1. Install Irori in Desktop Mode

Switch to **Desktop Mode** (Steam button › Power › Switch to Desktop). Download
`Irori-…-linux.flatpak` from the [latest release](https://github.com/irori-app/irori/releases/latest),
open Konsole and run:

```bash
flatpak remote-add --if-not-exists --user flathub https://dl.flathub.org/repo/flathub.flatpakrepo
flatpak install --user ./Irori-*-linux.flatpak
```

Irori isn’t on Flathub yet, so install each new version the same way.

## 2. Sign in and install games

Open Irori, sign in to your stores on the **Accounts** page and install a Proton build in the
**Wine manager**. Install a few games while you have a keyboard at hand.

## 3. Add Irori to Steam

In Steam (still in Desktop Mode), choose **Add a Non-Steam Game** and pick Irori. Back in Game
Mode, start it from your library. When Steam’s controller interface starts Irori, it opens
straight into **Big Picture** and never hides in the tray; a setting turns this off.

## 4. Play with the Deck’s controls

Big Picture runs the whole launcher with the controller: browse your library, install, change
settings and play. The Guide button opens and closes it. Irori reads each press once alongside
Steam Input: controllers Steam drives through its virtual controller are left alone, and key
presses that echo a controller press are dropped.

## Optional: games as Steam shortcuts

From a game’s menu, **Add to Steam** puts it in your Steam library under an “Irori” collection,
installed or not. Starting it from Steam starts Irori, which offers to install a game that isn’t
installed yet. Steam games themselves already appear in Irori’s library, read from Steam’s own
files.
