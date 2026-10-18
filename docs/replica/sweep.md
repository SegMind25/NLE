# Brand sweep

Command, run from the repository root with the replica-brand `sweep.py`:

```
python3 sweep.py . --config docs/replica/brand.json
```

`brand.json` lists what must not be left over from the original: its name, its author's handle and domain, and the
names of the platform it targets (AnyPS5, boykopovar, PS5, PlayStation, Sony, Orbis).

Result: **no match in `src/`, `tests/`, `Cargo.toml`, `install.sh` or the workflows.** The remaining matches are the
three places where the original is named on purpose:

| file | why it names the original |
| --- | --- |
| `README.md` | credits the idea and states that no code is shared (the original is GPL-2.0, this project is MIT) |
| `docs/replica/recon.md` | the recon map names the sources it studied |
| `docs/replica/brand.json` | the list the sweep itself reads |

The words "self" and "prx" were dropped from the list after the first run: they only matched ordinary code
(`self_exe`, `/proc/self/maps`) and prove nothing about leftovers.
