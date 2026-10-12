# Next

Next is an original Rust-first operating system: Helix is its kernel and Prism its desktop. The [Next 1.0 specification](docs/NEXT-1.0-SPEC.md) describes the intended product. **The current system is a development build, not Next 1.0.**

![Prism desktop Home, captured from the running QEMU guest](docs/images/prism-home.png)

The [Prism visual gallery](docs/PRISM-VISUAL-GALLERY.md) shows the current QEMU guest screens and interaction states. The [Prism visual tour](docs/PRISM-VISUAL-TOUR.md) adds captions and implementation schematics for the visible shell, Files workspace, and Settings preferences.

Prism uses Microsoft Selawik, the open-source metrically compatible replacement for Segoe UI; it does not bundle proprietary Segoe UI Variable. The raster atlas and shaping limits are documented in the [font provenance and notices](docs/third-party/README.md#font-assets).

## Prism capture index

These seventeen images are captures from the signed qemu64 guest image recorded
below. They document that tested image; later source changes need a fresh
capture before the pictures can represent them. The index links each thumbnail
to its full-size frame. The gallery and tour explain what each frame shows and
include implementation schematics.

<table>
  <tr>
    <td align="center"><a href="docs/images/prism-home.png"><img src="docs/images/prism-home.png" alt="Prism Home with pinned applications and taskbar" width="170"></a><br><sub>Home</sub></td>
    <td align="center"><a href="docs/images/prism-files.png"><img src="docs/images/prism-files.png" alt="Prism Files browsing the native store" width="170"></a><br><sub>Files</sub></td>
    <td align="center"><a href="docs/images/prism-files-tabs-root.png"><img src="docs/images/prism-files-tabs-root.png" alt="Files root tab beside a second tab" width="170"></a><br><sub>Files root tab</sub></td>
    <td align="center"><a href="docs/images/prism-files-tabs-nested.png"><img src="docs/images/prism-files-tabs-nested.png" alt="Files nested tab beside the retained root tab" width="170"></a><br><sub>Files nested tab</sub></td>
  </tr>
  <tr>
    <td align="center"><a href="docs/images/prism-files-tabs-ibeam.png"><img src="docs/images/prism-files-tabs-ibeam.png" alt="I-beam cursor over the Files location field" width="170"></a><br><sub>Files location cursor</sub></td>
    <td align="center"><a href="docs/images/prism-files-dual-pane.png"><img src="docs/images/prism-files-dual-pane.png" alt="Files panes with independent paths" width="170"></a><br><sub>Files dual pane</sub></td>
    <td align="center"><a href="docs/images/prism-settings.png"><img src="docs/images/prism-settings.png" alt="Settings Appearance page" width="170"></a><br><sub>Settings Appearance</sub></td>
    <td align="center"><a href="docs/images/prism-settings-accessibility.png"><img src="docs/images/prism-settings-accessibility.png" alt="Settings with saved Strong focus outline" width="170"></a><br><sub>Accessibility focus</sub></td>
  </tr>
  <tr>
    <td align="center"><a href="docs/images/prism-settings-text-scale.png"><img src="docs/images/prism-settings-text-scale.png" alt="Settings with saved 125 percent text size" width="170"></a><br><sub>Settings text size</sub></td>
    <td align="center"><a href="docs/images/prism-terminal.png"><img src="docs/images/prism-terminal.png" alt="Prism Terminal" width="170"></a><br><sub>Terminal</sub></td>
    <td align="center"><a href="docs/images/prism-editor.png"><img src="docs/images/prism-editor.png" alt="Prism Editor" width="170"></a><br><sub>Editor</sub></td>
    <td align="center"><a href="docs/images/prism-calculator.png"><img src="docs/images/prism-calculator.png" alt="Prism Calculator expression view" width="170"></a><br><sub>Calculator</sub></td>
  </tr>
  <tr>
    <td align="center"><a href="docs/images/prism-monitor.png"><img src="docs/images/prism-monitor.png" alt="Prism System Monitor" width="170"></a><br><sub>System Monitor</sub></td>
    <td align="center"><a href="docs/images/prism-snap-quadrant.png"><img src="docs/images/prism-snap-quadrant.png" alt="Terminal snapped into the lower-left quadrant" width="170"></a><br><sub>Quarter snap</sub></td>
    <td align="center"><a href="docs/images/prism-shortcuts.png"><img src="docs/images/prism-shortcuts.png" alt="Prism keyboard shortcut guide" width="170"></a><br><sub>Keyboard shortcuts</sub></td>
    <td align="center"><a href="docs/images/prism-notifications.png"><img src="docs/images/prism-notifications.png" alt="Prism notification center" width="170"></a><br><sub>Notifications</sub></td>
  </tr>
  <tr>
    <td align="center" colspan="4"><a href="docs/images/prism-files-copy-to-other-pane.png"><img src="docs/images/prism-files-copy-to-other-pane.png" alt="Files copying a selected file into the other pane" width="170"></a><br><sub>Files copy to other pane</sub></td>
  </tr>
</table>

NextBoot verifies a bounded Ed25519/SHA256 development deployment against its compiled public trust anchor, binds the separate Helix ELF and init image to those verified digests, exits UEFI boot services, and runs independently. The EFI loader itself is unsigned; this is not UEFI Secure Boot or a production trust chain. No host OS kernel runs inside the guest. Helix authenticates signed application envelopes and binds exact executable identities to bounded development capability grants before loading native applications into per-process address spaces, enforces capability handles and bounded user-buffer access, switches tasks on timer interrupts, and contains deliberate application faults. A native `nextd` supervisor exercises process launch/wait/kill, framed IPC, stale handles and process-slot reuse, plus a signed declarative worker dependency/restart/backoff/heartbeat policy. Default configuration retains one worker; a separately signed alternate proves two concurrent dependency-ordered worker instances. Experimental native interfaces now also include anonymous mappings, capability-backed bounded file streams, and manual/auto-reset event polling and waits. Event-only syscall 45 adds a bounded wait-any over up to 16 events; qemu64 guest evidence covers signal, reset, timeout, transfer, and cleanup behavior.

The tested QEMU profile also supports legacy virtio block reads/writes/flush, FAT32 boot-file reads, a small transactional COW data store, DHCPv4 acquisition, Ethernet ARP/ICMP/UDP echo, native fixed-peer UDP capability objects, bounded TCP HTTP and local synthetic-zone DNS/cache probes, PS/2 keyboard/mouse, and a software framebuffer desktop. Bounded IPv6/ICMPv6/NDP validation and router/prefix lifetime rules have host tests. The guest also performs a bounded DHCPv6 Solicit/Advertise/Request/Reply exchange and retains the accepted Reply as a candidate only; it does not install an address, run DAD, configure routes, or use the candidate for sockets, and renewal/rebind is not wired into the guest. An AC97 DMA probe plays 250 ms of stereo PCM; captured output verifies its 1 kHz waveform, and a host ownership harness checks its bounded DMA lifecycle. Prism has Terminal, Files with two independent panes, up to four tabs per pane, and an editable location field, System Monitor, persistent Appearance settings and Accessibility preferences, plus a bounded UTF-8 Editor panel with focus and window actions. Editor saves through COW and preserves edits on external-change conflicts. Files provides actual create, rename, permanent delete and conditional Copy with cancellation/overwrite checks; Super shortcuts and dirty AltF4 confirmation preserve pending edits and terminal input. Appearance persists Light, Dark or HighContrast and four accents across reboot. Standard/Strong Settings focus outline and 100%/125% Settings-only text size share the checksummed preference record and pass host save/reload tests. Home matches local app/file metadata and opens actual file content; a trusted modal chooser grants a scoped existing-file capability to an authenticated process after PS/2 consent. UTC clock/date uses a validated q35 RTC anchor plus elapsed monotonic time. These panels run in the kernel; they are not isolated userspace desktop services. The COW store preserves a file across two independent VM boots and has host crash-boundary tests. A gated host/test archive path preflights hashes and restores the entire current journal namespace in one COW transaction; it has no destination picker, encryption, or backup UI. See its [format and limits](docs/FS-FORMAT.md) and the [archive contract](docs/BACKUP-ARCHIVE.md).

Source 18's frozen Prism refresh and acceptance run remain archived in [Prism acceptance](docs/PRISM-ACCEPTANCE.md) and [its evidence bundle](docs/evidence/milestones/a6afc2715e8e/). This feature wave adds bounded event-only wait-any, guest DHCP renewal/rebind and recovery checks, the DHCPv6 candidate exchange, a 12-category Settings shell with saved text size, IPv6 ingress validation, static audio and network DMA ownership tests, a host/test-gated backup archive restore bridge, safer package path validation, guest Linux `getpid`/`uname`/`lseek`/`fstat`/`getdents64` fixtures, Files tabs, dual panes and an editable location field, four-way single-work-area snapping, a bounded host capture-consent policy model, and a QEMU-verified A/B trial/rollback path. The A/B policy remains a development slice: there is no production updater or health-confirmation service, and ordinary EFI variables are not a protected anti-rollback anchor. QEMU profiles check 1/2/4/8-vCPU topology while keeping APs stopped; the scheduler still runs only on the BSP. Host/TCG checks do not establish product readiness. Next is still a development build; real hardware support, SMP-safe operation, broad compatibility, full Settings behavior, assistive services, and a complete GUI toolkit remain unfinished.

The [Prism visual gallery](docs/PRISM-VISUAL-GALLERY.md) contains 17 QEMU guest captures from one image, including Files root/nested tabs, an I-beam cursor, independent panes, and cross-pane copy. The latest signed image SHA256 `63374568a17fcd592fdabe24008abba36975ab48732a2b044b0f3efa1a6c36a6` passed 134 qemu64 checks, including Linux `getcwd`, `chdir`, and relative `openat` in the original guest fixture (exit status 56). The previous image, SHA256 `1b226f5ee89a59656cf74a673ea381f7d56e9dbe764bb852c75d224a902d6c6b`, passed 128 checks including Linux `getpid`, `uname`, `lseek`, `fstat`, and `getdents64` (exit values 51–55). A prior signed image, SHA256 `8902846a7a484186361520c6c626846479710faef2afa8729bdad664e66825f3`, passed the deterministic DHCPv6 candidate peer and separate controlled DHCPv4 renewal/rebind/recovery checks. The earlier 2/4/8-vCPU topology profiles each passed 119 checks on image SHA256 `f4986796589aa9e5d75ad5eddbaf620a17613ccb35bbdfb66d7e17ca3a40fc0d`; all tested profiles keep APs stopped and scheduler work on the BSP. Host-only evidence covers a bounded capture-consent policy (no compositor integration), NextFS open/rename/unlink races, static virtio-net and AC97 DMA ownership, CPU topology parsing, package path confinement, and channel benchmarks. Reports, logs, and framebuffer captures are in [the October 12 runtime and Prism evidence](docs/evidence/milestones/runtime-prism-2026-10-12/).

A deliberately small [Linux x86-64 profile](docs/LINUX-PROFILE.md) executes original static ELF fixtures on host Linux and Helix, using Linux `SYSCALL` numbers. Fixtures exercise bounded file descriptors, shared inode lifetime, positional `pread64`/`pwrite64`, `fstat`, snapshot-based `getdents64`, anonymous memory/heap operations, monotonic clock/sleep, and thread-local bases. An original PE32+ fixture runs on Helix through GetStdHandle, WriteFile and ExitProcess shims, with explicit personality-gate checks; this has no actual Windows certification. These fixtures establish only their own behavior, not broad Linux compatibility. Signed bounded GPT storage policy can mount a specifically authorized preformatted COW journal, with real read-only and refusal cases; it does not install, format or repair disks. Dynamic ELF, general libc/POSIX applications, general Windows/NT applications, a stable SDK, guest package installation/update policy, GPT installation, full NextFS, audio services/mixing/capture, accelerated graphics, SMP and physical hardware certification remain outstanding. The [requirement report](docs/STATUS.md), [architecture](docs/ARCHITECTURE.md), and [roadmap](docs/ROADMAP.md) record the larger scope.

![Diagram of the bounded Linux ELF-to-Helix syscall and storage path](docs/images/linux-profile-flow.svg)

## Build and validate

On the prepared environment:

```bash
cd /workspace/Next
source tools/env.sh
bash tools/test.sh
bash tools/build.sh
python3 tools/boot_smoke.py
python3 tools/boot_failures.py
python3 tools/initrd_failures.py
python3 tools/package_failures.py
python3 tools/persistence_smoke.py
```

For a fresh compatible Debian x86-64 environment, run `bash tools/bootstrap.sh` first. It installs the pinned Rust 1.90.0 toolchain and verified Debian QEMU/OVMF/FAT tools below `/workspace/toolchains`, then fetches locked Rust dependencies. Signature verification uses `sha2` 0.10.9 and `ed25519-dalek` 2.2.0 with their pinned transitive dependencies; subsequent builds and host tests work offline. Linux fixtures use the host GNU assembler/linker. [Development instructions](docs/DEVELOPMENT.md) include the separate fixture/profile commands and VM controls.

Generated artifacts under `build/` include `next-dev.img`, `esp/EFI/BOOT/BOOTX64.EFI`, `esp/EFI/NEXT/HELIX.ELF`, `esp/EFI/NEXT/INITRD.NXI`, `esp/EFI/NEXT/SYSTEM.NXP`, native application ELFs, Linux fixtures, SHA256 manifests, and validation reports. The image contains a 64 MiB FAT32 boot volume followed by 1 MiB reserved development data space. `SYSTEM.NXP` is signed with the workspace development key and its kernel/init-image digests are checked by NextBoot. The EFI loader remains unsigned and the image is **not a disk installer**. CRCs/hash manifests alone are not signatures; this deployment signature does not establish Secure Boot, TPM measurements or anti-rollback policy.

`bash tools/run.sh` starts the QEMU profile interactively with serial input and a local VNC display by default. Commands include `help`, `uptime`, `ps`, `run /system/probe.elf`, `run /compat/linux/hello.elf`, `kill PID`, `cat /NEXT.TXT`, `ls`, `write /note hello`, `cat /note`, `mv /note /renamed`, and `rm /renamed`. Escape opens the Prism launcher; Tab cycles focus. Interactive runs use a disk snapshot overlay, so their writes disappear when the VM exits. The dedicated persistence test instead uses one disposable writable copy across two boots.

## Code

| Component | Implemented scope |
| --- | --- |
| `next-boot` | Anchored development deployment verification, boot-device reads, validated fixed-address Helix ELF/init image, BootInfo and firmware handoff |
| `helix-kernel` | Protected mappings, single-CPU native process runtime, timer context switches, fault containment, development drivers and console |
| `next-abi` | Experimental boot/native ABI definitions; defensive ELF, init-image and startup validation |
| `helix-core` | Bounded capabilities, page bookkeeping, scheduling, clock conversion, framed channel queues |
| `next-user` | Freestanding native runtime and separate supervisor, probe, fault and context-test programs |
| `prism` | Allocation-free framebuffer rendering, six development panels, bounded Editor and Calculator, keyboard/mouse window controls |
| `next-fs` | Read-only short-name FAT32 plus bounded COW file transactions and corruption recovery |
| `next-checks` | Host contract checks for firmware, input and block drivers |
| `next-package` | Ed25519/SHA256 verification, host packaging and host-only registry install/update/rollback; no guest package service or permission grants |
| `linux-fixtures` | Original static ELF programs for the explicit Linux syscall subset |

Crate manifests identify their licenses: the workspace default is Apache-2.0 and `next-abi` declares MIT. The [third-party inventory and notices](docs/third-party/README.md) and [CycloneDX SBOM](docs/third-party/sbom.cdx.json) record locked cryptography dependencies, including host/build/conditional packages that are not all linked into the guest. External Rust, QEMU, OVMF, GNU and Debian tooling retain upstream licenses and are development tools. No universal binary compatibility or hardware certification is claimed.
