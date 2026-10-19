# Architecture: NLE

Reads `recon.md` and `features.csv`.

## Stack

| layer | choice | why |
| --- | --- | --- |
| language | Rust 2021, one dependency (`libc`) | speed, no runtime, memory-safe where it can be; the unsafe parts are the FFI edges |
| runtime | `libnle` as `rlib` + `cdylib` | the CLI links it in; relinked programs link `libnle.so` dynamically |
| calling convention | `extern "win64"` for everything a guest can call | no thunks for plain functions; variadics use small `naked` assembly thunks |
| hardware | x86-64 Linux | the guest's machine code runs as it is; there is no CPU emulation |

No microservices, no database, no build scripts, no code generation.

## Pieces

```
src/pe.rs               bounds-checked PE32 / PE32+ parser (fuzzed by a random-mutation test)
src/loader.rs           maps images, applies relocations, binds imports, TLS, DLLMain, forwarded exports
src/runtime/            TEB / PEB / KUSER_SHARED_DATA via the gs register, handle table, path model, trap stubs
src/shim/               the reimplemented libraries, one `shims!` table per area
src/shim/seh.rs         .pdata unwinder, two-phase dispatch, hardware faults as exceptions, libgcc personality support
src/analyze.rs          compatibility report and verdict
src/relink.rs           PE image -> ET_EXEC ELF writer
src/launch.rs           start-up for `run` and for relinked programs (`nle_relinked_main`)
src/backend.rs          native / Wine selection
src/cli.rs              the command line
```

## How a call from the program reaches Linux

1. The loader resolves each import (`kernel32!CreateFileW`) by library family and name in `registry.rs` and writes the
   address of the Rust function into the import table.
2. The guest calls it directly with the Windows x64 convention. The function is `extern "win64"`, so the compiler does the
   register shuffling.
3. The function does the work with `libc` and returns a Windows-style value (and sets the thread's last error in the TEB).

Thread state lives where Windows code expects it: `gs:[0x30]` is the TEB, with the PEB, the TLS slots and the
SEH chain reachable from it. Each thread gets its own block and its own alternate signal stack.

## Relinking

`nle relink` writes an ELF executable at the PE's preferred base address:

* the PE headers and sections become `PT_LOAD` segments at their original virtual addresses (so absolute pointers
  and the import table need no change),
* a metadata area above the image holds the ELF header, program headers, an interpreter string, a `PT_NOTE`
  describing the image, the dynamic section and a tiny entry stub,
* the dynamic section names `libnle.so` and libc, with `DT_RUNPATH=$ORIGIN`,
* the entry stub calls `__libc_start_main`, which calls `nle_relinked_main`; that adopts the image the kernel already
  mapped (`loader::attach_mapped`), binds its imports and jumps to the entry point.

The original `.exe` is not needed at run time. Opening its old path opens the ELF itself, so a program that starts
itself again (many multi-process programs do) keeps working.

## Backends

| verdict | `auto` does |
| --- | --- |
| native | run natively |
| partial | run with Wine when it is installed, otherwise natively with a warning |
| unsupported | Wine when installed, otherwise refuse |

## Build order that was followed (thin vertical slice first)

1. PE parser + `inspect`. 2. Loader + a console "hello". 3. Kernel32 file / memory / thread basics, CRT. 4. Go and Rust
programs (no C runtime). 5. SEH and C++ exceptions. 6. DLLs, console, sockets. 7. Relinker. 8. Child processes.
Each step had a program in `tests/exe` or `tests/lang` that failed before and passes after.
