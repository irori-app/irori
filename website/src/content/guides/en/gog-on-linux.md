---
title: Play GOG games on Linux
seoTitle: 'How to play GOG games on Linux, with achievements and cloud saves'
description: 'Install your GOG games on Linux with Irori: native Linux versions, Windows versions through Proton, cloud saves, GOG Galaxy achievements and imported installers.'
summary: Native Linux installers, Windows games through Proton, achievements and cloud saves.
platforms: [linux, steamdeck]
order: 2
updated: 2026-11-03
related: [epic-games-on-linux, move-from-heroic, steam-deck]
---

GOG sells DRM-free games, many of them with Linux versions. Irori installs both kinds: the Linux
version when there is one, the Windows version through Proton when there isn’t, with GOG’s cloud
saves and achievements.

## 1. Install Irori and sign in

Install Irori from the [download page](/download/#linux), with the sign-in window package. Open
**Accounts**, choose GOG and sign in; the window closes by itself when you’re done, and your
library fills in.

## 2. Install a game

Choose a game and **Install**. When a game has a native Linux version, Irori offers it, and
installs it from GOG’s Linux installer without Wine. For a Windows version, install a Wine or
Proton build in the **Wine manager** first (GE-Proton is a good default).

Some games have private branches behind a password: choose the **Branch** in the install sheet
and enter the password there. GOG’s REDmod is supported for the games that use it.

## 3. Achievements and cloud saves

Many GOG games talk to GOG Galaxy for achievements, stats and leaderboards. Irori runs a small
Galaxy service for the games it starts, so achievements unlock and playtime is recorded without
the Galaxy client. Galaxy’s in-game overlay isn’t available.

Cloud saves sync before and after you play. If saves changed both on your computer and in the
cloud since the last sync, Irori asks which copy to keep instead of overwriting either.

## Already have installers?

If you downloaded GOG offline installers before, use **Import installers** and choose their
folder. Irori recognizes them, checks what is already installed, and adds the rest to the
download queue, so nothing is fetched twice.

## Several GOG accounts

Add more GOG accounts on the same Accounts card. Games keep the account they were installed with.
Galaxy listens on one port, so while a game of one GOG account runs, a game of another account
started alongside it gets no achievements.
