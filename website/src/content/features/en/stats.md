---
title: Stats that show how you play
seoTitle: 'Game playtime tracker with stats, achievements and a year in review'
description: 'Irori times every play session and merges store playtime, then shows weeks, streaks, your backlog, achievements from every store and a Year in review.'
tag: Stats
summary: Every session counted by Irori, merged with the stores’ own totals, shown as weeks, streaks and favourites.
icon: chart
screen: stats
order: 7
updated: 2026-11-08
related: [library, playing, performance]
highlights:
  - title: Sessions counted by Irori
    body: Every game Irori starts is timed until it ends, even games another launcher owns.
  - title: Store playtime merged
    body: Irori keeps the larger of its own total and the store’s, and counts games, never their DLC.
  - title: Weeks, streaks and a backlog
    body: Play time against the period before, your streaks, and a yearly goal for what you beat.
  - title: Achievements from every store
    body: Steam, GOG, Epic Games, Xbox and RetroAchievements, and the files DRM-free games keep.
  - title: A Year in review
    body: Any year you played, told as full-window stories, ending on a card you can save.
  - title: Kept on your computer
    body: Sessions are stored in Irori’s own state folder, never sent anywhere.
---

## Counted by Irori, not guessed

Irori times every game it starts, from launch until the game ends. For games another program owns,
such as Steam games or games from Battle.net, the EA app, Ubisoft Connect or Rockstar, it follows
the game’s own processes rather than the launcher, so a session doesn’t end while the game still
runs. The same goes for a game whose own small launcher exits once the game is up.

Stores keep their own playtime too. Irori merges it by keeping the larger total, and counts games
only, never their DLC, because some stores report a DLC’s playtime as a copy of the game’s.

## The Stats page

- **Play time** by week, month, year or all time, against the period before.
- **Current streak** and **longest streak**, in days, plus comebacks and new games.
- **By store** and **most played**, to see where the hours go.
- **A backlog** with a yearly goal, its pace and a timeline of what you beat.
- **A journal**: a note for each day you played a game, with that day’s screenshots.
- **An album** of your screenshots per game: the ones Steam keeps and the ones your desktop
  saves while a game you started from Irori runs. Irori only reads them.

Each game’s page shows its own playtime, a day-by-day timeline of its sessions and when you last
played it, and every game can have a play status: plan to play, playing, on hold, beaten or
abandoned.

## Year in review

Pick any year you played, and Irori tells it as full-window stories over your games’ art: your
top games, month by month, records, when you play, comebacks and what you started and beat. It
ends on a summary card you can save as an image.

## Achievements

Achievements come from GOG, Epic Games and Steam, from an Xbox account, and from
RetroAchievements for your ROMs. Steam games that split their achievements into Achievement Sets
show each set with its own progress, and a set for DLC you don’t own is marked as such instead of
looking like locked achievements.

DRM-free and offline copies often keep their Steam achievements next to the game. With “Read
achievements from game files” on, Irori reads the unlocks that Goldberg, gbe_fork, CODEX and
RUNE files hold, without changing them, and names them from Steam’s public list. After a session
Irori asks again, and can tell you what you unlocked.

## Private by default

Sessions are written to `sessions.jsonl` in Irori’s state folder on your computer. Irori sends no
telemetry or analytics, so your play history stays yours.
