#!/bin/sh
# Installs the latest NLE release without building it: downloads the Linux tarball from GitHub and puts it under
# ~/.local (PREFIX=/usr/local for a system-wide install, as root).
#
#   curl -fsSL https://raw.githubusercontent.com/SegMind25/NLE/main/get-nle.sh | sh
#   NLE_VERSION=v0.3.0 sh get-nle.sh        # a specific version instead of the latest
#   NLE_BASE_URL=http://mirror/dir sh get-nle.sh   # download the files from somewhere else (with NLE_VERSION)
set -eu

REPO="SegMind25/NLE"
PREFIX="${PREFIX:-$HOME/.local}"
VERSION="${NLE_VERSION:-latest}"

if [ "$(uname -m)" != "x86_64" ] || [ "$(uname -s)" != "Linux" ]; then
    echo "get-nle.sh: NLE runs Windows x64 code natively, so it needs x86-64 Linux." >&2
    exit 1
fi

# fetch URL: prints the file to stdout
if command -v curl >/dev/null 2>&1; then
    fetch() { curl -fsSL "$1"; }
elif command -v wget >/dev/null 2>&1; then
    fetch() { wget -q -O - "$1"; }
else
    echo "get-nle.sh: needs curl or wget" >&2
    exit 1
fi

if [ "$VERSION" = "latest" ]; then
    VERSION="$(fetch "https://api.github.com/repos/$REPO/releases/latest" | sed -n 's/.*"tag_name": *"\([^"]*\)".*/\1/p' | head -n 1)"
    [ -n "$VERSION" ] || { echo "get-nle.sh: cannot find the latest release of $REPO" >&2; exit 1; }
fi
name="nle-$VERSION-linux-x86_64"
base="${NLE_BASE_URL:-https://github.com/$REPO/releases/download/$VERSION}"

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
echo "downloading NLE $VERSION"
fetch "$base/$name.tar.gz" > "$tmp/$name.tar.gz"
if fetch "$base/SHA256SUMS" > "$tmp/SHA256SUMS" 2>/dev/null && command -v sha256sum >/dev/null 2>&1; then
    (cd "$tmp" && grep " $name.tar.gz\$" SHA256SUMS | sha256sum -c - >/dev/null) || { echo "get-nle.sh: the download does not match its checksum" >&2; exit 1; }
fi
tar -C "$tmp" -xzf "$tmp/$name.tar.gz"

src="$tmp/$name"
install -d "$PREFIX/bin" "$PREFIX/lib/nle" "$PREFIX/share/applications" "$PREFIX/share/icons/hicolor/256x256/apps" "$PREFIX/share/man/man1"
install -m 755 "$src/bin/nle" "$PREFIX/bin/nle"
install -m 755 "$src/lib/nle/libnle.so" "$PREFIX/lib/nle/libnle.so"
sed "s|^Exec=nle |Exec=$PREFIX/bin/nle |" "$src/share/applications/nle.desktop" > "$PREFIX/share/applications/nle.desktop"
install -m 644 "$src/share/icons/hicolor/256x256/apps/nle.png" "$PREFIX/share/icons/hicolor/256x256/apps/nle.png"
install -m 644 "$src/share/man/man1/nle.1.gz" "$PREFIX/share/man/man1/nle.1.gz"

echo "installed NLE $VERSION in $PREFIX (nle, the launcher entry in the application menu, man page)"
case ":$PATH:" in
    *":$PREFIX/bin:"*) ;;
    *) echo "note: $PREFIX/bin is not on your PATH" ;;
esac
"$PREFIX/bin/nle" doctor || true
