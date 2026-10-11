# NT development profile and acceptance gates

The Next 1.0 specification requires an original PE x86-64 loader, explicit NT personality and functional Win32 compatibility for a defined application class. The current development source adds a bounded PE32+ AMD64 console profile and three original API shims. Fresh image validation records these separately from the [older 03d541 snapshot](evidence/README.md), which has no PE execution. No Microsoft kernel, headers, object code or DLL implementation is reused.

Only `/compat/nt/` executable paths select the NT personality. The parser requires the fixed preferred image base `0x4000000000`, 4096-byte section alignment, 512-byte file alignment, at most 16 sections, a bounded image window with 16 KiB reserved for shims, an executable entrypoint and safe disjoint page/file ranges. W+X and ambiguous overlaps reject; gaps/BSS zero. PE32, DLL images, relocation, TLS, CLR, unwind and delay-import requirements are unsupported. Import resolution is a closed allowlist for `kernel32.dll`: GetStdHandle, WriteFile and ExitProcess. Unknown DLLs/names/ordinals and unsupported mandatory directories reject before execution.

The original Win64 fixture calls those imports using Microsoft x64 register/home-space conventions, checks nonvolatile integer/XMM state, validates bytes written and exits 45. A second fixture checks invalid handles/pointers and rejection of native/Linux syscall gates. This fixture is tested on Helix only; it has not been certified on actual Windows. The shim gate uses private internal identifiers, not a general NT system-service table. GetStdHandle supports only stdout/stderr. WriteFile is synchronous, at most 1024 bytes, requires a writable byte-count pointer and zero overlapped argument, and returns bounded success/failure. ExitProcess retains the low 32-bit exit result. There is no general NT handle service or LastError/TLS implementation.

The scope remains one original PE workflow, not unmodified Windows application support. The concrete parser/runtime gates below remain required as the profile expands.

| Gate | Required concrete tests |
| --- | --- |
| PE bounds | Every truncated header/table, count overflow, file/virtual range overflow, overlaps, entrypoint outside executable content and excessive image size rejects |
| Section permissions | W+X and page-overlap permission ambiguity reject; BSS/gaps zero; executable physical aliases remain protected |
| Imports/relocations | Unsupported mandatory directories/imports/relocation types reject; allowed records have complete bounds/type/target checks |
| Personality isolation | PE fixture cannot invoke native capabilities or Linux calls by number collision; invalid gate terminates or returns documented denial |
| Original fixture | Freestanding original PE executable exercises actual imported/NT calls, validates bytes/results/errors, and returns an exact recorded exit code |
| Handles and buffers | Rights/generation checks, invalid pointers, overflow, access modes, duplicate references and process cleanup are tested across actual calls |
| Failure containment | Invalid opcode/page fault terminates only the fixture; native supervisor and console remain operational |
| Resource exhaustion | Full process/handle/mapping tables return bounded errors and permit reuse without leaked references or stale mappings |
| User workflow | Terminal reports executable identity, exit/fault diagnostics and output; support designation names the exact fixture/interface profile |

Host parser tests and one original PE fixture can establish a narrow loader/runtime scenario. They do not establish unmodified Windows applications, kernel32/user32/GDI, registry, COM/OLE, Winsock, MSVC runtime, fonts/locales, .NET, Direct3D or compatibility with arbitrary PE32/PE32+ binaries. DLL resolution, ABI calling conventions, unwind/exception behavior, TLS, asynchronous I/O and NT object/security semantics each require separate coverage. Do not label the profile Windows-compatible without an explicit supported application class and reproducible workflows.
