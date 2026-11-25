---
title: Fast, light and quiet
seoTitle: 'A lightweight game launcher: 122 MiB empty, 148 MiB with 1,000 games, no Electron'
description: 'Irori is a native Rust GPU launcher: 122.4 MiB empty, 147.7 MiB with 1,000 seeded games, and 385.1 MiB on a 1,440-game real library.'
tag: Performance
summary: 'One native process drawn on your GPU: 122.4 MiB empty, 147.7 MiB with 1,000 seeded games, and 0.02 % idle CPU.'
icon: gauge
screen: library-list
order: 9
updated: 2026-11-25
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

Measured on Irori 1.1.0 on Linux (CachyOS, KDE Wayland, AMD Radeon RX 9070 XT, Mesa 26.3.1, 108.1 MB binary, `--settle 45 --idle 30` across 2 rounds, host overlays disabled):

| Scenario | Memory PSS (median) | Processes | Idle CPU (median) | Binary size |
| --- | --- | --- | --- | --- |
| 0 games (synthetic empty) | 122.4 MiB | 1 | 0.02 % | 108.1 MB |
| 300 games (rich synthetic + art) | 146.8 MiB | 1 | 0.00 % | 108.1 MB |
| 1,000 games (rich synthetic + art) | 147.7 MiB | 1 | 0.02 % | 108.1 MB |
| 1,440 games (real multi-store library) | 385.1 MiB | 1 | 0.03 % | 108.1 MB |

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
