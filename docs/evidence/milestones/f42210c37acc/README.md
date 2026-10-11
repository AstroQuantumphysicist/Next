# Development milestone f42210c37acc

This immutable snapshot records image SHA256
`f42210c37accd66683c1783336f122b5c770ffa165304ceb83621f83255de370`.
The exact 134-file `source-manifest.json` was copied from the captured third-wave
snapshot, never regenerated from the next source wave. Earlier milestones remain
unchanged. `validator-updates.json` and retained script copies separately identify
post-snapshot validation fixes; these are not an image source recapture.

`validation.txt` records 231 workspace Rust unit/integration tests plus separately
scoped installer, storage, Linux-profile, SDK, COW-verifier, GPT, coverage and audit
checks, formatting and strict Clippy. Separate invocations can repeat primitives;
do not sum them as unique tests. Typed SDK wrappers, numeric tracing models, GPT
planning/inspection and read-only COW verification have host proof, not newly
certified guest services. `audit-summary.json` records 105 partial, 90 pending
and zero verified requirements; no physical certification is present.

`boot-smoke.json` and `cpu-max/boot-smoke.json` each record 94 checks on this image.
New actual guest scenarios cover shared inode content/independent opens,
duplicate offsets, rename replacement, detached unlink lifetime/recreated-name
isolation; explicit FS/GS ownership across gates/preemption/overlapping processes;
a bounded supervisor-owned worker with crash restart/backoff/heartbeat recovery;
ACPI processor/IOAPIC/override discovery; and bounded UTF-8 Editor load/edit/save
with external-change conflict preservation. HPET is safely unavailable due to
`PhysicalRange`, so PIT remains the live clock. No guest HPET counter, SMP,
general service suite, user compositor or stable SDK is implied.

The three loader-refusal reports each contain ten passing cases. `persistence.json`
and both phase directories record five checks across two independent boots with
real writes/flushes, source-image protection and an unchanged FAT32 span.
`runtime-failures/` records six checks: PID1 plus its protected persistent worker
and two non-yielding console children exhaust the four slots, a further spawn
fails, stale PIDs reject, slots reuse safely and a fresh user fault is contained.

`observations/stack-overflow/` retains an earlier failing build's raw observations.
The fixed boot path reduces retained initialization stack state; this is not a
claim of an added stack or arbitrary stack certification. A prior VFS fixture
used an incorrect flag; only an explicitly labeled history summary is retained
for that attempt, not an invented raw log. `observations/persistence-validator/`
retains the initial failed persistence report/serial data: a validator matched a
previous command frame before the complete PID arrived. The corrected validator
waits for the full PID and synchronizes response/frame boundaries; its unchanged
image rerun passes. Failed observations are not presented as passing evidence.

Audio/framebuffer artifacts preserve the short AC97 waveform and completed
software frame. Automated properties do not establish application audio,
accessibility or latency acceptance. `build-manifest.json` and
`dependency-inventory.json` preserve artifact/dependency bookkeeping.

See [architecture](../../../ARCHITECTURE.md), [development](../../../DEVELOPMENT.md),
[native ABI](../../../NATIVE-ABI.md), [Linux profile](../../../LINUX-PROFILE.md)
and [NT profile](../../../NT-PROFILE.md) for exact bounds. Current unvalidated
identity/DNS/MMIO/other source work belongs to a later milestone. Full Next 1.0
remains unfinished; hashes audit recorded evidence, not product completeness.
