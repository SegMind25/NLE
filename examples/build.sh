#!/bin/sh
# Builds the example programs with MinGW-w64 (apt install gcc-mingw-w64-x86-64).
# A program with resources (menus, dialogs, icons) has a .rc file of the same name next to it.
set -eu
cd "$(dirname "$0")"
mkdir -p bin
for src in *.c; do
    name="${src%.c}"
    # programs with a WinMain are GUI-subsystem programs; the rest are console programs
    flags=""
    grep -q "WinMain" "$src" && flags="-mwindows"
    res=""
    if [ -f "$name.rc" ]; then
        x86_64-w64-mingw32-windres "$name.rc" -O coff -o "bin/$name.res.o"
        res="bin/$name.res.o"
    fi
    x86_64-w64-mingw32-gcc -O2 $flags -o "bin/$name.exe" "$src" $res -lgdi32 -luser32 -lcomctl32 -lopengl32
    echo "built bin/$name.exe"
done
rm -f bin/*.res.o
