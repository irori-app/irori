---
title: irori:// links and the command line
seoTitle: 'Irori command line and irori:// links'
description: 'Start games and open pages with irori:// links, and use Irori from a terminal: list, install, uninstall, refresh, import from Heroic or Lutris, and launch options.'
summary: Links that open pages and games, and commands that work without a window.
order: 4
updated: 2026-11-08
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
irori list --json                                installed games as JSON, for other tools
irori info <runner>/<app> [--platform P]
irori playtime <runner>/<app>
irori install <runner>/<app> [--path DIR] [--platform windows|linux]
irori uninstall <runner>/<app> [--keep-files]
```

`irori install` shows its progress in the terminal. <kbd>Ctrl</kbd> <kbd>C</kbd> stops it; run
the same command again to resume.

## Installed games as JSON

`irori list --json` prints the games Irori installs and runs itself, for tools that work on a
game's files or Wine prefix (SCSKiller compiles their shader caches ahead of time with it).
Steam's games, third-party launchers' games and the ROM library aren't listed: they run through
their own clients or emulators.

```json
{
  "version": 1,
  "games": [
    {
      "id": "legendary/Quail",
      "runner": "legendary",
      "appName": "Quail",
      "title": "Quail Quest",
      "platform": "windows",
      "installPath": "/home/me/Games/Irori/QuailQuest",
      "executable": "/home/me/Games/Irori/QuailQuest/Binaries/Quail.exe",
      "arguments": "-dx11",
      "launchCommand": ["/usr/bin/irori", "--launch", "legendary/Quail"],
      "wine": {
        "prefix": "/home/me/Games/Irori/Prefixes/Quail Quest",
        "kind": "proton",
        "name": "GE-Proton10-1",
        "buildPath": "/home/me/.local/share/Steam/compatibilitytools.d/GE-Proton10-1",
        "bin": "/home/me/.local/share/Steam/compatibilitytools.d/GE-Proton10-1/proton",
        "umu": true,
        "environment": [["DXVK_ASYNC", "1"]]
      }
    }
  ]
}
```

`wine` is `null` for a game that runs natively. `prefix` is what Irori passes as `WINEPREFIX`, or
as `STEAM_COMPAT_DATA_PATH` for Proton, whose `drive_c` is then in `pfx/`. `kind` is `wine`,
`proton`, `crossover` or `toolkit`. Within a `version`, fields are only ever added, so ignore the
ones you don't know. A game Irori starts gets `IRORI_GAME_ID=<runner>/<app>` in its
environment, the same id as `id`.

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
