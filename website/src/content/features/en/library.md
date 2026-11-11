---
title: One library for every game you own
seoTitle: 'Unified game library for Epic, GOG, Steam and more'
description: 'Irori puts Epic Games, GOG, Amazon, ZOOM, itch.io and Steam games, your ROMs and your own games in one fast library, with one entry per game across stores.'
tag: Library
summary: Every store, your ROMs and your own games in one grid that stays smooth at thousands of covers.
icon: library
screen: library
order: 1
updated: 2026-11-12
related: [stores, emulators, performance]
highlights:
  - title: Type to search
    body: Start typing anywhere in the grid or list. Ctrl K finds games and commands alike.
  - title: One entry per game
    body: A game you own on several stores is one entry that plays the installed copy.
  - title: Filters that answer questions
    body: By store, genre, game mode or age rating, grouped by play status, game mode or genre.
  - title: Smart categories
    body: Categories that fill themselves from rules, beside the ones you sort by hand.
  - title: Many games at once
    body: Pick several games to install, uninstall, hide, categorize or add to Steam in one go.
  - title: Grid, list or Focus
    body: Covers at the size you like, a sortable list, or the selected game’s whole page beside its cover.
---

## Everything in one place

Irori gathers your **Epic Games**, **GOG**, **Amazon Games**, **ZOOM Platform** and **itch.io**
libraries, your **Steam** games, your emulated games and anything you add yourself into one
library. Games from Battle.net, the EA app, Ubisoft Connect and the Rockstar Games Launcher join
it too, once you set those launchers up. On Linux your Microsoft Store and PC Game Pass games join
it once you sign in, and on Windows so do the games the Xbox app installed.

Each game keeps one page with its playtime, last session, settings, art and logs, whichever store
it came from.

## One entry per game

A game you own on more than one store shows up once. It plays the installed copy, its page
switches between the Steam, Epic Games or GOG copies, and each copy keeps its own playtime and
achievements. Irori matches copies by title only across different stores, and you can combine or
separate copies yourself, or turn the grouping off.

## Find any game in a second

Search is fuzzy, so “moth carto” finds *The Moth Cartographer*, and you can start typing anywhere
in the grid or the list. Filters narrow the library to one store, to emulated games, to what is
installed, to a genre, a game mode or an age, and the library can group by play status, game mode
or genre. On a wide window the header holds one-click sort, installed, favourites and grouping
controls, as many as fit. Categories can show as chips on the covers, and
<kbd>Ctrl</kbd> <kbd>Shift</kbd> <kbd>G</kbd> picks a random game when you can’t choose.

The command palette (<kbd>Ctrl</kbd> <kbd>K</kbd>, <kbd>Cmd</kbd> <kbd>K</kbd> on a Mac) goes
further: it finds games and actions, so you can play, install or open a setting without touching
the mouse.

## Categories that fill themselves

Categories are manual, holding the games you put in them, or smart, filled from rules: store,
tags, game mode, play status, age rating, installed, playtime, last played, HowLongToBeat length,
system, another category or a search, matching all or any of them, with rows that leave games
out. A saved view can become a smart category. Either kind can be coloured and pinned, and a
pinned one gets its own shelf in Big Picture. The Views menu shows how many games each holds, and
categories travel with backups and sync.

## Many games at once

<kbd>Ctrl</kbd>-click, <kbd>Shift</kbd>-click or <kbd>Ctrl</kbd> <kbd>A</kbd> picks several games,
then install, uninstall, hide, categorize or add them to Steam together. <kbd>Delete</kbd>
removes added games and ROMs from the library: ROM files are never deleted, and an added game’s
files only when you ask.

## Game modes and age ratings

Games know how they are played: single player, co-op, split screen and local, online and MMO,
from Steam, GOG and PCGamingWiki. Filter or group by them, search with `mode:coop`, and correct a
game’s modes yourself when the data is wrong.

Age ratings (ESRB, PEGI, USK, CERO, ClassInd, GRAC, ACB and more) show on each game’s page in
your region’s board, with their content descriptors, and a library filter keeps to games suitable
at an age you pick. Ratings come from the stores’ own data; Steam and GOG ratings are learned
when you open a game’s page, and unrated games are left out unless you include them.

## Your own games, your own art

Add any program as a game: a native app, a Windows game run through Wine, or a browser game, or
drop its program, shortcut or folder onto the library. Added games keep their own description,
developer, release date and genres, and you can change the title or the art of any game in the
library. With your own free Twitch application, IGDB fills in the descriptions, genres, release
dates and screenshots that store pages leave out, added games and ROMs above all, and adds its
covers and artworks to the art picker. It is off by default, and a game is looked up only when
you open it. Libraries from Heroic, Lutris, Bottles and Faugus Launcher import in one click, and on
Windows so does Playnite’s.

## Your colours, your size

On KDE Plasma, COSMIC and Omarchy Irori follows your desktop’s theme; elsewhere it takes the
system’s accent colour. Turn it on, and it follows the colours pywal, wallust or matugen take from
your wallpaper. An `appearance.json` in the config folder lays your own colours over any theme,
applied as you save it. <kbd>Ctrl</kbd> and <kbd>+</kbd>, <kbd>−</kbd> or <kbd>0</kbd> change the
interface size, on top of the text size Linux or Windows asks for.

## Built for big libraries

Every long list in Irori is virtualized, so only the covers on screen exist at any moment. Cover
art is decoded once into thumbnails on disk and held in a bounded GPU cache, and filtered and
sorted lists are rebuilt only when the library changes, so scrolling through thousands of games
stays smooth and memory stays flat.
