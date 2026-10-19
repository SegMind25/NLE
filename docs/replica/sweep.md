# Brand sweep

Command, run from the repository root with the replica-brand `sweep.py` (on the tracked files, not on `target/`):

```
python3 sweep.py . --config docs/replica/brand.json
```

`brand.json` lists what must not be left over: the original's name, its author's handle and domain, the names of the
platform it targets (AnyPS5, boykopovar, PS5, PlayStation, Sony, Orbis) and this project's own retired name (AnyEXE).

Result: **no match in `src/`, `tests/`, `examples/`, `tools/`, `assets/`, `Cargo.toml`, `install.sh` or the workflows.**
The remaining matches are the places where a name appears on purpose:

| file | why |
| --- | --- |
| `README.md` | credits the idea to AnyPS5 and states that no code is shared (the original is GPL-2.0, this project is MIT) |
| `docs/replica/recon.md` | the recon map names the sources it studied |
| `docs/replica/brand.md` | records that the tool was briefly called AnyEXE |
| `docs/replica/brand.json`, `sweep.md` | the list the sweep reads, and this record of its result |

The words "self" and "prx" are not on the list: they only matched ordinary code (`self_exe`, `/proc/self/maps`).
