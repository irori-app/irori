---
title: Play Epic Games on Linux
seoTitle: 'How to play Epic Games Store games on Linux'
description: 'Sign in to Epic Games in Irori, install your games and play them on Linux through Wine or Proton, with cloud saves and the EOS overlay. No Epic launcher needed.'
summary: Sign in, install and play your Epic Games library through Proton, with cloud saves.
platforms: [linux, steamdeck]
order: 1
updated: 2026-11-03
related: [gog-on-linux, steam-deck, move-from-heroic]
---

The Epic Games Store has no Linux app, but most of its games run well on Linux through Proton.
Irori speaks Epic’s protocol itself, so you don’t need the Epic Games Launcher, legendary or any
other helper.

## 1. Install Irori

Install Irori from your distribution’s repository or as a Flatpak; the
[download page](/download/#linux) has the commands for Debian and Ubuntu, Fedora and openSUSE,
Arch and its relatives. Install the sign-in window too: the packages ship it separately on Linux
because it brings WebKitGTK.

## 2. Sign in to Epic Games

Open **Accounts** and choose Epic Games. Epic’s sign-in page opens in Irori’s sign-in window, and
the window closes by itself once you have signed in. Without the sign-in window, or if you choose
to use the browser instead, the page opens in your browser and Irori watches the clipboard for
the code, or accepts it pasted.

Your library appears a moment later. Signed in with more than one Epic account? Add the others
from the same card: their games join one library.

## 3. Get a Wine or Proton build

Open the **Wine manager** and install a build. GE-Proton and Proton-CachyOS are good first
choices for most games; Wine-GE and Kron4ek builds are there too, along with the builds Irori
finds from your system, Steam, Lutris and Bottles. Pick the default in **Settings › Wine & Proton** (“Wine or Proton build”);
any game can use a different one in its own settings.

## 4. Install and play

Choose a game and **Install**. Pick the folder and, for games that offer one, the platform.
Downloads run in verified, parallel chunks, survive restarts and pause when you go offline. When
the download finishes, press **Play**.

Before the game starts, Irori prepares its Wine prefix, installs DXVK and VKD3D-Proton for the
build you chose, and syncs the game’s cloud saves when they are on. When the game ends, the saves
sync back and the session counts towards your playtime.

## Tips

- **Already installed with another launcher?** Import the folder instead of downloading the game
  again, or import your whole Heroic setup ([moving from Heroic](/guides/move-from-heroic/)).
- **Anticheat:** some online games refuse to run under Wine. Each game’s page shows its ProtonDB
  rating and anticheat information; check it before you buy.
- **Overlay:** the Epic Online Services overlay can be installed from Irori for games that use it.
- **From a terminal:** `irori install legendary/<app>` installs a game without opening the
  window, and Ctrl+C stops it; run it again to resume.
