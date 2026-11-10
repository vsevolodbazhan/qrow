# Website

This directory contains the Qrow landing page at <https://qrow.dev>. It is
one static page with no build step and no requests to other sites. It has
these sections:

- The hero: the name, the one-line description, and the app screenshot.
- **Why**: the reasons for the project and its inspiration.
- **Features**: one block for each main feature, with a screenshot and a
  link to its page in [docs](../docs).
- **Install** (`#install`): the install steps for macOS, Linux, and Windows,
  and the Gatekeeper steps for the first launch on macOS (`#gatekeeper`).

The page is the only place for the feature overview and the install steps.
The [project README](../README.md) links to it, and keeps the requirements,
the limitations, and the build steps. Update the page in the same change as
a feature, a release asset name, or an install step.

## Preview the page

```sh
python3 -m http.server 8765 --directory website
```

Then open <http://localhost:8765/>. Do not open `index.html` as a file:
some browsers do not load the fonts from `file://` URLs.

## Design

The page uses the look of the app:

- The colors come from the One Dark theme in
  [src/one-dark.json](../src/one-dark.json). The page has no light theme.
- The font is IBM Plex Sans, the default text font of the app. `fonts/` has
  WOFF2 subsets of the files in [assets/fonts](../assets/fonts), with the
  license.
- The screenshot has a soft shadow and no frame.
- The footer copies the status bar of the app.

## Update the fonts

Make the subsets again after a font update in `assets/fonts`:

```sh
for f in IBMPlexSans-Regular IBMPlexSans-Medium IBMPlexSans-SemiBold; do
    uv run --with fonttools --with brotli pyftsubset "assets/fonts/ibm-plex-sans/$f.ttf" \
        --unicodes="U+0000-00FF,U+0131,U+0152-0153,U+02C6,U+02DA,U+02DC,U+2000-206F,U+2190-21FF,U+2212,U+2215,U+2264-2265,U+2303,U+2318,U+2325,U+232B" \
        --layout-features='*' --flavor=woff2 \
        --output-file="website/fonts/$f.woff2"
done
```

## Update the screenshots

The page uses the screenshots in [assets/screenshots](../assets/screenshots):
`demo.png` for the hero, the `demo-*.png` files for the features, and the
files in `gatekeeper/` for the first launch on macOS. After a change of one
of these files, make the web media again:

```sh
sh website/optimize-media.sh
```

The script crops each screenshot to its window, removes the macOS shadow,
and writes a WebP image to `website/media/`. It scales the feature
screenshots to 1800 pixels wide, which is two times their largest width on
the page. The WebP images are lossless, so their colors stay exact. To add a
screenshot, add it to the list in the script, and set the `width` and
`height` of its `img` element to the size that the script prints.
Images below the hero load lazily. The page background
(`--bg` in `styles.css`) uses the panel color of the screenshot, `#27292e`.
Check it again after a new screenshot.

## Update the social preview

Messaging apps and social sites show `media/og.jpg` for links to the page.
The image shows the top of the page: the app icon, the name, and the
one-line description. Make it again after a change of the icon, the
description, or the page colors:

```sh
sh website/make-og.sh
```

The script draws the image from
[assets/app-icons/macos/qrow.png](../assets/app-icons/macos/qrow.png) and the
IBM Plex Sans fonts in [assets/fonts](../assets/fonts). The description and
the colors are in the script. Keep them the same as in `index.html` and
`styles.css`.

## Deployment

GitHub Pages hosts the page. The
[website workflow](../.github/workflows/website.yml) starts when a change
touches `website/` or the workflow:

- A pull request builds the site. It does not deploy it.
- A push to `main` builds the site and deploys it to <https://qrow.dev>.
- You can also start the workflow manually on `main`.

The site is a copy of `website/` without `README.md` and the scripts. The deployment takes approximately one minute after the
push. The repository settings (**Settings** > **Pages**) set the source to
**GitHub Actions** and the custom domain to `qrow.dev`.

The domain uses these DNS records:

| Name | Type | Value |
| --- | --- | --- |
| `qrow.dev` | `A` | `185.199.108.153`, `185.199.109.153`, `185.199.110.153`, `185.199.111.153` |
| `www.qrow.dev` | `CNAME` | `vsevolodbazhan.github.io` |
| `_github-pages-challenge-vsevolodbazhan.qrow.dev` | `TXT` | The code from the GitHub account settings (**Pages** > **Verified domains**) |

The TXT record verifies the domain for the GitHub account. Do not remove it:
without it, other GitHub users can claim the domain.

## Limitations

- The install section and the project README say that Linux and Windows
  archives are only in the nightly releases. Remove these statements after
  the first stable release with these archives.
- The Homebrew cask installs the DMG without an architecture suffix, which
  is the Apple Silicon build. The install section tells Intel users to
  download the `x86_64` DMG. Change it if the cask gets an Intel build.
- The page URLs in `og:url`, `og:image`, and the canonical link are
  `https://qrow.dev/`. Change them if the domain changes.
