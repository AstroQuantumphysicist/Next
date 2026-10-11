# Next 1.0 requirement status

Generated from `docs/requirements.json` by `python3 tools/spec_audit.py --write-report`.

**Next 1.0 is not ready.** Foundations in progress do not establish finished product behavior.

Coverage includes explicit NEXT identifiers, numbered sections/subsections, and the final capability table. Section rows are acceptance gates, not a claim that every sentence has an independent test.

Evidence distinguishes host-tested primitives, boot smoke tests, integrated system behavior, and certified hardware. Only integrated, hardware-validated contracts with documented acceptance may be verified.

| Requirement | Contract | Status | Evidence levels |
|---|---|---|---|
| NEXT-BOOT-001 | Next 1.0 is a **real-hardware x86-64 UEFI operating system**, capable of running on supported desktop PCs, laptops and professional workstations without another OS kernel or hypervisor underneath. | partial | boot_smoke, host_tested, integrated |
| NEXT-BOOT-003 | loader MUST validate ELF segments, sizes, overlaps, alignment, entrypoint mapping and W^X permissions. Unknown mandatory boot records MUST fail closed. | partial | boot_smoke, host_tested |
| NEXT-BOOT-004 | early panic displays serial diagnosis and halts; no blind reboot loops. Preserve a reboot-reason code in next available writable log facility. | partial | boot_smoke |
| NEXT-BOOT-005 | A/B system deployment with monotonic rollback metadata, image hashes, boot-attempt counter and recovery entry. User data partition is never part of an automatic OS rollback. | pending | none |
| NEXT-BOOT-006 | Secure Boot signature verification and TPM-measured boot are implemented on certified hardware supporting those capabilities. Owner-authorized developer builds may use separate test keys, with visible security-state warnings and a documented recovery method. | pending | none |
| NEXT-BOOT-007 | boot-critical input and display fallback must not depend on Prism or GPU acceleration. | partial | boot_smoke, integrated |
| NEXT-KRN-001 | no allocator allocations in interrupt handlers unless using a bounded IRQ-safe pool. | partial | boot_smoke, host_tested, integrated |
| NEXT-KRN-002 | an unhandled user fault kills or signals only the offending user process, not the whole kernel. | partial | boot_smoke, integrated |
| NEXT-KRN-003 | kernel panic is reserved for violated kernel invariants; expose panic ID, CPU, thread, backtrace if possible, and serial log. | pending | none |
| NEXT-KRN-004 | all public kernel structures have opaque handles; users cannot pass raw kernel pointers as authority. | partial | boot_smoke, host_tested, integrated |
| NEXT-KRN-005 | syscalls consume only validated user buffers; copyin/copyout protects against invalid mappings and integer wraparound. | partial | boot_smoke, host_tested, integrated |
| NEXT-KRN-006 | per-core state and interrupt priority are explicit. Lock ordering is written down; hard IRQ contexts must not block. | pending | none |
| NEXT-KRN-007 | monotonic time does not jump with wall clock changes; timers are not executed under locks they may reenter. | partial | boot_smoke, host_tested, integrated |
| NEXT-MEM-001 | no use of UEFI boot-services memory after `ExitBootServices` unless reclaimed according to UEFI memory type rules. | partial | boot_smoke, host_tested, integrated |
| NEXT-MEM-002 | reject overlapping/unaligned physical map entries and integer overflow during range normalization. | partial | host_tested |
| NEXT-MEM-003 | user processes cannot map kernel pages, MMIO, page tables or physical memory without explicit privileged device capabilities. | partial | boot_smoke, host_tested, integrated |
| NEXT-MEM-004 | all mappings prevent writable-and-executable pages unless a narrowly audited JIT transition is authorized; use temporary permission transitions and flush appropriate caches/TLB. | partial | boot_smoke, host_tested, integrated |
| NEXT-MEM-005 | validate pin/unpin while asynchronous I/O is in flight; prevent DMA-to-freed-page and mapping lifetime use-after-free. | pending | none |
| NEXT-MEM-006 | test COW across `fork()`, exec image replacement, exception delivery, shared pages, page faults under SMP contention, and user/kernel boundary addresses. | pending | none |
| NEXT-SCH-001 | a focused UI task under CPU load must be measurable and remain responsive within the published reference benchmark. | partial | boot_smoke |
| NEXT-SCH-002 | 1/2/4/8-core QEMU schedule, exit, wake, signal and preemption stress tests must pass without deadlock/livelock. | partial | boot_smoke, host_tested, integrated |
| NEXT-SCH-003 | CPU hotplug may be omitted in 1.0 but topology discovery must not assume fixed CPU count. | pending | none |
| NEXT-OBJ-001 | stale handle generations cannot alias newly allocated privileged objects; handle exhaustion returns a bounded failure. | partial | boot_smoke, host_tested, integrated |
| NEXT-OBJ-002 | duplicate, inherit and transfer tests must prove no escalation even under cross-process races. | partial | boot_smoke, host_tested, integrated |
| NEXT-OBJ-003 | object wait logic guarantees no missed wake-ups in signal/reset/close/cancel races. | partial | boot_smoke, host_tested, integrated |
| NEXT-IPC-001 | malformed frames/fuzz input never panic kernel, leak handles or cross privilege boundaries. | partial | boot_smoke, host_tested, integrated |
| NEXT-IPC-002 | IPC benchmark documents 1 B, 64 B, 4 KiB, 64 KiB payloads and handle-transfer overhead. | pending | none |
| NEXT-SEC-001 | package manifest permission requests are not grants. Installation and runtime permission decisions are separate. | partial | boot_smoke, host_tested, integrated |
| NEXT-SEC-002 | process spoofing another executable/app identity or permissions by renaming a file must be prevented through signed identity and launch token binding. | partial | boot_smoke, host_tested, integrated |
| NEXT-SEC-003 | external USB/DMA mappings follow IOMMU policy when available; cannot claim DMA isolation on unsupported configurations. | pending | none |
| NEXT-VFS-001 | no path traversal or mount boundary bypass through dot segments, symlink races, casefold, alternate data streams, device aliases, environment expansion, or NT `..`/reserved path syntax. | partial | host_tested |
| NEXT-VFS-002 | file I/O must be cancellation-safe; buffer ownership and partial reads/writes are documented. | partial | boot_smoke, host_tested, integrated |
| NEXT-VFS-003 | locking under `rename`/`unlink`/`open` must not deadlock in parallel directory operations. | pending | none |
| NEXT-PKG-001 | install cannot escape destination root via archives, symlinks, `..`, absolute paths, case aliases, hardlinks, special files or oversized metadata. | partial | host_tested |
| NEXT-PKG-002 | revoked/invalid/mismatched signatures block privileged installation; developer override clearly isolated. | partial | boot_smoke, host_tested |
| NEXT-PKG-003 | dependency/version migration failures do not corrupt pre-existing installed apps. | partial | host_tested |
| SECTION-0 | Vision, scope, and product rules | pending | none |
| SECTION-0.1 | Brand and architecture | pending | none |
| SECTION-0.2 | Product thesis | pending | none |
| SECTION-0.3 | Non-goals and hard constraints for 1.0 | pending | none |
| SECTION-0.4 | Product editions | pending | none |
| SECTION-0.5 | Definitions of success | pending | none |
| SECTION-1 | System diagram and trust boundaries | partial | boot_smoke, integrated |
| SECTION-2 | Supported computing platforms, hardware and portability | pending | none |
| SECTION-2.1 | Platform and processor architecture | partial | boot_smoke, integrated |
| SECTION-2.2 | Required hardware classes and support declarations | pending | none |
| SECTION-2.3 | Deliverable image and system format | partial | boot_smoke |
| SECTION-3 | Boot, memory-map handoff, and recovery | pending | none |
| SECTION-3.1 | Boot sequence | partial | boot_smoke, host_tested, integrated |
| SECTION-3.2 | Failure handling and recoverability | partial | boot_smoke |
| SECTION-4 | Helix kernel principles | pending | none |
| SECTION-4.1 | Kernel model | partial | boot_smoke, integrated |
| SECTION-4.2 | Native ABI conventions | partial | boot_smoke, host_tested, integrated |
| SECTION-4.3 | Interface stability | partial | boot_smoke, host_tested, integrated |
| SECTION-5 | Memory management and scheduling | pending | none |
| SECTION-5.1 | Physical memory management | partial | boot_smoke, host_tested |
| SECTION-5.2 | Virtual memory | partial | boot_smoke, host_tested, integrated |
| SECTION-5.3 | Scheduler | partial | boot_smoke, host_tested, integrated |
| SECTION-5.4 | Timestamps and clocks | partial | boot_smoke, host_tested, integrated |
| SECTION-6 | Helix objects, capabilities and synchronization | pending | none |
| SECTION-6.1 | Object invariant | partial | boot_smoke, host_tested, integrated |
| SECTION-6.2 | Native synchronization | partial | boot_smoke, host_tested, integrated |
| SECTION-6.3 | IPC protocol | partial | boot_smoke, host_tested, integrated |
| SECTION-7 | Process lifecycle, executable loading, and native runtime | pending | none |
| SECTION-7.1 | Process model | partial | boot_smoke, integrated |
| SECTION-7.2 | Image formats | partial | boot_smoke, host_tested, integrated |
| SECTION-7.3 | Thread and exception contract | partial | boot_smoke, host_tested, integrated |
| SECTION-7.4 | Runtime services | partial | boot_smoke, host_tested, integrated |
| SECTION-8 | Security model and threat analysis | pending | none |
| SECTION-8.1 | Security objective | pending | none |
| SECTION-8.2 | Authorization | partial | boot_smoke, host_tested, integrated |
| SECTION-8.3 | Hardening | partial | boot_smoke, host_tested, integrated |
| SECTION-8.4 | Identity and credentials | partial | boot_smoke, host_tested, integrated |
| SECTION-8.5 | Security guarantees and failure behavior | partial | boot_smoke, host_tested, integrated |
| SECTION-9 | I/O, devices and drivers | pending | none |
| SECTION-9.1 | Driver philosophy | partial | boot_smoke, host_tested, integrated |
| SECTION-9.2 | Hardware contracts | partial | boot_smoke, host_tested, integrated |
| SECTION-9.3 | Device subsystem API (conceptual) | pending | none |
| SECTION-9.4 | Hardware functionality | partial | boot_smoke, host_tested, integrated |
| SECTION-10 | Storage, VFS, filesystem, volumes and recovery | pending | none |
| SECTION-10.1 | VFS object model | partial | boot_smoke, host_tested, integrated |
| SECTION-10.2 | Volume layout | partial | boot_smoke, host_tested, integrated |
| SECTION-10.3 | NextFS design specification | partial | boot_smoke, host_tested, integrated |
| SECTION-10.4 | Backup and restore | pending | none |
| SECTION-11 | Networking and remote access | pending | none |
| SECTION-11.1 | Architecture | partial | boot_smoke, host_tested, integrated |
| SECTION-11.2 | Socket interface | partial | boot_smoke, host_tested, integrated |
| SECTION-11.3 | Network behavior and error handling | partial | boot_smoke, host_tested, integrated |
| SECTION-12 | Audio, media, input, accessibility and devices | pending | none |
| SECTION-12.1 | Audio service | partial | boot_smoke, host_tested |
| SECTION-12.2 | Input and IME | partial | boot_smoke, host_tested |
| SECTION-12.3 | Accessibility requirements | partial | boot_smoke, host_tested |
| SECTION-12.4 | Display and capture privacy | pending | none |
| SECTION-13 | Graphics stack and Prism compositor | pending | none |
| SECTION-13.1 | Final graphics capability profiles | partial | boot_smoke, host_tested |
| SECTION-13.2 | GPU architecture | pending | none |
| SECTION-13.3 | Window semantics | partial | boot_smoke, host_tested |
| SECTION-13.4 | Frame pacing and power | partial | boot_smoke, host_tested |
| SECTION-14 | Prism user experience — visual and behavioral specification | pending | none |
| SECTION-14.1 | Visual identity | partial | boot_smoke, host_tested, integrated |
| SECTION-14.2 | Shell layout | partial | boot_smoke, host_tested, integrated |
| SECTION-14.3 | Interaction standards | partial | boot_smoke, host_tested, integrated |
| SECTION-14.4 | Keyboard shortcuts (default) | partial | boot_smoke, host_tested |
| SECTION-14.5 | Core Prism applications | partial | boot_smoke, host_tested, integrated |
| SECTION-14.6 | UI quality requirements | partial | boot_smoke, host_tested, integrated |
| SECTION-15 | Compatibility architecture — principles and boundaries | pending | none |
| SECTION-15.1 | Distinct compatibility profiles | partial | boot_smoke, host_tested, integrated |
| SECTION-15.2 | Common runtime components | partial | boot_smoke, host_tested, integrated |
| SECTION-15.3 | Interop invariants | partial | boot_smoke, host_tested, integrated |
| SECTION-16 | POSIX runtime specification | pending | none |
| SECTION-16.1 | POSIX source/API target | pending | none |
| SECTION-16.2 | Processes and `fork` | pending | none |
| SECTION-16.3 | Terminal/PTY and shell | partial | boot_smoke, integrated |
| SECTION-16.4 | libc implementation rules | pending | none |
| SECTION-16.5 | Observable POSIX application behavior | pending | none |
| SECTION-17 | Linux x86-64 binary compatibility | pending | none |
| SECTION-17.1 | Binary ABI and execution environment | partial | boot_smoke, host_tested, integrated |
| SECTION-17.2 | ABI gateway | partial | boot_smoke, host_tested, integrated |
| SECTION-17.3 | Loader/dynamic linker | partial | boot_smoke, host_tested, integrated |
| SECTION-17.4 | GUI applications | pending | none |
| SECTION-17.5 | User-visible Linux compatibility metadata | pending | none |
| SECTION-18 | NT/Win32 compatibility specification | partial | boot_smoke, host_tested, integrated |
| SECTION-18.1 | Fundamental separation | partial | boot_smoke, host_tested, integrated |
| SECTION-18.2 | NT kernel object emulation | pending | none |
| SECTION-18.3 | PE32+ loader requirements | partial | boot_smoke, host_tested, integrated |
| SECTION-18.4 | Windows user-mode API surface | partial | boot_smoke, host_tested, integrated |
| SECTION-18.5 | Registry and settings | pending | none |
| SECTION-18.6 | Windows GUI model | pending | none |
| SECTION-18.7 | Compatibility environment | pending | none |
| SECTION-18.8 | Observable compatibility correctness | partial | boot_smoke, host_tested, integrated |
| SECTION-18.9 | Game, graphics and DRM compatibility boundaries | pending | none |
| SECTION-19 | Native Next SDK and application model | pending | none |
| SECTION-19.1 | Stable contracts | partial | boot_smoke, host_tested, integrated |
| SECTION-19.2 | SDK modules | partial | boot_smoke, host_tested, integrated |
| SECTION-19.3 | App life cycle | partial | boot_smoke, host_tested, integrated |
| SECTION-19.4 | Library/toolchain portability | partial | boot_smoke, host_tested, integrated |
| SECTION-20 | Packages, installer, distribution, updates and rollback | pending | none |
| SECTION-20.1 | Package types | partial | boot_smoke, host_tested |
| SECTION-20.2 | Package manager algorithms | partial | boot_smoke, host_tested |
| SECTION-20.3 | Installation modes | pending | none |
| SECTION-20.4 | Update system | pending | none |
| SECTION-20.5 | Licensing and distribution | partial | host_tested |
| SECTION-21 | Performance, power, observability and reliability | pending | none |
| SECTION-21.1 | Observable quality and performance characteristics | pending | none |
| SECTION-21.2 | Structured tracing | partial | host_tested |
| SECTION-21.3 | Reliability engineering | partial | boot_smoke, host_tested, integrated |
| SECTION-22 | Finished Prism application suite and shared platform behaviors | pending | none |
| SECTION-22.1 | Prism Home, Search and notifications | partial | boot_smoke, host_tested, integrated |
| SECTION-22.2 | Prism Files and the storage experience | partial | boot_smoke, host_tested, integrated |
| SECTION-22.3 | Next Settings: exhaustive top-level navigation | partial | boot_smoke, host_tested, integrated |
| SECTION-22.4 | Next Terminal, Monitor, Security, Backup and Store | partial | boot_smoke, host_tested |
| SECTION-22.5 | Essential first-party utility applications | partial | boot_smoke, host_tested, integrated |
| SECTION-23 | Desktop interactions, accessibility and internationalization | pending | none |
| SECTION-23.1 | Unified interaction contract | partial | boot_smoke, host_tested, integrated |
| SECTION-23.2 | Accessibility requirements | partial | boot_smoke, host_tested |
| SECTION-23.3 | Localization and typography | pending | none |
| SECTION-24 | Virtualization, containers, developer environment and system interoperability | pending | none |
| SECTION-24.1 | Next Hypervisor and VM manager | pending | none |
| SECTION-24.2 | Next Containers | pending | none |
| SECTION-24.3 | First-class development tools | pending | none |
| SECTION-24.4 | Local and remote productivity | pending | none |
| SECTION-25 | Gaming, graphics, media creation and high-performance workloads | pending | none |
| SECTION-25.1 | Gaming environment | pending | none |
| SECTION-25.2 | Graphics APIs and professional applications | pending | none |
| SECTION-25.3 | Audio and latency | pending | none |
| SECTION-25.4 | Power profiles | pending | none |
| SECTION-26 | Complete binary compatibility and application-environment coverage | pending | none |
| SECTION-26.1 | Executable recognition | pending | none |
| SECTION-26.2 | UNIX is not one binary format | pending | none |
| SECTION-26.3 | Windows compatibility breadth | pending | none |
| SECTION-26.4 | Windows + UNIX coexistence | pending | none |
| SECTION-26.5 | Transparent support designations | pending | none |
| SECTION-27 | Device classes, settings completeness and hardware experience | pending | none |
| SECTION-27.1 | Required classes on supported hardware | pending | none |
| SECTION-27.2 | Hardware support is explicit | pending | none |
| SECTION-27.3 | Multi-user computing | pending | none |
| SECTION-28 | Definitive Next 1.0 product contract | pending | none |
| SECTION-28.1 | What the final system is | pending | none |
| SECTION-28.2 | Indispensable final-version capabilities | pending | none |
| SECTION-28.3 | Truthful limits of the final product | pending | none |
| SECTION-29 | Reference specifications and architectural sources | pending | none |
| DELIVERABLE-boot-and-install | Boot and install: UEFI boot, signed deployment support, disk discovery, safe installer, local account, recovery media | partial | boot_smoke |
| DELIVERABLE-core-kernel | Core kernel: SMP, user/kernel isolation, preemptive threads, virtual memory, capability handles, IPC, robust syscalls | partial | boot_smoke, host_tested, integrated |
| DELIVERABLE-hardware | Hardware: Certified x86-64 reference PC(s) with real storage, input, network, audio and accelerated graphical desktop | pending | none |
| DELIVERABLE-core-storage | Core storage: Crash-consistent native VFS/filesystem, permissions, encryption option, snapshots or equivalent recovery path | partial | boot_smoke, host_tested, integrated |
| DELIVERABLE-networking | Networking: Interoperable IPv4/IPv6 basics, DNS, TCP/UDP, TLS integration, firewall and usable network settings | partial | boot_smoke, host_tested, integrated |
| DELIVERABLE-desktop | Desktop: Original GPU-accelerated Prism experience, windows, taskbar, launcher, Settings, Files, Search, Canvas, Spaces | partial | boot_smoke, host_tested |
| DELIVERABLE-application-platform | Application platform: Next SDK, native app packages, system APIs, browser, terminal, file/media utilities, accessibility | partial | boot_smoke, host_tested, integrated |
| DELIVERABLE-unix | UNIX: Native POSIX userspace capability plus Linux x86-64 ELF execution for a defined substantial app class | partial | boot_smoke, host_tested, integrated |
| DELIVERABLE-nt | NT: Original PE x86-64 loader, NT personality and functional Win32 application compatibility for a defined app class | partial | boot_smoke, host_tested, integrated |
| DELIVERABLE-integration | Integration: Shared desktop, clipboard, file access and policy across supported application personalities | partial | boot_smoke, host_tested, integrated |
| DELIVERABLE-security | Security: Privilege isolation, sandboxing, secure updates, permissions UI, disk encryption, multi-user controls | partial | boot_smoke, host_tested, integrated |
| DELIVERABLE-reliability | Reliability: Recovery, safe shutdown and sleep policies, logging, crash containment, rollback of failed deployments | partial | boot_smoke, host_tested, integrated |
| DELIVERABLE-developer | Developer: Rust/C/C++ native target capability, documented ABI/SDK and installable debugging/development tooling | partial | boot_smoke, host_tested, integrated |
| DELIVERABLE-premium-ux | Premium UX: Fast local interactions, coherent visual language, polished typography, accessibility and privacy by design | partial | boot_smoke, host_tested |
