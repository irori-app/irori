---
title: Ten stores and launchers, built in
seoTitle: 'Epic Games, GOG, Amazon, ZOOM, itch.io and Steam in one launcher'
description: 'Irori speaks the Epic Games, GOG and Amazon protocols itself, adds ZOOM Platform, itch.io and Steam, and sets up Battle.net, EA app, Ubisoft Connect and Rockstar.'
tag: Stores
summary: Store protocols written in Rust inside Irori, with several accounts per store and nothing extra to download.
icon: store
screen: accounts
order: 2
updated: 2026-11-03
related: [library, playing, downloads]
highlights:
  - title: No helper tools
    body: Epic, GOG and Amazon protocols are Rust ports of legendary, gogdl and nile, built into Irori.
  - title: Several accounts per store
    body: Epic Games, GOG, Amazon Games, ZOOM Platform and itch.io each take more than one account.
  - title: Cloud saves
    body: Epic Games and GOG saves sync before and after you play, and Irori asks when both copies changed.
  - title: GOG Galaxy features
    body: GOG achievements and playtime through a built-in Galaxy service for the games Irori starts.
  - title: Steam without a sign-in
    body: Installed Steam games appear from Steam’s own files; sign in only to add the rest.
  - title: Signed in by itself
    body: The sign-in window finishes on its own, so you never copy a code.
---

## Built in, not bolted on

Most launchers for Epic Games, GOG and Amazon Games run separate command-line tools behind the
scenes. Irori doesn’t. Its store protocols are Rust ports of the parts of legendary, gogdl and
nile that it needs, compiled into the launcher, so downloads, updates and cloud saves need no
external store tools to download, update or break.

| Store | What Irori does |
| --- | --- |
| Epic Games | Library, install, update, repair, move and import, cloud saves, the EOS overlay |
| GOG | Library, install and import, native Linux installers, cloud saves, achievements and playtime, private branches, REDmod |
| Amazon Games | Library, install, update and repair from Amazon’s signed manifests |
| ZOOM Platform | Library and installs, turned on in Settings |
| itch.io | Log in with an API key, then install, update and play what you own; turned on in Settings |
| Steam | Installed games with no sign-in; owned and Family games with one |
| Battle.net, EA app, Ubisoft Connect, Rockstar Games Launcher | Set up once in a Wine prefix of their own; their games join the library |

## Several accounts per store

Epic Games, GOG, Amazon Games, ZOOM Platform and itch.io each take several accounts, shown in
one library. A game installed with one account belongs to it: updates, repairs, cloud saves and
launches use that account. When several of your accounts own a game, the install sheet asks
which one installs it.

## Signing in

Store logins open in Irori’s sign-in window, a small helper program, and finish on their own:
Irori never needs anything copied. On Linux the window uses WebKitGTK; without it, the page
opens in your browser and Irori watches the clipboard for the code. Logins are stored encrypted,
with the key in your system keyring.

## Steam

Your installed Steam games are read from Steam’s own files, with Steam’s own art, and need no
login. Log in to Steam (a QR code for the Steam Mobile app, or your password with Steam Guard)
to add every game you own and your Steam Family’s games. Play hands the game to Steam with your
launch options and follows it until it ends, so playtime counts. Install, Verify files and
Uninstall open Steam’s own dialogs.

## itch.io

Turn itch.io on in Settings › General › Integrations, or from its account card, and log in with
an API key. Irori downloads the best upload for your system (resumable), unpacks zip and tar
archives and finds the program to start.

## Battle.net, EA app, Ubisoft Connect and Rockstar

Irori never handles these accounts itself. The Stores page sets each launcher up once, in a Wine
prefix of its own with the Wine build you pick (natively on Windows), using the vendor’s official
installer. You sign in and install games inside the launcher; Irori reads its local data, lists
the games and starts them, and follows the game rather than the launcher for playtime.
