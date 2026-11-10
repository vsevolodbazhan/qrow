#!/bin/sh
# Make the social preview website/media/og.jpg: the app icon, the name, and
# the one-line description on the page background, like the top of the page.
# Messaging apps and social sites show it for links to the page.
# Needs uv.
set -eu

site="$(cd "$(dirname "$0")" && pwd)"
repository="$(dirname "$site")"
mkdir -p "$site/media"

command -v uv >/dev/null 2>&1 || { echo "Missing required command: uv" >&2; exit 1; }

uv run --quiet --with pillow python -I - "$repository" "$site/media/og.jpg" <<'PY'
import sys
from PIL import Image, ImageDraw, ImageFont

repository, out = sys.argv[1], sys.argv[2]
fonts = f"{repository}/assets/fonts/ibm-plex-sans"
# 1200x630 is the usual preview size. Draw it at 2x for high-density screens.
scale = 2
width, height = 1200 * scale, 630 * scale
background, title_color, text_color = (39, 41, 46), (215, 218, 224), (171, 178, 191)  # --bg, --fg-strong, --fg
title, description = "Qrow", "Desktop SQL workbench, written in Rust."

image = Image.new("RGB", (width, height), background)
draw = ImageDraw.Draw(image)

icon_size = 200 * scale
icon = Image.open(f"{repository}/assets/app-icons/macos/qrow.png").convert("RGBA")
icon = icon.resize((icon_size, icon_size), Image.Resampling.LANCZOS)

title_font = ImageFont.truetype(f"{fonts}/IBMPlexSans-SemiBold.ttf", 120 * scale)
# Make the description as large as possible inside the side margins.
size = 48
while True:
    text_font = ImageFont.truetype(f"{fonts}/IBMPlexSans-Regular.ttf", size * scale)
    if text_font.getlength(description) <= (1200 - 2 * 90) * scale:
        break
    size -= 1


def draw_text(top, text, font, color, tracking=0.0):
    """Draw centered text from its cap top. Tracking is in em, like CSS letter-spacing."""
    spacing = tracking * font.size
    advance = font.getlength(text) + spacing * (len(text) - 1)
    left = (width - advance) / 2
    cap_top = font.getbbox("H", anchor="ls")[1]
    baseline = top - cap_top
    for index, character in enumerate(text):
        # Measure up to the end of this character, so its kerning with the
        # previous character counts too.
        x = left + font.getlength(text[:index + 1]) - font.getlength(character) + spacing * index
        draw.text((x, baseline), character, font=font, fill=color, anchor="ls")
    return baseline


def cap_height(font):
    return -font.getbbox("H", anchor="ls")[1]


def descent(font, text):
    """How far the text goes below its baseline, for example the tail of Q."""
    return font.getbbox(text, anchor="ls")[3]


gap_icon, gap_text = 52 * scale, 36 * scale
title_height = cap_height(title_font) + descent(title_font, title)
block = icon_size + gap_icon + title_height + gap_text + cap_height(text_font)
top = (height - block) / 2
image.paste(icon, ((width - icon_size) // 2, round(top)), icon)
top += icon_size + gap_icon
draw_text(top, title, title_font, title_color, tracking=-0.025)
top += title_height + gap_text
draw_text(top, description, text_font, text_color)

image.save(out, quality=90, optimize=True, progressive=True)
print(f"{out}: {width}x{height} px, description {size} px")
PY
