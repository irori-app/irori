#!/usr/bin/env python3
"""Compare the memory and idle CPU of Irori and Heroic on Linux, or benchmark
Irori across seeded library sizes (0, 300, 1,000 games with rich metadata and
cover/hero art) and an existing user profile.

By default, each app starts in its own session with an empty, throwaway
profile, so real logins and libraries stay out of it, and every helper process
can be found by session id. Pass `--real-profile` (with `--irori-only`) to
measure aggregate process/memory/CPU on the active profile without changing it.
See docs/BENCHMARKS.md for the method and its limits.
"""
import argparse
import csv
import json
import os
import shutil
import signal
import statistics
import struct
import subprocess
import tempfile
import time
import zlib
from pathlib import Path

SETTLE_SECONDS = 45
IDLE_SECONDS = 30
TICKS_PER_SECOND = os.sysconf("SC_CLK_TCK")


def _png_chunk(tag, data):
    crc = zlib.crc32(tag + data) & 0xFFFFFFFF
    return struct.pack(">I", len(data)) + tag + data + struct.pack(">I", crc)


def write_png(path, index, width, height):
    """Write a distinct patterned RGB PNG image using only the stdlib."""
    c1 = bytes(((index * 53 + 40) % 256, (index * 97 + 80) % 256, (index * 149 + 120) % 256))
    c2 = bytes(((c1[0] + 65) % 256, (c1[1] + 45) % 256, (c1[2] + 85) % 256))
    c3 = bytes(((c1[0] + 130) % 256, (c1[1] + 95) % 256, (c1[2] + 35) % 256))
    half_w = width // 2
    row_a = b"\x00" + c1 * half_w + c2 * (width - half_w)
    row_b = b"\x00" + c2 * half_w + c3 * (width - half_w)
    row_c = b"\x00" + c3 * width
    h1 = height // 3
    h2 = (2 * height) // 3
    raw = row_a * h1 + row_b * (h2 - h1) + row_c * (height - h2)
    ihdr = struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0)
    png = (
        b"\x89PNG\r\n\x1a\n"
        + _png_chunk(b"IHDR", ihdr)
        + _png_chunk(b"IDAT", zlib.compress(raw, level=1))
        + _png_chunk(b"IEND", b"")
    )
    path.write_bytes(png)


def seed_sideload_library(irori_home, count):
    """Populate `count` Linux sideloaded apps with rich metadata, covers, and hero art."""
    if count <= 0:
        return
    base = Path(irori_home)
    store_dir = base / "config" / "stores" / "sideload"
    covers_dir = base / "seed-covers"
    heroes_dir = base / "seed-heroes"
    games_dir = base / "seed-games"
    store_dir.mkdir(parents=True, exist_ok=True)
    covers_dir.mkdir(parents=True, exist_ok=True)
    heroes_dir.mkdir(parents=True, exist_ok=True)
    games_dir.mkdir(parents=True, exist_ok=True)

    dummy_exe = games_dir / "game.sh"
    dummy_exe.write_text("#!/bin/sh\nexit 0\n")
    dummy_exe.chmod(0o755)

    hero_paths = []
    for hero_idx in range(20):
        hero_path = heroes_dir / f"hero-{hero_idx:02d}.png"
        write_png(hero_path, hero_idx + 1000, width=1280, height=720)
        hero_paths.append(str(hero_path))

    apps = []
    genres_pool = ["Action", "RPG", "Strategy", "Adventure", "Indie", "Simulation", "Puzzle"]
    for idx in range(count):
        cover_path = covers_dir / f"cover-{idx:04d}.png"
        write_png(cover_path, idx, width=300, height=450)
        g1 = genres_pool[idx % len(genres_pool)]
        g2 = genres_pool[(idx + 2) % len(genres_pool)]
        g3 = genres_pool[(idx + 4) % len(genres_pool)]
        year = 2015 + (idx % 11)
        month = (idx % 12) + 1
        day = (idx % 28) + 1
        dev = f"Northwind Interactive {(idx % 15) + 1}"
        description = (
            f"Benchmark Game {idx + 1:04d} is a {g1.lower()} and {g2.lower()} title developed by {dev}. "
            f"This seeded entry exercises library text storage, detail-page layout, and filter indexing across "
            f"large collections without contacting external storefronts.\n\n"
            f"Features full controller navigation, custom launch profiles, local cover thumbnails, "
            f"widescreen hero artwork, and per-genre categorization ({g1}, {g2}, {g3}). "
            f"Originally released on {year:04d}-{month:02d}-{day:02d} with Linux native runtime support, "
            f"configurable shader cache sizing, save backup integration, and offline library metadata verification."
        )
        apps.append(
            {
                "app_name": f"bench{idx:06d}",
                "title": f"Benchmark Game {idx + 1:04d}",
                "executable": str(dummy_exe),
                "platform": "linux",
                "cover_art": str(cover_path),
                "background_art": hero_paths[idx % len(hero_paths)],
                "install_folder": str(games_dir),
                "launcher_args": "",
                "description": description,
                "developer": dev,
                "release_date": f"{year:04d}-{month:02d}-{day:02d}",
                "genres": [g1, g2, g3],
            }
        )
    (store_dir / "library.json").write_text(
        json.dumps({"version": 1, "apps": apps})
    )


def stat_fields(pid):
    # The process name can hold spaces and parentheses; fields resume after the last ")".
    return Path(f"/proc/{pid}/stat").read_text().rsplit(")", 1)[1].split()


def session_pids(sid):
    pids = []
    for entry in Path("/proc").iterdir():
        if entry.name.isdigit():
            try:
                if int(stat_fields(entry.name)[3]) == sid:
                    pids.append(int(entry.name))
            except (OSError, ValueError, IndexError):
                pass
    return pids


def pss_mib(pids):
    """PSS splits shared pages between processes, so several aren't over-counted."""
    total = 0
    for pid in pids:
        try:
            for line in Path(f"/proc/{pid}/smaps_rollup").read_text().splitlines():
                if line.startswith("Pss:"):
                    total += int(line.split()[1])
        except OSError:
            pass
    return total / 1024


def cpu_ticks(pids):
    total = 0
    for pid in pids:
        try:
            fields = stat_fields(pid)
            total += int(fields[11]) + int(fields[12])
        except (OSError, ValueError, IndexError):
            pass
    return total


def stop(sid, proc):
    try:
        os.killpg(sid, signal.SIGTERM)
    except ProcessLookupError:
        pass
    time.sleep(3)
    for pid in session_pids(sid):
        try:
            os.kill(pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
    proc.wait()


def binary_size_mb(program):
    try:
        return round(Path(program).resolve().stat().st_size / 1_000_000, 1)
    except OSError:
        return None


def real_library_size(program):
    try:
        out = subprocess.check_output([program, "list"], text=True, stderr=subprocess.DEVNULL)
        return sum(1 for line in out.splitlines() if line.strip())
    except Exception:
        return -1


def maybe_capture_screenshot(screenshot_dir, label):
    if not screenshot_dir or not shutil.which("spectacle"):
        return
    out_dir = Path(screenshot_dir)
    out_dir.mkdir(parents=True, exist_ok=True)
    out_file = out_dir / f"bench-{label}.png"
    subprocess.run(
        ["spectacle", "-a", "-b", "-n", "-o", str(out_file)],
        stdin=subprocess.DEVNULL,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        timeout=10,
        check=False,
    )


def clean_env(keep_overlays=False):
    env = dict(os.environ)
    if not keep_overlays:
        for key in list(env):
            upper = key.upper()
            if (
                upper.startswith("MANGOHUD")
                or upper in ("ENABLE_VKBASALT", "OBS_VKCAPTURE", "GALLIUM_HUD")
            ):
                env.pop(key, None)
    return env


def _run_session(
    app,
    profile_kind,
    program,
    env,
    round_number,
    library_size,
    settle,
    idle,
    screenshot_dir=None,
    shot_label=None,
):
    proc = subprocess.Popen(
        [program], env=env, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL, start_new_session=True,
    )
    sid = proc.pid
    try:
        time.sleep(settle)
        if screenshot_dir and round_number == 1 and shot_label:
            maybe_capture_screenshot(screenshot_dir, shot_label)
        before = cpu_ticks(session_pids(sid))
        start = time.monotonic()
        time.sleep(idle)
        pids = session_pids(sid)
        used = (cpu_ticks(pids) - before) / TICKS_PER_SECOND
        return {
            "app": app,
            "profile": profile_kind,
            "library_size": library_size,
            "round": round_number,
            "running": proc.poll() is None,
            "processes": len(pids),
            "pss_mib": round(pss_mib(pids), 1),
            "idle_cpu_percent": round(used / (time.monotonic() - start) * 100, 3),
            "binary_mb": binary_size_mb(program),
        }
    finally:
        stop(sid, proc)


def measure(
    app,
    program,
    round_number,
    library_size=0,
    settle=SETTLE_SECONDS,
    idle=IDLE_SECONDS,
    real_profile=False,
    screenshot_dir=None,
    shot_label=None,
    keep_overlays=False,
):
    if real_profile:
        env = clean_env(keep_overlays=keep_overlays)
        return _run_session(
            app, "real", program, env, round_number, library_size,
            settle, idle, screenshot_dir, shot_label,
        )

    with tempfile.TemporaryDirectory(prefix=f"{app}-bench-") as home:
        env = clean_env(keep_overlays=keep_overlays)
        irori_home = f"{home}/irori"
        env.update(
            HOME=home,
            XDG_CONFIG_HOME=f"{home}/.config",
            XDG_DATA_HOME=f"{home}/.local/share",
            XDG_CACHE_HOME=f"{home}/.cache",
            XDG_STATE_HOME=f"{home}/.local/state",
            IRORI_HOME=irori_home,
        )
        if app == "irori" and library_size > 0:
            seed_sideload_library(irori_home, library_size)
        return _run_session(
            app, "synthetic", program, env, round_number, library_size,
            settle, idle, screenshot_dir, shot_label,
        )


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--irori", default="irori", help="Irori program")
    parser.add_argument("--heroic", default="heroic", help="Heroic program")
    parser.add_argument("--irori-only", action="store_true", help="Benchmark Irori without Heroic")
    parser.add_argument(
        "--library-sizes",
        default="0",
        help="Comma-separated sideloaded library sizes to benchmark for Irori (e.g. 0,300,1000)",
    )
    parser.add_argument(
        "--real-profile",
        action="store_true",
        help="Measure Irori against the active user profile without modifying it (requires --irori-only)",
    )
    parser.add_argument(
        "--real-library-size",
        type=int,
        default=None,
        help="Override game count when using --real-profile (otherwise runs `irori list`)",
    )
    parser.add_argument("--screenshot-dir", default=None, help="Optional directory to save round-1 screenshots via spectacle")
    parser.add_argument(
        "--keep-overlays",
        action="store_true",
        help="Keep host HUD overlay env vars (MANGOHUD, ENABLE_VKBASALT, OBS_VKCAPTURE, GALLIUM_HUD)",
    )
    parser.add_argument("--rounds", type=int, default=3)
    parser.add_argument("--settle", type=int, default=SETTLE_SECONDS)
    parser.add_argument("--idle", type=int, default=IDLE_SECONDS)
    parser.add_argument("--output", default="results.csv", help="CSV file to write")
    args = parser.parse_args()

    if args.real_profile and not args.irori_only:
        parser.error("--real-profile requires --irori-only")

    sizes = [int(part.strip()) for part in args.library_sizes.split(",") if part.strip()]
    apps = {"irori": args.irori} if args.irori_only else {"irori": args.irori, "heroic": args.heroic}
    rows = []

    if args.real_profile:
        count = args.real_library_size if args.real_library_size is not None else real_library_size(args.irori)
        for number in range(1, args.rounds + 1):
            rows.append(
                measure(
                    "irori",
                    args.irori,
                    number,
                    count,
                    args.settle,
                    args.idle,
                    real_profile=True,
                    screenshot_dir=args.screenshot_dir,
                    shot_label="real",
                    keep_overlays=args.keep_overlays,
                )
            )
            print(rows[-1], flush=True)
    elif args.irori_only:
        for size in sizes:
            for number in range(1, args.rounds + 1):
                rows.append(
                    measure(
                        "irori",
                        args.irori,
                        number,
                        size,
                        args.settle,
                        args.idle,
                        screenshot_dir=args.screenshot_dir,
                        shot_label=f"{size}-games",
                        keep_overlays=args.keep_overlays,
                    )
                )
                print(rows[-1], flush=True)
    else:
        for number in range(1, args.rounds + 1):
            # Alternate the order so a warm disk cache doesn't always favour the same app.
            for app in sorted(apps, reverse=number % 2 == 1):
                size = sizes[0] if app == "irori" else 0
                rows.append(
                    measure(
                        app, apps[app], number, size, args.settle, args.idle,
                        keep_overlays=args.keep_overlays,
                    )
                )
                print(rows[-1], flush=True)

    with open(args.output, "w", newline="") as file:
        writer = csv.DictWriter(file, fieldnames=rows[0].keys())
        writer.writeheader()
        writer.writerows(rows)

    for app in apps:
        group_sizes = [rows[0]["library_size"]] if args.real_profile else (sizes if app == "irori" else [0])
        for size in group_sizes:
            subset = [r for r in rows if r["app"] == app and r["library_size"] == size]
            memory = statistics.median(r["pss_mib"] for r in subset)
            cpu = statistics.median(r["idle_cpu_percent"] for r in subset)
            print(f"{app} (games={size}): median {memory} MiB, median {cpu}% idle CPU")


if __name__ == "__main__":
    main()
