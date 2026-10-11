# Development milestone 418ea69787d9

This immutable snapshot records image SHA256
`418ea69787d9167202dcf8a0d98db74335c2f3d456bb8ffe68aca0f6048b5dc1`.
`source-manifest.json` captured 120 implementation/config/tool files before the
next source wave. It has not been regenerated from later code. The separate
03d541 milestone at the parent evidence directory remains unchanged.

`validation.txt` records 196 workspace Rust unit/integration tests, eight
separately exercised host installer tests, additional scoped host/profile checks,
formatting and warnings-denied Clippy. Separate invocations may repeat existing
primitives; their counts are not unique aggregate cases. The combined descriptor/
stream model includes 32,000 deterministic operations. `audit-summary.json`
records 103 partial and 92 pending requirements, with none verified and no
certified hardware evidence.

Both `boot-smoke.json` and `cpu-max/boot-smoke.json` record 83 checks on the same
image. New integrated scenarios include single-object blocking event/process
waits with deadlines and kill cleanup, bounded Linux file descriptors and an
original PE32+ fixture using three Win64 API shims. The PE fixture runs on Helix
only; no actual Windows certification is claimed. Existing CPU/native process,
COW/device/audio/input/desktop probes remain bounded. Network checks now include
a DHCP lease, ARP/ICMP/UDP echo and an original bounded TCP handshake/HTTP response/
close against a loopback host peer; no sockets or general Internet app support.

`boot-failures.json`, `initrd-failures.json` and `package-failures.json` each
record ten passing loader refusal cases, with per-case files under `failures/`.
One concurrent package attempt timed out after 45 seconds on the missing-bundle
case; nine other cases passed. A standalone rerun passed all ten. The observation
summary explicitly states that the original failed report/case files were
overwritten by rerun; the timeout cause was not independently established.
`package-suite-console.txt` belongs to the passing rerun. The preserved timeout
observation is not presented as an original raw log.

`persistence.json` plus both phase directories record create/read/delete across
two independent VM boots with real writes/flushes and no snapshot overlays,
protecting the FAT32 span and source image. `runtime-failures/` records five
checks covering actual process-slot exhaustion, console/PID1 survival, stale-PID
refusal, slot reuse and user-fault containment; source image unchanged.

`audio.wav` captures the short AC97 probe, and `screenshot.ppm` a completed
software framebuffer frame. Automated checks establish waveform/dimensions/color
properties, not application audio, visual/accessibility or latency acceptance.
`build-manifest.json` identifies generated artifacts and
`dependency-inventory.json` preserves the dependency/SBOM inventory used here.

Commands and profile bounds are in [development](../../../DEVELOPMENT.md),
[native ABI](../../../NATIVE-ABI.md), [Linux profile](../../../LINUX-PROFILE.md)
and [NT profile](../../../NT-PROFILE.md). These are development foundations:
full NextFS/POSIX/NT, general services, user compositor, production trust,
installer, SMP and physical certification remain outstanding. Hash auditing
checks bookkeeping and cannot independently prove the full specification.
