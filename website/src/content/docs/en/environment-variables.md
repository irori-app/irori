---
title: Environment variables
seoTitle: 'Irori environment variables: IRORI_HOME, IRORI_LOG and more'
description: 'The environment variables Irori reads: IRORI_HOME to move all its data, IRORI_LOG for logging, IRORI_UPDATE_EXPLANATION for packagers, and the Steam Input ones.'
summary: Move Irori’s data, turn up logging, and the variables packagers and Steam use.
order: 5
updated: 2026-11-03
related: [files-and-folders, troubleshooting, links-and-cli]
---

## For everyone

| Variable | Effect |
| --- | --- |
| `IRORI_HOME` | Puts all config, data and cache under one folder (`<root>/config`, `<root>/data`, `<root>/cache`). Useful for trying things without touching your real library. |
| `IRORI_LOG` | Filters what Irori logs, for example `IRORI_LOG=irori_services=debug,info`. The default is `info` in the app and `warn` for the commands without a window. |

Each config folder has one running Irori, so an Irori started with a different `IRORI_HOME` runs
alongside your usual one.

## For packagers

| Variable | Effect |
| --- | --- |
| `IRORI_UPDATE_EXPLANATION` | At build or run time: Irori never looks for its own new versions and shows this text in Settings › About instead, for example “Update Irori with your distribution’s package manager.” |
| `IRORI_SIGN_IDENTITY` | The macOS signing identity `packaging/package.py` uses (default `-`, an ad-hoc signature). |
| `IRORI_WINDOWS_CRT_DIR` | An explicit x64 `Microsoft.VC*.CRT` folder for Windows packaging, when it can’t be found. |

## Set by Steam

Irori reads a few variables Steam sets, to behave well on the Steam Deck and in Steam Big Picture:

| Variable | What Irori does |
| --- | --- |
| `XDG_CURRENT_DESKTOP=gamescope`, `SteamGamepadUI=1` | Steam’s controller interface started Irori, so it opens in Big Picture and never starts hidden in the tray |
| `SDL_GAMECONTROLLER_IGNORE_DEVICES`, `SDL_GAMECONTROLLER_IGNORE_DEVICES_EXCEPT` | Controllers these leave out are not read, because Steam Input drives them through its virtual controller |

## For development

`IRORI_RELEASE_TEST_URL`, `IRORI_RUNTIME_SMOKE` and `IRORI_KEYRING_SMOKE` turn on tests that are
skipped by default, because they need a local mock server, download a real runtime or write a real
keyring key. They matter only when working on that code.
