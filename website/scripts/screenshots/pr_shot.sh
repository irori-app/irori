#!/usr/bin/env bash
# Capture dark (Irori Night) and light (Irori Day) screenshots of a single
# Irori route on a headless Linux X server using an isolated demo IRORI_HOME.
#
# Usage:
#   website/scripts/screenshots/pr_shot.sh <irori-bin> <route> <out-dir> [width] [height]
#
# Example:
#   cargo build --locked --profile quick -p irori
#   website/scripts/screenshots/pr_shot.sh target/quick/irori irori://library /tmp/pr-shots
#   website/scripts/screenshots/pr_shot.sh target/quick/irori irori://game/hero /tmp/pr-shots 1440 900

set -euo pipefail

if [ "$#" -lt 3 ] || [ "$#" -gt 5 ]; then
  echo "Usage: $0 <irori-bin> <route> <out-dir> [width] [height]" >&2
  exit 2
fi

BIN=$1
ROUTE=$2
OUT_DIR=$3
WIDTH=${4:-1440}
HEIGHT=${5:-900}

SCRIPT_DIR=$(cd "$(dirname "$0")" && pwd)
REPO_ROOT=$(cd "$SCRIPT_DIR/../../.." && pwd)
SEED_SCRIPT="$SCRIPT_DIR/seed_demo.py"
COVERS_JSON="$REPO_ROOT/website/src/data/game-covers.json"

if [ ! -x "$BIN" ]; then
  echo "error: Irori binary not found or not executable: $BIN" >&2
  echo "Build it first with: cargo build --locked --profile quick -p irori" >&2
  exit 1
fi

missing=()
for cmd in Xvfb xdotool import python3; do
  if ! command -v "$cmd" >/dev/null 2>&1; then
    missing+=("$cmd")
  fi
done
if [ "${#missing[@]}" -gt 0 ]; then
  echo "error: missing required Linux screenshot tools: ${missing[*]}" >&2
  echo "Install Xvfb, xdotool, and ImageMagick (import), or capture manually with IRORI_HOME." >&2
  exit 1
fi

TMP_ROOT=$(mktemp -d "${TMPDIR:-/tmp}/irori-pr-shot.XXXXXX")
DEMO_HOME="$TMP_ROOT/home"
GAMES_DIR="$TMP_ROOT/games"
LOG_FILE="$TMP_ROOT/irori.log"
XVFB_PID=""
APP_PID=""

cleanup() {
  if [ -n "$APP_PID" ]; then
    kill "$APP_PID" 2>/dev/null || true
    wait "$APP_PID" 2>/dev/null || true
  fi
  if [ -n "$XVFB_PID" ]; then
    kill "$XVFB_PID" 2>/dev/null || true
    wait "$XVFB_PID" 2>/dev/null || true
  fi
  rm -rf "$TMP_ROOT"
}
trap cleanup EXIT

mkdir -p "$OUT_DIR" "$DEMO_HOME" "$GAMES_DIR"
python3 "$SEED_SCRIPT" "$COVERS_JSON" "$DEMO_HOME" "$GAMES_DIR" >/dev/null

# Support shorthand `irori://game/hero` and `irori://game/timeline` from seed_demo.py.
case "$ROUTE" in
  irori://game/hero)
    ROUTE="irori://game/$(cat "$DEMO_HOME/.demo-hero")"
    ;;
  irori://game/timeline)
    ROUTE="irori://game/$(cat "$DEMO_HOME/.demo-timeline")"
    ;;
esac

# Pick a free X display number.
DISP_NUM=88
while [ -e "/tmp/.X${DISP_NUM}-lock" ]; do
  DISP_NUM=$((DISP_NUM + 1))
done
DISP=":${DISP_NUM}"

Xvfb "$DISP" -screen 0 "${WIDTH}x${HEIGHT}x24" -nolisten tcp >/dev/null 2>&1 &
XVFB_PID=$!
sleep 1

patch_theme() {
  local mode=$1
  python3 - "$DEMO_HOME/config/settings.json" "$mode" <<'PY'
import json, sys
path, mode = sys.argv[1], sys.argv[2]
with open(path, encoding="utf-8") as fh:
    data = json.load(fh)
data.setdefault("iroriUi", {}).update({
    "theme": mode,
    "themeName": "",
    "bigPictureFullscreen": False,
})
with open(path, "w", encoding="utf-8") as fh:
    json.dump(data, fh, indent=1)
PY
}

capture_mode() {
  local mode=$1
  local out_png="$OUT_DIR/${mode}.png"
  patch_theme "$mode"

  env -u WAYLAND_DISPLAY DISPLAY="$DISP" IRORI_HOME="$DEMO_HOME" IRORI_LOG=warn \
    "$BIN" >"$LOG_FILE" 2>&1 &
  APP_PID=$!

  local wid=""
  for _ in $(seq 60); do
    wid=$(DISPLAY="$DISP" xdotool search --onlyvisible --class irori 2>/dev/null | head -n 1 || true)
    if [ -n "$wid" ]; then
      break
    fi
    sleep 0.5
  done

  if [ -z "$wid" ]; then
    echo "error: Irori window did not appear under Xvfb (see log excerpt below):" >&2
    tail -n 30 "$LOG_FILE" >&2 || true
    exit 1
  fi

  DISPLAY="$DISP" xdotool windowmove "$wid" 0 0 windowsize "$wid" "$WIDTH" "$HEIGHT"
  sleep 4

  env -u WAYLAND_DISPLAY DISPLAY="$DISP" IRORI_HOME="$DEMO_HOME" IRORI_LOG=warn \
    "$BIN" "$ROUTE" >/dev/null 2>&1 || true
  sleep 4

  DISPLAY="$DISP" import -window root "$out_png"
  echo "Captured $out_png ($ROUTE, theme=$mode, ${WIDTH}x${HEIGHT})"

  kill "$APP_PID" 2>/dev/null || true
  wait "$APP_PID" 2>/dev/null || true
  APP_PID=""
  sleep 1
}

capture_mode dark
capture_mode light
