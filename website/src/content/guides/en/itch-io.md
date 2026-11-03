---
title: Install and play your itch.io games
seoTitle: 'itch.io launcher for Linux, macOS and Windows'
description: 'Turn on itch.io in Irori, log in with an API key and install, update and play the games you own on itch.io, on Linux, macOS and Windows.'
summary: Turn itch.io on, paste an API key, and your purchases install and update like any other store.
platforms: [linux, macos, windows]
order: 4
updated: 2026-11-03
related: [epic-games-on-linux, gog-on-linux, move-from-lutris]
---

itch.io is home to indie games, game jams and pay-what-you-want releases. Irori can install and
update the games you own there, on every system it runs on.

## 1. Turn on itch.io

itch.io is opt-in. Turn it on in **Settings › General › Integrations** (“itch.io store”), or from
its card on the **Accounts** page.

## 2. Log in with an API key

itch.io lets programs sign in with an API key. Create one at
[itch.io/user/settings/api-keys](https://itch.io/user/settings/api-keys) and paste it into
Irori. The key is kept with your other logins, encrypted, in Irori’s config folder. You can add
more than one itch.io account.

## 3. Install and play

Your purchases appear in the library. **Install** downloads the upload that fits your system,
and the download resumes if it is interrupted. Irori unpacks zip and tar archives (gzip, xz and
bzip2) into the game’s folder; single files such as AppImages or portable `.exe` files are copied
as they are. Disk images, 7-Zip and RAR files are refused.

The program to start comes from the game’s `.itch.toml` when it has one, else from a search of
its folder. Windows-only games run through Wine or Proton on Linux and macOS.

## Updates

When a developer uploads a new build, the update shows on the Updates page. Updates unpack next
to the game and then swap folders, so a failed update leaves the old files in place.
