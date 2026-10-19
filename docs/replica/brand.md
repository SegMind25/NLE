# Brand: NLE (Native Linux Exe)

Name: **NLE**, short for **Native Linux Exe**. It says what the tool does: a Windows `.exe` becomes a native Linux program.
Three letters, easy to type as a command (`nle app.exe`), nothing in common with the original's name or the console it
targets, so it is not confusingly similar to it in sound, look or meaning. The tool was briefly called "AnyEXE"; that name
is retired and the sweep below makes sure none of it is left.

## Logo

Files in `assets/` (sources are SVG; PNGs are rendered by `tools/render-assets.sh`):

| file | use |
| --- | --- |
| `logo-mark.svg` / `.png` | the square mark: app icon, avatar |
| `logo.svg` / `.png` | mark + wordmark + tagline, for light backgrounds |
| `logo-dark.svg` / `.png` | the same for dark backgrounds |
| `favicon.png` | 256 px icon |
| `social-preview.png` | 1280 x 640 image for the repository's social preview |

The mark is an **N followed by a block cursor**: the first letter of the name sitting at a shell prompt. The letterforms
are drawn as paths, so the logo never depends on an installed font. The wordmark repeats the cursor after the E.

| token | value |
| --- | --- |
| ink gradient | `#38BDF8` to `#34D399` (sky to terminal green) |
| cursor | `#34D399` |
| tile | `#14203D` to `#070B16`, border `#35497F` |
| text on light | `#0B1220`, tagline `#51607F` |
| text on dark | `#EAF2FF`, tagline `#9FB0CF` |

The logo uses no Windows, Tux or console imagery: those marks belong to their owners.

## Checks to run before a public launch

None of these can be assumed from here:

- trademark search for "NLE" in the software class in the user's jurisdictions (the three-letter name is short and
  likely to collide with something),
- crate name on crates.io, the GitHub organisation and repository, a domain,
- a package name for the distributions the user cares about.

## Voice

Plain, technical, honest about limits. The README lists what does **not** work next to what does.

## Credit

The README thanks AnyPS5 for the idea (a relinker that converts executables into native ones) and says that no code is
shared. The original is GPL-2.0; NLE is MIT and written from public specifications.

## Sweep

`python3 replica-brand/sweep.py . --config docs/replica/brand.json` finds any left-over name of the original or of the
retired name. Its output is recorded in `docs/replica/sweep.md`.
