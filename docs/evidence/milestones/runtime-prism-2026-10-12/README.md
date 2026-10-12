# Runtime and Prism evidence — 2026-10-12

The latest signed development image SHA-256 is
`f4986796589aa9e5d75ad5eddbaf620a17613ccb35bbdfb66d7e17ca3a40fc0d`. Its current
QEMU q35/TCG smoke reports passed on 1, 2, 4, and 8 configured vCPUs. The 1-vCPU
report has 120 checks; each multi-vCPU report has 119. QMP CPU objects and
ACPI/MADT enabled-CPU counts match in every profile. AP startup remains disabled
and the scheduler runs on the BSP only. None of these tests establishes SMP
scheduling or physical-hardware support.

The [feature-wave evidence](feature-wave/README.md) contains the exact current
reports, logs, guest captures, and host test results. Its Prism gallery includes
12 lossless PNG captures taken from the current guest image: Home, six panels,
two saved Accessibility preferences, keyboard shortcuts, notifications, and a
BottomLeft quarter snap.

## Earlier source-bound evidence

The remaining bundles in this directory are retained as historical evidence
for their own image hashes; they do not describe the latest image above.

- `final-qemu64/` and `smp-two-vcpu/` contain earlier 119-check qemu64 and
  two-vCPU reports with separate image hash `b79790f446b5bde892fbde78d63e3a177d558d2d1319556680d9f48f24efb8ce`.
- `ab-uefi-ovmf/ab-nvram-smoke.json` records five earlier boots sharing OVMF
  variables: three B trial attempts followed by rollback to A and a confirmed A
  boot. Trial staging is test-only; standard builds still have no updater or
  health-confirmation service.
- The malformed-media reports contain ten rejection cases each and confirm the
  source image stayed unchanged. Detailed logs are under
  `ab-uefi-ovmf/{boot,initrd,package}-failures/`.
- Earlier `qemu-max`, `qemu64-gallery`, `settings-refresh`, DHCP renewal/rebind,
  and ObjectWaitMany runs remain separate source-bound records.

Earlier host bundles are likewise preserved under their original scope:

- `workspace-validation.txt` is the previous source snapshot's workspace test
  run. The latest full `bash tools/test.sh` log is under `feature-wave/`.
- `nextfs-tests.txt` records its earlier 42 NextFS tests. The current 53-test
  host-backup run is in the feature-wave validation bundle.
- `package-host-tests.txt` records its earlier 42 package tests. Current pinned
  install-root hardening is documented with 47 host-tool tests in the new
  bundle; guest package installation remains absent.
- `network-runtime-host.json` and `network-dma-host.json` are earlier
  source-bound DHCPv4/runtime and 56-test queue/IPv6 records. The latest
  78-test bounded DHCPv6/client-model and DMA harness is in the new bundle; no
  guest DHCPv6 address assignment, DAD, routes, or broad IPv6 operation is
  established.
- `ac97-dma-host.json` records six host ownership tests, not certified audio
  hardware. `appearance-journal-tests.txt` and `acpi-topology-tests.txt` are
  prior preference and ACPI records; current coverage is in the feature-wave
  bundle.
- `ap-transition-review/` and `channel-host-benchmark.json` retain earlier
  compiled transition-stage and host benchmark evidence.

These results add bounded evidence to partial requirements; none establishes
Next 1.0 readiness. The top-level `SHA256SUMS` validates all files in this evidence directory, including
the current feature wave; that subdirectory also has a focused manifest.
