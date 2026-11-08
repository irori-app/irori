---
title: Big Picture, made for the couch
seoTitle: 'Big Picture mode and controller support for your TV and Steam Deck'
description: 'Irori’s Big Picture mode runs the whole launcher with an Xbox, PlayStation, DualSense, Switch Pro or Steam Deck controller, with an in-game overlay in beta.'
tag: Big Picture
summary: The whole launcher with a controller, opening by itself in the Steam Deck’s Game Mode and scaled for a TV.
icon: tv
screen: bigpicture
order: 6
updated: 2026-11-08
related: [emulators, library, playing]
highlights:
  - title: Eight themes, six layouts
    body: Shelves, Spotlight, Cinema, Wall, Carousel or Compact, in a theme you pick, edit live or share as a file.
  - title: Every controller you own
    body: Xbox, PlayStation including DualSense, Switch Pro and Steam Deck controllers, plugged in at any time, with their charge shown.
  - title: The Guide button
    body: Opens and closes Big Picture from anywhere in Irori, or the overlay while a game runs.
  - title: An in-game overlay, in beta
    body: Volume, screenshots, achievements and your journal over the game, without leaving it.
  - title: Steam Deck Game Mode
    body: Irori opens straight into Big Picture when Steam’s controller interface starts it.
  - title: Up to 200 % for TVs
    body: Scale the interface for the distance you sit from the screen.
---

## The whole launcher, with a controller

Big Picture isn’t a separate app. It shows your library, the continue-playing shelf, installed
games and per-system shelves for emulated games, and every screen stays usable with a controller,
the arrow keys, Enter and Escape. Press the Guide button, or <kbd>Ctrl</kbd> <kbd>Shift</kbd>
<kbd>B</kbd>, to open or close it from anywhere.

A game’s settings open over its page without leaving Big Picture, and its live log pages with
the shoulder buttons and filters down to errors and warnings or the text you type.

## The in-game overlay (beta)

Turn it on in Settings › General › Controller. Then the Guide button over a game Irori started
opens a panel above the game instead of Big Picture; on Windows and macOS
<kbd>Ctrl</kbd> <kbd>Shift</kbd> <kbd>O</kbd> opens it too.

- **Home**: the system’s volume and the game’s own, a screenshot, back to Irori, and quitting
  the game, which asks first.
- **Screenshots** taken during this session.
- **Achievements**: how many are unlocked, what this session unlocked and what is next.
- **Journal**: today’s note for the game.

Without opening it, Guide and RB takes a screenshot and Guide with Up or Down changes the
volume. The overlay costs nothing while it is closed.

Where it can appear depends on the system:

| | Linux | macOS | Windows |
| --- | --- | --- | --- |
| Over a fullscreen game | Yes on KDE Plasma, sway, Hyprland, COSMIC, niri and labwc; GNOME may keep it behind | Yes | Windowed and borderless games only; not over exclusive fullscreen Direct3D games |
| Opens with | Guide | Guide, <kbd>Ctrl</kbd> <kbd>Shift</kbd> <kbd>O</kbd> | Guide (not on Xbox controllers, whose Guide belongs to the Game Bar), <kbd>Ctrl</kbd> <kbd>Shift</kbd> <kbd>O</kbd> |
| The game sees presses made in it | No, except games that read controllers through Steam Input or SDL’s HIDAPI | Yes | Yes |
| Game volume | Yes | No, only the system’s | Yes |

In Steam’s Game Mode Steam decides what is on screen, so the overlay isn’t visible there; use
Steam’s overlay. On Linux, games Steam or another launcher starts get only the system volume.

## Steam Deck

When Steam’s controller interface starts Irori (the Deck’s Game Mode, or Steam Big Picture),
Irori opens in Big Picture and never starts hidden in the tray. A setting turns that off. Irori
reads each press once alongside Steam Input: controllers Steam drives through its virtual
controller are left alone, and key presses that echo a controller press are dropped.

## Calm by design

Controllers are read on a dedicated thread that blocks until an event arrives, so Big Picture uses
no CPU while you decide what to play. The window is opaque in Big Picture, which also spares your
compositor the work of blurring what is behind it.
