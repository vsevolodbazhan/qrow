# Website

This directory contains the Qrow landing page: one static page that links to
the GitHub repository. It has no build step and makes no requests to other
sites.

## Preview the page

```sh
python3 -m http.server 8765 --directory website
```

Then open <http://localhost:8765/>. Do not open `index.html` as a file:
some browsers do not load the fonts from `file://` URLs.

## Design

The page uses the look of the app:

- The dark colors come from the One Dark theme in
  [src/one-dark.json](../src/one-dark.json). The light colors come from the
  GPUI Kit Default Light theme. The page follows the system theme. The button
  in the footer changes it, and the browser keeps that choice.
- The fonts are the app defaults: IBM Plex Sans for text and JetBrains Mono
  for code. `fonts/` has WOFF2 subsets of the files in
  [assets/fonts](../assets/fonts), with their licenses.
- The header copies the title bar of the app, and the footer copies its status
  bar. The icons are Lucide icons, the icon set of GPUI Kit.

## Update the fonts

Make the subsets again after a font update in `assets/fonts`:

```sh
for f in ibm-plex-sans/IBMPlexSans-Regular ibm-plex-sans/IBMPlexSans-Medium \
         ibm-plex-sans/IBMPlexSans-SemiBold jetbrains-mono/JetBrainsMono-Regular; do
    uv run --with fonttools --with brotli pyftsubset "assets/fonts/$f.ttf" \
        --unicodes="U+0000-00FF,U+0131,U+0152-0153,U+02C6,U+02DA,U+02DC,U+2000-206F,U+2190-21FF,U+2212,U+2215,U+2264-2265,U+2303,U+2318,U+2325,U+232B" \
        --layout-features='*' --flavor=woff2 \
        --output-file="website/fonts/$(basename "$f").woff2"
done
```

## Update the screenshot

The screenshot shows `qrow --demo`, so it contains only synthetic data.

1. Capture the Qrow window without its shadow as `website/capture/hero.png`
   (One Dark theme) and `website/capture/hero-light.png` (light theme). Use a
   window of 1360 × 850 points. Git ignores this directory.
2. Convert the captures:

   ```sh
   sh website/optimize-media.sh
   ```

   The script writes `hero.webp`, `hero-light.webp`, and the social preview
   `og.jpg` to `website/media/`.

## Limitations

- The page is not published yet. It has no hosting or deploy workflow.
