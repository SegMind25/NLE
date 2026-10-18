# Brand: AnyEXE

Name: **AnyEXE**. Descriptive and compound, in the style the skill lists. It says what the tool does (run *any* `.exe`).
It is not confusingly similar to the original's name in sound, look or meaning: the original is named after a console;
this one is named after a file extension. The prefix "Any" is a common English word and is the only thing in common.

Checks to run before a public launch, none of which can be assumed from here:

- trademark search for "AnyEXE" in the software class in the user's jurisdictions,
- crate name on crates.io, GitHub organisation / repository, a domain,
- a package name for the distributions the user cares about.

Voice: plain, technical, honest about limits. The README lists what does **not** work before it lists what does.

Credit: the README thanks the original for the idea (a relinker that converts executables into native ones) and says
that no code is shared. The original is GPL-2.0; AnyEXE is MIT and written from public specifications.

Sweep: `python3 replica-brand/sweep.py . --config docs/replica/brand.json` finds any left-over name of the original.
Its output is recorded in `docs/replica/sweep.md`.
