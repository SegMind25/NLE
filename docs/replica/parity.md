## Parity: 84.9 / 100

features 84.9  (26 counted, must-haves 15 of 15 done)

## By area, weakest first
- compat                        22.2  (6 features)
- network                       83.3  (2 features)
- core                          91.3  (8 features)
- launch                       100.0  (1 features)
- relink                       100.0  (3 features)
- analysis                     100.0  (1 features)
- filesystem                   100.0  (1 features)
- registry                     100.0  (1 features)
- console                      100.0  (1 features)
- text                         100.0  (1 features)
- process                      100.0  (1 features)

## Missing, in build order
- [should] compat: 32-bit Windows programs, no  (out of scope: the native layer is x64 only)
- [should] compat: Menus, dialog resources and common controls, no  (functions exist so programs start, but no menu bar, no RT_DIALOG / RT_MENU resources, no comctl32, list / combo boxes or scroll bars)
- [should] core: MSVC C++ exceptions (`_CxxThrowException`), no  (not implemented; programs that throw MSVC exceptions stop with a clear message)
- [could] compat: .NET programs, no  (out of scope: needs a CLR)
- [could] compat: Direct3D / OpenGL graphics, no  (out of scope)
- [could] compat: Windows on ARM64 hosts, no  (x86-64 hosts only; native execution is the design)
- [could] network: Overlapped and IOCP networking, partial  (IOCP object exists for files and pipes; overlapped sockets are not implemented)

## Yours, not in the original (not scored)
- Machine-readable report
- List every API the runtime implements
- Check that the host can run it
- Let the kernel run the converted file directly
- Process listing (Toolhelp)
- Wine as a fallback backend
- Choose the backend explicitly
- Isolated Windows environment (environment variables / directories)
- Clear error when a missing API is called
- Trace what the program does
- Sandboxing
- Headless display driven by a script
