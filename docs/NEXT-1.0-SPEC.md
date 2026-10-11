# NEXT 1.0 — Final Product Specification

> **Operating system:** **Next** (`NEXT`)  
> **Kernel:** **Helix** — original Rust-first hybrid NT/POSIX-inspired kernel  
> **Desktop:** **Prism** — original compositor, graphical shell and application experience  
> **Native platform:** **Next Runtime** and **Next SDK**  
> **Specification edition:** 1.0 — target end-state, not an implementation progress report  
> **Scope:** Functionality, architecture, interfaces, UX, hardware and compatibility that constitute the completed Next 1.0 product  
> **Important:** This file defines what must exist and how the finished system must behave. It prescribes no development schedule, task ordering or agent workflow.

---

## Product identity and non-negotiable principles

**Next is an independently implemented, general-purpose, installable operating system** for real computers. The running system does not depend on Linux, Windows, BSD, XNU or an existing desktop environment as its kernel or shell. Core first-party components—Helix, Prism, system services, native SDK and compatibility glue—are written in **Rust** except for the minimal architecture-specific assembly, hardware-interface glue or other explicitly justified boundary code.

**Helix** combines UNIX/POSIX process and filesystem semantics with NT-inspired object, handle, security, synchronization and asynchronous-I/O semantics through a shared resource model. It exposes separate compatible interfaces instead of claiming POSIX and NT binaries are interchangeable. **Prism** presents one exceptionally fast, coherent, accessible, local-first, modern graphical desktop for native and supported foreign applications.

The guiding product goal is the best overall computer experience: familiar but cleaner than Windows 11, powerful for developers, comfortable for everyday users, excellent at multi-window productivity, transparent about compatibility, robust under failure, private by default and performant on supported hardware.

**Compatibility honesty:** “Run any NT or UNIX application” is an aspirational design objective, not a technically universal guarantee. Executables differ in CPU architecture, binary format, syscalls, APIs, libraries, drivers, frameworks, DRM and services. Next must implement broad native compatibility with explicitly identified targets and disclose any unsupported application behavior. **No loader stub, emulator-only demo or branded desktop on another OS qualifies as Next 1.0.**

### Specification language

- **MUST** — required for the described completed product or declared supported profile.
- **MUST NOT** — prohibited system behavior.
- **SHOULD** — default product behavior unless there is an explicitly documented functional reason to differ.
- **MAY** — discretionary optional function that must not be falsely advertised as required.
- Numbered `NEXT-<DOMAIN>-<NUMBER>` identifiers represent system-level requirements or interface invariants, not a project task list.

---

## Contents

- [0. Vision, scope, and product rules](#0-vision-scope-and-product-rules)
- [1. System diagram and trust boundaries](#1-system-diagram-and-trust-boundaries)
- [2. Supported computing platforms, hardware and portability](#2-supported-computing-platforms-hardware-and-portability)
- [3. Boot, memory-map handoff, and recovery](#3-boot-memory-map-handoff-and-recovery)
- [4. Helix kernel principles](#4-helix-kernel-principles)
- [5. Memory management and scheduling](#5-memory-management-and-scheduling)
- [6. Helix objects, capabilities and synchronization](#6-helix-objects-capabilities-and-synchronization)
- [7. Process lifecycle, executable loading, and native runtime](#7-process-lifecycle-executable-loading-and-native-runtime)
- [8. Security model and threat analysis](#8-security-model-and-threat-analysis)
- [9. I/O, devices and drivers](#9-io-devices-and-drivers)
- [10. Storage, VFS, filesystem, volumes and recovery](#10-storage-vfs-filesystem-volumes-and-recovery)
- [11. Networking and remote access](#11-networking-and-remote-access)
- [12. Audio, media, input, accessibility and devices](#12-audio-media-input-accessibility-and-devices)
- [13. Graphics stack and Prism compositor](#13-graphics-stack-and-prism-compositor)
- [14. Prism user experience — visual and behavioral specification](#14-prism-user-experience-visual-and-behavioral-specification)
- [15. Compatibility architecture — principles and boundaries](#15-compatibility-architecture-principles-and-boundaries)
- [16. POSIX runtime specification](#16-posix-runtime-specification)
- [17. Linux x86-64 binary compatibility](#17-linux-x86-64-binary-compatibility)
- [18. NT/Win32 compatibility specification](#18-ntwin32-compatibility-specification)
- [19. Native Next SDK and application model](#19-native-next-sdk-and-application-model)
- [20. Packages, installer, distribution, updates and rollback](#20-packages-installer-distribution-updates-and-rollback)
- [21. Performance, power, observability and reliability](#21-performance-power-observability-and-reliability)
- [22. Finished Prism application suite and shared platform behaviors](#22-finished-prism-application-suite-and-shared-platform-behaviors)
- [23. Desktop interactions, accessibility and internationalization](#23-desktop-interactions-accessibility-and-internationalization)
- [24. Virtualization, containers, developer environment and system interoperability](#24-virtualization-containers-developer-environment-and-system-interoperability)
- [25. Gaming, graphics, media creation and high-performance workloads](#25-gaming-graphics-media-creation-and-high-performance-workloads)
- [26. Complete binary compatibility and application-environment coverage](#26-complete-binary-compatibility-and-application-environment-coverage)
- [27. Device classes, settings completeness and hardware experience](#27-device-classes-settings-completeness-and-hardware-experience)
- [28. Definitive Next 1.0 product contract](#28-definitive-next-10-product-contract)
- [29. Reference specifications and architectural sources](#29-reference-specifications-and-architectural-sources)

---

# 0. Vision, scope, and product rules

## 0.1 Brand and architecture

- OS: **Next**; engineering identifier: `NEXT`.
- Kernel: **Helix**.
- Desktop/compositor: **Prism**.
- Native application platform: **Next Runtime**.
- Native API/SDK: **Next SDK**.
- Default file manager: **Prism Files**.
- Workspace/window manager: **Prism Canvas**.
- Launcher and command palette: **Prism Home/Search**.
- Security center: **Next Security**.
- Native package/store interface: **Next Store**.
- Optional tagline: *One system. Every possibility.* This is an internal candidate, not a mandatory legal identity.

Note: product names and marks require trademark clearance before commercial launch.

## 0.2 Product thesis

Next should feel familiar to someone coming from Windows 11, while being more cohesive and flexible: a recognizable desktop, bottom taskbar, application launcher, familiar file concepts and windows; unified preferences; native tiling and workspaces; exceptional keyboard experience; accessible design; no advertising in system surfaces; strong privacy and local account support; exceptionally reliable software installation, updates, backup, and recovery. Developers get a first-class UNIX environment; NT applications receive a planned compatibility runtime. All compatible apps appear in the same Prism shell.

## 0.3 Non-goals and hard constraints for 1.0

- Do **not** claim certification as UNIX, full POSIX conformance, or arbitrary Windows/NT compatibility without completed conformance and independently measured evidence.
- Do **not** load Windows kernel drivers (`.sys`) directly into Helix, redistribute Microsoft DLLs without authorization, or assume Windows Store/DRM service equivalence.
- Do **not** require cloud login, telemetry, subscriptions, remote AI, or an app store account for basic operation.
- Do **not** promise native Direct3D 12/Vulkan game compatibility without driver and runtime implementation.
- Do **not** use arbitrary Internet downloads as trusted boot-time kernel modules.
- The bundled browser MAY use a securely maintained, legally redistributable engine; neither a newly invented browser engine nor a mandatory online AI assistant is a component of the Next system architecture.
- Do not represent basic framebuffer output, an ELF/PE loader alone, or a minimal UI as equivalent to the full final-product hardware, graphics, storage and compatibility capabilities.
- External upstream specifications are permitted; copying proprietary code or using leaked source is forbidden.

## 0.4 Product editions

A unified **Next 1.0** desktop/laptop/workstation operating system without artificially feature-locked Home/Pro editions. Optional organization-management extensions are not required for a personal computer to function. Every supported CPU architecture and hardware family uses the same system/API design, with architecture-specific binaries and drivers where necessary.

## 0.5 Definitions of success

At the declared launch compatibility level and on **each certified reference machine**, a person can:

1. Boot from USB, enter a live Prism environment, inspect hardware and storage, install with clear disk-selection/backup warnings, create a local account, and reboot into an encrypted installation where supported.
2. Use pointer/keyboard, taskbar, launcher, notification center, window snapping, multiple windows/desktops, file manager, settings, terminal, browser/application launch, audio, networking, and screen capture.
3. Compile/run native applications and a clearly specified POSIX example set.
4. Launch and use a substantial publicly identified collection of Linux ELF and Windows PE/Win32 software using their corresponding supported compatibility runtime, with a visible per-version support catalog; see §17 and §18.
5. Update with integrity verification and recover from a deliberately interrupted or failed update without losing user data.
6. View understandable application permissions and logs; isolate failing apps without crashing the desktop or kernel.
7. Shut down, restart, lock, unlock, suspend/resume and recover from common device failures on the stated certified hardware matrix.

No product passes if it achieves only a wallpaper and movable windows or merely boots one `hello world` Windows/ELF binary.

---

# 1. System diagram and trust boundaries

```text
                    NEXT 1.0 SYSTEM
+-----------------------------------------------------------------+
| Prism: shell | compositor | Home | Canvas | Files | Settings    |
+-----------------------------------------------------------------+
| Native Next apps | POSIX source apps | Linux ELF | PE/Win32 apps|
+-----------------------------------------------------------------+
| Next Runtime     | POSIX libc        | Linux ABI | NT/Win32 DLLs |
+-----------------------------------------------------------------+
| System services: launchd-like init (nextd), accounts, security,  |
| auth, VFS, package broker, network, audio, device manager, logs, |
| font/text, clipboard, portals, crash reporting, update manager  |
+----------------------+------------------------------------------+
| HELIX privileged core: objects, handles, virtual memory,        |
| scheduler, processes/threads, syscalls, interrupts, IPC,        |
| capabilities, timers, low-level device/driver services          |
+----------------------+------------------------------------------+
| HAL x86-64 UEFI | ACPI/APIC | MMU | PCIe | NVMe | USB | GPU     |
+-----------------------------------------------------------------+
| REAL HARDWARE or explicitly supported QEMU/virtio VMs           |
+-----------------------------------------------------------------+
```

Trust levels:

- **T0 Hardware/Firmware:** external trust, secure-boot measurement where available; firmware bugs remain possible.
- **T1 Helix ring 0:** minimum privileged kernel, vetted in-kernel low-level drivers only.
- **T2 privileged userspace services:** capability-constrained processes; failures individually supervised.
- **T3 application sandboxes:** native/compat applications, untrusted by default.
- **T4 external packages/network input:** cryptographically authenticated where applicable; always untrusted until policy allows.

All interfaces crossing boundaries MUST version messages, validate lengths/alignments/rights, apply quotas, and have testable denial behavior. An NT handle and a UNIX file descriptor can refer to the same object but must never bypass T1 security checks.

---

# 2. Supported computing platforms, hardware and portability

## 2.1 Platform and processor architecture

- `NEXT-BOOT-001`: Next 1.0 is a **real-hardware x86-64 UEFI operating system**, capable of running on supported desktop PCs, laptops and professional workstations without another OS kernel or hypervisor underneath.
- Helix supports SMP/multicore execution, x86-64 long mode, APIC/x2APIC as available, high-resolution and calibrated timers, NX, CPU feature detection, interrupt remapping where supported, MMU translation and four-level paging; optional five-level paging must obey canonical-address rules.
- The production target requires UEFI firmware, GPT boot/storage conventions, ACPI platform discovery, PCI/PCIe device enumeration, and actual installed devices, not simulated substitutes presented as hardware support.
- Generic HAL interfaces isolate x86-64-specific state from architecture-neutral scheduling, VM, IPC, storage and driver services. The design supports an eventual independently certified AArch64 build; support must not be claimed without working implementations and appropriate ABI/CPU translation rules.
- User-facing minimum specification: **8 GiB RAM, 64 GiB disk, supported UEFI CPU and graphics output**; recommended **16 GiB RAM and 256 GiB NVMe**. Actual device, firmware, power-management and performance constraints are published for each supported hardware profile.

## 2.2 Required hardware classes and support declarations

- Reliable UEFI boot; firmware memory-map validation and ACPI discovery.
- Physical/virtual memory, CPUs/threads, storage controllers, NVMe, GPU display, keyboard/pointer, audio, Ethernet and tested Wi-Fi/Bluetooth peripherals.
- USB xHCI, hubs, removable drives, hotplug and safe ejection; PCIe BAR/interrupt/DMA protections; approved device-specific driver capabilities.
- Displays include verified multi-monitor configurations, HiDPI, compositor output, correct hot-unplug/replug behavior, and refresh rates within driver capability.
- Laptop profiles have battery charging/level, CPU/platform power states, suspend/resume, lid, touchpad, keyboard and thermal behavior as appropriate.
- A **published support catalog** identifies CPU generation, motherboard or laptop model, firmware version, PCI/vendor/device IDs, GPU driver, network and audio drivers, features and known limitations. An unsupported device does not become supported because Next can enumerate it.
- QEMU/OVMF and virtio are also supported for testing and VM users, but virtual-machine operation is not a substitute for actual certified PC operation.

## 2.3 Deliverable image and system format

The final distribution includes an installable UEFI system image, live/recovery environment, signed boot and OS deployment artifacts where signing infrastructure is configured, kernel and debug symbol artifacts, native application packages, SDK libraries/documentation, device-support metadata, package-signature roots, and integrity manifests. Builds retain exact component versions and license/source attribution. System and application targets use stable ABI names and architecture identifiers; native Rust/C/C++ developers receive the necessary headers, metadata, toolchain support and debuggable binaries.

# 3. Boot, memory-map handoff, and recovery

## 3.1 Boot sequence

```text
UEFI -> BOOTX64.EFI (NextBoot)
     -> verify boot artifacts / choose A or B deployment
     -> gather GOP, memory map, RSDP, RNG, TPM evidence if available
     -> load Helix ELF + initrd; assign randomized kernel base
     -> ExitBootServices with correct memory-map key retry logic
     -> trampoline into kernel with BootInfo pointer
Helix -> paging / early console / phys allocator / CPU init
      -> GDT/IDT/TSS / interrupts / timers / APIC
      -> virtual allocator / heap / ACPI / PCI discovery
      -> scheduler / process address spaces / syscall entry
      -> spawn pid1 nextd from signed init image
nextd -> drivers/services -> system session -> login -> Prism
```

Boot information pseudo-contract (fields are normative; exact layout frozen in generated `next-abi` definitions):

```rust
#[repr(C)]
pub struct BootInfoHeader {
    pub magic: u64,           // NEXT_BOOT_MAGIC
    pub abi_major: u16,
    pub abi_minor: u16,
    pub struct_len: u32,
    pub flags: u64,
    pub checksum: u64,        // documented checksum over header + sections
}
// Followed by individually sized, validated records for:
// memory regions, framebuffer, ACPI RSDP, kernel image,
// initrd, entropy seed, boot deployment, serial port, TPM measurements.
```

Requirements:

- `NEXT-BOOT-003`: loader MUST validate ELF segments, sizes, overlaps, alignment, entrypoint mapping and W^X permissions. Unknown mandatory boot records MUST fail closed.
- `NEXT-BOOT-004`: early panic displays serial diagnosis and halts; no blind reboot loops. Preserve a reboot-reason code in next available writable log facility.
- `NEXT-BOOT-005`: A/B system deployment with monotonic rollback metadata, image hashes, boot-attempt counter and recovery entry. User data partition is never part of an automatic OS rollback.
- `NEXT-BOOT-006`: Secure Boot signature verification and TPM-measured boot are implemented on certified hardware supporting those capabilities. Owner-authorized developer builds may use separate test keys, with visible security-state warnings and a documented recovery method.
- `NEXT-BOOT-007`: boot-critical input and display fallback must not depend on Prism or GPU acceleration.

## 3.2 Failure handling and recoverability

For a corrupt kernel, wrong architecture, missing initrd, invalid BootInfo, overlapping memory regions, missing ACPI, unresolved dependencies, interrupted update, rollback failure, full root disk, insufficient entropy and unsupported boot controllers, Next must recover, provide a useful actionable diagnostic, or refuse to proceed safely. A recoverable boot failure must expose the local recovery environment when available. No failure condition should result in an unexplained permanent black screen when a functioning diagnostic output is available.

---

# 4. Helix kernel principles

## 4.1 Kernel model

**Hybrid design, original implementation.** Performance-critical primitives in privileged core; most policy and extensible services in userspace. All kernel allocations and blocking operations are explicitly categorized as interrupt-safe, atomic-context safe, or sleepable. Preserve object identity and security across native/compat personalities.

Kernel modules (logical crates): `arch`, `boot`, `mm`, `sched`, `proc`, `object`, `handle`, `vfs`, `io`, `ipc`, `sync`, `security`, `timer`, `interrupt`, `device`, `syscall`, `trace`, `debug`.

- `NEXT-KRN-001`: no allocator allocations in interrupt handlers unless using a bounded IRQ-safe pool.
- `NEXT-KRN-002`: an unhandled user fault kills or signals only the offending user process, not the whole kernel.
- `NEXT-KRN-003`: kernel panic is reserved for violated kernel invariants; expose panic ID, CPU, thread, backtrace if possible, and serial log.
- `NEXT-KRN-004`: all public kernel structures have opaque handles; users cannot pass raw kernel pointers as authority.
- `NEXT-KRN-005`: syscalls consume only validated user buffers; copyin/copyout protects against invalid mappings and integer wraparound.
- `NEXT-KRN-006`: per-core state and interrupt priority are explicit. Lock ordering is written down; hard IRQ contexts must not block.
- `NEXT-KRN-007`: monotonic time does not jump with wall clock changes; timers are not executed under locks they may reenter.

## 4.2 Native ABI conventions

Architecture: x86-64 `syscall/sysret` fast path where safe; fallback `iretq` on noncanonical or otherwise unsafe return state. Native syscall numbers occupy a versioned Next-exclusive namespace; do not reuse NT or Linux syscall numbers as native identifiers.

**Draft native syscall invocation:** `rax` = Next syscall number; `rdi, rsi, rdx, r10, r8, r9` = up to six arguments; `rcx/r11` clobbered by `syscall`; `rax` = signed result with explicitly defined error encoding. This is a **proposal subject to ABI-freeze**, not an assertion of compatibility with any external ABI. All userland stubs must be generated from one schema and fuzzed with invalid pointers.

Syscall table v0 minimum:

| ID proposal | Call | Result and conditions |
|---|---|---|
| 0 | `abi_query` | version/capabilities of caller personality |
| 1 | `handle_close` | closes one owned handle |
| 2 | `handle_dup` | duplicate with requested rights subset |
| 3 | `channel_create` | connected message endpoints |
| 4 | `channel_send` | bounded typed message + transferred handles |
| 5 | `channel_recv` | receive message, timeout, cancellation |
| 6 | `vmo_create` | virtual-memory object with initial size |
| 7 | `vmo_map` | address-space mapping, W^X enforcement |
| 8 | `vmo_unmap` | unmap region and synchronize TLB |
| 9 | `thread_create` | create suspended user thread |
| 10 | `thread_start` | schedule new thread |
| 11 | `thread_exit` | exit current thread |
| 12 | `process_spawn` | start signed/authorized native image |
| 13 | `process_wait` | wait with timeout/status |
| 14 | `object_wait_many` | wait on multiple waitable objects |
| 15 | `clock_read` | monotonic/wall clocks |
| 16 | `log_write` | bounded structured user log |
| 17 | `resource_query` | capability-limited hardware/service metadata |
| 18 | `io_submit` | async I/O request to service endpoint |
| 19 | `io_complete` | receive asynchronous completion |

**Important:** numbers are reserved only until documented freeze. Separate Linux syscall dispatcher and NT ABI emulation dispatch use their respective exact calling conventions, table versions, errno/NTSTATUS mapping and semantics. Expose a tested ABI personality per process; never guess at runtime based on syscall number collisions.

## 4.3 Interface stability

- Native ABI has major/minor versions. Minor changes may add calls or optional flags; never silently change an existing structure layout.
- Syscall structs have `size` and optional extension fields, are little-endian in image metadata only, and native-endian for in-memory x86-64 structures where specified.
- Unknown optional flags -> `NOT_SUPPORTED`; unknown mandatory version -> `ABI_MISMATCH`.
- Never expose unstable Rust trait/object layout across process boundaries; IPC/FFI uses explicit schema.
- Golden ABI tests must compile headers from Rust and C (and C++ where relevant), compare `sizeof`, alignment, offsets, and binary fixtures.

# 5. Memory management and scheduling

## 5.1 Physical memory management

- Parse and normalize UEFI regions into `usable`, `reserved`, `ACPI reclaim`, `ACPI NVS`, `MMIO`, `firmware runtime`, `bad`, `loader occupied`; only freed-safe regions feed allocator.
- Initialize bitmap allocator for bootstrap, then per-NUMA-node buddy allocator (orders covering 4 KiB through large contiguous blocks) with per-CPU small-page caches. Use 64-bit physical addresses and overflow-safe arithmetic.
- Track page ownership/refcount, zero-on-user-allocation, pin count, mapping count, and writeback states. `FREE` pages are never mapped writable into untrusted userspace until cleared.
- Reserve OOM emergency memory and metadata pages, support hot-removal only after explicitly required validation; track memory pressure and page faults.
- `NEXT-MEM-001`: no use of UEFI boot-services memory after `ExitBootServices` unless reclaimed according to UEFI memory type rules.
- `NEXT-MEM-002`: reject overlapping/unaligned physical map entries and integer overflow during range normalization.
- `NEXT-MEM-003`: user processes cannot map kernel pages, MMIO, page tables or physical memory without explicit privileged device capabilities.

## 5.2 Virtual memory

- Per-process CR3/address space, kernel high-half mapping, distinct user address range, ASLR, NX, W^X, page guard regions, copy-on-write, lazy zero pages, demand paging, shared memory objects, stack-growth policy with maximum size.
- Support anonymous memory, file-backed mappings, read-only/shared/copy-on-write, huge pages opportunistically, memory-mapped GPU buffers with IOMMU where supported.
- Kernel virtual allocation includes explicit executable vs writable regions, strong lifetime bookkeeping, guard pages for stacks, kernel heap slab allocator, page poisoning/debug builds.
- Page-fault handler classifies read/write/execute, present/absent, kernel/user, COW, guard, and invalid addresses; user errors translate via active personality (`SIGSEGV`/`SIGBUS` for POSIX, suitable NT exception for NT) without kernel panic.
- `NEXT-MEM-004`: all mappings prevent writable-and-executable pages unless a narrowly audited JIT transition is authorized; use temporary permission transitions and flush appropriate caches/TLB.
- `NEXT-MEM-005`: validate pin/unpin while asynchronous I/O is in flight; prevent DMA-to-freed-page and mapping lifetime use-after-free.
- `NEXT-MEM-006`: test COW across `fork()`, exec image replacement, exception delivery, shared pages, page faults under SMP contention, and user/kernel boundary addresses.

## 5.3 Scheduler

An original preemptive SMP scheduler. Baseline: per-CPU run queues, task wakeup queues, priority bands, target-latency/fairness accounting, load balancing, idle threads, RT admission limits, priority inheritance on critical locks, and tickless idle when safe. Avoid claiming scheduler superiority without measured results.

Classes:

| Class | Goal | Constraints |
|---|---|---|
| `Realtime` | explicitly authorized low-latency audio/industrial work | CPU budget, starvation prevention, admin-controlled rights |
| `Interactive` | foreground input, compositor and UI | responsive wakeup, bounded service time |
| `Normal` | regular applications | proportional fairness |
| `Batch` | compiling, rendering, transforms | high throughput, no UI starvation |
| `Background` | indexing, package downloads, housekeeping | energy/resource efficiency |
| `Idle` | true idle work only | opportunistic |

Core rules:

- A context switch preserves all required GPR/FPU/SIMD/TLS state; use lazy/eager extended state only when CPU security requirements are satisfied.
- Thread priorities are effective priorities, not privileges. Real-time grants require policy capability; a misbehaving app must not starve the login, input, pager, and crash-recovery services.
- Mutexes and join/wait protocols must model priority inversion; blocking IPC must propagate effective scheduling priority only under bounded constraints.
- Process groups and job-like containers apply CPU/memory/IO quotas with pressure notifications to GUI.
- `NEXT-SCH-001`: a focused UI task under CPU load must be measurable and remain responsive within the published reference benchmark.
- `NEXT-SCH-002`: 1/2/4/8-core QEMU schedule, exit, wake, signal and preemption stress tests must pass without deadlock/livelock.
- `NEXT-SCH-003`: CPU hotplug may be omitted in 1.0 but topology discovery must not assume fixed CPU count.

## 5.4 Timestamps and clocks

Expose monotonic, boottime, and realtime clocks; implement high-resolution timers with calibrated invariant TSC or validated architecture clocksource, stable timer conversion, and NTP/user-directed wall-clock adjustments in userspace. Scheduling must never use wall-clock jumps. Define timeout semantics under suspension and cancellation.

---

# 6. Helix objects, capabilities and synchronization

## 6.1 Object invariant

All kernel-visible resources have a common lifetime/accounting model:

```rust
pub struct KernelObjectHeader {
    // Conceptual internal fields, not a stable public ABI:
    // ObjectTypeId, ObjectId, reference/lifetime bookkeeping,
    // waitability state, security label, audit class.
}

pub struct HandleEntry {
    // Opaque object reference; effective access mask;
    // generation; inheritable/transferable flags; holder identity.
}
```

**Rule:** user handles are unforgeable per-process indexed values with generation numbers and rights; they do **not** directly expose kernel addresses. Remove/destroy on final reference, taking in-flight asynchronous operations and transferred handles into account.

Object types: `Process`, `Thread`, `AddressSpace`, `MemoryObject`, `File`, `Directory`, `Channel`, `Event`, `Semaphore`, `Mutex`, `Timer`, `CompletionPort`, `Socket`, `Device`, `Job`, `Session`, `DisplaySurface`, `Interrupt`, `ServiceEndpoint`.

- A name is not a capability. Global object names resolve only through policy-controlled namespaces.
- Rights are fine-grained: `READ`, `WRITE`, `EXECUTE`, `MAP`, `WAIT`, `SIGNAL`, `DUPLICATE`, `TRANSFER`, `INSPECT`, `ADMIN`; service-specific rights MAY refine these.
- Access checks occur on object-open/transfer and on operations whose security context can change. Handle duplication cannot enlarge rights.
- Process exit closes handles and cancels/settles pending I/O; leaked kernel object detection runs in debug builds.
- `NEXT-OBJ-001`: stale handle generations cannot alias newly allocated privileged objects; handle exhaustion returns a bounded failure.
- `NEXT-OBJ-002`: duplicate, inherit and transfer tests must prove no escalation even under cross-process races.
- `NEXT-OBJ-003`: object wait logic guarantees no missed wake-ups in signal/reset/close/cancel races.

## 6.2 Native synchronization

Native primitives: spinlock, irqsave spinlock, sleepable mutex, rwlock, condition variable, event (auto/manual reset), semaphore, futex-equivalent keyed wait, completion port, and timer. Define exactly which are consumptive, edge-/level-triggered, and resettable.

Default lock hierarchy (lower to higher; documented exceptions are permitted only when deadlock safety is preserved): scheduler queues -> process/table -> object handles -> VM regions -> VFS node locks -> device queues. Locks protecting user-visible objects must avoid callback into arbitrary user/service code while held. Global namespace locks must never be held over potentially blocking I/O.

**Windows synchronization is not just POSIX synchronization under a renamed symbol**: preserve manual/auto-reset events, abandoned mutex signaling, keyed wait semantics, wait-many behavior, cancellation, and exception edge cases as individual tested contracts.

## 6.3 IPC protocol

A process may communicate through a capability endpoint. A message has a fixed header (`magic`, major/minor, opcode, flags, correlation ID, payload length, handle count, deadline), bounded payload, optional handle transfer list and optional shared memory descriptor. Serialize little-endian for on-wire/on-disk formats; IPC byte order is explicitly documented. Multi-megabyte payloads MUST travel via shared-memory objects, not repeated kernel copies.

```text
Client                    Kernel                    Service
  | open endpoint(cap)       |                          |
  |------------------------->|                          |
  | send(request, handles)   | validate+charge quota    |
  |------------------------->|------------------------->|
  |                          |       response           |
  |<-------------------------|<-------------------------|
  | close/timeout/cancel     |                          |
```

- Each request is auditable and cancellable; server death causes deterministic `PEER_CLOSED` to waiting callers.
- Require a request ID; replies cannot satisfy the wrong outstanding call. Enforce per-peer memory, handle and message-rate quotas.
- `NEXT-IPC-001`: malformed frames/fuzz input never panic kernel, leak handles or cross privilege boundaries.
- `NEXT-IPC-002`: IPC benchmark documents 1 B, 64 B, 4 KiB, 64 KiB payloads and handle-transfer overhead.

---

# 7. Process lifecycle, executable loading, and native runtime

## 7.1 Process model

A process owns an address space, kernel-visible PID with generation, security credentials, namespaces, handle table, thread group, resource limits, environment, current directory, standard channels, signal/exception state, and parent/job relationships.

Lifecycle states: `NEW -> READY -> RUNNING -> STOPPING -> EXITED -> REAPED`. `SUSPENDED` is an orthogonal thread state. `ZOMBIE` POSIX state is represented as retained exit status until waited upon.

Native spawn must be atomic from the caller's perspective: either creates a fully initialized suspended process or returns an error without partially visible threads/handles. Init sets up bootstrap thread, TLS, stack guard, args/env, runtime entry, and loader bookkeeping; scheduler transition starts execution.

## 7.2 Image formats

- **Next native:** ELF64 on x86-64 with a Next ABI note, signed package manifest and capability list. `.nextapp` is an application bundle format, not a replacement CPU object-file format.
- **Linux:** ELF64 x86-64, `PT_INTERP`, dynamic relocations, VDSO/vvar as required by compatibility target, native Linux syscall ABI and library environment.
- **Windows:** PE32+ x86-64 with DOS/NT headers, section mapping, imports/exports, relocations, TLS callbacks, PE exceptions/unwinding, manifests, resources and secure DLL resolution. PE32/x86 is a distinct advertised profile only with correct WOW-style thunking and 32-bit ABI semantics.
- Unsupported architecture is rejected clearly or routed to an explicitly installed compatible emulator/VM; never execute mismatched machine code.

## 7.3 Thread and exception contract

- Preserve x86-64 ABI stack alignment, TLS base, exception stack behavior, FPU state, signal mask, alt stacks where relevant, and APC-like NT delivery semantics where implemented.
- Crash information includes thread, executable build ID, personality, fault address, exception class, recent syscalls, and a symbolicated stack when symbols are available.
- Kernel's job is to deliver structured fault events; user-space compatibility runtime maps them to UNIX signal actions or Windows SEH/VEH exception dispatch where coverage exists.

## 7.4 Runtime services

`nextd` (PID 1 equivalent) starts audited services from a declarative dependency graph. Requirements: restart policy with rate limits, per-service capabilities, per-service resource budgets, watchdog, health endpoints, boot ordering, structured logs, crash loop breaker, session creation and system state transitions. Keep `nextd` free of graphical dependencies; Prism crash must not take down the service supervisor.

Services MUST have single clear owners: `identityd`, `authd`, `vfsd`, `deviced`, `netd`, `audiod`, `displayd`, `packaged`, `updated`, `sessiond`, `logd`, `powerd`, `timed`, `fontd`, `searchd`, `compatd` as applicable. These names are design contracts, not evidence of implementations.

---

# 8. Security model and threat analysis

## 8.1 Security objective

Protect user data, kernel integrity, boot chain, device access, identity, IPC permissions, package installation, compatibility sandboxes, display capture, credentials and updates. No compatibility layer may bypass the common Helix security reference monitor.

Assets: authentication secrets, disk keys, user files, camera/mic, clipboard, display content, browser/profile data, SSH keys, signed OS system images, update keys, driver DMA regions, network traffic, privileged service capabilities.

Adversaries: malicious downloaded PE/ELF program, compromised package, privilege-escalating local user, malicious web content, rogue USB device, unauthorized network peer, untrusted firmware/driver data, power-loss/corrupt-disk conditions, compromised update server.

Trust caveat: a Rust kernel does not eliminate logic flaws, architecture bugs, unsafe FFI vulnerabilities or malicious privileged components.

## 8.2 Authorization

- Credentials attach to security principals (user, service, application sandbox). POSIX UID/GID and NT token representations map to **one Next principal and rights model**, preserving visible API semantics without turning POSIX root or NT Administrator into unconditional omnipotence.
- Native app permissions: `filesystem.read_selected`, `filesystem.write_selected`, `network.client`, `network.listen`, `camera`, `microphone`, `screen.capture`, `clipboard.read_background`, `devices.usb`, `notifications`, `location`, `process.inspect`, `debug`, `system.admin`.
- GUI portal grants are scoped, revocable, persistent only with explicit consent, and enforced by services, not UI text alone.
- Per-user containers isolate user home and registry-like state. Compatibility runtimes are untrusted application processes; bridge servers use minimum rights.
- `NEXT-SEC-001`: package manifest permission requests are not grants. Installation and runtime permission decisions are separate.
- `NEXT-SEC-002`: process spoofing another executable/app identity or permissions by renaming a file must be prevented through signed identity and launch token binding.
- `NEXT-SEC-003`: external USB/DMA mappings follow IOMMU policy when available; cannot claim DMA isolation on unsupported configurations.

## 8.3 Hardening

ASLR, NX, W^X, guard pages, stack canaries where toolchain supports them, kernel/user page isolation, speculative-execution mitigations selected per CPU, constant-time cryptography where needed, quotas, syscall argument validation, safe defaults, structured audit logs, tamper-resistant boot integrity where hardware supports it. Disable debug features and developer unsigned-code mode by default in release images.

No cloud connection must be required for local login, printing, media playback, software already installed, or file management. Crash reporting/telemetry is opt-in; logs available for local inspection without submission. Next privacy documentation lists network endpoints and their purposes.

## 8.4 Identity and credentials

- Local user provisioning, strong password hashing via an independently reviewed library/algorithm, lockout/backoff policy without easy permanent denial-of-service, hardware-key/FIDO integration optional, session lock, privilege elevation via an authenticated system dialog.
- Full-disk encryption on supported installations; encryption passphrase/recovery key shown to user during provisioning with clear backup instructions. Never transmit recovery keys automatically.
- TPM-sealed disk keys are supported on qualifying hardware, with explicit owner recovery mechanisms when PCR measurements change after firmware or system updates.
- Sensitive buffers zeroized when practical, keys never placed in generic logs, telemetry or core dumps.

## 8.5 Security guarantees and failure behavior

All privileged interfaces—including syscall dispatch, PE/ELF loading, IPC messages, filesystem parsers, networking, image/font codecs, USB descriptors and ACPI input—reject invalid lengths, pointers, capability rights, stale generations and hostile recursive structures without granting privilege or corrupting kernel state. Revoked rights take effect under documented concurrency rules. Path traversal, DLL injection, symlink races, rogue registry aliases, package downgrade, rollback tampering and guest-to-host memory access are explicitly prevented by enforcement mechanisms, not visual warnings. The shipped system includes an auditable dependency inventory, security update mechanism and locally inspectable security state. No implementation technology alone constitutes formal security certification.

---

# 9. I/O, devices and drivers

## 9.1 Driver philosophy

Next supplies its own driver interfaces and implementations. A driver is either:

1. **Kernel-resident:** essential early devices and latency-critical trusted low-level functions with audited memory safety.
2. **Userspace driver service:** PCI/USB/device capabilities delegated through restricted kernel-controlled MMIO, IRQ and DMA APIs, with restart isolation.

Every driver has `probe -> claim -> configure -> start -> suspend/resume -> stop -> release`; removal/errors must cancel in-flight I/O and free DMA resources safely. No assumption that interrupt completion order equals command submission order.

## 9.2 Hardware contracts

- HAL: paging, IRQ/APIC, time source, CPU discovery, MMIO, I/O ports, cache policies, IOMMU and DMA mapping, MSI/MSI-X, power transitions.
- ACPI: RSDP/RSDT/XSDT validation, MADT/APIC, MCFG, FADT, HPET where used; AML interpreter for required laptop features or explicitly certify platforms that work with a limited subset. Missing AML support means **no claim of general laptop support**.
- PCIe: recursive bus enumeration; BAR sizing and mapping, capabilities, MSI-X, hotplug policy; reject malicious config data.
- NVMe: admin/io queue setup, PRP/SGL buffer validation, namespaces, flush, timeouts, reset, error recovery. Test under simulated media faults.
- USB: xHCI enumeration and HID first, then USB mass storage and audio, safe descriptor parsing, disconnect races and sandbox policy.
- Virtio: modern virtqueues, negotiated feature bits, DMA barriers, block/net/gpu/input; fail closed on unsupported features.
- Display fallback: UEFI GOP framebuffer until a validated display driver takes over.
- Real GPU acceleration requires functioning native GPU drivers, synchronization, memory management, command queues and suitable graphics APIs; device enumeration alone is insufficient. Publish supported rendering capabilities per certified GPU.

## 9.3 Device subsystem API (conceptual)

```rust
pub trait DeviceDriver {
    fn probe(&self, caps: &DeviceCaps) -> ProbeResult;
    fn attach(&self, ctx: &DeviceContext) -> Result<DriverInstance, DriverError>;
    // The runtime manages IRQ routing, DMA mappings and shutdown ordering.
}
```

This illustrative trait is not an ABI: drivers are versioned service processes or kernel modules with separate stable boundary formats. No unstable Rust trait object crosses a kernel/userspace boundary.

## 9.4 Hardware functionality

For each certified hardware profile: cold/warm boot, PCI enumeration, storage RW+flush, keyboard/mouse, multi-monitor if advertised, audio, Ethernet/Wi-Fi if advertised, sleep/resume, thermal/power behavior, USB hotplug, suspend under I/O load, low battery/emergency shutdown where relevant, and crash recovery. Publish exact device/controller firmware IDs.

# 10. Storage, VFS, filesystem, volumes and recovery

## 10.1 VFS object model

One VFS exposes streams/inodes, directories, mount points, devices, sockets where appropriate, metadata, watches and ACL data through capability-backed file objects. Use vnode-like opaque records and per-open state. Metadata caches must observe rename/unlink/permission changes. `open`, `openat`, `read`, `write`, `pread`, `pwrite`, `seek`, `stat`, `link`, `symlink`, `rename`, `fsync`, file locks, directory iteration, inotify-like watches (Linux personality) and NT file-sharing/delete dispositions map to the common VFS with explicit semantic adapters.

**Rules:**

- POSIX pathnames are `/`-separated, per-process root/CWD and mount namespace; bytes and Unicode policy documented. Windows names are UTF-16 in the NT API and translated carefully; never assume case-insensitive POSIX paths.
- Case preservation always; case sensitivity by filesystem/directory policy. Windows per-directory case-insensitive lookup requires normalized lookup indexes and collision rules; security code must not rely on casefolding that differs from runtime semantics.
- Symlink/hardlink traversal is permission-checked, bounded, no namespace escape. Mounts may be private to application containers.
- POSIX unlink-open-file and NT delete-pending/share-modes cannot be naively collapsed; implement distinct per-open semantics with compatibility locks/shares.
- One file can be concurrently opened through POSIX and NT personalities. Coherency, leases, byte-range locks, share flags and rename behavior MUST be specified and race tested.
- Durable `fsync`/flush contract: users can trust that successful flush means best-effort propagation to compliant hardware; document limitations in caches/controllers.

`NEXT-VFS-001`: no path traversal or mount boundary bypass through dot segments, symlink races, casefold, alternate data streams, device aliases, environment expansion, or NT `..`/reserved path syntax.  
`NEXT-VFS-002`: file I/O must be cancellation-safe; buffer ownership and partial reads/writes are documented.  
`NEXT-VFS-003`: locking under `rename`/`unlink`/`open` must not deadlock in parallel directory operations.

## 10.2 Volume layout

Reference GPT layout:

```text
GPT disk
  EFI System Partition (FAT32, UEFI boot and recovery assets)
  Next System A (read-only, verified deployment)
  Next System B (read-only, verified deployment)
  Next Data (encrypted where supported; users, apps, state)
  Optional recovery/tools partition
```

For development images, a simplified FAT+single-system partition MAY be used, clearly tagged non-production. Production must have signed update and rollback semantics; avoid rewriting user data during system A/B swap.

## 10.3 NextFS design specification

**NextFS** is the intended native copy-on-write, checksummed, snapshot-capable Next filesystem. It is an original project and must **not** be rushed into primary storage before crash-consistency validation. Initial 1.0 can use a rigorously implemented, limited existing on-disk filesystem format such as FAT32 on ESP and a conventional well-specified data filesystem **implemented in Rust by the team**, provided it passes the same durability/security tests; adjust 1.0 storage claims accordingly.

NextFS target features:

1. 4 KiB block baseline, extent-based file data and allocation trees; page/block size encoded in superblock.
2. COW metadata with transaction commit pointer; checksum all metadata, optional data checksums.
3. Generation-numbered superblocks, separate roots for inode tree, extent map, free-space tree, snapshot table and checksum tree.
4. Copy-on-write snapshots for user data and package states, quota accounting, compression optional and versioned.
5. Transaction commit ordering and flush barrier specification; torn-write detection and recovery read-only mode.
6. Directory B+tree or equivalent indexed structure, explicit UTF-8 byte namespace policy, extended attributes and security labels.
7. On-disk format version, feature bits for backward-compatible and incompatible features, CRC/checksum coverage and endianness documented with golden bytes.
8. Atomic rename and metadata-update transactions; operations define visibility pre/post crash; no silent partial overwrites.
9. Encrypted block layer preferred for disk encryption, independent of filesystem data format; key slots/recovery documented.
10. Comprehensive `fsck` verifier that cannot silently delete unknown data; dry-run and salvage options, image fuzz tests.

**NextFS transaction skeleton:** `reserve extents -> COW new data/metadata -> persist data -> persist new root -> flush -> atomically advance superblock generation -> release old extents after refcount reconciliation`. Ordering guarantees must be justified and fault-injection tested at every write boundary.

Disk failure tests: random power cut at every transaction stage, bad blocks, partial block writes, lost flushes, out-of-space metadata commit, full data volume, allocation failure, concurrent snapshots, corrupted directory entry, and recovery under low memory.

## 10.4 Backup and restore

Next Backup supports user-selected destinations, local snapshots, encrypted external backups, incremental manifests, file version browsing and verified restore. Backup is distinct from A/B OS update rollback. GUI must warn if snapshots reside on the same physical disk and therefore do not protect against device failure. Never erase external destinations without explicit choice.

---

# 11. Networking and remote access

## 11.1 Architecture

Write an original Rust `netd` networking service and limited kernel packet/device plumbing. Separate link-layer driver, packet queue, protocol engine, socket broker, firewall/policy, resolver, clock and certificate trust. Prefer userspace protocol processing for isolation until performance tests justify moving components.

The final stack includes Ethernet/virtio on certified devices, ARP, IPv4, IPv6 (including neighbor discovery and SLAAC), ICMP, UDP, TCP, DHCPv4 and DHCPv6 as applicable, DNS, mDNS under explicit local-network policy, TLS integration, loopback, routing, multicast, Wi-Fi management, a VPN interface, firewall controls, per-app network permissions and network observability. Device-supported hardware offload, multi-queue I/O and QoS are reflected in the capability catalog rather than implied for all adapters.

Protocol interoperability takes priority over a custom protocol. Use published IETF specifications and interoperability fixtures.

## 11.2 Socket interface

POSIX `socket`, `bind`, `connect`, `listen`, `accept`, `send`, `recv`, `poll/select/epoll` and Windows Winsock-like calls map to a common socket object while preserving error conventions and event semantics. Nonblocking operation, wakeup ordering, backlog behavior and shutdown-half-close must be tested.

Network state ownership: `netd` controls interfaces, IP addresses, DNS, route table and per-app outbound/listening permissions; application sockets are capability-gated. TCP implementation supports sequence/window validation, retransmission timers, congestion behavior, SACK, RTO policy, PMTUD/ICMP handling, and connection teardown; no "toy TCP" shipped as reliable production networking.

## 11.3 Network behavior and error handling

- IPv4/IPv6 DNS and address resolution, TCP loopback and internet communication, UDP reordering tolerance, fragmentation safeguards, IPv6 neighbor discovery, certificate validation, DHCP renewal and interface reconnects behave according to their published protocol contracts.
- The firewall denies unauthorized listeners on untrusted networks by default and mediates outbound connection rights according to selected policy.
- TCP remains correct under packet loss, duplicates, reordering, latency spikes and disconnect/reconnect; bounded retransmission and queue policies prevent untrusted peers from exhausting the system.
- Invalid network frames cannot cause parser memory corruption, unbounded allocation or unhandled privileged panic.
- Network Settings displays connection state, IP/DNS details, trusted/untrusted profile, proxy, VPN and per-app access.

---

# 12. Audio, media, input, accessibility and devices

## 12.1 Audio service

`audiod` handles device enumeration, per-app streams, sample format conversion, resampling, mixing, volume/mute, recording access, routing, latency reporting and device hotplug. Distinguish application voice/music classes; prioritize glitch-free audio over fragile low-latency promises.

The audio service supports verified physical HD Audio and USB audio devices on certified machines, together with virtual audio interfaces for VM profiles. Playback and capture operate simultaneously; use a real-time-safe audio callback path: no heap allocation or unbounded locking in mixer loop, buffers preallocated, underrun detection and precise timestamps.

## 12.2 Input and IME

- Input device events carry monotonic timestamps, device ID, action, keycode/usage code, modifiers, and seat ID; pointer coordinates in appropriate compositor space.
- Keyboard layouts, dead keys, IMEs, text composition, Unicode input, clipboard, key repeat, sticky keys, mouse acceleration, high-resolution scroll, touchpad gestures and accessibility events.
- Global hotkeys reserved by Prism are explicitly documented; applications must request privileged capture permissions. Password fields suppress sensitive key events/clipboard exposure.
- Support configurable per-app/desktop input focus, pointer confinement only with permission and safe escape key.

## 12.3 Accessibility requirements

Prism MUST offer full keyboard navigation, predictable tab order, accessible labels/roles/states, configurable font scaling, high contrast, reduced motion, focus indicators, text caret tracking, mono audio, key repeat controls and screen-reader infrastructure. WCAG is web-specific and does not alone certify an OS UI; maintain a desktop accessibility checklist with keyboard-only, low-vision and assistive-technology tests.

Screen reader, speech synthesis, braille, alternative input, accessible installer, OS recovery prompts, and magnifier are required for broad accessibility ambition. Next describes the supported assistive technologies and remaining compatibility limitations rather than claiming universal access without foundation.

## 12.4 Display and capture privacy

Only Prism's display service can access composited screen content; screen capture invokes an explicit portal with source preview and per-application consent. Camera/microphone indicators are visible and tied to enforced underlying access, not application self-report. Audio capture must disclose device and active app.

---

# 13. Graphics stack and Prism compositor

## 13.1 Final graphics capability profiles

Prism is a production graphical desktop with a real GPU-accelerated compositor on certified hardware, not merely a UEFI framebuffer demo. Every supported GPU has a truthful capability profile:

- **Software-safe mode:** CPU-rendered emergency session, single-display support as feasible, input, recovery and diagnostic tools even when the vendor GPU driver fails.
- **Desktop acceleration:** Native display modesetting, compositor-accelerated 2D composition, texture/surface buffers, fences, vsync and high-DPI output.
- **3D/API support:** Native interoperable 3D API implementations (Vulkan-compatible where provided), shader compiler and graphics memory isolation for the declared GPU families.
- **Advanced presentation:** Multi-monitor, HDR/color profile, VRR, high-refresh output, decode/encode or compute, individually declared only where supported and verified.

The user can see supported features and limitations in Display Settings. GPU device enumeration, a driver name or successful framebuffer scanout do not count as functioning 3D acceleration.

## 13.2 GPU architecture

`displayd` owns output configuration, composition schedule, buffer submission, damage regions, surfaces, cursor, capture authorization and user input dispatch. Driver/accelerator module controls command queues, GPU virtual memory, fences, interrupts, shader compilation and memory residency. Put risky shader/image parsing outside privileged core when possible.

**Compositor contract:** clients create surfaces, submit display lists or pixel/GPU buffers, receive frame callbacks and damage notifications; compositor never displays pixels from unvalidated or unmapped buffers; GPU fence lifetimes synchronize acquire/render/present. Surface metadata includes logical size, pixel size, scale, color space, alpha, z-order, input/opaque regions and security flags.

Prism's **native presentation protocol** is versioned and local IPC-based; external GUI protocols (X11, Wayland) require separate compatibility services and are not inherently supported by POSIX/Linux ABI. For Linux GUI compatibility, the final system provides a **Wayland protocol frontend**; X11 support requires a separately functional bridge or server, with declared supported protocol extensions. Both integrate foreign windows into the original Prism compositor. The compositor remains original.

## 13.3 Window semantics

- Toplevel, dialog, popup, tooltip, layer/system surface, lock screen, notification and drag icon are distinct surface roles.
- Focus/activation is mediated and cannot be stolen by background programs without explicit user gesture.
- Resize provides configure/ack handshake; stale generation cannot override a newer size; app-unresponsive indicator independent of input compositor.
- Multi-monitor positioning uses logical coordinates; fractional scaling re-rasterizes text at target scale and avoids blurry scaled screenshots by default.
- Fullscreen apps may request direct scanout only after strict permission/composition checks; overlays/privacy indicators must not be bypassed.
- Text rendering requires shaping, bidi, fallback fonts, grapheme-aware selection, emoji support, subpixel rules that avoid color artifacts, and accessibility representation.
- App crash does not crash `displayd`; shell crash triggers supervised Prism restart without kernel reboot.

## 13.4 Frame pacing and power

Input timestamp -> event dispatch -> app frame -> compositor composition -> scanout telemetry per frame. Target smooth 60/120/144/240 Hz where display/driver permit, with reduced refresh/no redraw when idle, occlusion culling and damage-only redraw. Define jitter percentiles, GPU busy time, missed-frame count, input-to-photon estimation and compositor CPU use. Avoid arbitrary animation if it decreases real responsiveness.

---

# 14. Prism user experience — visual and behavioral specification

## 14.1 Visual identity

Prism is visually recognizable as a premium modern successor to familiar Windows desktop patterns, but an **original design**. Principles: calm, legible, coherent, visually precise, responsive, power-efficient, keyboard-friendly, accessible, and uncluttered.

**Default design tokens (user-adjustable through themes):**

| Token | Dark default | Light default | Meaning |
|---|---|---|---|
| `background` | `#101114` | `#F7F8FA` | desktop base |
| `surface` | `#1B1D22` | `#FFFFFF` | app surfaces |
| `surface-raised` | `#24262C` | `#F0F2F6` | dialogs/popovers |
| `text-primary` | `#F7F8FA` | `#16181D` | body/headlines |
| `text-secondary` | `#B2B7C1` | `#5A606B` | secondary |
| `accent` | `#536DFF` | `#405CF4` | interaction/focus |
| `danger` | `#F06A73` | `#C93446` | destructive |
| `success` | `#5ECC96` | `#087A50` | confirmed state |
| `outline` | `#343740` | `#D8DCE5` | subtle boundaries |

Design system: 4-pixel spacing base; surface radii 8/12/16 px based on component; consistent 1 px borders and soft elevation; variable UI font with extensive Unicode support; icon family authored as stroke-consistent SVGs; 24 px baseline icon geometry; responsive layouts; 44 x 44 px recommended comfortable touch targets with smaller density optional. Font selection must satisfy redistribution licensing.

Prism transitions: target 80–140 ms for micro-interactions and 140–220 ms for panel/window changes; interruptible/reversible animations; reduced-motion sets near-instant transitions. Durations are targets, not arguments against responsiveness.

## 14.2 Shell layout

Desktop contains wallpaper, icons (toggle), windows, taskbar (Prism Bar), system status, workspace switcher and an accessible window overview. Background wallpaper must not be expensive to render continuously. Lock screen is a separate secure compositor role inaccessible to regular apps.

**Prism Bar:** center/left alignment, grouped running apps, pinned apps, single-click open/focus, middle-click new instance where supported, progress status, quick previews, pinned/recent jump lists, multi-display modes, auto-hide optional, notification badge only for real app events, tray/status area, clock, network/battery/audio indicators and power menu. Never insert ads or web suggestions into taskbar.

**Prism Home:** keyboard-searchable installed applications, all-apps directory, favorites, recent files with opt-in indexing, quick actions, local calculator/command dispatcher, accessible power/settings controls. Search must function entirely offline for local resources. Suggestions from the web only when separately enabled. No unexpected execution of arbitrary shell strings from search text.

**Prism Canvas:** floating and tiling modes, snapping zones, user-created layout presets, split resizing, per-workspace layout persistence, multi-monitor placement, window groups, keyboard focus traversal, maximize/minimize/fullscreen, virtual desktop overview, app restore when supported. Layout engine MUST respect minimum-size hints without pushing windows irretrievably offscreen; preserve restore positions and adapt to monitor removal.

**Prism Spaces:** named workspaces (Development, School, Gaming etc are only examples), move windows between spaces, per-space pinned app sets optional, restore sets of launchable applications and placements after permission. Do not forcibly restore sensitive windows across different user sessions.

## 14.3 Interaction standards

- Single click selects/activates according to control type; double-click opens files by default with configurable behavior.
- Right-click opens context menu; touch long-press equivalent. Menu placement avoids screen edge clipping.
- Window controls consistent placement according to selected UI preset, with large hit boxes and keyboard alternatives.
- All destructive actions disclose impact, provide confirmation where irreversible, and where possible offer undo.
- Errors explain next action in plain language, give a detailed diagnostic expansion, and avoid cryptic status-only dialogs.
- Startup must not display prompts for promotions, account sign-up, or default-app manipulation.
- Touchpad multitouch gestures are discoverable, rebindable, and should support motion accessibility preference.

## 14.4 Keyboard shortcuts (default)

| Action | Shortcut | Notes |
|---|---|---|
| Prism Home | `Super` | toggle launcher |
| Global Search | `Super + Space` | editable global command search |
| Next Settings | `Super + I` | consistent familiar shortcut |
| Prism Files | `Super + E` | file manager |
| Window switcher | `Alt + Tab` | ordered by recency |
| Task overview | `Super + Tab` | grouped by space |
| Lock session | `Super + L` | cannot be intercepted by untrusted apps |
| Snap left/right | `Super + Left/Right` | preview and cycle zones |
| Maximize/restore | `Super + Up/Down` | context dependent |
| Close window | `Alt + F4` | ask app to close; force quit separate |
| Move space | `Super + Ctrl + Left/Right` | switch workspace |
| Screenshot | `Super + Shift + S` | permission-aware capture UI |
| Terminal | `Super + Enter` | optional power-user shortcut |
| Help | `F1` | contextual help where applicable |

Use platform-aware display of `Super`, `Ctrl`, `Alt`, accessible labels, conflict resolver and user overrides. Secure shortcuts (lock/switch user) must not be interceptable by a normal app.

## 14.5 Core Prism applications

| Application | Final functional contract | Required behavior |
|---|---|---|
| Prism Files | sidebar, tabs, copy/move, create, rename, delete/trash, search, properties, device mounts | large files, permissions, failure recovery, Unicode paths |
| Prism Settings | categorized display, input, appearance, network, storage, apps, privacy, accessibility, system | changes persist/revert safely (display timeout) |
| Next Terminal | Unicode grid, selection, scrollback, profiles, tabs, shell integration | shell PTY, resizing, large output, secure paste |
| Next Monitor | CPU/memory/storage/network/GPU when available, processes, startup and services | not misleading when GPU counters unavailable |
| Next Store | signed catalog/local packages, install/update/remove, permissions | unsigned/revoked packages clearly identified |
| Next Security | app grants, encryption, session, firewall, update integrity, developer mode | security toggle enforces real policy |
| Next Backup | snapshots/backup targets, restore UI, verification | restore on independent test disk |
| Next Devices | named hardware, driver status, diagnostics | distinguish missing driver from hardware failure |
| Next Media | basic audio/video playback using supported codecs | truthful codec coverage/licensing |
| Next Photos | image browser/viewer, EXIF orientation, basic crop/export | safe parser, no overwrite without consent |
| Next Editor | UTF-8 editor with tabs, line numbers, basic highlighting | large-file limits explicit |
| Next Welcome | accessible onboarding, tips, recovery-key instructions | fully offline, skippable |

A full browser MAY be a separately licensed third-party app packaged by consent; developing an engine is not a Next 1.0 blocker. If `Next Browser` is advertised, supply a real interoperable browser engine with its own security process architecture and appropriate source/licensing compliance.

## 14.6 UI quality requirements

The final interface remains legible and usable in dark/light themes, 100%/125%/150%/200% scaling, 1920x1080/2560x1440/4K displays, single/dual display layouts, left-to-right and right-to-left text, complex-script shaping, high contrast and reduced motion. Controls expose consistent focus, hover and disabled states; window chrome remains readable on diverse wallpapers; error messages are informative and reversible actions remain discoverable.

# 15. Compatibility architecture — principles and boundaries

## 15.1 Distinct compatibility profiles

There is **no universal UNIX binary ABI** and no guarantee that implementing NT system calls alone implements Win32. Next exposes separate process personalities and compatibility services:

| Profile | Execution type | Externally observable semantics |
|---|---|---|
| `NEXT-NATIVE` | Native Next binaries | Stable native syscalls, SDK, capability and Prism interfaces |
| `POSIX-SOURCE` | Source-compiled UNIX applications | Portable C/POSIX behavior via Next libc, shell, linker and toolchain |
| `LINUX-X64` | Unmodified Linux x86-64 ELF binaries | Linux syscall numbers, process behavior, libc/dynamic loader and required environment |
| `NT-X64` | Windows x86-64 PE binaries | NT objects, user/kernel ABI interfaces, Win32 APIs and Windows environment |
| `NT-X86` | Windows 32-bit x86 PE binaries | Separate WOW-style user mode, 32-bit structures/calling conventions, thunking |
| `FREEBSD-X64` | Optional independently certified FreeBSD ABI profile | FreeBSD ELF loading, syscalls, libc and user-space assumptions |
| `DARWIN` | Optional independently certified Mach-O profile | Mach-O, Mach/XNU APIs and legally supported frameworks; POSIX alone is insufficient |
| `LINUX-ARM64` / `NT-ARM64` | Architecture-specific personalities | AArch64 kernel/ABI support plus correctly compiled native runtimes |

An advertised profile must be implemented end to end. The system must not display FreeBSD/Darwin/ARM64 as natively supported solely because they are architecturally contemplated. Process personality is determined at image load and dispatches the appropriate syscall numbering, signal/exception model, TLS, file paths, loader and runtime libraries. User-mode instruction translation or guest virtualization is separate from ABI compatibility.

## 15.2 Common runtime components

- **Image Broker** determines format, ABI/version, CPU ISA and dependencies; asks user for permission only when policy actually requires it.
- **Runtime Resolver** chooses correct loader, library namespace, registry/config view, font availability, graphics subsystem and profile.
- **Compatibility Container** gives per-app mutable state, filesystem aliases, registry-like store, quota and permissions; sharing is opt-in.
- **Service Bridge** presents GUI/windows, clipboard, audio, network and printing to application personalities through typed IPC.
- **Compatibility Database** stores tested program builds, required features, failures and verifiable provenance.
- **Capability Reporter** gives apps truthful versions/features, never lies by pretending to implement behavior that will corrupt data.

Installers that modify global registry/system locations must be virtualized per application or prompt before shared changes. No compatibility runtime receives unmediated access to raw disks or system services.

## 15.3 Interop invariants

- Shared file: one underlying VFS object, different path encodings and rules; handle-to-FD translation maintains rights and sharing constraints.
- Shared memory: segment permissions enforced by Helix; mapping flags adapted without violating W^X.
- Threads: host scheduler state shared, userspace runtime maintains compatible TLS and exception structures; no duplicated independent thread scheduler.
- Time: monotonic/wall clocks normalized, per-runtime high-resolution counters preserve expected units and overflow behavior.
- Processes: tool-visible process IDs and parent/child behavior adapted, `fork()` limited to POSIX-capable process state; NT applications generally use create-process semantics.
- Error: map only defined cases into `errno`, `NTSTATUS`, Win32 `GetLastError`, and Winsock errors; preserve information in structured logs.
- Security: no API path may escalate beyond caller capabilities simply because NT token emulates Administrator or POSIX user ID is zero inside container.

---

# 16. POSIX runtime specification

## 16.1 POSIX source/API target

Target POSIX.1-2024 (Issue 8) interfaces applicable to a desktop operating system and clearly document extensions or unimplemented optional interfaces. `libnextc` is a Rust-implemented libc exposing the required C ABI, headers, startup objects, dynamic library support and POSIX utilities. The implementation includes:

- Filesystem and descriptor operations (`open`, `openat`, `read`, `write`, `close`, `lseek`, `stat`, `fstatat`, `rename`, `unlink`, links, permissions and file descriptor flags).
- Processes and job control (`fork`, `execve`, `posix_spawn`, `waitpid`, signals, sessions, process groups, PTYs and termios).
- Virtual memory and synchronization (`mmap`, `munmap`, `mprotect`, POSIX shared memory, pthreads, mutexes, rwlocks, condition variables, semaphores and timers).
- Pipes, sockets, DNS (`getaddrinfo`), network polling (`select`, `poll`) and appropriate extension-specific readiness APIs.
- Clocks, random numbers, filesystem paths, environment, locale/character conversion, standard I/O, libc allocation, C11 runtime hooks and terminal/shell utilities.
- Correct error/errno behavior, buffer lengths, calling conventions, structure layouts, Unicode handling and multithreaded cancellation semantics.

Portable applications must be able to compile with the supplied Next C/C++/Rust toolchain without requiring a Linux kernel. UNIX/POSIX certification or trademark usage is contingent on satisfying the applicable external conformance process.

## 16.2 Processes and `fork`

Implement `fork` with COW address spaces; child has exactly one live calling thread, as prescribed behavior for multithreaded fork semantics. After a multithreaded fork, restrictions on async-signal-safe operations before exec must be reflected in libc and tests. File descriptor inheritance, signal mask, CWD, credentials, mapping flags and `waitpid` status must match POSIX behavior. `execve` atomically replaces address space but preserves/clears selected attributes according to documented rules and `FD_CLOEXEC`.

## 16.3 Terminal/PTY and shell

Terminals require PTY master/slave, line discipline (canonical/raw), echo control, signals for Ctrl+C/Ctrl+Z, resize signals, UTF-8, foreground process group, job control and basic ANSI/VT escape sequences. Implement a functional POSIX shell `nextsh` in Rust; for full script compatibility include parser/expansion/pipeline/redirection semantics tested against a reference conformance suite. Bash and Zsh are supported where source-ported implementations or functioning binary compatibility runtimes and redistribution licenses permit them.

## 16.4 libc implementation rules

- Each exported symbol has a test verifying behavior and `errno` on failures, type layouts and calling convention. Use header generation from one schema rather than manually drifting declarations.
- Locale and character conversion cannot just assume ASCII. Support UTF-8 locale, wide-character APIs, error handling for invalid sequences; add locale database iteratively.
- `malloc/free/realloc/calloc/aligned_alloc` provide thread safety, overflow checks, double-free handling where detectable, and performance regression suite.
- `printf/scanf`, string/memory, math, date/time, C11 atomics runtime hooks and dynamic loader exports are mandatory for general C/C++ source portability.
- Runtime C++ support needs a compatible C++ ABI runtime, exceptions/unwinding, libstdc++ or libc++ port, pthread integration, RTTI and compiler toolchain. Do not claim C++ binary compatibility until covered.

## 16.5 Observable POSIX application behavior

Standard portable file utilities, text-processing tools, POSIX shell scripts, C/C++ executables, multithreaded programs, `fork/exec` pipelines, network clients/servers, dynamically linked libraries and curses/TUI applications operate with standard POSIX-visible behavior. No Linux kernel underlies the POSIX source build or execution path.

---

# 17. Linux x86-64 binary compatibility

## 17.1 Binary ABI and execution environment

Execute selected unmodified **Linux x86-64 ELF** binaries on Next by implementing the **Linux x86-64 system-call ABI** and supplying a compatible Linux userspace runtime/library set. This is distinct from compiling POSIX applications for Next. A compatible Linux binary may expect glibc, musl, `/proc`, `/sys`, `/dev`, `/run`, kernel-specific `ioctl`s, signals and Linux-specific syscalls. Each expectation must be implemented or compatibility-scoped.

## 17.2 ABI gateway

- Linux x86-64 syscall instruction/register conventions are separate from Next's native table and dispatched only for Linux-personality tasks.
- Implement negative errno convention, correct restart/interrupt rules, `seccomp` behavior only if declared, `arch_prctl`, TLS, thread local `errno` through compatible libc, signal frames, process creation, `futex`, `mmap`, clock and network calls.
- The Linux syscall dispatch table identifies each supported interface as `implemented`, `unsupported` or `emulated`; a fake zero return is forbidden.
- Where syscall names match native calls, do not assume semantics match. Linux `clone` flag semantics, `epoll`, `eventfd`, `timerfd`, `signalfd`, `prctl`, `set_tid_address`, and robust-futex lists require exact target-specific behavior.
- Model `/proc` and `/sys` as virtual per-process/system filesystem endpoints exposing truthful Next information compatible with supported readers; never expose host kernel addresses.
- Build VDSO-compatible facilities for supported clocks/time functions and supply `AT_*` ELF auxiliary vector fields with correct meaning.

## 17.3 Loader/dynamic linker

- Parse ELF32/ELF64 only if appropriate to profile. The x86-64 ELF64 little-endian profile is the mandatory Linux binary target. Support for other profiles is separately declared.
- Map `PT_LOAD` regions, `PT_INTERP`, TLS, GNU_RELRO, GNU_STACK, program headers and auxiliary vector.
- Dynamic ELF requires a compatible dynamic interpreter, symbol versioning (notably `GLIBC_*` where used), lazy/eager relocation rules, TLS relocations, IFUNC if selected libraries require it, and copy-relocation treatment.
- `libnext-linux` userland may supply a compatibility libc built by the project; using a legally obtained third-party glibc/musl package as a runtime is subject to project policy and licensing and **does not turn Next into a Linux distribution**. The from-scratch default is to implement needed interfaces in Rust and generate compatible ABI wrappers, but this dramatically constrains available binary coverage.
- Reject unsupported ELF features with explanatory diagnostics, never map arbitrary RWX code on parse failure.

## 17.4 GUI applications

Linux X11/Wayland programs require more than Linux syscalls. A **Wayland frontend service** should accept Linux-client protocol messages and translate surfaces/input into Prism compositor objects. XWayland-compatible behavior requires a real X11 server and is a separate substantial undertaking, not implicitly available. The final system supports declared Linux GUI applications through functioning display-protocol compatibility and a published app compatibility catalog.

## 17.5 User-visible Linux compatibility metadata

Next Applications displays each imported Linux app's architecture, executable interpreter, runtime dependencies, app-specific filesystem view, desktop integration status, permission grants, required Linux ABI facilities and known limitations. A support catalog records exact application build identities, working features and incompatible APIs; it never treats successful ELF parsing as successful application execution.

# 18. NT/Win32 compatibility specification

## 18.1 Fundamental separation

NT-compatible execution requires:

1. **PE loader** (executable format).
2. **NT process personality** (kernel object, handle, memory, process/thread, I/O and exception semantics).
3. **NTDLL implementation** (native user-mode service ABI and data types).
4. **Win32 API libraries** (kernel32/kernelbase, user32, gdi32, advapi32 and much more).
5. **Windows-specific environment** (registry, services, activation/context assemblies, filesystem namespaces, appdata, fonts, locale, installers).
6. **Graphics/media and optionally COM/.NET** subsystems.

None is equivalent to the others. A PE binary that loads but immediately fails imports is **not** evidence of broad NT compatibility.

## 18.2 NT kernel object emulation

`NextNT` maps handles, object directories, file objects, events, semaphores, mutexes, sections, process/thread objects, waitable timers, IOCP and job objects to Helix objects. NT handle rights masks/duplication/inheritance must be modeled independently from Next native rights. NTSTATUS values and structured exceptions require precise mapping.

NT namespace examples: `\\??\\C:`, `\\Device`, `\\BaseNamedObjects` and DOS device symlink views; per-user and per-session isolation. Never allow a DOS device alias to escape sandbox roots by constructing a crafted NT path.

NT compatibility must implement `NtCreateFile` family semantics for sharing, desired access, creation dispositions, synchronous/asynchronous I/O, file offsets, sections and reparse-point/redirect protections as supported; map to host VFS with explicitly tested semantic adapters.

## 18.3 PE32+ loader requirements

- DOS/PE signature validation, COFF header, optional header, machine type, DLL characteristics, sections, relocation table, import/export table, forwarded exports, delay imports, TLS directory/callbacks, resources, exception/unwind directory.
- Resolve imported DLLs from a fixed sanitized per-container search path. Avoid current-working-directory DLL injection by default and log policy deviations.
- Validate image and section bounds with overflow-safe math, protection flags and overlapping-region constraints. Apply relocations only for supported relocation types; unknown required relocation -> fail.
- Implement dynamic loader refcount, DLL attach/detach, thread notifications and synchronization semantics; loader-lock rules prevent re-entrant deadlock.
- Support `.exe`, `.dll` and console programs as tested; drivers `.sys` are **explicitly unsupported**.

## 18.4 Windows user-mode API surface

The NextNT runtime implements the Windows interfaces required by its supported application ecosystem, including PE/NTDLL startup, CRT support, file/process/thread/memory operations, events and completion ports, `kernel32`/`kernelbase`, `user32`, GDI/GDI+, registry (`advapi32`), Winsock, common controls, shell functions, COM/OLE, Unicode/locale, clipboard, printers, services emulation where appropriate, multimedia and supported Direct3D translation. Framework support such as .NET is implemented or integrated as a separately identified capability. The Win32 runtime uses Helix objects and Prism surfaces underneath, preserving documented Windows-visible semantics. Missing required imports must produce actionable compatibility diagnostics instead of causing silent failure.

## 18.5 Registry and settings

Registry keys/values with type fidelity (`REG_SZ`, `REG_DWORD`, `REG_BINARY`, multi-string etc), per-container hives, atomic persistence, permissions, notifications, WOW64 view only when x86 profile enabled. Avoid conflating UTF-16 strings with UTF-8 host values; preserve lengths, terminators and invalid-string handling. Registry modifications are not allowed to mutate Next native system settings except through authorized mediation.

## 18.6 Windows GUI model

Implement `HWND`/window class registration, message queues and dispatch, paint/invalidate, parent/child windows, focus and keyboard/mouse events, modal dialogs, DPI rules and GDI drawing. An `HWND` is an emulated Windows object mapped by user-mode runtime to Prism surfaces; Prism still owns focus and security. Win32 window chrome can receive Prism-consistent default decoration unless application-drawn controls require exact behavior.

## 18.7 Compatibility environment

Each Windows app gets isolated virtual drives (`C:`, optional `D:`), application data, registry and runtime libraries. Shared user Documents/Downloads access via permission broker, not blanket drive mapping. Windows executable installers must have restricted filesystem/network rights until explicitly granted; do not equate 'installer' with 'trusted administrator'. Provide diagnostic UI with runtime version, missing imports, last NTSTATUS, event trace, and app-specific workarounds.

## 18.8 Observable compatibility correctness

Public API outcomes must match documented behavior for filesystem sharing, synchronous/asynchronous I/O, process creation, thread-local storage, Unicode paths, NTSTATUS/Win32 errors, registry values, event/wait behavior, window messages, repaint/resize, DPI scaling, clipboard, dialogs and GDI text/image rendering. Compatibility means correct user-observable behavior, not simply launching a process or reaching its entry point.

Next's compatibility catalog distinguishes executable parsing, process startup, interactive functioning, feature completeness and verified app usability. Runtime errors name the missing interface, DLL, architecture or permission where this can be determined reliably.

## 18.9 Game, graphics and DRM compatibility boundaries

Game support is certified per exact title/build; universal game compatibility must not be claimed. Full Direct3D compatibility demands a GPU driver stack plus shader translation (Direct3D to Vulkan or another native backend), significant Windows graphics APIs, window/input/audio integration, anti-cheat cooperation and DRM compliance. Virtualization may be offered with a properly licensed Windows guest as a separate **fallback** and identified as such; a VM does not meet native binary compatibility acceptance.

**No arbitrary NT kernel driver execution.** Windows anti-cheat operating inside kernel mode is not supported without specifically engineered vendor cooperation and a compatible secure API model. Mark affected games unsupported, not "supported with admin mode".

---

# 19. Native Next SDK and application model

## 19.1 Stable contracts

The Next SDK provides C-compatible stable system ABI definitions and a Rust-friendly wrapper. Public Rust source-level APIs may evolve semantically using SemVer; cross-process/wire/FFI boundaries do not expose compiler-private Rust layouts. Native applications packaged as `.nextapp` bundles: manifest, architecture-tagged executable, shared libraries, resources, locale catalogs, app icon, signature and declared permissions.

Bundle example (conceptual manifest):

```toml
schema = 1
id = "org.example.editor"
name = "Example Editor"
version = "1.0.0"
minimum_os = "1.0"
executable = "bin/example-editor"
architectures = ["x86_64"]

[permissions]
filesystem = "user-selected"
network = "none"
clipboard = "foreground-only"

[desktop]
categories = ["development", "utility"]
icon = "assets/icon.svg"
file_types = ["text/plain"]
```

Signature is over canonical package manifest + full content tree; package manager enforces size/path traversal and signature checks before extraction. Permission UI shows only requested permissions and actual runtime grants.

## 19.2 SDK modules

- `next::process`, `next::handles`, `next::fs`, `next::io`, `next::net`, `next::time`, `next::security`, `next::ipc`, `next::logging`.
- `prism::app`, `prism::window`, `prism::layout`, `prism::controls`, `prism::text`, `prism::notifications`, `prism::dialog`, `prism::accessibility`, `prism::canvas`.
- `next::audio`, `next::video`, `next::devices` behind capabilities.
- Developer CLI: `next dev`, `next build`, `next test`, `next package`, `next sign`, `next run`, `next doctor`, `next publish` (publishing requires explicit credentials/confirmation).
- Templates: `console`, `gui`, `service`, `driver-service`, `plugin`, with manifest, API metadata, examples and optional project test setup.

## 19.3 App life cycle

`INSTALLED -> STARTING -> RUNNING -> SUSPENDING -> BACKGROUND -> CLOSING -> EXITED` with crash/relaunch paths. All apps receive activation intent, file-open intents, and shutdown events. Save/restore state is app-controlled and explicitly bounded; no global magical process serialization guarantee. Resource shutdown attempts graceful closure first, then forced termination only after user/admin approval or policy.

## 19.4 Library/toolchain portability

Rust `std` port for Next is a separate deliverable requiring target ABI, allocator, threading, clock, env, file, process and socket mappings. Until available, Rust applications are `no_std` or depend on a Next-specific runtime library. C/C++ targets require a libc and linking/runtime toolchain. Clearly publish target triples and supported toolchain versions; never imply arbitrary `cargo build` packages compile unchanged before `std` and dependencies exist.

# 20. Packages, installer, distribution, updates and rollback

## 20.1 Package types

- `.nextapp`: standalone signed GUI or CLI app, optionally with vendored application-specific libraries, manifest and resources.
- `.nextpkg`: system-level signed bundle of services, drivers or runtime libraries; elevated authority required.
- `.nextos`: immutable signed OS deployment image with hash manifest, kernel/build ID, compatibility requirements, rollback metadata.
- Windows PE installers (`.exe`/`.msi`) are **foreign inputs** handled by NT runtime sandbox; MSI processing requires its own emulation and is not automatically part of PE execution.
- Linux ELF packages/archives are **foreign inputs**; support a deliberate container/runtime import interface. Never blindly run foreign maintainer scripts with system capabilities.

## 20.2 Package manager algorithms

Package signatures verified before metadata is trusted; repository metadata signed with rollback protection, expiry policy, rotation mechanism and pinned trust root. Package dependency solver must detect cycles, architecture mismatch, incompatible ABI version and unresolvable constraints. Transaction steps: stage -> verify -> resolve -> policy prompt -> commit -> activate -> clean old state, with durable journal and rollback of incomplete transaction. Concurrent package operations serialized or transactionally isolated. Resume downloads by hash; support local offline packages and offline mirrors.

`NEXT-PKG-001`: install cannot escape destination root via archives, symlinks, `..`, absolute paths, case aliases, hardlinks, special files or oversized metadata.  
`NEXT-PKG-002`: revoked/invalid/mismatched signatures block privileged installation; developer override clearly isolated.  
`NEXT-PKG-003`: dependency/version migration failures do not corrupt pre-existing installed apps.

## 20.3 Installation modes

**Live environment:** bootable USB image with Prism, limited but real hardware diagnostics, network support where available, partitioning preview and installer.  
**Install new disk:** choose disk, show capacity and exact destructive changes, create GPT/ESP/system A/B/data.  
**Install alongside:** discover existing partitions, validate available space and boot entries, shrink only when data-safe implementation supports it, never overwrite unknown filesystem. If safe resizing is unavailable, clearly refuse rather than guessing.  
**Advanced:** custom partitions and encryption with validation.

Steps:

1. Language/region/accessibility and keyboard selection.
2. Hardware/storage check including unsupported-device warnings.
3. Optional network connection; offline path remains functional.
4. Disk selection with clear volume names, backup warning and explicit destructive confirmation.
5. Optional or default-enforced encryption policy; recovery key presentation and verification.
6. Local username/passphrase, with supported additional accounts and user-switching controls.
7. Time zone, appearance, privacy settings (no opt-out trick design).
8. Install with progress based on actual bytes/stages, not fake animation.
9. Final integrity check, set UEFI boot entry, offer restart and recovery instructions.

Installer must not send plaintext encryption keys/passwords into logs, and must redact personal data in support exports. Provide accessible keyboard-only installation path, meaningful errors and retry/resume for recoverable steps.

## 20.4 Update system

`updated` validates signed catalog and target OS image; downloads/stages to inactive partition/deployment; validates storage/capacity and boot metadata; switches boot attempt; marks success only after boot health and essential services ready; otherwise automatically returns to last known good. User can inspect release notes, security changes, free disk requirements and rollback availability. Deferral and reboot behavior must be respectful and never arbitrarily terminate active work without clear policy and warning.

Invalid signatures, truncated downloads, stale catalogs, wrong architecture, insufficient storage, power interruptions, failed new-system initialization, missing Prism, corrupted rollback metadata, clock skew and network outages must result in safe and clearly explainable behavior with no unapproved deletion of user data. Where the last known-good deployment is intact, the boot system can restore it without first loading the failed deployment.

## 20.5 Licensing and distribution

Next first-party components ship under expressly declared project licenses with source or redistribution obligations satisfied. Each imported third-party dependency needs a license record and redistribution review; copyrighted standards are referenced, not pasted wholesale. Never reuse proprietary Windows source, undisclosed reverse-engineered materials, or vendor firmware without distribution rights. Bundled codecs, fonts, shaders and encryption libraries also need licensing and patent reviews where relevant.

---

# 21. Performance, power, observability and reliability

## 21.1 Observable quality and performance characteristics

Performance targets apply to a named certified hardware profile with exact firmware, GPU driver and power settings. They are engineering product requirements, not claims that an implementation already reaches them.

| Observable behavior | Intended result | Conditions |
|---|---|---|
| UEFI handoff to usable login | Target under 12 seconds | Reference NVMe PC; excludes firmware startup |
| Local application/file search | p95 under 100 ms | Indexed, local queries without network |
| Interactive visible response | p95 under 50 ms | Typical UI action through to rendered feedback |
| Prism composition | Smooth 60/120+ Hz as supported | Certified accelerated GPU and display |
| Idle resident memory | Target under 1.5 GiB | Lightweight default desktop, well-defined accounting |
| Storage and file integrity | No silent corruption | Correct flush, power-loss handling, checksums |
| Recovery from failed system update | Automatic last-known-good boot | Supported deployment and recovery partition |
| Application crash isolation | No kernel or unrelated-app crash | Ordinary app process failure |
| Sleep/wake and battery | Stable and measurable | Certified laptop hardware |
| Accessibility and offline UX | Fully operational core desktop | No online account or network required |

No benchmark result can substitute for correctness, data integrity, security or appropriate behavior on supported hardware. In particular, avoid claiming a system can be mathematically guaranteed free of all vulnerabilities.

## 21.2 Structured tracing

Single event format with monotonic timestamp, CPU/thread/process, service, category, event ID, latency/size/status, correlation ID and sensitivity label. Enable low-overhead ring buffers and configurable categories for boot, scheduler, VM faults, syscalls, filesystem, GPU frames, IPC, network, power, packages, compatibility runtime and crash events. Default privacy redacts filenames, arguments, personal identifiers and secret material except via explicitly authorized local diagnostic mode.

`next trace capture`, `next trace summary`, `next doctor`, `next logs`, and `next status` CLI operations must work on offline installations. Crash bundles include exact kernel/app build IDs and machine capability profile. A support export defaults to local file with clear review before sharing.

## 21.3 Reliability engineering

- Watchdog for essential services, but restart/backoff and circuit breakers prevent infinite hot loops.
- Memory pressure manager attempts cache reclaim and process-targeted OOM recovery before kernel panic.
- Storage exhaustion keeps enough emergency capacity for account login, logs and safe shutdown.
- Thermal/power emergency policy follows certified hardware sensors; unsafe drivers must fail closed.
- Suspend/resume is a complete state machine across devices, timers, display, networking, audio, encryption and user session. Treat it as a release-blocking integration feature on laptops.
- Kernel panics attempt dump-to-preallocated crash region where safe; never risk data corruption by arbitrary panic-time writes.

---

# 22. Finished Prism application suite and shared platform behaviors

This section specifies the complete user-visible system shipped with Next 1.0. These are products that must perform real operations through Helix and system services, not static demonstrations. First-party desktop applications use the same Prism design language, permission model, keyboard conventions, theming, localization and accessibility APIs.

## 22.1 Prism Home, Search and notifications

- **Prism Home** lists pinned, recently used and all installed apps; supports folders/categories, keyboard-only navigation, drag-and-drop ordering, instant launch, application actions, shutdown/reboot/sleep/lock and multi-user session switch. The search box is omnipresent but does not require a cloud connection.
- **Prism Search** indexes authorized local filenames, metadata, application names, settings and explicit opt-in content sources. It understands commands, arithmetic, unit conversion, direct file-opening actions and exact application launches. Results indicate their source and required permissions. Indexing has per-location inclusion/exclusion, pause, rebuild and storage controls. Sensitive folders, password vaults and private sessions are excluded by default.
- **Notifications** are grouped per app with visible origin, dismissal, priorities, sound policy, quiet hours, per-app allow/deny controls, history, lock-screen redaction and actionable buttons. The application must have permission to send persistent or high-priority alerts. Dismissing a notification never silently performs a destructive action.
- **Quick Settings** expose Wi-Fi, Bluetooth, volume, output/input devices, display brightness, battery/power mode, focus mode, screen recording status, night light and theme. Indicators reflect the true hardware/system state and never use decorative fake toggles.
- **Prism Bar** provides pinned/running apps, grouped previews, progress indicators, per-window context actions, tray menus, task-switching, multi-monitor rules, notification center and the clock. App indicators cannot impersonate privileged system warnings.

## 22.2 Prism Files and the storage experience

- File manager includes path/address entry, tabbed and dual-pane browsing, breadcrumbs, tree navigation, type-aware previews, sorting, grouping, hidden files, file properties, volume properties and network locations.
- Standard operations: copy, move, rename, paste, links, duplicate, trash/restore, permanent delete with confirmation, drag-and-drop, compress/extract common archive formats, hashes/checksums, conflict resolution and permissions editing.
- Large file operations show truthful bytes processed, ETA when calculable, throughput, pause/resume where safe, skip/retry, per-file errors and atomicity guarantees. Surviving a compositor crash must not destroy an in-progress copy job.
- Search across authorized volumes supports Unicode and per-volume case-sensitivity rules. Windows compatibility environments expose mapped drives without granting full-disk access.
- User-facing storage settings include disks, partitions, space breakdown, snapshots, encryption state, backup destinations, external volume mount/eject and filesystem-health indicators. Unsafe eject prompts must explain outstanding writes.

## 22.3 Next Settings: exhaustive top-level navigation

Settings must be unified, discoverable through Prism Search, searchable within the application, reversible where possible and apply only to the chosen account or device scope. Required sections:

1. **System:** About, host name, architecture, software version, hardware information, language, time, date, startup/session options, logs, recovery and updates.
2. **Display:** Monitor arrangement, resolution, refresh rate, scaling, orientation, HDR/color profile where supported, night light, brightness and multi-monitor behavior.
3. **Appearance:** Dark/light/automatic theme, accent, wallpapers, fonts, density, animations, taskbar, Home, gestures, Canvas and Spaces.
4. **Sound:** Playback/recording devices, default routes, per-app volume, microphone permissions, sample rate where meaningful, latency profile, accessibility audio.
5. **Network & Internet:** Ethernet, Wi-Fi, VPN, proxy, DNS, hostname, IPv4/IPv6, firewall, hotspot where supported, connection metering and diagnostics.
6. **Bluetooth & Devices:** Pairing, device trust, USB/PCI device inventory, keyboard, pointing devices, touch, stylus, printers, scanners and firmware/device status.
7. **Apps:** Installed software, launch permissions, file associations, defaults, autostart, per-app storage/cache, native/Linux/Windows environment details and uninstall/repair.
8. **Accounts:** Local users, sign-in credentials, lock settings, optional online service identities, session management and family/accessibility policies when supported.
9. **Privacy & Security:** App permissions, sandbox rules, disk encryption, secure boot, certificates, firewall, developer mode, audit events, optional telemetry and data export.
10. **Storage & Backup:** Volumes, snapshots, cloud/external backup integrations when configured, quotas, cleanup, restore points and external media policy.
11. **Accessibility:** Screen reader, captions, keyboard/motor support, contrast, scaling, magnifier, reduced motion, speech, pointer enhancements and language-specific input.
12. **Developer:** Runtime versions, Linux/NT compatibility profiles, terminals, virtualization/container settings, debug policy, package signing and developer API status.

Each setting has a clearly specified source of truth, valid-value range, persistence scope, permission requirement, change notification and error behavior. A display mode change automatically reverts if it becomes unusable and is not confirmed.

## 22.4 Next Terminal, Monitor, Security, Backup and Store

- **Next Terminal:** GPU-accelerated when possible but reliable software fallback; PTYs, UTF-8 grapheme clusters, bidi handling where applicable, true color, links, mouse support, scrollback, search, tabs, split panes, profiles, safe paste, key rebinding, font ligatures optional, terminal recording with explicit consent and integrated native/POSIX/Linux/NT command environments.
- **Next Monitor:** Process/service hierarchy, CPU by core, memory/residency, virtual-memory maps (authorized), disk I/O, network traffic, GPU engines where supported, thermal/power and application responsiveness. Supports resource limits, priority hints, graceful stop, force terminate and diagnostic capture. No counterfeit counters when telemetry is unavailable.
- **Next Security:** All app grants, device encryption, update integrity, certificate trust, session policies, network firewall, secure-boot status, security history and policy exceptions displayed in language users understand.
- **Next Backup:** System deployment rollback and separate user-data backup/restore; scheduled and manual backup, destination encryption, file/folder selection, integrity verification, point-in-time browsing and disaster-recovery UI. OS rollback does not silently rewind user documents.
- **Next Store:** Search, categories, developer details, verified publisher identity, permissions, version history, install/update/remove, restore of previous app release where package metadata permits, offline local packages and safe handling of dependencies. A store account is optional; independent installation and sideloading remain supported with appropriate warnings.
- **Next Welcome/Help:** Offline product tour and contextual help, device setup and migration guides, keyboard shortcut reference, accessibility onboarding and an honest hardware/compatibility report.

## 22.5 Essential first-party utility applications

The shipped base image must include functional image viewing and basic editing (**Next Photos**), video and audio playback (**Next Media**), a text/code editor (**Next Editor**), archive utility, calculator, screenshot/annotation tool, clipboard manager with privacy controls, fonts viewer/installer, PDF/document viewer using a properly licensed renderer, print queue UI, network diagnostic utility, and settings-accessible help. Complex professional creative editors are not required to count as a complete OS, but their file dialogs, media devices and graphics APIs must have published support contracts.

A modern interoperable **web browser** is a required user-facing application. Its engine may be an independently maintained, legally redistributable third-party component: Next's kernel, display server and Prism remain original. Browser security updates are independent of OS kernel updates, and browsing, cookies and credentials are not tied to a Next online account. Building a new browser engine is not a requirement.

# 23. Desktop interactions, accessibility and internationalization

## 23.1 Unified interaction contract

- The same app appears coherently in task switching, dock previews, global search, notification permissions, task management, window snapping and file associations regardless of its Next/POSIX/Linux/NT origin, subject to truthful runtime limitations.
- Copy and paste work across application personalities for text, Unicode, PNG/raster images, URLs and declared file references. Rich text, HTML and custom formats use explicit translators with size and trust limits. Clipboard history can be disabled; passwords and secret markers must not be retained by default.
- Drag and drop across apps supports files, text and images via rights-limited, expiring data offers; cross-runtime transfers cannot bypass file permissions.
- Open/save file dialogs share one consistent Prism design and broker access to user-selected resources. A Windows application's `C:\\Users\\...` view and a Linux app's `/home/...` view can map to the same authorized file with correct respective path semantics.
- Window switching, maximize, full screen, per-monitor placement, app restart, keyboard layouts, accessibility trees and theme changes must behave predictably regardless of runtime origin. Compatibility failures are reported as compatibility failures, not silently concealed.
- Users may customize shortcut schemes (Windows-like, developer-focused, accessible). Reserved security shortcuts cannot be captured or overridden by unprivileged apps.

## 23.2 Accessibility requirements

- Full keyboard navigation, programmatically exposed control names/roles/states, robust focus movement, screen-reader-compatible events and live regions across the shell, login, installer and all native system apps.
- System screen reader and magnifier; configurable high contrast, accent contrast, large text, touch targets, reduced transparency/motion, color filtering and visual notification alternatives.
- Sticky/filter/toggle key behavior, adjustable key repeat and pointer acceleration, dwell controls where supported, on-screen keyboard, configurable gestures and switch-access hooks.
- Live captions and voice control may use local models if shipped, but core accessibility must work offline and never require remote AI.
- Accessibility policy applies to secure prompts and authentication surfaces; compatible foreign applications may require translation/adaptation and are labeled if unsupported.

## 23.3 Localization and typography

- Full UTF-8 internal text model, correct UTF-16 conversion for Win32, Unicode normalization policy without data loss, grapheme-aware cursor/selection, complex-script shaping, fallback fonts, emoji and bidirectional text.
- Locale-aware date, time, units, currency formatting, plural rules and collation; independent interface language, regional format and keyboard layout.
- Pluggable IME framework for CJK composition and other scripts; right-to-left shell layout and translated accessibility descriptions; bundled locale/timezone data updated with the OS.
- No security-sensitive Unicode spoofing through invisible filename controls without clear display warnings; maintain underlying exact path bytes/characters.

# 24. Virtualization, containers, developer environment and system interoperability

## 24.1 Next Hypervisor and VM manager

Next must be able to host hardware-accelerated virtual machines on CPUs with supported VT-x/AMD-V or ARM virtualization extensions. The native VM service owns isolated guest memory, virtual CPUs, interrupts, virtio devices, disk images, virtual networking, snapshots and VM lifecycle. Untrusted guest code never runs at Helix kernel privilege.

A Prism **Virtual Machines** application provides VM creation, import/export, CPU/RAM/disk allocation, ISO boot, snapshots, console/display, guest shutdown, virtual networking and USB passthrough when safely supported. Running a Windows guest requires an appropriately licensed Windows installation; this is explicitly distinct from NextNT native API compatibility.

## 24.2 Next Containers

Containers are isolated Next processes with their own process/resource views, mount namespaces or equivalent object-scope isolation, virtual network namespaces when selected, filesystem overlay, per-container policy and CPU/memory/I/O quotas. Linux OCI image/runtime compatibility requires the relevant Linux ABI, filesystem and namespace semantics; it must not be described as supported solely because a tarball can be unpacked. No root inside a container implicitly grants Helix administrative privilege.

## 24.3 First-class development tools

Next ships a native terminal, shell, SSH client/server with secure defaults, key management, Git-compatible version-control tooling via lawful port or implementation, compilers/build tools as installable SDK packages, debugger, profiler, package manager and code-signing tools.

Supported development targets include Rust, C and C++ on the native Next ABI, with a consistent SDK, target triples, runtime libraries, linking, debuginfo, symbols, package manifests and reproducible application builds. Language runtimes such as Python, JavaScript/TypeScript, Java and Go are supported where ported or via compatible Linux binaries; availability must be described per runtime and architecture.

The developer experience includes a local process inspector, trace viewer, syscall/object inspection under appropriate permissions, compatibility error diagnostics, structured logs and documented native GUI/API examples. Developer mode relaxes only explicitly user-approved development policies and never disables kernel memory safety or universal isolation.

## 24.4 Local and remote productivity

- SSH/SFTP, SMB-compatible file sharing or an interoperable file-service bridge, network drives, printing through IPP where supported and remote desktop access with consent, authentication and configurable unattended access policy.
- Screen capture, recording and audio capture brokered through Prism and visibly indicated, with protected-session redaction and application/window selection.
- Standard networking capabilities include IPv4/IPv6, DHCP, DNS, TLS, proxy, VPN integration, local discovery policies and firewall profile selection.
- Optional user-controlled sync of files, settings and preferences must be encrypted in transit, clearly permissioned and designed with conflict reporting. Next core features must remain fully functional without an online account.

# 25. Gaming, graphics, media creation and high-performance workloads

## 25.1 Gaming environment

Next Gaming is an integrated mode in Prism, not a separate second operating system. It provides a controller-navigable launcher, game library including imported user-owned installations, executable compatibility profiles, full-screen/windowed/borderless switching, frame timing and FPS overlay, shader/cache management, graphics settings, controller remapping, per-game performance/power profiles, input latency trace and optional game recording.

**Compatibility is implementation-defined and testable**. An executable is not a supported game merely because the PE loader accepts it. A supported title must have functioning rendering, audio, input, storage, save data and its declared online requirements. Anti-cheat requiring unsupported Windows kernel drivers is not represented as native-compatible. Third-party DRM and online service behavior are outside Next's unilateral control.

## 25.2 Graphics APIs and professional applications

- Native hardware-accelerated graphics interface with Vulkan-compatible support on certified GPUs; native Prism surface and presentation interfaces available to Rust/C++ developers.
- Windows graphics API compatibility services for DirectDraw/Direct3D generations on implemented API versions using independently developed translations or properly licensed dependencies; correctly implement shader behavior, synchronization, resource lifetime and presentation semantics.
- Video decode/encode acceleration for codecs and GPUs where driver capabilities and licensing permit, with a CPU fallback when feasible. Media capture, color conversion and audio/video timestamps must be exposed to native APIs.
- Per-display ICC/managed color where supported, SDR/HDR, HDR metadata, fractional scaling, VRR, high-DPI, multiple refresh rates and calibrated display profiles. Unsupported output capabilities must not appear as enabled.
- GPU compute and specialized professional device APIs are optional profile-specific capabilities unless the reference device explicitly supports them.

## 25.3 Audio and latency

The native sound server must provide stable playback/capture, per-app and per-device volume, hardware mixing negotiation, exclusive/low-latency stream policies, microphone privacy, loopback recording with permission, MIDI where compatible devices are available, Bluetooth audio where supported and device-change resilience. A misbehaving app cannot monopolize the audio device indefinitely.

## 25.4 Power profiles

Battery Saver, Balanced and Performance profiles manage scheduling hints, background limits, display refresh policies, brightness behavior and hardware power states within safe thermal limits. Power management never changes kernel authorization rules. Sessions survive suspend/resume on certified laptops without lost audio routing, permanent dead input devices or corrupted volumes.

# 26. Complete binary compatibility and application-environment coverage

## 26.1 Executable recognition

The launch broker detects executable format, architecture, dynamic loader, dependencies and declared privileges, then chooses exactly one execution personality. Native Next binaries use Next's stable ABI; Linux ELF binaries use Linux ABI services; Windows PE binaries use NextNT; foreign-architecture programs require a separately identified CPU translator or VM. File extension alone must not control execution behavior.

## 26.2 UNIX is not one binary format

The term **UNIX applications** includes multiple ecosystems. Next 1.0 must expose a POSIX-conformant design target for source-ported UNIX software and a Linux ELF binary personality. Additional FreeBSD ELF and Darwin Mach-O personalities are individually named and independently specified if delivered. No single POSIX syscall layer makes native binaries from Linux, FreeBSD, macOS, Solaris and other systems mutually executable. The OS must not claim universal native support unless each relevant API, loader and framework actually exists.

Linux graphical programs need compatible display-protocol services (Wayland and, for X11 applications, a functioning X11 compatibility server) and an appropriately compatible userspace library environment. Linux applications using GPU ioctls and specialized container APIs may require substantial additional emulation.

## 26.3 Windows compatibility breadth

NextNT encompasses x86-64 PE applications and, when the 32-bit runtime and instruction support are implemented, x86 PE32 applications. It must account for ntdll, kernel32/kernelbase, user32, GDI, registry, COM/OLE, shell services, Winsock, fonts, locales, MSVC/C runtime conventions, common controls, multimedia, Direct3D interoperability and optional framework compatibility including .NET.

Windows applications should receive independent install environments by default, preserving shared-file access only where the user grants it. The OS exposes missing API/import information, workarounds, release notes for compatibility fixes and per-app runtime configuration in a comprehensible interface.

`*.sys` Windows drivers, proprietary DRM modules, hardware activation servers and uncooperative anti-cheat implementations cannot be made fully compatible through the Win32 personality alone. They may remain unsupported even when the surrounding game/application works. Users must not be led to believe otherwise.

## 26.4 Windows + UNIX coexistence

- Files can be transferred between runtime personalities using authorized shared VFS objects; rename, locking, deletion, Unicode, case sensitivity and symlink behavior must preserve the semantics promised by each side.
- Network sockets are mediated by one native networking service; each runtime gets correct API-level error codes and socket semantics.
- Clipboard, drag/drop, audio, display, printing, open/save dialogs, screenshots, notifications and file associations integrate into Prism across compatible apps.
- Apps cannot gain privileged access simply by presenting an NT Administrator token or UNIX UID 0 inside an isolated compatibility environment.
- Compatibility properties and failures are inspectable through **Next Applications** without editing environment variables or registry files by hand.

## 26.5 Transparent support designations

Every foreign application may be classified as **Verified**, **Compatible**, **Limited**, **Runs in VM**, or **Unsupported**. The classification describes specific builds, processor architecture, dependencies, drivers, and exact supported features; a generic application name without version detail does not certify all releases. Users can still launch unsupported software at their own risk under unprivileged policies, unless blocked by security requirements.

# 27. Device classes, settings completeness and hardware experience

## 27.1 Required classes on supported hardware

- ACPI, timers, APIC, SMP, PCI/PCIe, MSI/MSI-X, IOMMU and DMA isolation, NVMe and basic block devices, GPT and EFI system partitions.
- USB xHCI with hubs, hotplug, HID keyboard/mouse, external storage and declared USB Audio Class device support.
- Ethernet, selected Wi-Fi adapters, Bluetooth on specified controllers, secure network onboarding, airplane mode and connected-device status.
- At least one certified hardware-accelerated graphics family and an explicit supported multi-monitor configuration; software output as a safe troubleshooting fallback.
- HD Audio or supported USB audio, microphones, webcam/video capture as declared, printers/scanners on supported protocols.
- Laptops: batteries, charger state, lid, suspend/resume, keyboard backlight where exposed, touchpad, high-DPI scaling and thermal throttling feedback.
- Firmware/device inventory, device trust and errors in Prism Settings, with driver restart/isolation where architecture supports it.

## 27.2 Hardware support is explicit

Next 1.0 is a real-hardware OS with a published support matrix. A certified device profile includes vendor/model IDs, firmware versions, CPU/ISA, GPU, graphics tier, disk controller, audio/network/input, power/sleep and applicable limitations. Unsupported devices appear honestly in Settings with actionable errors. Next never implies that all commodity PC peripherals or vendor-specific drivers work because a generic PCI/USB interface exists.

## 27.3 Multi-user computing

Multiple local users have separate home directories, encryption/session credentials, privacy settings, application permissions, taskbar layout, notifications and processes. Fast user switching preserves isolation, and administrative elevation uses secure UI and short-lived authority. Guest sessions may be enabled by an administrator but cannot view other user files. Session lock reliably prevents screen capture/access by unprivileged processes.

# 28. Definitive Next 1.0 product contract

## 28.1 What the final system is

Next 1.0 is an independently implemented, installable Rust-first desktop operating system with a custom Helix kernel, original Prism desktop, native Next API/SDK, genuine storage/network/audio/security services, broad explicit POSIX/Linux/NT compatibility interfaces, and sufficient certified drivers to run productively on real supported PCs. It is not a Linux distribution, a Windows derivative, a themed desktop on another operating system, or solely a VM-based compatibility shell.

## 28.2 Indispensable final-version capabilities

| Product dimension | Required final-version outcome |
|---|---|
| Boot and install | UEFI boot, signed deployment support, disk discovery, safe installer, local account, recovery media |
| Core kernel | SMP, user/kernel isolation, preemptive threads, virtual memory, capability handles, IPC, robust syscalls |
| Hardware | Certified x86-64 reference PC(s) with real storage, input, network, audio and accelerated graphical desktop |
| Core storage | Crash-consistent native VFS/filesystem, permissions, encryption option, snapshots or equivalent recovery path |
| Networking | Interoperable IPv4/IPv6 basics, DNS, TCP/UDP, TLS integration, firewall and usable network settings |
| Desktop | Original GPU-accelerated Prism experience, windows, taskbar, launcher, Settings, Files, Search, Canvas, Spaces |
| Application platform | Next SDK, native app packages, system APIs, browser, terminal, file/media utilities, accessibility |
| UNIX | Native POSIX userspace capability plus Linux x86-64 ELF execution for a defined substantial app class |
| NT | Original PE x86-64 loader, NT personality and functional Win32 application compatibility for a defined app class |
| Integration | Shared desktop, clipboard, file access and policy across supported application personalities |
| Security | Privilege isolation, sandboxing, secure updates, permissions UI, disk encryption, multi-user controls |
| Reliability | Recovery, safe shutdown and sleep policies, logging, crash containment, rollback of failed deployments |
| Developer | Rust/C/C++ native target capability, documented ABI/SDK and installable debugging/development tooling |
| Premium UX | Fast local interactions, coherent visual language, polished typography, accessibility and privacy by design |

## 28.3 Truthful limits of the final product

A completed product need not execute literally **every** program ever built for Windows or every UNIX variant. Such a claim would require reproducing separate system kernels, undocumented APIs, CPU architectures, third-party frameworks, device drivers, online services and licensing conditions. Next instead must provide a clear, extensible architecture and broad verified compatibility for named targets. The user-facing compatibility database is part of the product.

This document specifies the finished product and the interoperable contracts of its constituent systems; it does not instruct how work must be organized.

---

# 29. Reference specifications and architectural sources

Authoritative external standards and API references are used as functional interoperability targets, not copied source code. The implementation must independently satisfy applicable redistribution and licensing obligations.

- [UEFI Specification](https://uefi.org/specifications) — boot, firmware services and handoff.
- [ACPI Specifications](https://uefi.org/specifications) — platform discovery and power state information.
- [POSIX / The Open Group Base Specifications](https://pubs.opengroup.org/onlinepubs/9799919799/) — portable UNIX source interfaces.
- [ELF gABI](https://refspecs.linuxfoundation.org/elf/gabi4+/contents.html) — executable and shared-object format concepts.
- [Linux man-pages](https://man7.org/linux/man-pages/) — Linux-specific syscall behavior and user-space interfaces.
- [Microsoft PE/COFF specification](https://learn.microsoft.com/en-us/windows/win32/debug/pe-format) — executable format interoperability.
- [Microsoft Windows API documentation](https://learn.microsoft.com/en-us/windows/win32/api/) — published Win32 API contracts.
- [Microsoft Windows Driver and NT object documentation](https://learn.microsoft.com/en-us/windows-hardware/drivers/) — documented NT concepts and contracts, not permission to reuse Microsoft kernel code.
- [Vulkan specifications](https://registry.khronos.org/vulkan/) — graphical and compute API interoperability.
- [Unicode Standard](https://www.unicode.org/standard/standard.html) — text processing, shaping-related references and encoding.
- [W3C WAI accessibility guidance](https://www.w3.org/WAI/) — accessibility quality principles; desktop-native semantics remain Next-specific.
- [Rust Reference](https://doc.rust-lang.org/reference/) — Rust language and ABI boundaries.

