---
title: Play Amazon Games and Prime Gaming on Linux
seoTitle: 'How to play Amazon Games and Prime Gaming games on Linux'
description: 'Sign in to Amazon Games in Irori, install the games you claimed and play them on Linux through Wine or Proton. No Amazon Games app and no nile needed.'
summary: Sign in once, install the games you claimed, play them through Proton.
platforms: [linux, steamdeck]
order: 3
updated: 2026-11-03
related: [epic-games-on-linux, gog-on-linux, steam-deck]
---

The Amazon Games app is Windows only. Irori talks to Amazon Games itself, with a Rust port of the
parts of nile it needs, so the games you own or claimed install and update on Linux.

## 1. Sign in

Install Irori from the [download page](/download/#linux) with the sign-in window, open
**Accounts** and choose Amazon Games. Irori registers itself as a device on your account, as the
Amazon Games app does, and the sign-in window closes once you’re in. You can add more than one
Amazon account.

## 2. Install a Wine or Proton build

Amazon’s games are Windows games, so open the **Wine manager** and install a build such as
GE-Proton or Proton-CachyOS, then make it the default in **Settings › Wine & Proton**.

## 3. Install and play

Choose a game and **Install**. Irori reads Amazon’s signed manifests, downloads the files in
verified chunks and can repair an install later by checking every file against them. Press
**Play** when it’s done; updates appear on the Updates page with everything else.

## Tips

- Check a game’s ProtonDB rating and anticheat information on its page before you buy.
- Turn on the game’s log in its settings to see what Wine reports if it doesn’t start.
- Want it on the couch? The [Steam Deck guide](/guides/steam-deck/) adds Irori to Game Mode.
