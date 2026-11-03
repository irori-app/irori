---
title: Files and folders
seoTitle: 'Where Irori keeps settings, logins, logs and data'
description: 'Where Irori stores its settings, encrypted store logins, per-game settings, library state, logs and crash reports on Linux, macOS and Windows.'
summary: Where settings, logins, state, logs and crash reports live on each system.
order: 6
updated: 2026-11-03
related: [environment-variables, troubleshooting, installation]
---

## The three folders

| Data | Linux | macOS | Windows |
| --- | --- | --- | --- |
| Settings and logins | XDG config folder, `irori` | `~/Library/Application Support/irori` | Roaming AppData, `irori` |
| Runtimes and data | XDG data folder, `irori` | `~/Library/Application Support/irori` | Local AppData, `irori` |
| Cache | XDG cache folder, `irori` | `~/Library/Caches/irori` | Local AppData, `irori` |

On Linux these are usually `~/.config/irori`, `~/.local/share/irori` and `~/.cache/irori`.
[`IRORI_HOME`](/docs/environment-variables/) moves all three under one folder.

## What is where

| Path | Holds |
| --- | --- |
| `<config>/stores/<store>/` | A store’s settings and your login |
| `<config>/stores/<store>/accounts/<n>/` | Further accounts for stores with several |
| `<config>/games/<runner>/<app>.json` | A game’s own settings: only what differs from your defaults |
| `<data>/logs/irori.log` | The app’s log |
| `<data>/logs/crash-<time>.txt` | A crash report, kept only on your computer |
| `<config>/state/library.json` | Favourites, hidden games, categories, recent games, playtime and art you changed |
| `<config>/state/downloads.json` | The download queue, so downloads resume after a restart |
| `<config>/state/sessions.jsonl` | Every play session, for Stats |
| `<config>/state/updates.json` | Versions you chose to skip |
| `<config>/state/app-update.json` | When Irori last looked for a new version of itself |
| `<data>/engines/<id>/<version>/` | Emulators Irori installed, side by side by version |
| `<cache>/stores/<store>/` | A store’s cached library and art |

Files are written to a temporary copy first and then renamed, so a crash never leaves a half-written
file. Irori’s settings use Heroic’s names, so a Heroic configuration reads unchanged.

## Store logins

Logins are JSON files under `<config>/stores/<store>/`, encrypted with AES-256-GCM. The key is one
random value in your system keyring: Secret Service on Linux (GNOME Keyring, KWallet), Keychain on
macOS, Credential Manager on Windows. Irori keeps tokens, never store passwords.

Without a keyring, logins stay usable as plain files only you can read, and are encrypted the next
time they are read with a keyring available. If the keyring is locked or its key is gone, Irori
leaves the files alone, says so, and the stores look logged out until it is unlocked or you sign in
again.

## Steam

Irori reads Steam from its usual folder (`~/.steam/steam` on Linux, `~/Library/Application
Support/Steam` on macOS, `Program Files (x86)\Steam` on Windows), or the folder you set. On Linux it
also finds `~/.local/share/Steam`, Debian’s installation, Flatpak Steam and Snap Steam, and it
scans every library folder Steam knows about.
