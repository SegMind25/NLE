#!/bin/sh
# Renders the PNG versions of the logo and the social preview from the SVG / HTML sources,
# using a headless Chromium (CHROME=/path/to/chrome to choose one).
set -eu
cd "$(dirname "$0")/.."
# The headless shell honours --window-size exactly; full Chrome subtracts its window frame.
CHROME="${CHROME:-$(ls /opt/pw-browsers/chromium_headless_shell-*/chrome-linux/headless_shell 2>/dev/null | head -n 1)}"
CHROME="${CHROME:-$(command -v chromium || command -v chromium-browser || command -v google-chrome)}"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

shot() { # shot <source> <output> <width> <height> [scale] [transparent]
    "$CHROME" --headless --no-sandbox --disable-gpu --hide-scrollbars --allow-file-access-from-files \
        --force-device-scale-factor="${5:-1}" ${6:+--default-background-color=00000000} \
        --window-size="$3,$4" --screenshot="$2" "file://$1" >/dev/null 2>&1
}

page() { # page <svg> <width> <height>: an HTML page that shows the SVG edge to edge
    printf '<!doctype html><meta charset=utf-8><style>html,body{margin:0;background:transparent}img{display:block;width:%spx;height:%spx}</style><img src="file://%s">' "$2" "$3" "$PWD/$1"
}

page assets/logo-mark.svg 512 512 > "$tmp/mark.html"
shot "$tmp/mark.html" assets/logo-mark.png 512 512 2 transparent
page assets/logo.svg 1000 360 > "$tmp/logo.html"
shot "$tmp/logo.html" assets/logo.png 1000 360 2 transparent
page assets/logo-dark.svg 1000 360 > "$tmp/logo-dark.html"
shot "$tmp/logo-dark.html" assets/logo-dark.png 1000 360 2 transparent
page assets/logo-mark.svg 256 256 > "$tmp/fav.html"
shot "$tmp/fav.html" assets/favicon.png 256 256 1 transparent
shot "$PWD/tools/social-preview.html" assets/social-preview.png 1280 640 1
ls -l assets/*.png
