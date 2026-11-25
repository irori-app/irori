# Benchmarks

How the numbers in the README and on [irori.app](https://irori.app) were measured,
so anyone can check them.

## Irori 1.1.0 across 0, 300, and 1,000 games

Measured on 24 November 2026 on CachyOS (Linux `7.3.5-2-cachyos-bore`, KDE Plasma
on Wayland) with an AMD Radeon RX 9070 XT (`radv`, Mesa `26.3.1-arch3.5`) using
the `irori 1.1.0` release binary (`/usr/bin/irori`, `108.1 MB`). Note that this
run uses the current `1.1.0` release binary as the pre-v1.3 baseline and should
be re-run once `v1.3.0` is tagged.

Each scenario ran 2 rounds in a throwaway `IRORI_HOME` with `--settle 20` and
`--idle 15` (`compare.py --irori-only`). For 300 and 1,000 games, `compare.py`
seeded `stores/sideload/library.json` with Linux entries and distinct local
`300x450` PNG cover art so Irori loads covers into its thumbnail and GPU cache.

| Library size | Memory PSS (median) | Memory PSS (rounds) | Processes | Idle CPU (median) | Binary size |
| --- | --- | --- | --- | --- | --- |
| 0 games | 88.3 MiB | 76.4, 100.1 MiB | 1 | 2.30 % | 108.1 MB |
| 300 games (with covers) | 119.4 MiB | 121.4, 117.4 MiB | 1 | 2.00 % | 108.1 MB |
| 1,000 games (with covers) | 126.0 MiB | 128.0, 123.9 MiB | 1 | 1.90 % | 108.1 MB |

To repeat this run on Linux:

```bash
python3 docs/benchmarks/compare.py --irori /usr/bin/irori --irori-only \
  --library-sizes 0,300,1000 --rounds 2 --settle 20 --idle 15 --output results.csv
```

## Irori 0.2.0 and Heroic 2.22.4 (Historical baseline)

| | Irori 0.2.0 | Heroic 2.22.4 |
| --- | --- | --- |
| Memory (median of 3 rounds) | 87.5 MiB | 478.2 MiB |
| Memory (all rounds) | 87.4 to 92.2 MiB | 472.7 to 483.4 MiB |
| Processes | 1 | 7 |
| CPU while idle, % of one core | 0.0, 0.0, 0.0 | 6.6, 0.2, 7.1 |
| Program size | 76 MB | 368 MB |

Measured on 30 October 2026 on Nobara (Linux 7.2, KDE on Wayland) with an AMD
Radeon RX 7900 XTX (`--settle 45`, `--idle 30`, empty profile). Irori was the
`irori` binary from the 0.2.0 Linux tarball, and Heroic was Fedora's
`heroic-games-launcher` package. Program size is that binary for Irori and
`/usr/lib64/heroic` for Heroic, which includes the store tools Heroic ships with.

## Method

[`benchmarks/compare.py`](benchmarks/compare.py) runs each app with a throwaway
home directory: no logins, and the window open at its default size. In comparison
mode (`--irori` and `--heroic`), it alternates between both apps. In each round it:

1. Starts the app in a new session, so every process it spawns can be found.
2. Waits for startup work to settle (`--settle`, 45 seconds by default).
3. Counts the session's CPU time over the idle window (`--idle`, 30 seconds by
   default), as a percentage of one core.
4. Reads memory as the sum of each process's PSS from `/proc/<pid>/smaps_rollup`.
   PSS divides shared pages between the processes using them, so an app with
   several processes isn't counted twice for the same memory.
5. Closes the app.

To repeat the Heroic comparison when both apps are installed:

```bash
python3 docs/benchmarks/compare.py --irori irori --heroic heroic
```

## Limits

- Sideloaded 300- and 1,000-game runs measure cold open and library rendering with
  local covers, not background store catalog sync or active scrolling stress.
- Shorter settle windows (`20` seconds vs `45` seconds in `0.2.0`) can still
  catch one-time first-run background tasks in the idle CPU sample.
- Video memory (VRAM) isn't counted for either app.
- Each benchmark reflects one machine; other drivers, compositors, and window
  sizes will produce different numbers.
