# Website

This directory contains the Qrow landing page. The page is static HTML, CSS,
and a small script. It has no build step and makes no requests to other sites.

## Preview the page

Serve the directory and open it in a browser:

```sh
python3 -m http.server 8765 --directory website
```

Then open <http://localhost:8765/>. Do not open `index.html` as a file:
some browsers do not load the fonts and the videos from `file://` URLs.

## Design

The page uses the look of the app, so that the site and the app agree:

- The dark colors come from the One Dark theme in
  [src/one-dark.json](../src/one-dark.json). The light colors come from the
  GPUI Kit Default Light theme. The page follows the system theme. The button
  in the footer changes it, and the browser keeps that choice.
- The fonts are the app defaults: IBM Plex Sans for text and JetBrains Mono
  for code. `fonts/` has WOFF2 subsets of the files in
  [assets/fonts](../assets/fonts), with their licenses.
- The parts of the page copy parts of the app: the title bar, the
  Connections tree, query tabs, the editor, the result grid, and the status
  bar. The result grid follows the app rules: headers align left, and only
  numbers align right.
- The icons are Lucide icons, the icon set of GPUI Kit.

## Update the fonts

Make the subsets again after a font update in `assets/fonts`. The page uses
IBM Plex Sans Regular, Medium, and SemiBold, and JetBrains Mono Regular:

```sh
for f in ibm-plex-sans/IBMPlexSans-Regular ibm-plex-sans/IBMPlexSans-Medium \
         ibm-plex-sans/IBMPlexSans-SemiBold jetbrains-mono/JetBrainsMono-Regular; do
    uv run --with fonttools --with brotli pyftsubset "assets/fonts/$f.ttf" \
        --unicodes="U+0000-00FF,U+0131,U+0152-0153,U+02C6,U+02DA,U+02DC,U+2000-206F,U+2190-21FF,U+2212,U+2215,U+2264-2265,U+2303,U+2318,U+2325,U+232B" \
        --layout-features='*' --flavor=woff2 \
        --output-file="website/fonts/$(basename "$f").woff2"
done
```

## Update the screenshots and videos

The media show `qrow --demo`, so they contain only synthetic data.

1. Capture stills as PNG and videos as MP4 into `website/capture/`. Use a
   window of 1440 × 900 points and the One Dark theme. Git ignores this
   directory.
2. Convert the captures:

   ```sh
   sh website/optimize-media.sh
   ```

   The script writes WebP stills, a JPEG social preview (`og.jpg`), MP4
   videos, and a JPEG poster for each video to `website/media/`.
3. Keep the file names that `index.html` uses. Update the captions and the
   `alt` texts when the content of a capture changes.

## Limitations

- The page is not published yet. It has no hosting or deploy workflow.
- The product name can change. The name shows in the page title, the
  headings, the copy, and the alt texts.
