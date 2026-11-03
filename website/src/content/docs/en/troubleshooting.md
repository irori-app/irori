---
title: Troubleshooting
seoTitle: 'Irori troubleshooting: sign-in, logins, games that won’t start'
description: 'Fixes for common Irori problems: no sign-in window on Linux, stores looking logged out, Keychain prompts, SmartScreen, games that don’t start and missing BIOS files.'
summary: Sign-in, logins, prompts, games that won’t start, and where to look next.
order: 7
updated: 2026-11-03
related: [faq, files-and-folders, environment-variables]
---

## Sign-in opens my browser instead of a window (Linux)

The sign-in window is a separate program, `irori-sign-in`, that needs WebKitGTK 4.1. The packages
ship it separately; install it. Without it, Irori opens the store’s page in your browser and
watches the clipboard for the code or address it needs, and you can paste it into the dialog too.
If you built Irori yourself, build the window as well: `cargo build --release -p irori-sign-in`.

## My stores look logged out

Logins are encrypted with a key kept in your system keyring. If the keyring is locked, or its key
is gone, Irori leaves the login files alone, shows a notice at start, and the stores look logged
out. Unlock the keyring and restart Irori, or sign in again. Sandboxed packages need access to
`org.freedesktop.secrets` to reach the keyring.

## macOS asks for Keychain access after every update

That happens with ad-hoc signed builds, whose signature changes each time, such as one you built
yourself. The release build is signed with a Developer ID, so it asks once.

## Windows SmartScreen warns about the installer

Windows builds aren’t signed yet. Choose **More info**, then **Run anyway**. You can check the file
against `SHA256SUMS` in its release, or verify its build attestation with
`gh attestation verify <file> -R irori-app/irori`.

## A Windows game doesn’t start

1. Open the game’s log from its page, or let it open by itself when the game starts. Wine’s errors
   are there.
2. Try another Wine or Proton build in the game’s settings. On a Mac, try another family:
   Game Porting Toolkit, Wine-Crossover or Wine-Staging-macOS.
3. Check the game’s ProtonDB and anticheat information on its page. Some anticheat refuses to run
   under Wine at all.
4. Game Porting Toolkit needs Apple silicon and macOS 14 or later. Intel Wine builds on Apple silicon
   need Rosetta 2; Irori says so when it’s missing.

## An emulated game asks for a BIOS

Irori never downloads BIOS files. Choose the folder where you keep yours when the dialog asks;
Irori finds the right files there, even under other names, and links them before the game starts.

## A controller acts twice on a Steam Deck

Irori drops arrow, Enter and Escape presses that echo a controller press, and leaves alone the
controllers Steam drives through its virtual controller (those listed in
`SDL_GAMECONTROLLER_IGNORE_DEVICES`).

## GOG achievements don’t unlock

GOG’s Galaxy service listens on one port. While a game of one GOG account runs, a game of another
GOG account started alongside it gets no achievements. Galaxy’s in-game overlay isn’t available.

## Window blur doesn’t show (Linux)

Translucency uses your compositor’s blur. Irori asks the compositor what it can do, and stays opaque
on Automatic where nothing blurs. Sway, and X11 without KWin, don’t blur by themselves.

## Getting help

Run Irori with `IRORI_LOG=debug` for a detailed log at `<data>/logs/irori.log`. When Irori
crashes, it writes `crash-<time>.txt` beside it. Then
[open an issue](https://github.com/irori-app/irori/issues) with what you did and what you saw.
Irori can upload a log for you to share; logs mask tokens and passwords, but read one before you
share it.
