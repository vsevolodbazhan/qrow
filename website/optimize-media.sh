#!/bin/sh
# Make the web media in website/media/ from the README screenshot
# assets/screenshots/demo.png: crop it to the window (remove the transparent
# shadow), then write a WebP image and a JPEG social preview.
# Needs uv, cwebp, and ffmpeg.
set -eu

site="$(cd "$(dirname "$0")" && pwd)"
repository="$(dirname "$site")"
src="${1:-$repository/assets/screenshots/demo.png}"
out="$site/media"
mkdir -p "$out"

for tool in uv cwebp ffmpeg; do
    command -v "$tool" >/dev/null 2>&1 || { echo "Missing required command: $tool" >&2; exit 1; }
done

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

uv run --quiet --with pillow python -I - "$src" "$work" <<'PY'
import sys
from PIL import Image

src, work = sys.argv[1], sys.argv[2]
image = Image.open(src).convert("RGBA")
# The window is opaque. The shadow around it is partly transparent.
box = image.getchannel("A").point(lambda a: 255 if a >= 250 else 0).getbbox()
window = image.crop(box)
window.save(f"{work}/hero.png")
# The social preview has no transparency: put the corners on the page color.
background = Image.new("RGBA", window.size, (30, 32, 36, 255))
Image.alpha_composite(background, window).convert("RGB").save(f"{work}/og.png")
print(f"Window: {window.size[0]}x{window.size[1]} px")
PY

cwebp -quiet -q 82 -m 6 -exact "$work/hero.png" -o "$out/hero.webp"
echo "$out/hero.webp"
ffmpeg -loglevel error -y -i "$work/og.png" -vf "scale=1600:-2" -q:v 3 "$out/og.jpg"
echo "$out/og.jpg"
