#!/bin/sh
# Builds NLE in release mode and installs it.
#   ./install.sh                  -> ~/.local/bin/nle, ~/.local/lib/nle/libnle.so
#   PREFIX=/usr/local ./install.sh
set -eu

PREFIX="${PREFIX:-$HOME/.local}"
cd "$(dirname "$0")"

if ! command -v cargo >/dev/null 2>&1; then
    echo "install.sh: cargo is not installed. Get Rust 1.88 or newer from https://rustup.rs" >&2
    exit 1
fi
if [ "$(uname -m)" != "x86_64" ] || [ "$(uname -s)" != "Linux" ]; then
    echo "install.sh: NLE runs Windows x64 code natively, so it needs x86-64 Linux." >&2
    exit 1
fi

cargo build --release

install -d "$PREFIX/bin" "$PREFIX/lib/nle"
install -m 755 target/release/nle "$PREFIX/bin/nle"
install -m 755 target/release/libnle.so "$PREFIX/lib/nle/libnle.so"

echo "installed: $PREFIX/bin/nle (runtime library in $PREFIX/lib/nle)"
case ":$PATH:" in
    *":$PREFIX/bin:"*) ;;
    *) echo "note: $PREFIX/bin is not on your PATH" ;;
esac
"$PREFIX/bin/nle" doctor || true
