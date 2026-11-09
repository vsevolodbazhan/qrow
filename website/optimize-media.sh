#!/bin/sh
# Convert the raw screenshots in website/capture/ to the web media in website/media/.
# Needs cwebp and ffmpeg.
set -eu

site="$(cd "$(dirname "$0")" && pwd)"
src="${1:-$site/capture}"
out="$site/media"
mkdir -p "$out"

for tool in cwebp ffmpeg; do
    command -v "$tool" >/dev/null 2>&1 || { echo "Missing required command: $tool" >&2; exit 1; }
done

for name in hero hero-light; do
    [ -e "$src/$name.png" ] || { echo "Missing $src/$name.png" >&2; exit 1; }
    cwebp -quiet -q 82 -m 6 "$src/$name.png" -o "$out/$name.webp"
    echo "$out/$name.webp"
done

# Social preview image: JPEG, because some sites do not read WebP.
ffmpeg -loglevel error -y -i "$src/hero.png" -vf "scale=1600:-2" -q:v 3 "$out/og.jpg"
echo "$out/og.jpg"
