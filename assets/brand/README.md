# Irori brand

| File | Use |
| --- | --- |
| `irori-mark.svg` | The app icon: Linux, Windows, the tray, favicons. Also `packaging/irori.svg`. |
| `irori-macos.svg` | The app icon on Apple's icon grid (824 pt tile on a 1024 canvas, with shadow). Source of `packaging/irori.icns`. |
| `irori-glyph.svg` | Tight-cropped bonfire and sword mark without canvas padding. |
| `irori-wordmark.svg` | The name in `currentColor` lettering with `+` and `○` accents. |
| `irori-lockup-dark.svg`, `irori-lockup-light.svg` | Mark and name side by side (`1620x512`), for dark and light backgrounds. |
| `irori-banner.svg` | Dark hero banner (`1024x571`) with tagline for website and promotional use. |

**The mark.** An RPG bonfire with a sword planted through the hearth logs, drawn as an open borderless silhouette so it stands clearly in desktop menus and light or dark UI themes. The `irori` wordmark pairs bold geometric letters with gamepad-inspired `+` and `○` accents above the `i` stems.

**Colours.** Outline `#0a0321`, hearth logs `#2f2946`, flame `#c9268a` -> `#ff5f3a` -> `#fddb15`.

**Rebuilding the icons** (needs `resvg`; `iconutil` is on macOS):

```bash
cp assets/brand/irori-mark.svg packaging/irori.svg
mkdir -p /tmp/irori.iconset
for s in 16 32 128 256 512; do
  resvg -w $s -h $s assets/brand/irori-macos.svg /tmp/irori.iconset/icon_${s}x${s}.png
  resvg -w $((s*2)) -h $((s*2)) assets/brand/irori-macos.svg /tmp/irori.iconset/icon_${s}x${s}@2x.png
done
iconutil -c icns /tmp/irori.iconset -o packaging/irori.icns
```

`packaging/irori.ico` holds PNG frames of `irori-mark.svg` at 16, 24, 32, 48, 64, 128 and 256 px.
