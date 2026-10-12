# Runtime and Prism evidence — 2026-10-12

The original gallery and max-profile QEMU reports use the same signed
development image SHA-256:
`708660fd20f13325b7da0cd1dbe52f1f2b71a8ffb959270d63225f636ac5269e`.
The image was built from this feature wave. It is a q35/TCG guest with one
virtual CPU; neither profile certifies physical hardware.

## Guest runs

- `final-qemu64/boot-smoke.json` records 118 passed checks on the freshly built signed image `4cc895e716afda58cf932d0dbebabec9f755e39fb566bbd000741c82301b32bb`. It includes event-only wait-many and captures all nine Prism screens. The report, serial log, audio, screenshot, and PNG frames are retained together.

- `qemu64-gallery/boot-smoke.json` records 117 passed checks using QEMU's
  `qemu64` CPU profile. The completed run captured all nine current Prism
  screens. `qemu64-gallery/prism-*.png` are the same guest frames used by the
  [visual gallery](../../../PRISM-VISUAL-GALLERY.md); the screenshot suite
  checks frame dimensions and nonblank output, not full visual quality or
  accessibility.
- `settings-refresh/boot-smoke.json` records a later 118-check qemu64 run on
  image `e69aa9ca8de049c2a12d048e28e56525a0c352992b2385b200daba70de4ab484`.
  Its Settings frame shows 12 categories: Appearance has active theme/accent
  controls, System is read-only, and the other ten categories are informational
  with disabled rows. It is rendering evidence, not a claim that those disabled
  pages configure hardware or system services.
- `qemu-max/boot-smoke.json` records 116 passed checks using QEMU's `max` CPU
  profile. It exercises scheduler-backed event waits and the Linux positional
  file fixture, which exits 46. Both runs retain compressed serial output, audio capture,
  and the presented framebuffer image beside their reports.
- `dhcp-guest-smoke.json` records a separate QEMU guest run on image
  `e69aa9ca8de049c2a12d048e28e56525a0c352992b2385b200daba70de4ab484`.
  A controlled Ethernet peer drove the actual guest's T1 ACK refresh, T2
  broadcast rebind with an alternate server, NAK/discovery restart, and
  expiry/discovery restart. The report includes event times, timer windows,
  source hashes, and QEMU limits.
- `boot-failures.json`, `initrd-failures.json`, and `package-failures.json`
  record ten bounded malformed-input cases each against the same final image.
  All reject the altered boot media and preserve the original image. Their text
  logs are included for a quick readable check.

## Host checks

- `workspace-validation.txt` is the successful `bash tools/test.sh` run. The
  workspace suite includes 218 `next-checks` host contract tests and 191 Prism
  tests; focused host checks are also recorded in the same output.
- `nextfs-tests.txt` records 41 passing NextFS tests, including concurrent
  directory open/rename/unlink and stale-identity checks. This is a host race
  test; it does not prove guest SMP safety or disk durability under concurrent
  mutation.
- `network-runtime-host.json` records 30 passing DHCP acquisition, renewal,
  rebinding, expiry, and restart tests against the production runtime with a
  deterministic clock and modeled device queues. The guest DHCP timer and
  recovery path is also exercised in `dhcp-guest-smoke.json` using QEMU TCG and
  a deterministic Ethernet peer; this is not hardware certification.
- `network-dma-host.json` records 45 host tests compiled against the production
  virtio-network queue, including static-buffer ownership through descriptor
  publication, timeout, malformed completion, and valid completion. Its source
  hashes identify the exact files tested. This validates the permanent static
  staging-buffer lifetime only; it is not allocator pinning, user-page DMA, or
  guest hardware verification.
- `channel-host-benchmark.json` benchmarks the production Helix core framing
  and bounded queue APIs on the host. It excludes kernel syscalls, handle
  authorization/lifetime, devices, and guest latency. The 4 KiB and 64 KiB
  logical payloads are fragmented into 256-byte frames.
- `panic-header-tests.txt` tests the bounded panic metadata formatter. The
  kernel reports a BSP/thread placeholder and no backtrace; this run does not
  inject a guest panic or provide a durable reboot record.

The final qemu64 gallery run uses image
`4cc895e716afda58cf932d0dbebabec9f755e39fb566bbd000741c82301b32bb`. The workspace suite and `tools/build.sh` both pass
against the current source tree. Each retained guest run identifies its exact
image digest; FAT image timestamps can differ between builds. Run
`sha256sum -c SHA256SUMS` from this directory to check the retained files. The
requirement ledger links selected artifacts by their exact content hash.
