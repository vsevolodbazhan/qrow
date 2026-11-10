#!/bin/sh
# Make the page screenshot website/media/hero.webp from the README screenshot
# assets/screenshots/demo.png: crop it to the window and remove the macOS
# shadow, then write a lossless WebP image. Lossless keeps the exact colors, so
# the page background (--bg) matches the app panels.
# Needs uv and cwebp.
set -eu

site="$(cd "$(dirname "$0")" && pwd)"
repository="$(dirname "$site")"
src="${1:-$repository/assets/screenshots/demo.png}"
out="$site/media"
mkdir -p "$out"

for tool in uv cwebp; do
    command -v "$tool" >/dev/null 2>&1 || { echo "Missing required command: $tool" >&2; exit 1; }
done

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

uv run --quiet --with pillow python -I - "$src" "$work" <<'PY'
import sys
from PIL import Image, ImageChops

src, work = sys.argv[1], sys.argv[2]
image = Image.open(src).convert("RGBA")
# The window is opaque. The shadow around it is partly transparent.
box = image.getchannel("A").point(lambda a: 255 if a >= 250 else 0).getbbox()
window = image.crop(box)
# Remove what is left of the shadow: black pixels that are not fully opaque.
# The antialiased window edge is gray, so it stays.
r, g, b, a = window.split()
dark = ImageChops.lighter(ImageChops.lighter(r, g), b).point(lambda v: 255 if v < 24 else 0)
partial = a.point(lambda v: 255 if v < 255 else 0)
window.putalpha(ImageChops.subtract(a, ImageChops.multiply(dark, partial)))
window.save(f"{work}/hero.png")
print(f"Window: {window.size[0]}x{window.size[1]} px")
PY

cwebp -quiet -lossless -z 9 -exact "$work/hero.png" -o "$out/hero.webp"
echo "$out/hero.webp"
