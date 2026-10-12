# Runtime and Prism evidence — 2026-10-12

The current signed development image SHA-256 is `b79790f446b5bde892fbde78d63e3a177d558d2d1319556680d9f48f24efb8ce`. It passed 119 qemu64 guest checks and a separate 119-check two-vCPU profile. The qemu64 report captures ten current Prism screens, including the saved Accessibility focus-outline setting. The two-vCPU report confirms two QMP CPU objects and two enabled ACPI/MADT records while AP startup remains disabled and the scheduler stays on the BSP. Neither QEMU profile certifies physical hardware.

## Guest runs

- `final-qemu64/boot-smoke.json` binds the current image hash to the 119-check qemu64 run, serial log, audio capture, framebuffer and ten PNG frames used in the [Prism visual gallery](../../../PRISM-VISUAL-GALLERY.md). The Accessibility capture follows keyboard navigation, saves the Strong outline preference, waits for the kernel's accepted-save record, then captures a completed frame.
- `smp-two-vcpu/boot-smoke.json` uses the same image and observes two configured virtual CPUs plus two enabled MADT processor entries. APs remain stopped; this is not SMP execution or scheduling evidence.
- `ab-uefi-ovmf/ab-nvram-smoke.json` records five separate boots that share OVMF variables: three B trial attempts followed by rollback to A and a confirmed A boot. The test-only loader feature stages the trial image; standard builds still have no updater or health-confirmation service.
- The three malformed-media reports each contain ten rejection cases and confirm the source image stayed unchanged. Detailed current serial logs are retained under `ab-uefi-ovmf/{boot,initrd,package}-failures/`.
- Earlier `qemu-max`, `qemu64-gallery`, `settings-refresh`, DHCP renewal/rebind, and ObjectWaitMany runs remain as separate source-bound records; their image hashes identify their exact snapshots.

## Host checks

- `workspace-validation.txt` is the passing `bash tools/test.sh` run for this source snapshot.
- `nextfs-tests.txt` records 42 NextFS tests, including bounded rendezvous failure when a participant misses a phase and concurrent directory open/rename/unlink progress. These are host tests, not guest SMP or physical-storage stress.
- `package-host-tests.txt` records 42 package tests; verification rejects file/directory path-prefix conflicts before extraction and checks case aliases, oversized metadata and no-mutation behavior. Hostile-ancestor races and guest package installation remain absent.
- `network-runtime-host.json` records 41 production-source runtime/parser tests, including DHCP state transitions and the bounded IPv6/ICMPv6 Router Advertisement model. `network-dma-host.json` records 56 virtio-network queue and IPv6 parsing tests. Neither establishes IPv6 address assignment, DAD, neighbor cache, routes, DHCPv6, or guest IPv6 operation.
- `ac97-dma-host.json` records six production-source DMA ownership tests. It retains DMA ownership through timeout/malformed/FIFO-error states and releases only after valid completion or a confirmed reset; it does not integrate allocator pinning or certify audio hardware.
- `appearance-journal-tests.txt` records 43 crash-boundary and persistence tests for the shared appearance preference record, including the focus-outline preference. The guest frame proves only that the Settings save was accepted and rendered in this QEMU run.
- `acpi-topology-tests.txt`, `ap-transition-review/`, and `channel-host-benchmark.json` retain their focused topology, compiled transition-stage, and host benchmark evidence.

These results extend partial requirement evidence; no requirement is promoted to verified. `sha256sum -c SHA256SUMS` checks every retained file in this directory; the focused A/B, qemu64, two-vCPU and Settings bundles also carry their own manifests.
