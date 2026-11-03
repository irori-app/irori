---
title: Battle.net, EA app and Ubisoft Connect on Linux
seoTitle: 'Run Battle.net, EA app, Ubisoft Connect and Rockstar on Linux'
description: 'Set up Battle.net, the EA app, Ubisoft Connect or the Rockstar Games Launcher on Linux with Irori, each in its own Wine prefix, and see their games in one library.'
summary: Each launcher in a Wine prefix of its own, its games in your library, your login never seen.
platforms: [linux, steamdeck]
order: 7
updated: 2026-11-03
related: [epic-games-on-linux, steam-deck, move-from-lutris]
---

Battle.net, the EA app, Ubisoft Connect and the Rockstar Games Launcher keep their own accounts.
Irori doesn’t replace them: it installs each one for you, in a Wine prefix of its own, and brings
the games you install there into your library.

## 1. Choose a Wine build

Open the **Stores** page and find **Other launchers**. Pick the **Wine build for new launchers**;
install one in the Wine manager first if you have none.

## 2. Set up a launcher

Choose **Set up Battle.net** (or the EA app, Ubisoft Connect or the Rockstar Games Launcher).
Irori creates a prefix named after the launcher, downloads the vendor’s official installer and
runs it; follow the installer as you would on Windows. Turn on **Show their games** so the games
appear in your library.

## 3. Sign in and install games in the launcher

Use **Open** to start the launcher, sign in and install games there, as usual. Irori never sees
your login. It reads the launcher’s own records of what is installed and lists those games; use
**Look for games** after installing something new.

## 4. Play from Irori

Press **Play** on any of those games. Irori starts it through the launcher, then follows the game
itself rather than the launcher: while the game runs, Irori looks for processes from its install
folder, and the session ends when they are gone. Playtime counts even when the launcher stays
open. Irori never installs, updates or removes these games; the launcher does.

## Before you buy

Many online games from these launchers use anticheat that refuses to run under Wine. Look a game
up on ProtonDB, or on its page in Irori, before you buy it.

On Windows the same page simply shows the games you installed with these launchers, with no Wine
involved.
