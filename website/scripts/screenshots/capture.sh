#!/bin/bash
# Capture Irori's screens on a headless 2x X server, for the website and README.
# Usage: capture.sh <irori binary> <demo IRORI_HOME> <out dir> [only-shot-name]
set -u
BIN=$1; HOME_DIR=$2; OUT=$3; ONLY=${4:-}
DISP=:77
mkdir -p "$OUT/bigpicture" /tmp/bigpicture

Xvfb $DISP -screen 0 2880x1800x24 -nolisten tcp >/dev/null 2>&1 &
XVFB=$!
trap 'kill $XVFB 2>/dev/null; pkill -f "^$BIN" 2>/dev/null' EXIT
sleep 1
echo "Xft.dpi: 192" | DISPLAY=$DISP xrdb -merge

run() { env -u WAYLAND_DISPLAY GPUI_X11_SCALE_FACTOR=2 HOME=${DEMO_USER_HOME:-/home/alex} DISPLAY=$DISP IRORI_HOME="$HOME_DIR" IRORI_LOG=warn "$BIN" "$@"; }

settings() { # merge JSON into iroriUi
  python3 - "$HOME_DIR/config/settings.json" "$1" <<'E'
import json,sys
p,patch=sys.argv[1],json.loads(sys.argv[2])
s=json.load(open(p))
s.setdefault("iroriUi",{}).update(patch)
json.dump(s,open(p,"w"),indent=1)
E
}

start() {
  pkill -f "^$BIN" 2>/dev/null; sleep 1
  run >/tmp/irori-capture.log 2>&1 &
  for _ in $(seq 60); do
    WID=$(DISPLAY=$DISP xdotool search --onlyvisible --class irori 2>/dev/null | head -1)
    [ -n "$WID" ] && break; sleep 0.5
  done
  DISPLAY=$DISP xdotool windowmove "$WID" 0 0 windowsize "$WID" 2880 1800
  sleep 8   # library, art and thumbnails settle
}

go() { run "$1" >/dev/null 2>&1; sleep "${2:-4}"; }

shot() { # name, wait
  [ -n "$ONLY" ] && [ "$ONLY" != "$1" ] && return
  sleep "${2:-1}"
  DISPLAY=$DISP import -window root "/tmp/$1.png"
  local big small
  case $1 in bigpicture/*) big=2560x1600; small=1280x800;; *) big=2880x1800; small=1440x900;; esac
  convert "/tmp/$1.png" -crop "${big}+0+0" +repage "/tmp/$1-2x.png"
  convert "/tmp/$1-2x.png" -filter Lanczos -resize "$small" "/tmp/$1-1x.png"
  cwebp -quiet -q 84 "/tmp/$1-2x.png" -o "$OUT/$1@2x.webp"
  cwebp -quiet -q 86 "/tmp/$1-1x.png" -o "$OUT/$1.webp"
  echo "captured $1"
}

for MODE in dark light; do
  P=""; [ $MODE = light ] && P="light-"
  settings "{\"theme\":\"$MODE\",\"themeName\":\"\",\"bigPictureFullscreen\":false}"
  start
  go irori://library;   shot ${P}library 4
  go irori://downloads; shot ${P}downloads
  go irori://stats;     shot ${P}stats 3
  go irori://wine;      shot ${P}wine
  go irori://emulators; shot ${P}emulators
  go "irori://game/$(cat "$HOME_DIR/.demo-hero")"; shot ${P}game 6
  if [ $MODE = dark ]; then
    go irori://accounts;             shot accounts
    go irori://settings/appearance;  shot settings-appearance
    go "irori://game/$(cat "$HOME_DIR/.demo-timeline")" 8
    # Down to the play history's timeline.
    DISPLAY=$DISP xdotool mousemove 1900 1200 click --repeat ${KILN_SCROLL:-18} --delay 60 5
    shot game-kiln 3
  fi
done

if [ -z "$ONLY" ] || [ "$ONLY" = library-list ]; then
  settings '{"theme":"dark","libraryLayout":"list"}'
  start; go irori://library; shot library-list 4
  settings '{"libraryLayout":"grid"}'
fi

# Big Picture, in the window (not fullscreen) so its size is exact.
bp() { # name theme layout width height
  [ -n "$ONLY" ] && [ "$ONLY" != "$1" ] && return
  settings "{\"theme\":\"dark\",\"bigPictureTheme\":\"$2\",\"bigPictureLayout\":\"$3\",\"bigPictureFullscreen\":false}"
  start
  DISPLAY=$DISP xdotool windowsize "$WID" "$4" "$5"
  go irori://bigpicture 6
  shot "$1" 2
}
bp bigpicture "" shelves 2880 1800
bp bigpicture/glacier-shelves builtin:glacier shelves 2560 1600
bp bigpicture/handheld-compact builtin:handheld compact 2560 1600
bp bigpicture/hearth-spotlight builtin:hearth spotlight 2560 1600
bp bigpicture/irori-carousel builtin:irori carousel 2560 1600
bp bigpicture/moss-wall builtin:moss wall 2560 1600
bp bigpicture/neon-arcade-carousel builtin:neon-arcade carousel 2560 1600
bp bigpicture/noir-cinema builtin:noir cinema 2560 1600
settings '{"theme":"dark","bigPictureTheme":"","bigPictureLayout":"shelves"}'
echo done
