---
title: Fast, light and quiet
seoTitle: 'A lightweight game launcher: 88 MiB, zero idle CPU, no Electron'
description: 'Irori is one native Rust process drawn on your GPU: about 88 MiB of memory, zero CPU while idle and a 76 MB program, against 478 MiB for Heroic on the same PC.'
tag: Performance
summary: One native process drawn on your GPU, about 88 MiB of memory and zero CPU while it waits.
icon: gauge
screen: library-list
order: 9
updated: 2026-11-03
related: [library, bigpicture, downloads]
highlights:
  - title: One process
    body: The whole launcher is one native program, with no browser engine inside its window.
  - title: Drawn on your GPU
    body: The interface is rendered by GPUI on your graphics card.
  - title: Zero CPU while idle
    body: Nothing polls when nothing happens, down to the controller thread.
  - title: Bounded memory
    body: Art is thumbnailed once to disk and kept in a bounded GPU cache.
  - title: Virtualized lists
    body: Only what is on screen exists, so thousands of games scroll smoothly.
  - title: Live language switch
    body: Change between 19 languages without a restart.
---

## The numbers

Measured on Irori 0.2.0 on Linux with an empty profile and the window open:

| | Irori 0.2.0 | Heroic Games Launcher 2.22.4 |
| --- | --- | --- |
| Memory | about 88 MiB | about 478 MiB |
| Processes | 1 | 7 |
| CPU while idle | 0 % | not measured |
| Program size | 76 MB | not measured |

Heroic ran on the same PC. A large library hasn’t been measured yet.

## Why it’s light

Irori is written in Rust with GPUI, and draws its interface on your graphics card. There is no
Electron and no browser inside the window: store sign-in runs in a separate small program only
while you sign in. A small runtime with three worker threads handles network and disk work, so the
interface never waits on I/O.

Cover art is downloaded once, cut into thumbnails per size and stored on disk, then held in a
bounded GPU cache; at most two images are decoded at a time. Every long list is virtualized,
filtered and sorted lists are rebuilt only when their inputs change, and nothing polls while
nothing happens.

## Calm on your system

When you play, Irori keeps your computer awake and otherwise stays out of the way. Downloads use
a dedicated engine with parallel chunks, and update checks for tools happen once a session and
daily, from lists cached for a day.
