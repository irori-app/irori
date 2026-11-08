---
title: Eleven stores and launchers, built in
seoTitle: 'Epic Games, GOG, Amazon, ZOOM, itch.io and Steam in one launcher'
description: 'Irori speaks the Epic Games, GOG and Amazon protocols itself, adds ZOOM, itch.io and Steam, and lists Battle.net, EA app, Ubisoft, Rockstar and Xbox app games.'
tag: Stores
summary: Store protocols written in Rust inside Irori, with several accounts per store and nothing extra to download.
icon: store
screen: accounts
order: 2
updated: 2026-11-08
related: [library, playing, downloads]
highlights:
  - title: No helper tools
    body: Epic, GOG and Amazon protocols are Rust ports of legendary, gogdl and nile, built into Irori.
  - title: Several accounts per store
    body: Epic Games, GOG, Amazon Games, ZOOM Platform and itch.io each take more than one account.
  - title: Cloud saves
    body: Epic Games and GOG saves sync before and after you play, and Irori asks when both copies changed.
  - title: Steam, with or without a sign-in
    body: Installed games from Steam’s own files, the whole library from the running client or an account.
  - title: Xbox app games on Windows
    body: Games the Xbox app installed start in one click; an Xbox account adds playtime and achievements.
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
| Steam | Installed games with no sign-in; the whole library from the running client or an account; the Proton version per game on Linux |
| Battle.net, EA app, Ubisoft Connect, Rockstar Games Launcher | Set up once in a Wine prefix of their own; their games join the library |
| Xbox app and Microsoft Store | On Windows, installed games listed and started; an optional Xbox account adds playtime and achievements on every system |

## Several accounts per store

Epic Games, GOG, Amazon Games, ZOOM Platform and itch.io each take several accounts, shown in
one library. A game installed with one account belongs to it: updates, repairs, cloud saves and
launches use that account. When several of your accounts own a game, the install sheet asks
which one installs it.

## Signing in and browsing

Store logins open in Irori’s sign-in window, a small helper program, and finish on their own:
Irori never needs anything copied. On Linux the window uses WebKitGTK; without it, the page
opens in your browser and Irori watches the clipboard for the code. Logins are stored encrypted,
with the key in your system keyring.

The same helper shows a store’s website in a window of its own, from the Stores page’s Open
store button or a game’s Store page. Each store keeps its own cookies there, apart from Irori’s
login, and a signed-in store’s library refreshes when you close the window, so a game you just
bought appears. Without the helper, the store opens in your browser.

## Steam

Your installed Steam games are read from Steam’s own files, with Steam’s own art, and need no
login. To list every game you own without signing in, Irori can read the library once from the
running Steam client: it explains first, asks before restarting Steam and, unless you keep it
on, closes that access again after reading, because while it is open any program on the computer
could control Steam. Or log in (a QR code for the Steam Mobile app, or your password with Steam
Guard) to add every game you own and your Steam Family’s games, and to see your friends, message
them and join a game when Steam reports a lobby or server to join.

Play hands the game to Steam with your launch options and follows it until it ends, so playtime
counts. Install, Verify files and Uninstall open Steam’s own dialogs. On Linux, a Steam game’s
settings choose the Proton build Steam runs it with, GE-Proton included, next to its ProtonDB
rating. Steam rewrites its settings when it exits, so Irori changes them only while Steam is
closed and asks to close it first; in Steam’s Game Mode the choice stays in Steam. Irori’s
Flatpak reads a Flatpak Steam and starts the Steam installed on your system.

## itch.io

Turn itch.io on in Settings › General › Integrations, or from its account card, and log in with
an API key. Irori downloads the best upload for your system (resumable), unpacks zip and tar
archives and finds the program to start.

## Battle.net, EA app, Ubisoft Connect and Rockstar

Irori never handles these accounts itself. The Stores page sets each launcher up once, in a Wine
prefix of its own with the Wine build you pick (natively on Windows), using the vendor’s official
installer. You sign in and install games inside the launcher; Irori reads its local data, lists
the games and starts them, and follows the game rather than the launcher for playtime.

## Xbox app and Microsoft Store

On Windows, games the Xbox app or the Microsoft Store installed, PC Game Pass included, show up
by themselves with their own art and size, and start in one click; the Xbox app installs, updates
and removes them. An optional Xbox account works on every system: it adds the account’s
playtime, last played times, achievements and store art, and lists PC games you played that
aren’t installed, so their hours count in your stats. Only Windows installs and plays them.
