# Architecture

NLE has no emulator and no translator. The program's own x86-64 instructions run on the CPU. What NLE provides is
everything *around* them: a loader that puts the image in memory the way Windows would, thread state at the addresses
Windows code looks at, and Rust implementations of the library functions the program imports.

```
 app.exe ──parse──► pe.rs ──► loader.rs ──map / relocate / bind──► program runs in this process
                                 │                                       │ calls (Windows x64 ABI)
                                 ▼                                       ▼
                            registry.rs ◄──── shim/*.rs  (kernel32, ntdll, msvcrt/ucrt, ws2_32, ...)
                                                    │
                                                    └──► libc / the Linux kernel
 app.exe ──relink──► relink.rs ──► ELF file ──kernel + ld.so──► nle_relinked_main ──► same loader path
```

## Calling convention

Every function a guest can call is declared `extern "win64"` in Rust, so the compiler emits the Microsoft x64 convention
(arguments in rcx, rdx, r8, r9, 32 bytes of shadow space, callee-saved xmm6 to xmm15). Imports are bound by writing the
function's address into the import address table. There is no per-call glue.

Variadic functions (`printf`, `scanf`, `sprintf`, ...) are the exception: the Windows convention passes the first four
arguments in registers and the rest on the stack, with no `va_list` in sight. Each variadic export is a small
`#[unsafe(naked)]` thunk that spills `rdx`, `r8` and `r9` next to the stack arguments and hands the formatter a pointer to
an array of 8-byte slots. `crt_printf.rs` and the `scanf` code read the arguments from that array.

## Thread state

Windows code reads `gs:[0x30]` (the TEB), from which it reaches the PEB, thread-local storage and the exception list.
`runtime/teb.rs` allocates a TEB per thread and points the `gs` base at it with `arch_prctl(ARCH_SET_GS)`. The PEB holds
the image base, the process parameters (command line, environment) and the loader data; `KUSER_SHARED_DATA` is mapped
at `0x7ffe0000` only when the program references it (Go does), with a small thread that keeps its tick count and time
fields current.

Static TLS (`__declspec(thread)`, Rust's `thread_local!`, Go) uses `ThreadLocalStoragePointer` in the TEB; blocks are
created per thread from each module's TLS directory, and TLS callbacks run on thread start and exit. Threads are
`pthread`s, each with its own alternate signal stack so that stack overflows and faults can be reported.

## Memory and files

`VirtualAlloc` and friends are `mmap`; `VirtualQuery` reads `/proc/self/maps`; file mappings use `memfd`. Heaps
(`HeapAlloc`, `GlobalAlloc`, `LocalAlloc`) are backed by the process allocator (16-byte aligned, as on x64 Windows).

Paths follow Windows rules and map onto Linux in `runtime/paths.rs`:

* `C:\` is `<prefix>/drive_c` and `Z:\` is `/`; a relative path uses the current directory,
* components are matched case-insensitively against the real directory contents,
* `\\.\pipe\name` is a pipe namespace, `NUL`, `CON`, `CONIN$` and `CONOUT$` are devices,
* the code page is UTF-8 (65001), so the "A" and "W" variants agree.

Handles are small multiples of four in a table (`runtime/handles.rs`). Waiting uses one global lock with a condition
variable; every state change wakes the waiters, which re-check their predicate. That keeps `WaitForMultipleObjects` with
`bWaitAll` correct without lock ordering. Critical sections, SRW locks and condition variables are futex based and work
when zero-initialised, as the Windows ones do.

## Exceptions

`shim/seh.rs` implements structured exception handling the way the x64 Windows runtime does:

* `RtlLookupFunctionEntry` finds the `RUNTIME_FUNCTION` for an address in the module's `.pdata`,
* `RtlVirtualUnwind` interprets `UNWIND_INFO` (push, alloc, save, frame register, chained entries, epilogues),
* `RtlRaiseException` / `RaiseException` run the two-phase dispatch: search for a handler, then unwind and call
  termination handlers; `__C_specific_handler` evaluates `__try` / `__except` tables,
* MinGW C++ exceptions work through the same machinery: the guest's libgcc personality routine
  (`__gxx_personality_seh0`) is called with the context records it expects, including its reuse of
  `STATUS_GCC_UNWIND` for the cleanup phase,
* CPU faults (SIGSEGV, SIGFPE, SIGILL, SIGBUS) arrive in a signal handler on the alternate stack, which rewrites the
  interrupted context into a Windows `CONTEXT` and dispatches it as an exception; `RtlRestoreContext` resumes execution
  with an `iretq` (user selectors `0x33` / `0x2b`),
* when the stack walk reaches code that is not part of any PE image (the Rust runtime), the walk continues at the
  context the handler recorded, so exceptions can cross our own frames safely.

## The relinker

`relink.rs` turns a PE image into an `ET_EXEC` ELF file:

| segment | content |
| --- | --- |
| `PT_LOAD` per section | the PE headers and each section at `ImageBase + RVA`, with the section's permissions |
| `PT_LOAD` (R+X) above the image | ELF header, program headers, interpreter path, `PT_NOTE`, hash table, dynamic symbols and strings, relocations, entry stub |
| `PT_LOAD` (R+W) | the dynamic section and a small GOT |
| `PT_INTERP`, `PT_DYNAMIC`, `PT_NOTE`, `PT_GNU_STACK` | as usual |

The dynamic section has `DT_NEEDED` entries for `libnle.so` and `libc.so.6` and `DT_RUNPATH=$ORIGIN` (override with
`--rpath`). The entry stub calls `__libc_start_main(nle_relinked_main, ...)`, so libc initialises normally. The
`PT_NOTE` (name `NLE`) records the image base, size and the original file name.

`nle_relinked_main` finds that note, sets up the runtime exactly as `nle run` would, and calls
`loader::attach_mapped`: the image is already in memory, so only import binding, TLS and DLL loading remain. Because the PE sits at its original addresses, nothing in the program
has to be rewritten.

Programs that locate themselves with `GetModuleFileName` get `<dir>/<original>.exe`. That file is not shipped, so
opening that exact path opens the ELF instead; a program that starts itself again therefore starts the ELF.

## Windows

Programs with windows (`user32`, `gdi32`) are served by `src/gui/`, described in [GUI.md](GUI.md). In short: window
classes, windows, the message loop and the default window procedure are reimplemented; everything a window shows is
drawn into memory by a software renderer (so drawing needs no display); an X11 backend, loaded with `dlopen`,
puts the finished pixels on the screen and brings keyboard and mouse input back. A scripted headless display replaces
X for tests. BUTTON, STATIC and EDIT are ordinary window procedures registered as built-in classes.

## Backends

`backend.rs` decides between the native layer and Wine from the `inspect` verdict:

| verdict | meaning | `--backend auto` |
| --- | --- | --- |
| native | every import is implemented | native |
| partial | some imports are missing | Wine if found (`NLE_WINE`, `wine64`, `wine`, `wine-stable`), else native with a warning |
| unsupported | 32-bit or ARM64 code, .NET, a DLL given as the program | Wine if found, else refuse |

## Adding a Windows function

Functions are declared in tables with the `shims!` macro:

```rust
crate::shims! { "kernel32" => MY_SHIMS;
    fn GetTickCount64() -> u64 {
        monotonic_ms()
    }
}
```

The body is ordinary Rust; the macro generates the `extern "win64"` wrapper, the exported name and the table entry. Add
the table to `shim::tables()`. Names can differ from the Rust identifier (`fn Rust as "ExportName"(...)`), and `shim_extra!`
registers aliases and data exports. Then:

1. add a check to a program in `tests/exe` (or a new one, and list it in `tests/windows_programs.rs`),
2. `cargo test --release`,
3. regenerate the list: `target/release/nle coverage --markdown > docs/COVERAGE.md` (CI fails when it is stale).

`NLE_TRACE=api` prints the libraries and functions the program looks up at run time (`LoadLibrary`, `GetProcAddress`).
An unimplemented import is reported with the caller's address when the program calls it, which is usually enough to
find what is missing.

## Safety notes

The runtime is `unsafe` at its edges (guest pointers, raw segments, signal handlers) and checked inside: the PE parser
validates every offset and is exercised with random mutations in the unit tests. The guest is not sandboxed: it can do
anything the user can do, including through the file system view `Z:\`.
