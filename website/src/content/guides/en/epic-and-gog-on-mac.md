---
title: Play Epic and GOG games on a Mac
seoTitle: 'Play Epic Games and GOG games on macOS with Game Porting Toolkit'
description: 'Run Windows games from Epic Games and GOG on macOS with Irori, using Game Porting Toolkit, Wine-Crossover, Wine-Staging or your CrossOver bottles.'
summary: Game Porting Toolkit, Wine or your CrossOver bottles, with Mac versions installed natively.
platforms: [macos]
order: 6
updated: 2026-11-03
related: [epic-games-on-linux, gog-on-linux, itch-io]
---

Irori runs natively on macOS, on Apple silicon and Intel. Mac versions of games install and start
as Mac apps; Windows versions run through Wine, CrossOver or Apple’s Game Porting Toolkit.

## 1. Install Irori

```bash
brew install --cask irori-app/tap/irori
```

Or download the `.dmg` for Apple silicon or Intel from the
[latest release](https://github.com/irori-app/irori/releases/latest) and drag Irori to
Applications. The app is signed with Irori’s Developer ID and notarized by Apple, so macOS asks
once for Keychain access to the key that protects your logins.

## 2. Sign in

Open **Accounts** and sign in to Epic Games and GOG. Irori shows the sign-in window itself, using
macOS’s own WebKit, and it closes by itself when you’re done.

## 3. Choose how Windows games run

The **Wine manager** has a tab for each family Irori downloads, and one for what is already on
your Mac:

| Tab | What it is |
| --- | --- |
| Game-Porting-Toolkit | Apple’s translation layer, built for Wine. Apple silicon and macOS 14 or later only |
| Wine-Crossover | A downloadable Wine with CrossOver’s changes; not the CrossOver app |
| Wine-Staging-macOS | Wine Staging builds for macOS |
| On this system | Wine app bundles, Homebrew Wine, CrossOver and CrossOver Preview, and Whisky’s Wine |

Intel Wine builds on Apple silicon need Rosetta 2; Irori checks for it before it sets up a prefix
and tells you if it’s missing.

## 4. Using the CrossOver app

To run a game in a CrossOver bottle, create the bottle in CrossOver first, then name it in the
game’s Wine settings (“CrossOver bottle”). Bottles moved to another drive work too: point
Settings › General › Folders › “CrossOver bottles folder” at them. Irori never initializes or
deletes a shared bottle; manage it in CrossOver.

## 5. Play

Install a game and press **Play**. Turn on **Retina mode** in a game’s settings to render at your
display’s full resolution, and try **Msync** and the **Metal HUD** when tuning. Cloud saves for
Epic Games and GOG sync before and after you play, as on every system.

A runtime being available doesn’t mean every game works: DRM, anticheat and the graphics API a
game uses still matter. Try another Wine family when one doesn’t work.
