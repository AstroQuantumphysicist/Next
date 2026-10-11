The development trace ring stores fixed-size numeric records without allocation.
A record has monotonic nanoseconds, a never-reused sequence, numeric CPU/thread/
process source/service, category, severity, stable development event ID,
correlation ID, and four numeric fields. Sequence exhaustion rejects events;
timestamp regressions clamp to the previous timestamp and increment an explicit
counter. Capture and export filters do not consume records. All accounting
saturates instead of wrapping.

Kernel integration currently supports one boot CPU. Its 64-record ring uses a
nonblocking guard: interrupt/reentrant producers drop and count events rather
than waiting. Full rings overwrite the oldest record and count overwrites. The
local `trace` command copies and prints the latest 16 records plus counters,
releasing the guard before console formatting. Current hooks cover boot-ready,
process lifecycle/faults, kernel panic, and trusted filesystem replacement,
rename and unlink outcomes. Numeric status and size fields contain no filenames,
user arguments, credentials or key material. The API intentionally has no text
payload; callers must preserve that privacy rule.

This is bounded local development observability. It does not implement SMP
per-CPU rings, user/service trace capabilities, full syscall/category coverage,
support-export review, build-ID crash bundles, stack backtraces, protected crash
storage, or production `next doctor`/capture/summary commands. Panic-time tracing
never writes filesystem or block-device data.
