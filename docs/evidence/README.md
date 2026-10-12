# Validation evidence

The latest ABI and Prism run is in [fstat-getdents-files-r3](milestones/runtime-prism-2026-10-12/fstat-getdents-files-r3/): signed image SHA256 `1b226f5ee89a59656cf74a673ea381f7d56e9dbe764bb852c75d224a902d6c6b` passed 128 qemu64 checks, including Linux `fstat`/`getdents64` and independent Files pane navigation. Its 16 captures are shown in the [visual gallery](../PRISM-VISUAL-GALLERY.md). The previous [feature-wave-final](milestones/runtime-prism-2026-10-12/feature-wave-final/) image remains the source for the bounded DHCPv6 peer exchange.

![Current Prism guest screens](../images/prism-overview.png)

## Historical validation snapshot

The following material records an older development image SHA256
`03d54128375ad7f167c724ebdf98baca06b5add89f88edce457bf426df326c39`.
Its source-bound artifacts remain useful for their original scope; current
guest validation is recorded in [the fstat/getdents Files bundle](milestones/runtime-prism-2026-10-12/fstat-getdents-files-r3/),
with the earlier network peer checks retained in [feature-wave-final](milestones/runtime-prism-2026-10-12/feature-wave-final/).

`validation.txt` records 174 workspace Rust tests, eight separately exercised
host installer tests, and additional storage, Linux-profile, input/console,
audit, formatting and warnings-denied Clippy checks. Separate test invocations
can repeat primitives already covered by the workspace run; do not sum them as
unique test cases. The requirement counts printed during that run predate this
reconciliation; `audit-summary.json` is the current inventory result.

`boot-smoke.json` and `serial.log` record 77 positive assertions on QEMU
q35/TCG/OVMF with the `qemu64` CPU. `cpu-max/boot-smoke.json` records 77 checks
with the `max` CPU on the same image. They cover development signed deployment
binding, independent guarded boot, FX512 context policy, native isolated
processes/preemption/fault containment, capability IPC moves, bounded native
file/event/anonymous-memory interfaces, retained wait results, static Linux
hello/memory/clock fixtures, COW storage, DHCP/ARP/ICMP/UDP traffic, AC97 DMA,
PS/2 input and kernel-hosted software Prism panels. Neither VM profile establishes
SMP, general POSIX/Linux compatibility, blocking event waits, a user compositor,
physical hardware qualification or Next 1.0 readiness.

`boot-failures.json`, `initrd-failures.json` and `package-failures.json` each
record ten rejected media mutations. Per-case serial logs and reports are retained
under `failures/`. The tests require actionable diagnostics and no observed kernel
entry during bounded observation, preserve the source image, and do not prove
permanent halt or comprehensive recovery. Signature refusal authenticates a
bounded deployment against the development anchor compiled into an unsigned EFI
loader; firmware Secure Boot, TPM and production anti-rollback remain absent.

`persistence.json` and `persistence/{create,verify}/` record two independent VM
boots using real virtio writes/flushes on one disposable image without snapshot
overlays. A file created in the first boot is read and removed in the second;
the protected 64 MiB FAT32 span and source image remain unchanged. Together with
host COW crash/torn-write tests this establishes the named bounded model and clean
VM reboot workflow, not arbitrary physical power-loss durability.

`audio.wav` captures the actual AC97 output; the report verifies 48 kHz stereo
format, equal active channels and the expected 1 kHz waveform. This establishes
the short DMA probe, not audio services, capture or latency acceptance.
`screenshot.ppm` records a completed software presentation; automated screenshot
checks validate dimensions and color diversity, not visual layout/accessibility.

`build-manifest.json` identifies generated artifacts. `source-manifest.json`
identifies 106 implementation/config/build/validation files at this milestone,
including README; it excludes generated directories, scratch work and keys.
Hashes check bookkeeping and do not independently prove technical contracts.
The original specification hash remains unchanged.

Regenerate with the commands in [DEVELOPMENT.md](../DEVELOPMENT.md), then
reconcile requirement scopes and hashes with `python3 tools/spec_audit.py
--self-test` and `python3 tools/spec_audit.py --write-report`. All 195 requirements
remain partial or pending; no certified physical hardware evidence is present.
