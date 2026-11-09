#!/bin/sh
# Convert raw captures in website/capture/ to the web media in website/media/.
# Stills: PNG -> WebP. Videos: MP4 (H.264) re-encoded with faststart, plus a JPEG poster.
# Needs cwebp, sips (macOS), and ffmpeg.
set -eu

site="$(cd "$(dirname "$0")" && pwd)"
src="${1:-$site/capture}"
out="$site/media"
mkdir -p "$out"

for tool in cwebp ffmpeg; do
    command -v "$tool" >/dev/null 2>&1 || { echo "Missing required command: $tool" >&2; exit 1; }
done

for png in "$src"/*.png; do
    [ -e "$png" ] || continue
    name="$(basename "$png" .png)"
    cwebp -quiet -q 82 -m 6 "$png" -o "$out/$name.webp"
    echo "$out/$name.webp"
done

# Social preview image: JPEG, because some sites do not read WebP.
if [ -e "$src/hero.png" ]; then
    ffmpeg -loglevel error -y -i "$src/hero.png" -vf "scale=1600:-2" -q:v 3 "$out/og.jpg"
    echo "$out/og.jpg"
fi

for mp4 in "$src"/*.mp4; do
    [ -e "$mp4" ] || continue
    name="$(basename "$mp4" .mp4)"
    ffmpeg -loglevel error -y -i "$mp4" -an -c:v libx264 -preset slow -crf 26 \
        -vf "scale=1600:-2,fps=30" -pix_fmt yuv420p -movflags +faststart "$out/$name.mp4"
    ffmpeg -loglevel error -y -sseof -0.1 -i "$out/$name.mp4" -frames:v 1 -q:v 3 "$out/$name-poster.jpg"
    echo "$out/$name.mp4"
done
