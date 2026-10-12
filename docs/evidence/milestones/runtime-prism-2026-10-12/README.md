# Runtime and Prism evidence — 2026-10-12

Both QEMU reports use the same signed development image SHA-256:
`708660fd20f13325b7da0cd1dbe52f1f2b71a8ffb959270d63225f636ac5269e`.
The image was built from this feature wave. It is a q35/TCG guest with one
virtual CPU; neither profile certifies physical hardware.

## Guest runs

- `qemu64-gallery/boot-smoke.json` records 117 passed checks using QEMU's
  `qemu64` CPU profile. The completed run captured all nine current Prism
  screens. `qemu64-gallery/prism-*.png` are the same guest frames used by the
  [visual gallery](../../../PRISM-VISUAL-GALLERY.md); the screenshot suite
  checks frame dimensions and nonblank output, not full visual quality or
  accessibility.
- `qemu-max/boot-smoke.json` records 116 passed checks using QEMU's `max` CPU
  profile. It exercises scheduler-backed event waits and the Linux positional
  file fixture, which exits 46. Both runs retain compressed serial output, audio capture,
  and the presented framebuffer image beside their reports.
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
  deterministic clock and modeled device queues. The guest currently proves
  DHCP lease acquisition only.
- `channel-host-benchmark.json` benchmarks the production Helix core framing
  and bounded queue APIs on the host. It excludes kernel syscalls, handle
  authorization/lifetime, devices, and guest latency. The 4 KiB and 64 KiB
  logical payloads are fragmented into 256-byte frames.
- `panic-header-tests.txt` tests the bounded panic metadata formatter. The
  kernel reports a BSP/thread placeholder and no backtrace; this run does not
  inject a guest panic or provide a durable reboot record.

`bash tools/build.sh` also passed and produced the image identified above. Run
`sha256sum -c SHA256SUMS` from this directory to check the retained files.
The requirement ledger links selected artifacts by their exact content hash.
