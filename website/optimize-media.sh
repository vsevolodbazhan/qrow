#!/bin/sh
# Make the page images in website/media from the screenshots in
# assets/screenshots: crop each image to its window, remove the macOS shadow,
# and write a lossless WebP image. Lossless keeps the exact colors, so the
# page background (--bg) matches the app panels. Feature screenshots are
# scaled to the largest size that the page shows.
# Needs uv and cwebp.
set -eu

site="$(cd "$(dirname "$0")" && pwd)"
repository="$(dirname "$site")"
shots="$repository/assets/screenshots"
out="$site/media"
mkdir -p "$out"

for tool in uv cwebp; do
    command -v "$tool" >/dev/null 2>&1 || { echo "Missing required command: $tool" >&2; exit 1; }
done

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

# Output name, maximum width in pixels (0 keeps the width), and source in
# assets/screenshots. The source is last, so it can contain spaces.
cat >"$work/list" <<'EOF'
hero 0 demo.png
connections 1800 demo-connections.png
results 1800 demo-results.png
export 1800 demo-export.png
catalog 1800 demo-catalog.png
dbt 1800 demo-dbt.png
assistant 1800 demo-assistant.png
gatekeeper-warning 0 gatekeeper/warning.png
gatekeeper-settings 0 gatekeeper/settings.png
EOF

uv run --quiet --with pillow python -I - "$work" "$shots" <<'PY'
import sys
from PIL import Image, ImageChops

work, shots = sys.argv[1], sys.argv[2]
with open(f"{work}/list") as entries:
    for line in entries:
        name, width, src = line.rstrip("\n").split(" ", 2)
        image = Image.open(f"{shots}/{src}").convert("RGBA")
        # The window is opaque. The shadow around it is partly transparent.
        box = image.getchannel("A").point(lambda a: 255 if a >= 250 else 0).getbbox()
        window = image.crop(box)
        # Remove what is left of the shadow: black pixels that are not fully
        # opaque. The antialiased window edge is gray, so it stays.
        r, g, b, a = window.split()
        dark = ImageChops.lighter(ImageChops.lighter(r, g), b).point(lambda v: 255 if v < 24 else 0)
        partial = a.point(lambda v: 255 if v < 255 else 0)
        window.putalpha(ImageChops.subtract(a, ImageChops.multiply(dark, partial)))
        width = int(width)
        if width and window.width > width:
            window = window.resize((width, round(window.height * width / window.width)), Image.LANCZOS)
        window.save(f"{work}/{name}.png")
        print(f"{name}: {window.width}x{window.height} px")
PY

while read -r name _; do
    cwebp -quiet -lossless -z 9 -exact "$work/$name.png" -o "$out/$name.webp"
    echo "$out/$name.webp"
done <"$work/list"
