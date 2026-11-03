---
title: Frequently asked questions
seoTitle: 'Irori FAQ: questions about the game launcher'
description: 'Answers about Irori: how it relates to Heroic, what it costs, privacy, which stores and systems it supports, Wine and Proton, emulators, Steam and translations.'
summary: Heroic, privacy, stores, Wine, emulators, Steam and more.
order: 8
updated: 2026-11-03
related: [getting-started, troubleshooting, installation]
---

## Is Irori a fork of Heroic?

No. Irori is written from scratch in Rust and draws its interface with GPUI on your graphics card.
It matches what Heroic Games Launcher can do, using Heroic as a reference for behaviour only, and
fixes Heroic’s quirks instead of copying them. Its store support is a Rust port of legendary, gogdl
and nile, the tools Heroic runs.

## What does it cost?

Nothing. Irori is free software under the MIT licence. Its source is on
[GitHub](https://github.com/irori-app/irori).

## Does Irori collect data about me?

No. Irori sends no telemetry, analytics or crash reports. Without you asking, it contacts only a
connectivity check, GitHub for public lists (anticheat compatibility, known game fixes and Wine and
Proton releases), and the stores you signed in to. The
[code signing policy](/code-signing/) lists every connection.

## Do I have to sign in to anything?

Only to the stores whose games you want to see. Steam games are read from Steam’s own files without
a sign-in, and emulated and added games need no account at all.

## Which stores does it support?

Epic Games, GOG, Amazon Games, ZOOM Platform and itch.io, installed and updated by Irori; Steam,
read from Steam and played through it; and Battle.net, the EA app, Ubisoft Connect and the Rockstar
Games Launcher, set up in their own Wine prefix with their games in your library.

## Can I use more than one account per store?

Yes, for Epic Games, GOG, Amazon Games, ZOOM Platform and itch.io. Each installed game launches with
the account it was installed with.

## How do Windows games run on Linux and macOS?

On Linux through Wine, Proton or umu; on macOS through Wine, CrossOver or Apple’s Game Porting
Toolkit. Irori downloads and manages these builds for you, and each game can use its own. On
Windows, games run natively.

## Does Irori download BIOS files or games for emulators?

Never. Irori installs emulators from their projects’ own servers on request. Point it at your own
BIOS and game folders, and it checks a game has what it needs before it starts.

## Does Irori replace Steam?

No. Irori lists your Steam games and starts them through Steam, which keeps its own Proton version,
launch options and overlay. Install, Verify files and Uninstall open Steam’s own dialogs.

## Can I bring my Heroic or Lutris library?

Yes. Irori imports Heroic’s accounts, installs, settings, favourites and categories in one click,
and reads Lutris’s games, categories and playtime without changing anything in Lutris.

## Which languages is it in?

English, Czech, German, Spanish, French, Italian, Dutch, Polish, Portuguese, Brazilian Portuguese,
Romanian, Swedish, Turkish, Russian, Ukrainian, Japanese, Korean, and Simplified and Traditional
Chinese: 19 in all, switchable without a restart.

## Why “Irori”?

An *irori* (囲炉裏) is the sunken hearth of a traditional Japanese house, the place everyone
gathers. Irori gathers your games around one fire.
