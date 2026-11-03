---
title: Stats that show how you play
seoTitle: 'Game playtime tracker with weekly stats and streaks'
description: 'Irori tracks every play session itself and merges store playtime, then shows hours per week, streaks, all-time totals, time per store and your most played games.'
tag: Stats
summary: Every session counted by Irori, merged with the stores’ own totals, shown as weeks, streaks and favourites.
icon: chart
screen: stats
order: 7
updated: 2026-11-03
related: [library, playing, performance]
highlights:
  - title: Sessions counted by Irori
    body: Every game Irori starts is timed until it ends, even games another launcher owns.
  - title: Store playtime merged
    body: Irori keeps the larger of its own total and the store’s, so nothing is counted twice.
  - title: Games, not their DLC
    body: Playtime counts games only, never repeated per DLC.
  - title: Weeks and streaks
    body: This week against last, your current streak and your longest one.
  - title: Per store and most played
    body: See where your hours go, and which games you keep coming back to.
  - title: Kept on your computer
    body: Sessions are stored in Irori’s own state folder, never sent anywhere.
---

## Counted by Irori, not guessed

Irori times every game it starts, from launch until the game ends. For games another program owns,
such as Steam games or games from Battle.net, the EA app, Ubisoft Connect or Rockstar, it follows
the game’s own processes rather than the launcher, so a session doesn’t end while the game still
runs.

Stores keep their own playtime too. Irori merges it by keeping the larger total, and counts games
only, never their DLC, because some stores report a DLC’s playtime as a copy of the game’s.

## The Stats page

- **This week**, with how it compares to last week.
- **Current streak** and **longest streak**, in days.
- **All time**, across every store.
- **Play time per week**, as a chart of recent weeks.
- **By store** and **most played**, to see where the hours go.

Each game’s page shows its own playtime and when you last played it.

## Private by default

Sessions are written to `sessions.jsonl` in Irori’s state folder on your computer. Irori sends no
telemetry or analytics, so your play history stays yours.
