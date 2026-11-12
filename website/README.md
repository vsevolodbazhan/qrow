# Website

This directory contains the Qrow landing page at <https://qrow.dev>: one
static page with the download links and the installation steps. It has no
build step.

## Downloads

The **Install** button scrolls to the Install section. The section selects
the system and the processor of the visitor and shows the download and the
installation steps for them. The visitor can select another system and
processor. Elements with a `data-arch` attribute show only for that
processor, for example the Homebrew command, which installs only the Apple
Silicon build. The package manager commands are in closed sections under the
download, because they are another way to install. [install.js](install.js) asks the GitHub API for the latest
stable release and links the download buttons to its files. This is the only
request to another site. Without the script, or if the request fails, the
page shows all systems, and the download buttons open the latest release
page.

The script finds the file of a button from its `data-asset` attribute: the
release file is `Qrow-VERSION-` followed by that value. If you change the
names of the release files, change these attributes in the same change. A
test compares them with the files that the release workflow uploads.

Limitations:

- Browsers hide the processor architecture. On a Mac, the page shows the
  Apple Silicon build, unless the browser shows an Intel or AMD graphics
  processor. Safari always shows Apple Silicon. The other build is one click
  away.
- The GitHub API accepts 60 requests in an hour from one IP address without
  sign-in. After that, the buttons open the latest release page.
- Phones and tablets get no system. The section shows macOS first.

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

## Update the screenshot

The page uses the first screenshot of the project README,
[assets/screenshots/demo.png](../assets/screenshots/demo.png).
After a change of that file, make the web media again:

```sh
sh website/optimize-media.sh
```

The script crops the screenshot to the window, removes the macOS shadow,
and writes `website/media/hero.webp`. The WebP image is lossless, so its
colors stay exact. The page background
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

- The page URLs in `og:url`, `og:image`, and the canonical link are
  `https://qrow.dev/`. Change them if the domain changes.
