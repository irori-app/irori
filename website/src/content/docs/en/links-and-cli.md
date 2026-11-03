---
title: irori:// links and the command line
seoTitle: 'Irori command line and irori:// links'
description: 'Start games and open pages with irori:// links, and use Irori from a terminal: list, install, uninstall, refresh, import from Heroic or Lutris, and launch options.'
summary: Links that open pages and games, and commands that work without a window.
order: 4
updated: 2026-11-03
related: [shortcuts, environment-variables, files-and-folders]
---

## Games are named runner/app

Commands and links name a game as `<runner>/<app>`: the store’s id and the game’s id in that
store. Store ids are `legendary` (Epic Games, `epic` works too), `gog`, `nile` (Amazon Games,
`amazon` works too), `zoom`, `itch`, `steam`, `sideload` (games you added), `emulation` and
`launchers`. `irori list` prints every game with its store and id.

## Commands without a window

These run in the terminal and exit; no window opens.

```text
irori accounts                                   which stores are logged in
irori import-heroic                              import Heroic accounts, installs and settings
irori import-lutris                              import Lutris games, categories and playtime (read-only)
irori refresh                                    refresh every library
irori list [--installed]
irori info <runner>/<app> [--platform P]
irori playtime <runner>/<app>
irori install <runner>/<app> [--path DIR] [--platform windows|linux]
irori uninstall <runner>/<app> [--keep-files]
```

`irori install` shows its progress in the terminal. <kbd>Ctrl</kbd> <kbd>C</kbd> stops it; run
the same command again to resume.

## Options for the app

| Option | Effect |
| --- | --- |
| `-l`, `--launch <runner>/<app>` | Start a game. A game that isn’t installed shows its install options instead |
| `--big-picture`, `--console` | Open in Big Picture |
| `--minimized`, `--tray` | Start hidden in the tray |
| `-V`, `--version` | Print the version |
| `-h`, `--help` | Print help |

Only one Irori runs per config folder. Starting it again hands the arguments to the running Irori,
so `irori --launch gog/<app>` from a shortcut starts the game in the window you already have.

## irori:// links

| Link | Opens |
| --- | --- |
| `irori://launch/<runner>/<app>` | Starts the game, after asking |
| `irori://game/<runner>/<app>`, `irori://open/<runner>/<app>` | The game’s page |
| `irori://library`, `downloads`, `updates`, `accounts`, `stores`, `stats`, `wine`, `emulators` | That page |
| `irori://bigpicture` | Big Picture |
| `irori://settings/<section>` | A section of Settings |
| `irori://lutris/<slug>` | The Lutris script installer for that lutris.net game |
| `heroic://launch/...` | Heroic’s launch links work too |

Any web page or program can open a link, so `irori://launch` always asks before it starts a game.
`--launch` from the command line doesn’t ask. Lutris’s own `lutris:<slug>` is accepted as an
argument too.

The packages register the `irori://` handler for you: the desktop entry on Linux, the app bundle
on macOS and the installer on Windows.
