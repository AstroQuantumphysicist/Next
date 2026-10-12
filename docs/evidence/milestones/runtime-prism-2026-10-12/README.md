# Runtime and Prism evidence — 2026-10-12

The latest signed development image SHA-256 is
`6b245fa85c2c49ad1da63cf7d6fdfb813e3d351c3275217c736d7b1c43b1da15`. Its qemu64
desktop smoke passed 138 checks, including two Linux clock fixture runs for
`clock_gettime`, `clock_getres`, monotonic sleep and copyout bounds, Home
arithmetic search, Files tabs/panes/copy, and notification controls. The
[clock and notification bundle](prism-clock-notifications-wave/README.md)
contains the report, source hashes, capture manifest, serial trace, and 20
current Prism screenshots. The previous [Prism and Linux bundle](prism-linux-search-wave/README.md)
retains the earlier image's source-bound evidence. The earlier [cwd and Prism bundle](cwd-copy-wave/README.md) and
[fstat/getdents Files bundle](fstat-getdents-files-r3/README.md) retain their
source-bound reports for their own images. The [feature-wave bundle](feature-wave-final/README.md)
records the separate DHCPv6 peer exchange on its own image. APs remain stopped
and the scheduler runs only on the BSP; these tests do not establish SMP
scheduling or physical-hardware support.

## Earlier source-bound evidence

The earlier `feature-wave/` and topology profiles are retained as historical
evidence for their own image hashes; they do not describe the latest image
above. The previous 1-vCPU report had 120 checks and the earlier 2/4/8-vCPU
profiles had 119 each on SHA-256
`f4986796589aa9e5d75ad5eddbaf620a17613ccb35bbdfb66d7e17ca3a40fc0d`.

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
  run. The current wave's `validation.md` records the latest full
  `bash tools/test.sh` result; older logs remain tied to their own source snapshots.
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
