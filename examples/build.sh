#!/bin/sh
# Builds the example programs with MinGW-w64 (apt install gcc-mingw-w64-x86-64).
set -eu
cd "$(dirname "$0")"
mkdir -p bin
for src in *.c; do
    x86_64-w64-mingw32-gcc -O2 -o "bin/${src%.c}.exe" "$src" -lgdi32 -luser32
    echo "built bin/${src%.c}.exe"
done
