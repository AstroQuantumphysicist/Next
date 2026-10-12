# Developing Next

This checkout builds an independent development operating system. The [specification](NEXT-1.0-SPEC.md) describes the finished product; it is not agent instructions or a report of implemented behavior. [STATUS.md](STATUS.md) records outstanding acceptance gates. Passing host or VM checks does not make this Next 1.0.

Run commands from the repository root. The prepared environment has a pinned local toolchain/sysroot under `/workspace/toolchains`; source `tools/env.sh` in each new shell.

## Build and host validation

```sh
bash tools/bootstrap.sh
source tools/env.sh
bash tools/test.sh
bash tools/build.sh
```

Run bootstrap only when the toolchain/dependencies need preparation. It installs Rust 1.90.0 with `x86_64-unknown-uefi` and freestanding `x86_64-unknown-none`, prepares verified Debian QEMU/OVMF/FAT dependencies, and fetches the locked Rust crates. Initial setup needs network access; Rust builds subsequently use the lockfile and offline mode. Deployment authentication uses pinned `sha2`/`ed25519-dalek` dependencies with default features disabled. See the [third-party inventory](third-party/README.md) for exact versions/licenses and transitive/build dependencies. Python tools use only the standard library. Original Linux fixtures use GNU `as` and `ld` from the host development environment.

`test.sh` checks formatting, host Rust tests, Clippy for host/UEFI/kernel/user targets, block-driver contract tests, the exact Linux parser profile, Python image/archive tests, and the specification audit. Current test counts are reported by the commands and evidence artifacts; they change as coverage grows.

`build.sh` prepares/reuses the workspace development signing key, compiles its public trust anchor into NextBoot, builds Helix and separate native executables, invokes `build-linux.sh` for the static Linux fixtures, creates `INITRD.NXI`, signs/verifies the deployment bundle with `build-package.sh`, and constructs the image. Native binaries are freestanding `no_std` programs with the experimental Next note/startup contract; there is no general Rust `std`, libc or C++ runtime port. Development private keys stay in the local build workspace and must not be included in distributable boot artifacts.

The output image has a 64 MiB FAT32 boot volume and 1 MiB trailing reserved data space. It contains `EFI/BOOT/BOOTX64.EFI`, `EFI/NEXT/HELIX.ELF`, `EFI/NEXT/INITRD.NXI`, `EFI/NEXT/SYSTEM.NXP` and `/NEXT.TXT`. The archive contains native supervisor/probe/fault/context programs and explicit Linux/NT fixtures. NextBoot authenticates the bounded signed deployment and compares both actual boot files with its verified SHA256 digests before execution. The EFI loader itself remains unsigned; this is not UEFI Secure Boot, TPM measurement or production root provisioning. There is no GPT installation, anti-rollback state, authenticated A/B update manager or repository trust lifecycle.

The Linux fixture can be built and checked separately:

```sh
bash tools/build-linux.sh
rustc --edition=2021 --test tools/linux_profile_tests.rs -o build/linux-profile-tests
build/linux-profile-tests
```

The build command executes the identical `build/compat/linux/hello.elf` directly on host Linux, checking its output and exit status 37. The profile tests exercise the exact bounded parser linked into Helix, including rejection of interpreter/dynamic requirements, wrong machine, unsafe flags and out-of-window mappings. The normal VM smoke then executes that same ELF through the separate Linux dispatcher. This is a documented small syscall profile, not general Linux binary support.

Original PE fixtures build separately with `bash tools/build-nt.sh`. `objdump -p build/compat/nt/hello.exe` shows the emitted PE headers/imports. Normal VM smoke executes both original PE fixtures on Helix and requires exit 45 and gate rejection; no actual Windows execution is claimed. [Linux profile](LINUX-PROFILE.md), [NT profile](NT-PROFILE.md) and [native ABI](NATIVE-ABI.md) state current bounds. [Service configuration](SERVICE-CONFIG.md) documents signed dependency and authority binding.

`runtime_failures.py` independently exhausts process slots with non-yielding executables, requires bounded NoMemory refusal, checks console/supervisor protection, kills/reuses slots with fresh PIDs and contains a fresh user fault. It uses a snapshot overlay and requires the source image to remain unchanged. Its report is under `build/runtime-failures/`. `service_pair_smoke.py` prepares a separately signed disposable mask3 configuration with unchanged kernel/nextd payload, then checks real dependency-ordered worker readiness, dependency failure teardown/restart and final concurrent lifetime. The default mask1 image retains two console slots. This test does not deliver a general service suite or per-service budgets.

`next-package` with `--features host-tools` also offers host-directory install/update/uninstall/list/rollback commands. It authenticates bundles, stages immutable content and atomically commits a bounded host registry; tests use real temporary directories. See its [installer contract](../crates/next-package/README.md) before use. It is not installed in the Next guest, does not grant declared permissions, and does not provide OS installation, a dependency solver or automatic recovery of crash-stale locks/staging directories.

## Guest validation

```sh
python3 tools/boot_smoke.py
python3 tools/boot_smoke.py --capture-prism-gallery
python3 tools/boot_failures.py
python3 tools/initrd_failures.py
python3 tools/package_failures.py
python3 tools/persistence_smoke.py
python3 tools/runtime_failures.py
python3 tools/service_pair_smoke.py
python3 tools/latency_smoke.py --timeout 240
```

`boot_smoke.py` boots the generated image on QEMU q35/TCG with OVMF, one CPU, legacy virtio block/network, AC97, PS/2 input and a software framebuffer. It verifies development deployment authentication, boot ownership, kernel/user page protections, native process lifecycle/IPC and blocking event waits, bounded Linux file descriptors, original PE imports and personality isolation, deliberate child faults, noncooperative timer switches with register state, storage commands, the Linux fixture, clock/input and rendered output. The AC97 DMA probe plays 12,000 frames of 48 kHz signed stereo PCM; the runner captures its output in `audio.wav` and verifies matching channels and the 1 kHz waveform. This tests device playback, not microphone capture or an application audio service. Consult the generated check list for the exact current assertions. Network probes acquire a DHCPv4 lease and exchange ARP/ICMP/UDP plus a bounded TCP HTTP request/response with local host peers through QEMU user networking. They provide no socket or general Internet application interface. The runner starts bounded UDP, TCP and synthetic-zone DNS peers. Run positive VM validators sequentially: the DNS runner checks peer request counts, so another guest using that port can invalidate attribution.

`boot_failures.py` modifies disposable image copies to test missing, truncated, oversized and malformed kernels. `initrd_failures.py` similarly tests missing/corrupt archives, unknown versions/reserved fields, malformed ranges, traversal and missing supervisor. They require useful loader diagnostics and no kernel entry during a bounded observation. Rejection is not signature validation or physical-hardware certification.

`package_failures.py` additionally checks actual loader rejection of missing/truncated/oversized deployment bundles, unknown versions/signers/permissions, bad signatures and payload digests. It changes structurally valid standalone kernel bytes and reseals a changed init archive's CRC to verify that neither can bypass the signed deployment's digest binding. Cases use no private signing key and leave the original image unchanged. Reports are under `build/package-failures.json` and `build/failures/pkg*/`. This validates the development anchor and format; it does not establish firmware authentication, production key management or anti-rollback policy.

`persistence_smoke.py` copies the image under `work/`, boots a writable create phase and a separate verify phase, reads `/persistent` from the same disk copy, and removes it. It requires the 64 MiB FAT32/ESP bytes and original generated image to remain unchanged. The child runner accepts `--persistent-data` only for disposable images under `work/`; ordinary smoke and interactive runs use snapshots. A final read-only verifier checks the actual guest-written journal and durable deletion without changing media. This verifies clean VM reboot persistence using actual guest writes/flushes. The offline recovery tool additionally exports/verifies the actual committed guest generation read-only and checks source bytes remain unchanged. Host COW tests cover modeled write/flush cuts and torn writes; neither scope certifies arbitrary physical power-loss behavior.

Reports/logs include `build/boot-smoke.json`, `boot-failures.json`, `initrd-failures.json`, `persistence.json`, `serial.log` and `screenshot.ppm`. Mutation logs are under `build/failures/`; persistence logs/screenshots are under `build/persistence/{create,verify}/`. Hash manifests identify generated artifacts but do not authenticate them.

## Interactive development VM

```sh
bash tools/run.sh
```

The default VNC viewer address is `127.0.0.1:5900`; serial commands use the launching terminal. `NEXT_DISPLAY`, `NEXT_QEMU`, `NEXT_RAM` and `NEXT_SMP` configure the runner. Additional exposed CPUs are not brought into a working SMP scheduler. Firmware variables are temporary and disk writes use a snapshot overlay. The smoke runner's `--cpu` option can select an additional QEMU CPU profile; this checks the same bounded development context policy, not support for arbitrary CPUs or physical machines.

The development console accepts:

```text
help
status
uptime
ps
date
run /system/probe.elf
run /system/spin.elf
kill PID
run /compat/linux/hello.elf
cat /NEXT.TXT
ls
write /note hello world
cat /note
mv /note /renamed
rm /renamed
clear
```

`ps` reports actual bounded native runtime state. `run` selects native or Linux personality explicitly from the archive path; unsupported images fail. PID 1 is protected from console kill. File commands operate on the small COW data store, except `/NEXT.TXT`, which is read from FAT32. Writes persist during the current interactive session but disappear with its snapshot overlay. See [FS-FORMAT.md](FS-FORMAT.md) for eight-file/4 KiB limits, canonical names, flush ordering and read-only recovery behavior.

Escape or the menu key opens Prism Home. Up/Down or Tab selects a launcher action and Enter applies it. Tab outside the launcher cycles panel focus. The launcher opens Terminal, Files, System Monitor and Settings, and offers move/resize/minimize/maximize/restore/close. During keyboard move/resize, arrow keys adjust the window, Enter commits and Escape cancels. The PS/2 pointer supports focus and window interaction. Files displays supplied real file metadata and bounded content previews; Settings is a read-only diagnostic panel. Typing in Home matches bounded local panel/file metadata and can open the selected file in Editor. Existing-file consent appears in a trusted modal bound to the authenticated caller; the development portal fixture exercises approval, read-only write denial, UI rejection and timeout/cancellation. UTC date is anchored to validated q35 CMOS with elapsed PIT time, with unavailable clocks reported. These kernel-hosted panels are a development desktop, not isolated graphical apps or the finished Prism feature set.

## Implementation boundaries

Helix owns protected mappings and four bounded process slots. Native ELF images use a fixed high user window, unique ABI 0.1 note, guarded stack, generation-checked capabilities, bounded copy helpers and framed channels. PIT/PIC interrupts preempt user tasks using integer and 512-byte x87/MMX/SSE context saves. Unsaved optional user state is disabled: OSXSAVE/AVX, FSGSBASE, PKU, CET and UINTR, with cleared TLS bases and reset latent PKRU. Explicit bounded native FS and Linux FS/GS setters/getters are exposed and restored across entries/context switches; language-runtime TLS relocation and extended-state ownership remain absent. Deliberate supervisor/RX/NX/guard/invalid-opcode child faults are contained; unexpected kernel faults remain fatal. The native supervisor remains alive after lifecycle/IPC tests.

The experimental process wait ABI writes a signed 64-bit exit result through a checked output pointer and returns syscall status separately. Wait results survive repeated calls and duplicated handles; bad copyout cannot consume a retained result. Process waits still poll rather than atomically blocking on a general wait-many/cancel mechanism.

Rendering completes in the checked software back buffer before presentation to the front framebuffer. Frame-generation markers let guest tests request a screenshot after the intended visible state has been copied. There is no GPU fence/vblank or tear-free guarantee, and the fixed buffer limits total stride pixels to 1920 × 1080.

The system still lacks active APIC routing/SMP, general virtual memory/language TLS, complete service/session management, stable syscall schema and SDK, user-space VFS, full NextFS, production networking, audio services/mixing/capture, accelerated graphics, accessibility and user compositor isolation. Linux support is the documented static syscall subset; POSIX source applications, Linux dynamic libraries and general NT/Win32 execution remain pending beyond the original three-API PE fixtures. Installer, production signing/trust policy, package installation/repositories, rollback/recovery and hardware certification remain separate gates.

`latency_smoke.py` records at least 20 cycles in each same-run phase: focus Files, focus Terminal, and type a verified status command through real PS/2 input. Two timer-preempted non-yielding children exhaust the remaining slots during load. Reports contain raw host timestamps, frame generations, PIDs and median/p95/max by action. Run it without another positive VM. Its fixed-order TCG measurements are development references, with no default pass threshold or hardware/IRQ guarantee; see [LATENCY-REFERENCE.md](LATENCY-REFERENCE.md).

## Evidence maintenance

```sh
python3 tools/spec_audit.py --self-test
python3 tools/spec_audit.py --write-report
```

Keep the uploaded specification unchanged. Record bounded evidence with its command, scope, validation level, artifact and matching SHA256; retain missing acceptance work rather than promoting a feature from a screenshot or host model. Host tests, VM smoke, integrated scenarios and certified physical profiles are distinct scopes. [ROADMAP.md](ROADMAP.md) describes the next concrete integration work.

## Complete inventory report

`python3 tools/spec_coverage.py` audits every recorded evidence hash and writes `build/coverage/coverage.json` plus a standalone searchable `build/coverage/index.html`. The report includes all explicit identifiers, numbered section gates and final deliverables, preserving contracts, pending/partial status, remaining work and linked scoped evidence. Run `python3 tools/spec_coverage.py --self-test` for report integrity/escaping checks. This reports the recorded validated milestones; it does not turn ongoing working-tree features, host tests or VM workflows into certification. A stale/missing evidence artifact fails generation.
