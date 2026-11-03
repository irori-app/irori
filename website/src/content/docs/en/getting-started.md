---
title: Getting started
seoTitle: 'Getting started with Irori'
description: 'Your first ten minutes with Irori: install it, bring over Heroic or Lutris, sign in to your stores, set up Wine or Proton and play your first game.'
summary: Install, import, sign in and play your first game.
order: 1
updated: 2026-11-03
related: [installation, shortcuts, troubleshooting]
---

## 1. Install Irori

Irori runs on Linux, macOS and Windows. Install it from your package manager, Homebrew, Scoop or
an installer; [Installation](/docs/installation/) has every method.

## 2. Bring your old launcher along

On its first start Irori looks for Heroic Games Launcher and Lutris. When it finds one, onboarding
offers to import it:

- **Heroic:** store logins, installed games, per-game settings, favourites and categories.
- **Lutris:** Windows, Linux and emulated games, favourites, categories and playtime.

Neither launcher is changed. The same imports run from a terminal with `irori import-heroic` and
`irori import-lutris`.

## 3. Sign in to your stores

Open **Accounts**. Sign in to Epic Games, GOG and Amazon Games; turn on ZOOM Platform and itch.io
in Settings if you use them. Each store can take several accounts. Your installed Steam games
appear without a sign-in; sign in to Steam to add the rest of your Steam library.

Logins open in Irori’s sign-in window and finish by themselves. On Linux that window is a separate
package that needs WebKitGTK; without it, the page opens in your browser and Irori picks the code
up from the clipboard.

## 4. Set up Wine or Proton (Linux and macOS)

Windows games need a compatibility layer on Linux and macOS. Open the **Wine manager** and install
a build: GE-Proton or Proton-CachyOS on Linux, Game Porting Toolkit (Apple silicon, macOS 14 or
later), Wine-Crossover or Wine-Staging-macOS on a Mac. Choose the default in **Settings › Wine &
Proton**. On Windows, skip this: games run natively.

## 5. Play

Pick a game, choose **Install**, and press **Play** when it’s done. The download queue is on the
**Downloads** page (<kbd>Ctrl</kbd> <kbd>2</kbd>), and the command palette
(<kbd>Ctrl</kbd> <kbd>K</kbd>) finds any game or action.

## Next steps

- Add your ROM folders on the **Emulators** page to play emulated games.
- Add any other program as a game with **Add game**.
- Try **Big Picture** (<kbd>Ctrl</kbd> <kbd>Shift</kbd> <kbd>B</kbd>) with a controller.
- Learn the [keyboard shortcuts](/docs/shortcuts/).
