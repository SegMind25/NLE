#!/bin/sh
# Builds the Linux packages from an existing release build (cargo build --release):
#
#   packaging/build.sh                 # everything: tar.gz, .deb, .rpm, Arch .pkg.tar.zst, AppImage, SHA256SUMS
#   packaging/build.sh deb appimage    # only these (tar deb rpm arch appimage)
#
# The packages are written to dist/ (NLE_OUT). nfpm and appimagetool are downloaded once into target/packaging-tools
# unless they are already on the PATH. Set NLE_BUILD to use binaries from somewhere other than target/release.
#
# The binaries should be built on an old Linux (the release workflow uses AlmaLinux 8, glibc 2.28) so the packages run on
# every distribution newer than that.
set -eu
cd "$(dirname "$0")/.."

VERSION="${NLE_VERSION:-$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n 1)}"
BUILD="${NLE_BUILD:-target/release}"
OUT="${NLE_OUT:-dist}"
TOOLS="${NLE_TOOLS:-$PWD/target/packaging-tools}"
NFPM_VERSION="${NFPM_VERSION:-2.41.1}"
APPIMAGETOOL_VERSION="${APPIMAGETOOL_VERSION:-1.9.0}"
STAGE="$OUT/.stage"
WANT="${*:-tar deb rpm arch appimage}"

[ -x "$BUILD/nle" ] && [ -f "$BUILD/libnle.so" ] || {
    echo "packaging/build.sh: $BUILD/nle and $BUILD/libnle.so are missing; run cargo build --release first" >&2
    exit 1
}
want() { case " $WANT " in *" $1 "*) return 0 ;; *) return 1 ;; esac }

rm -rf "$OUT"
mkdir -p "$OUT" "$TOOLS"
# absolute paths: the nfpm configuration is written with them
OUT="$(cd "$OUT" && pwd)"
BUILD="$(cd "$BUILD" && pwd)"
STAGE="$OUT/.stage"
mkdir -p "$STAGE"
export NLE_VERSION="$VERSION"
gzip -9n < packaging/nle.1 > "$STAGE/nle.1.gz"

fetch() { # fetch URL DEST
    if command -v curl >/dev/null 2>&1; then curl -fsSL -o "$2" "$1"; else wget -q -O "$2" "$1"; fi
}

if want tar; then
    name="nle-v$VERSION-linux-x86_64"
    d="$STAGE/$name"
    mkdir -p "$d/bin" "$d/lib/nle" "$d/share/applications" "$d/share/icons/hicolor/256x256/apps" "$d/share/man/man1"
    cp "$BUILD/nle" "$d/bin/"
    cp "$BUILD/libnle.so" "$d/lib/nle/"
    cp assets/nle.desktop "$d/share/applications/"
    cp assets/favicon.png "$d/share/icons/hicolor/256x256/apps/nle.png"
    cp "$STAGE/nle.1.gz" "$d/share/man/man1/"
    cp README.md LICENSE "$d/"
    tar -C "$STAGE" -czf "$OUT/$name.tar.gz" "$name"
    echo "built $OUT/$name.tar.gz"
fi

if want deb || want rpm || want arch; then
    NFPM="$(command -v nfpm || true)"
    if [ -z "$NFPM" ]; then
        NFPM="$TOOLS/nfpm"
        if [ ! -x "$NFPM" ]; then
            fetch "https://github.com/goreleaser/nfpm/releases/download/v$NFPM_VERSION/nfpm_${NFPM_VERSION}_Linux_x86_64.tar.gz" "$TOOLS/nfpm.tgz"
            tar -C "$TOOLS" -xzf "$TOOLS/nfpm.tgz" nfpm
        fi
    fi
    sed "s|@BUILD@|$BUILD|g; s|@STAGE@|$STAGE|g" packaging/nfpm.yaml > "$STAGE/nfpm.yaml"
    for kind in deb rpm archlinux; do
        case "$kind" in archlinux) key=arch ;; *) key=$kind ;; esac
        if want "$key"; then
            "$NFPM" package -p "$kind" -f "$STAGE/nfpm.yaml" -t "$OUT/" >/dev/null
            echo "built $(ls -t "$OUT" | grep -E '\.(deb|rpm|pkg\.tar\.zst)$' | head -n 1)"
        fi
    done
fi

if want appimage; then
    TOOL="$(command -v appimagetool || true)"
    if [ -z "$TOOL" ]; then
        TOOL="$TOOLS/appimagetool"
        if [ ! -x "$TOOL" ]; then
            fetch "https://github.com/AppImage/appimagetool/releases/download/$APPIMAGETOOL_VERSION/appimagetool-x86_64.AppImage" "$TOOL"
            chmod +x "$TOOL"
        fi
    fi
    command -v desktop-file-validate >/dev/null 2>&1 || {
        echo "packaging/build.sh: appimagetool needs desktop-file-validate (apt install desktop-file-utils)" >&2
        exit 1
    }
    app="$STAGE/NLE.AppDir"
    mkdir -p "$app/usr/bin" "$app/usr/lib/nle"
    cp "$BUILD/nle" "$app/usr/bin/"
    cp "$BUILD/libnle.so" "$app/usr/lib/nle/"
    cp packaging/AppRun "$app/AppRun"
    cp assets/nle.desktop "$app/nle.desktop"
    cp assets/favicon.png "$app/nle.png"
    cp assets/favicon.png "$app/.DirIcon"
    # no FUSE is needed to build: the tool unpacks itself and runs
    ARCH=x86_64 APPIMAGE_EXTRACT_AND_RUN=1 "$TOOL" --no-appstream "$app" "$OUT/NLE-$VERSION-x86_64.AppImage" >/dev/null
    echo "built $OUT/NLE-$VERSION-x86_64.AppImage"
fi

rm -rf "$STAGE"
(cd "$OUT" && sha256sum -- * > SHA256SUMS)
echo "checksums in $OUT/SHA256SUMS"
