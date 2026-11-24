#!/usr/bin/env python3
"""Seed an IRORI_HOME with a demo library for screenshots.

Real games and their public store art; accounts, play time, sessions and
installs are invented. No real account or personal data goes in.
Usage: seed_demo.py <game-covers.json> <IRORI_HOME> <games-dir>
"""
import json, os, random, sys
from datetime import datetime, timedelta, timezone

covers_path, home, games_dir = sys.argv[1:4]
games = json.load(open(covers_path))
random.seed(7)
now = datetime.now(timezone.utc).replace(microsecond=0)
iso = lambda d: d.isoformat().replace("+00:00", "Z")
cfg = os.path.join(home, "config")
state = os.path.join(cfg, "state")
cache = os.path.join(home, "cache")


def write(path, data, text=False):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w") as f:
        f.write(data if text else json.dumps(data, indent=1))
    os.chmod(path, 0o600)


def art(appid):
    base = f"https://shared.cloudflare.steamstatic.com/store_item_assets/steam/apps/{appid}"
    return {
        "cover": f"{base}/library_600x900_2x.jpg",
        "hero": f"{base}/library_hero.jpg",
        "logo": f"{base}/logo.png",
        "wide": f"{base}/header.jpg",
    }


DEVS = {
    "Cyberpunk 2077": ("CD PROJEKT RED", ["RPG", "Action"]),
    "The Witcher 3: Wild Hunt": ("CD PROJEKT RED", ["RPG"]),
    "Baldur's Gate 3": ("Larian Studios", ["RPG", "Strategy"]),
    "Hades": ("Supergiant Games", ["Action", "Roguelike"]),
    "Hades II": ("Supergiant Games", ["Action", "Roguelike"]),
    "Hollow Knight": ("Team Cherry", ["Metroidvania", "Action"]),
    "Celeste": ("Maddy Makes Games", ["Platformer"]),
    "Stardew Valley": ("ConcernedApe", ["Simulation", "RPG"]),
    "Outer Wilds": ("Mobius Digital", ["Adventure", "Exploration"]),
}

# Alternate stores so both filters have games; the first ones are played most.
epic, gog = [], []
for i, g in enumerate(games):
    (gog if i % 2 else epic).append(g)

epic_records, gog_records = [], []
installs_epic, installs_gog = {}, []
keys = []
for i, g in enumerate(games):
    a = art(g["appid"])
    dev, genres = DEVS.get(g["title"], ("", []))
    installed = i < 16
    folder = os.path.join(games_dir, g["file"])
    size = random.randint(4, 90) * 1024**3
    if g in epic:
        app = g["file"].replace("-", "")[:20]
        keys.append(f"legendary:{app}")
        epic_records.append({
            "app_name": app, "app_title": g["title"],
            "asset_infos": {"Windows": {"app_name": app, "namespace": app, "build_version": "1.0",
                                        "catalog_item_id": app, "label_name": "Live-Windows",
                                        "asset_id": "", "metadata": {}, "sidecar_rvn": 0}},
            "metadata": {"id": app, "namespace": app, "title": g["title"], "developer": dev,
                         "categories": [{"path": "games"}],
                         "keyImages": [{"type": "DieselGameBoxTall", "url": a["cover"]},
                                       {"type": "DieselGameBox", "url": a["hero"]},
                                       {"type": "DieselGameBoxLogo", "url": a["logo"]}],
                         "releaseInfo": [{"appId": app, "platform": ["Windows"]}],
                         "customAttributes": {"CanRunOffline": {"type": "STRING", "value": "true"}}},
        })
        if installed:
            os.makedirs(folder, exist_ok=True)
            installs_epic[app] = {"app_name": app, "title": g["title"], "version": "1.0",
                                  "platform": "Windows", "install_path": folder,
                                  "executable": "Game.exe", "install_size": size,
                                  "can_run_offline": True}
    else:
        app = str(1000000000 + g["appid"])
        keys.append(f"gog:{app}")
        gog_records.append({"runner": "gog", "app_name": app, "title": g["title"],
                            "developer": dev, "genres": genres,
                            "art_cover": a["cover"], "art_background": a["hero"],
                            "art_logo": a["logo"], "installable": True,
                            "platforms": ["windows"], "rating": random.randint(78, 96)})
        if installed:
            os.makedirs(folder, exist_ok=True)
            installs_gog.append({"appName": app, "platform": "windows", "executable": "Game.exe",
                                 "install_path": folder, "install_size": f"{size / 1024**3:.1f} GiB",
                                 "is_dlc": False, "version": "1.0", "installedDLCs": [],
                                 "pinnedVersion": False})

far = 4102444800
write(f"{cfg}/stores/legendary/session.json",
      {"access_token": "demo", "refresh_token": "demo", "expires_at": far,
       "refresh_expires_at": far, "account_id": "demo", "display_name": "Alex"})
write(f"{cfg}/stores/legendary/library.json", epic_records)
write(f"{cfg}/stores/legendary/installs.json", installs_epic)
write(f"{cfg}/stores/gog/auth.json",
      {"46899977096215655": {"access_token": "demo", "refresh_token": "demo", "user_id": "1",
                             "expires_in": 999999999, "loginTime": int(now.timestamp())}})
write(f"{cfg}/stores/gog/user.json", {"id": "1", "username": "Alex", "avatar": None})
write(f"{cache}/stores/gog/library.json", gog_records)
write(f"{cfg}/stores/gog/installed.json", {"installed": installs_gog})

# Play history: the first games are the ones being played lately.
sessions, playtime, status = [], {}, {}
statuses = ["playing", "playing", "beaten", "playing", "planToPlay", "onHold", "beaten",
            "abandoned", "beaten", "planToPlay"]
for rank, key in enumerate(keys[:20]):
    total = 0
    days = 120 - rank * 4
    first = None
    for d in range(days, -1, -1):
        if random.random() < max(0.08, 0.5 - rank * 0.03):
            start = (now - timedelta(days=d)).replace(hour=random.choice([12, 18, 19, 20, 21, 22]),
                                                      minute=random.randint(0, 59), second=0)
            secs = random.randint(20, 180) * 60
            if start + timedelta(seconds=secs) > now:
                continue
            sessions.append({"game": key, "started": iso(start), "seconds": secs})
            total += secs
            first = first or start
    if total:
        last = max(s["started"] for s in sessions if s["game"] == key)
        playtime[key] = {"total_minutes": total // 60, "extra_seconds": 0,
                         "last_played": last, "first_played": iso(first)}
    if rank < len(statuses):
        status[key] = {"status": statuses[rank], "since": iso(now - timedelta(days=30 + rank))}
sessions.sort(key=lambda s: s["started"])
recent = sorted(playtime, key=lambda k: playtime[k]["last_played"], reverse=True)

write(f"{state}/sessions.jsonl", "".join(json.dumps(s) + "\n" for s in sessions), text=True)
write(f"{state}/library.json", {
    "favourites": [keys[0], keys[2], keys[5], keys[8]], "hidden": [], "recent": recent,
    "categories": {"Cozy": [keys[i] for i in (6, 11, 15)], "With friends": [keys[i] for i in (2, 9)]},
    "playtime": playtime, "play_status": status,
    "ratings": {keys[0]: 5, keys[2]: 5, keys[5]: 4}, "first_seen": {},
    "backlog_goals": {str(now.year): 12},
})
pending = next(r for r in epic_records if r["app_name"] not in installs_epic)
write(f"{state}/downloads.json", {
    "active": [{"id": {"runner": "legendary", "app_name": pending["app_name"]},
                "title": pending["app_title"],
                "kind": {"Install": {"path": games_dir, "platform": "windows"}},
                "state": "Paused", "added_at": int(now.timestamp()) - 600,
                "started_at": int(now.timestamp()) - 500, "finished_at": None,
                "download_size": 42 * 1024**3, "error": None}],
    "finished": [], "paused": True})
write(f"{state}/onboarding-done", "done\n", text=True)
write(f"{state}/getting-started.json", {"look": True, "play": True})
write(f"{cfg}/settings.json", {"checkForUpdatesOnStartup": False,
                               "iroriCheckForAppUpdates": False,
                               "defaultInstallPath": games_dir})
# Link every game to its Steam app, so its page fills from Steam's public
# store page (description, screenshots, trailers) as Irori does for users.
for i, g in enumerate(games):
    runner, app = keys[i].split(":", 1)
    write(f"{cfg}/games/{runner}/{app}.json", {"iroriSteamAppId": str(g["appid"])})
by_title = {g["title"]: keys[i] for i, g in enumerate(games)}
for name, title in ((".demo-hero", "Baldur's Gate 3"), (".demo-timeline", "Outer Wilds")):
    write(os.path.join(home, name), by_title[title].replace(":", "/", 1), text=True)
print(f"seeded {len(epic_records)} Epic, {len(gog_records)} GOG, {len(sessions)} sessions")
