#!/usr/bin/env python3
"""Compare the memory and idle CPU of Irori and Heroic on Linux, or benchmark
Irori across seeded library sizes (0, 300, 1,000 games with local covers).

Each app starts in its own session with an empty, throwaway profile, so real
logins and libraries stay out of it, and every helper process can be found by
session id. See docs/BENCHMARKS.md for the method and its limits.
"""
import argparse
import csv
import json
import os
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


def write_cover_png(path, index, width=300, height=450):
    """Write a distinct valid RGB PNG cover image using only the stdlib."""
    r = (index * 53 + 40) % 256
    g = (index * 97 + 80) % 256
    b = (index * 149 + 120) % 256
    band_r = (r + 70) % 256
    band_g = (g + 40) % 256
    band_b = (b + 90) % 256
    top_row = b"\x00" + bytes((r, g, b)) * width
    band_row = b"\x00" + bytes((band_r, band_g, band_b)) * width
    split = height // 2
    raw = top_row * split + band_row * (height - split)
    ihdr = struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0)
    png = (
        b"\x89PNG\r\n\x1a\n"
        + _png_chunk(b"IHDR", ihdr)
        + _png_chunk(b"IDAT", zlib.compress(raw, level=1))
        + _png_chunk(b"IEND", b"")
    )
    path.write_bytes(png)


def seed_sideload_library(irori_home, count):
    """Populate `count` Linux sideloaded apps with distinct 300x450 covers."""
    if count <= 0:
        return
    base = Path(irori_home)
    store_dir = base / "config" / "stores" / "sideload"
    covers_dir = base / "seed-covers"
    games_dir = base / "seed-games"
    store_dir.mkdir(parents=True, exist_ok=True)
    covers_dir.mkdir(parents=True, exist_ok=True)
    games_dir.mkdir(parents=True, exist_ok=True)

    dummy_exe = games_dir / "game.sh"
    dummy_exe.write_text("#!/bin/sh\nexit 0\n")
    dummy_exe.chmod(0o755)

    apps = []
    genres_pool = ["Action", "RPG", "Strategy", "Adventure", "Indie"]
    for idx in range(count):
        cover_path = covers_dir / f"cover-{idx:04d}.png"
        write_cover_png(cover_path, idx)
        apps.append(
            {
                "app_name": f"bench{idx:06d}",
                "title": f"Benchmark Game {idx + 1:04d}",
                "executable": str(dummy_exe),
                "platform": "linux",
                "cover_art": str(cover_path),
                "install_folder": str(games_dir),
                "launcher_args": "",
                "genres": [genres_pool[idx % len(genres_pool)]],
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


def measure(app, program, round_number, library_size=0, settle=SETTLE_SECONDS, idle=IDLE_SECONDS):
    with tempfile.TemporaryDirectory(prefix=f"{app}-bench-") as home:
        env = dict(os.environ)
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
        proc = subprocess.Popen(
            [program], env=env, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL, start_new_session=True,
        )
        sid = proc.pid
        try:
            time.sleep(settle)
            before = cpu_ticks(session_pids(sid))
            start = time.monotonic()
            time.sleep(idle)
            pids = session_pids(sid)
            used = (cpu_ticks(pids) - before) / TICKS_PER_SECOND
            return {
                "app": app,
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
    parser.add_argument("--rounds", type=int, default=3)
    parser.add_argument("--settle", type=int, default=SETTLE_SECONDS)
    parser.add_argument("--idle", type=int, default=IDLE_SECONDS)
    parser.add_argument("--output", default="results.csv", help="CSV file to write")
    args = parser.parse_args()

    sizes = [int(part.strip()) for part in args.library_sizes.split(",") if part.strip()]
    apps = {"irori": args.irori} if args.irori_only else {"irori": args.irori, "heroic": args.heroic}
    rows = []

    if args.irori_only:
        for size in sizes:
            for number in range(1, args.rounds + 1):
                rows.append(measure("irori", args.irori, number, size, args.settle, args.idle))
                print(rows[-1], flush=True)
    else:
        for number in range(1, args.rounds + 1):
            # Alternate the order so a warm disk cache doesn't always favour the same app.
            for app in sorted(apps, reverse=number % 2 == 1):
                size = sizes[0] if app == "irori" else 0
                rows.append(measure(app, apps[app], number, size, args.settle, args.idle))
                print(rows[-1], flush=True)

    with open(args.output, "w", newline="") as file:
        writer = csv.DictWriter(file, fieldnames=rows[0].keys())
        writer.writeheader()
        writer.writerows(rows)

    for app in apps:
        for size in (sizes if app == "irori" else [0]):
            subset = [r for r in rows if r["app"] == app and r["library_size"] == size]
            memory = statistics.median(r["pss_mib"] for r in subset)
            cpu = statistics.median(r["idle_cpu_percent"] for r in subset)
            print(f"{app} (games={size}): median {memory} MiB, median {cpu}% idle CPU")


if __name__ == "__main__":
    main()
