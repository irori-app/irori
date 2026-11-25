---
title: Fast, light and quiet
seoTitle: 'A lightweight game launcher: 88 MiB empty, 126 MiB with 1,000 games, no Electron'
description: 'Irori is one native Rust process drawn on your GPU: 88.3 MiB empty, 119.4 MiB with 300 games, and 126.0 MiB with 1,000 games (108.1 MB binary), against 478 MiB for Heroic 2.22.4 in the 0.2.0 empty-profile baseline.'
tag: Performance
summary: One native process drawn on your GPU: 88.3 MiB empty and 126.0 MiB with 1,000 games and cover art.
icon: gauge
screen: library-list
order: 9
updated: 2026-11-24
related: [library, bigpicture, downloads]
highlights:
  - title: One process
    body: The whole launcher is one native program, with no browser engine inside its window.
  - title: Drawn on your GPU
    body: The interface is rendered by GPUI on your graphics card.
  - title: Event-driven idle loop
    body: Nothing polls in a busy loop while idle, down to the controller thread.
  - title: Bounded memory
    body: Art is thumbnailed once to disk and kept in a bounded GPU cache.
  - title: Virtualized lists
    body: Only what is on screen exists, so thousands of games scroll smoothly.
  - title: Live language switch
    body: Change between 19 languages without a restart.
---

## The numbers

Measured on Irori 1.1.0 on Linux (CachyOS, KDE Wayland, AMD Radeon RX 9070 XT, Mesa 26.3.1, 108.1 MB binary, `--settle 20 --idle 15` across 2 rounds):

| Scenario | Memory PSS (median) | Processes | Idle CPU (median) | Binary size |
| --- | --- | --- | --- | --- |
| 0 games (empty profile) | 88.3 MiB | 1 | 2.30 % | 108.1 MB |
| 300 games (with covers) | 119.4 MiB | 1 | 2.00 % | 108.1 MB |
| 1,000 games (with covers) | 126.0 MiB | 1 | 1.90 % | 108.1 MB |

Historical 0.2.0 empty-profile baseline (`--settle 45 --idle 30`, Nobara Linux, RX 7900 XTX):

| | Irori 0.2.0 | Heroic Games Launcher 2.22.4 |
| --- | --- | --- |
| Memory | 87.5 MiB | 478.2 MiB |
| Processes | 1 | 7 |
| CPU while idle | 0.0 % | 6.6 % (median) |
| Program size | 76 MB | 368 MB |

## Why it's light

Irori is written in Rust with GPUI, and draws its interface on your graphics card. There is no Electron and no browser inside the window: store sign-in runs in a separate small program only while you sign in. A small runtime handles network and disk work, so the interface never waits on I/O.

Cover art is downloaded once, cut into thumbnails per size and stored on disk, then held in a bounded GPU cache; at most two images are decoded at a time. Every long list is virtualized, and filtered and sorted lists are rebuilt only when their inputs change.

## Calm on your system

When you play, Irori keeps your computer awake and otherwise stays out of the way. Downloads use a dedicated engine with parallel chunks, and update checks for tools happen once a session and daily, from lists cached for a day.
