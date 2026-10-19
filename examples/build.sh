#!/bin/sh
# Builds the example programs with MinGW-w64 (apt install gcc-mingw-w64-x86-64).
set -eu
cd "$(dirname "$0")"
mkdir -p bin
for src in *.c; do
    # programs with a WinMain are GUI-subsystem programs; the rest are console programs
    flags=""
    grep -q "WinMain" "$src" && flags="-mwindows"
    x86_64-w64-mingw32-gcc -O2 $flags -o "bin/${src%.c}.exe" "$src" -lgdi32 -luser32 -lcomctl32
    echo "built bin/${src%.c}.exe"
done
