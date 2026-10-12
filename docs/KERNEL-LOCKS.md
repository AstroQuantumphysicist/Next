# Kernel lock and interrupt rules (development profile)

This note records the synchronization visible in the current x86-64 kernel. It
is a source audit, not a claim that the kernel has lockdep, numeric lock-rank
annotations, or a complete global lock hierarchy. Its execution model is one
boot CPU; AP startup code exists, but shared services are not SMP-safe.

## Observed nested order

Only these nested edges are established by current call paths:

| Outer guard | Inner guard/resource | Source path |
| --- | --- | --- |
| `kernelportal::LOCK` | `filesystem::LOCK` (`MutationLock`) | Portal approval holds its broker guard while opening a file; the file backend reaches the mounted-store adapter. |
| `linux_fs::LOCK` | `filesystem::LOCK` (`MutationLock`) | Descriptor operations hold the owner-table guard while calling `file`; file backend I/O reaches the mounted-store adapter. |
| `network_socket::LOCK` | `network_runtime::LOCK` | Socket/DNS health checks and the socket pump call the network runtime while the socket guard is held. |

Treat each row as an outer-to-inner rule if adding a reverse call path. The
current code does not assign numeric ranks or dynamically check these edges.
Filesystem journal operations then use the store and block-device adapter
while the mutation guard is held; that adapter has no separate blocking lock.

The remaining shared-state guards are local try-only gates: `native_surface`,
`trace`, `console_log`, and the serial transmit queue. Input decoder state uses
a one-shot atomic busy claim. They return, use a fallback snapshot, or count a
drop on contention; no source path establishes a nested order among them.
`storage::CLAIMED`, `volumes::CLAIMED`, and clock initialization atomics are
one-time ownership/publication states, not mutexes.

The native process runtime and event store are serialized by the current
single-CPU entry model and interrupt masking, not by a runtime mutex. Native
syscall entry masks interrupts; trusted console operations use an
`InterruptGuard` that restores the previous IF state. This mechanism does not
provide inter-CPU exclusion.

## Hard-IRQ behavior

The installed timer IRQ path runs with IF clear. Its acknowledgement and tick
accounting use bounded port/MMIO accesses and atomics; the timer callback does
not acquire a waiting lock, allocate, sleep, or context-switch while holding a
lock. A quota-clock failure may log through the serial queue. That queue uses a
single compare-exchange attempt and drops on contention, then performs a
bounded UART poll if acquired. This is non-waiting lock behavior, but it is not
a constant-time interrupt-latency guarantee.

Guards used by the shared adapters above use one-shot `compare_exchange` and
return `Busy`/`WouldBlock` (or skip the operation) on contention; none spins
until a guard becomes free. The mounted filesystem's `MutationLock` has a
focused contention test that checks prompt `Busy` return and release. These
properties support the no-wait rule for those guards. They do not prove every
future hard-IRQ call path safe: filesystem, network setup, volume discovery,
and hardware initialization must remain outside hard IRQ context. The current
keyboard/mouse service is bounded polling from the bootstrap loop, not an IRQ
handler.

## Limits of this audit

The source has no general rank checker, per-core lock ownership, interrupt
priority policy table, or SMP lock discipline. AP bring-up does not make these
single-CPU stores safe to use from another CPU. Numeric ranks and a complete
IRQ-call-graph audit remain unimplemented; this document addresses only the
observed order and non-waiting guard behavior above.
