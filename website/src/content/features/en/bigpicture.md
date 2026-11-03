---
title: Big Picture, made for the couch
seoTitle: 'Big Picture mode and controller support for your TV and Steam Deck'
description: 'Irori’s Big Picture mode runs the whole launcher with an Xbox, PlayStation, DualSense, Switch Pro or Steam Deck controller, scaled up to 200 % for a TV.'
tag: Big Picture
summary: The whole launcher with a controller, opening by itself in the Steam Deck’s Game Mode and scaled for a TV.
icon: tv
screen: bigpicture
order: 6
updated: 2026-11-03
related: [emulators, library, playing]
highlights:
  - title: Every controller you own
    body: Xbox, PlayStation including DualSense, Switch Pro and Steam Deck controllers.
  - title: Hot-plug and battery
    body: Plug a controller in or out at any time, and see how much charge it has left.
  - title: The Guide button
    body: Opens and closes Big Picture from anywhere in Irori.
  - title: Steam Deck Game Mode
    body: Irori opens straight into Big Picture when Steam’s controller interface starts it.
  - title: Up to 200 % for TVs
    body: Scale the interface for the distance you sit from the screen.
  - title: Zero CPU while it waits
    body: Controllers are read on a thread that sleeps until something is pressed.
---

## The whole launcher, with a controller

Big Picture isn’t a separate app. It shows your library, the continue-playing shelf, installed
games and per-system shelves for emulated games, and every screen stays usable with a controller,
the arrow keys, Enter and Escape. Press the Guide button, or <kbd>Ctrl</kbd> <kbd>Shift</kbd>
<kbd>B</kbd>, to open or close it from anywhere.

## Steam Deck

When Steam’s controller interface starts Irori (the Deck’s Game Mode, or Steam Big Picture),
Irori opens in Big Picture and never starts hidden in the tray. A setting turns that off. Irori
reads each press once alongside Steam Input: controllers Steam drives through its virtual
controller are left alone, and key presses that echo a controller press are dropped.

## Calm by design

Controllers are read on a dedicated thread that blocks until an event arrives, so Big Picture uses
no CPU while you decide what to play. The window is opaque in Big Picture, which also spares your
compositor the work of blurring what is behind it.
