# Immutable fourth-wave development milestone

Image SHA256: `d4f7b7da7ef83a3705132449c3df14909e756ff52ced30c38b0c51b992f62da1`.

The exact 143-file implementation snapshot was copied unchanged from `work/fourth-wave/source-manifest.json` (snapshot SHA256 `ca46d3602a73e4ac0432ae4d3580aa9dd102407bcba71d974b9f85bb762da2dc`). Later RTC/search/portal source changes are outside this image. Previous milestone directories remain unchanged.

Retained scopes:

- 256 workspace Rust tests; separately scoped installer 8, storage 7, Linux profile 28 and SDK 3 tests, plus COW/GPT/coverage/audit Python checks and formatting/Clippy. Separate suites can repeat primitives.
- QEMU qemu64 and max CPU profiles each pass 103 assertions, including signed application principals/tamper denial, advancing owned UC HPET counter, read-only LAPIC/IOAPIC discovery, local synthetic DNS retry/cache/expiry and numeric process/namespace trace. PIT remains the scheduler clock.
- Ten cases each for kernel, init archive and signed-deployment refusal: 30 total.
- Two independent writable VM boots pass six persistence checks, including read-only checking of the actual guest-written COW journal.
- Six runtime checks cover bounded process-slot exhaustion/reuse, protected supervisor/worker and fresh user-fault containment.
- 120 real PS/2 focus/typing latency samples retain host timestamps/frame generations and verified noncooperative timer load. This fixed-order TCG development reference includes concurrent host work; no default performance threshold, IRQ/hardware/vblank guarantee or certification follows.

`observations/dns-peer-attribution/` retains the failed persistence attempt and raw guest output. A concurrently running latency VM reached the persistence validator's DNS peer and invalidated its request-count attribution. The isolated persistence rerun passed on the same image. `observations/scheduling-context.json` records the host-validation limitation; this is not evidence of a guest DNS/storage defect.

`validator-updates.json` records any post-snapshot validation versions separately; it does not regenerate the image source snapshot. The inventory remains 108 partial, 87 pending, zero verified requirements. Full Next 1.0 acceptance and physical hardware profiles remain unverified.
