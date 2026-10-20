# Installing NLE

NLE is for x86-64 Linux. Every release at <https://github.com/SegMind25/NLE/releases> has these files:

| file | for | what it contains |
| --- | --- | --- |
| `nle_VERSION_amd64.deb` | Debian, Ubuntu, Linux Mint, Pop!_OS, Kali, ... | `/usr/bin/nle`, `/usr/lib/nle/libnle.so`, the application-menu entry and icon, the man page, docs |
| `nle-VERSION-1.x86_64.rpm` | Fedora, RHEL, CentOS Stream, openSUSE, ... | the same |
| `nle-VERSION-1-x86_64.pkg.tar.zst` | Arch, Manjaro, EndeavourOS, ... | the same |
| `NLE-VERSION-x86_64.AppImage` | any distribution, no installation | one file; opens the launcher, or acts like `nle` when given arguments |
| `nle-vVERSION-linux-x86_64.tar.gz` | any distribution | `bin/`, `lib/nle/`, `share/` to copy into a prefix |
| `SHA256SUMS` | checking downloads | `sha256sum -c SHA256SUMS --ignore-missing` |

The binaries are built on AlmaLinux 8 (glibc 2.28), so they run on Debian 10 and newer, Ubuntu 18.04 and newer, RHEL 8 and
newer, Fedora, openSUSE, Arch and the like. Older systems can build from source (below).

## Packages

```console
$ sudo apt install ./nle_0.3.0_amd64.deb              # Debian, Ubuntu, Mint
$ sudo dnf install ./nle-0.3.0-1.x86_64.rpm           # Fedora, RHEL (zypper install on openSUSE)
$ sudo pacman -U nle-0.3.0-1-x86_64.pkg.tar.zst       # Arch
$ sudo apt remove nle                                  # removes it again (dnf remove, pacman -R)
```

The packages suggest Wine (only used for programs the native layer cannot run) and recommend an X11 library and a
TrueType font (Liberation or DejaVu) for programs with windows; package managers install the recommended ones by default.

## AppImage

```console
$ chmod +x NLE-0.3.0-x86_64.AppImage
$ ./NLE-0.3.0-x86_64.AppImage                  # the launcher window
$ ./NLE-0.3.0-x86_64.AppImage app.exe          # run a program
$ ./NLE-0.3.0-x86_64.AppImage relink app.exe   # the other commands work too
```

AppImages need FUSE 2 (`libfuse2` on Debian and Ubuntu); without it run with `APPIMAGE_EXTRACT_AND_RUN=1`. A launcher such
as `appimaged` or AppImageLauncher adds NLE to the application menu and as a handler for `.exe` files.

## Without a package manager

```console
$ curl -fsSL https://raw.githubusercontent.com/SegMind25/NLE/main/get-nle.sh | sh
```

downloads the latest tarball, checks it against `SHA256SUMS`, and installs into `~/.local` (`PREFIX=/usr/local` for the
whole system, `NLE_VERSION=v0.3.0` for a specific version). Add `~/.local/bin` to your `PATH` if it is not there.

## From source

You need a Rust toolchain (1.88 or newer) and a C linker.

```console
$ git clone https://github.com/SegMind25/NLE && cd NLE
$ ./install.sh                                   # ~/.local; PREFIX=/usr/local ./install.sh for a system install
```

On Arch there is also `packaging/PKGBUILD`, which builds a release's source.

## After installing

```console
$ nle doctor              # checks the machine: address space, fonts, display, libnle.so, Wine
$ nle gui                 # the launcher window
$ sudo nle binfmt install # optional: let the kernel run ./program.exe directly
```

Programs with windows need an X11 display (`DISPLAY`); Wayland desktops provide one through XWayland.

## How the packages are built

`.github/workflows/release.yml` builds `nle` and `libnle.so` in an AlmaLinux 8 container whenever `main` gets a commit whose
`Cargo.toml` version has no tag yet (or a `v*` tag is pushed), checks that they need no newer glibc than 2.28, then runs
`packaging/build.sh`. That script makes every package from the same binaries:

* [nfpm](https://nfpm.goreleaser.com) writes the `.deb`, `.rpm` and Arch package from `packaging/nfpm.yaml`
* the AppImage is an AppDir (`packaging/AppRun` starts the launcher when it gets no arguments) packed with `appimagetool`
* the tarball and `SHA256SUMS`

The same script runs in CI on every push, so a change that breaks a package is noticed before a release. To build them
yourself: `cargo build --release && packaging/build.sh` (add the names `tar deb rpm arch appimage` to build only some); the
results are in `dist/`.
