# Recon map: AnyPS5 concept, applied to Windows executables (desktop / CLI)

Scope: the *core loop* of AnyPS5 only: take an executable made for another platform and make it a native program of the
host, without emulating a CPU. The slice cloned for Windows `.exe` files is: **inspect, run, relink, report coverage.**
For: anyone who wants to run a Windows x64 console or service program as a plain Linux program.
Date: 2026-10-18

Method: clean-room. Only the public README and the public docs of the original were read, and only for the *idea* and for
the *user-visible behaviour* (commands, outputs, limits). No code of the original was copied or adapted; the original
is licensed GPL-2.0 and this project is MIT, so the two never share code. Every line of AnyEXE was written from the
published Windows x64 ABI, the PE/COFF and ELF specifications and the documented behaviour of the Win32 API.

## Sources

| # | source | URL | notes |
| --- | --- | --- | --- |
| 1 | README | https://github.com/boykopovar/AnyPS5 | pitch: "converts the executable to the host's native format" + "implementations of system libraries for dynamic linking", "no emulation or separate runtime" |
| 2 | usage docs | docs/user/USAGE.md in the same repository | what the user types, what comes out |
| 3 | architecture docs | docs/dev/ARCHITECTURE.md in the same repository | relinker + libraries split; only read at concept level |
| 4 | Microsoft PE format specification | learn.microsoft.com/windows/win32/debug/pe-format | headers, sections, imports, TLS, relocations, exception tables |
| 5 | Microsoft x64 calling convention and exception handling docs | learn.microsoft.com/cpp/build/x64-software-conventions | `extern "win64"` ABI, `.pdata` / UNWIND_INFO |
| 6 | System V ABI (x86-64) and ELF specification | refspecs.linuxbase.org | program headers, dynamic section, `PT_NOTE` |
| 7 | Win32 API reference | learn.microsoft.com/windows/win32/api | behaviour and error codes of every shim |

## Core loop

Point the tool at a program that was built for another operating system, and get that program running as an ordinary
native process, with the other system's library calls answered by a reimplementation that is linked in.

## Screens (the command line is the UI)

| ID | screen | how to reach | purpose | key components | states seen |
| --- | --- | --- | --- | --- | --- |
| S01 | run | `anyexe app.exe [args]` / `anyexe run` | execute a program | backend choice, exit code pass-through | native, wine, refused, missing API |
| S02 | inspect | `anyexe inspect app.exe` | report what the program needs and whether it can run | per-library counts, verdict, notes | native / partial / unsupported / not a PE |
| S03 | relink | `anyexe relink app.exe -o dir` | produce a standalone native executable | ELF writer, runtime library copy, bundled DLLs | ok, unresolved imports, 32-bit rejected |
| S04 | coverage | `anyexe coverage` | list the implemented library functions | per-library table, markdown / json | n/a |
| S05 | doctor | `anyexe doctor` | check that the host can run native programs | address space, gs register, segments | all ok / failures |
| S06 | binfmt | `anyexe binfmt install` | let the kernel start `.exe` files directly | binfmt_misc registration | registered / not registered / needs root |

## Flows

```
F01 Run a program without converting it
    S02 (optional) -> S01
    happy path commands: 1
    edge: missing API (named in the error), 32-bit program (refused), Wine fallback when installed

F02 Ship a program as a native Linux file
    S02 -> S03 -> run ./out/app directly
    happy path commands: 2
    edge: program needs its DLLs (copied next to the output), program starts child processes (launcher copied)

F03 Find out why a program does not work
    S02 -> S04 (is the missing function known?) -> ANYEXE_TRACE=api S01
```

## Components

| component | variants | states | used on |
| --- | --- | --- | --- |
| PE loader | main image, DLL, relinked (already mapped by the kernel) | mapped, bound, running | S01, S03 |
| API shims | kernel32, ntdll, msvcrt/ucrt, advapi32, ws2_32, shell32, ole32, others | implemented / stub / missing | S01, S03, S04 |
| Relinker | ET_EXEC with PE segments at the original base | ok / rejected | S03 |
| Backend selector | native, wine, auto | chosen by verdict | S01 |
| Windows view | `C:` drive, environment, TEB / PEB | per process | S01, S03 |

## Inferred data model

```
Report      format, subsystem, image base, imports per library, bundled DLLs, notes, verdict
            evidence: S02 output of the original's "analyze" step
            confidence: medium
Descriptor  image base, image size, original file name (stored in a PT_NOTE of the relinked file)
            evidence: S03, the original keeps a note describing the converted image
            confidence: guess (format is our own)
Prefix      directory tree standing in for C:\
            evidence: platform behaviour, no equivalent in the original
            confidence: high
```

## Feature matrix

See `features.csv`. Must: 15, should: 9, could: 6, skip: 1. "original" means "expected of this kind of tool": the
original's own behaviour plus what Windows programs rely on the platform to do.

## Out of scope (cannot or should not be cloned)

- Hardware-specific parts of the original (graphics shaders, game controllers): they have no Windows-executable analogue here.
- Anything proprietary of the original platform: nothing of it is used.
- Being a security sandbox: programs run with the user's rights.

## Size

Screens 6, flows 3, entities 3. Hard parts: structured exception handling and C++ exceptions through reimplemented
runtime code; the Windows x64 variadic ABI; thread blocks (TEB) without kernel help; relinking a PE image into an
ELF that the kernel maps at the right addresses. Size: XL (done as a vertical slice: see `architecture.md`).
