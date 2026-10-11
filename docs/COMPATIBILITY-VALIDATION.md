# Compatibility validation gates

The frozen evidence for image `03d54128375ad7f167c724ebdf98baca06b5add89f88edce457bf426df326c39` remains in [evidence](evidence/README.md). New interfaces require fresh tests and another image snapshot. This document specifies concrete checks for the next feature wave; it does not record their completion.

## Linux descriptor adapter

Use one deterministic reference model per process: descriptor → shared open description, description → path/content/offset/reference count, plus a bounded filesystem namespace. Generate repeatable operation sequences with an explicit seed, recording the sequence on failure. Model allocation and offsets independently of implementation storage slots.

| Trigger | Observable acceptance |
| --- | --- |
| Open an existing file read-only; read in varying chunk sizes | Exact bytes and shared offset; EOF returns zero without modifying output tail |
| Open absent path without create, or malformed/traversal path | ENOENT or documented EINVAL/EACCES; namespace, descriptor table and image generation unchanged |
| Open with unknown access/creation flags | Reject before file creation/truncation; existing content remains unchanged |
| Fill descriptor table; create/truncate through one further open | EMFILE before any namespace/content mutation; all prior descriptors remain valid |
| Duplicate descriptor; alternating reads/seeks | Both descriptors share offset; closing one preserves the other's identity and data |
| Duplicate onto a live descriptor, including self-duplication | Replacement cleans prior reference once; self-duplication preserves identity/offset |
| Read/write on wrong-access descriptor, stale descriptor or negative descriptor | EBADF; no offset/content/output mutation |
| Bad pointer, overflowed length or partially unmapped output | EFAULT before read offset changes; no partial copy to valid prefix unless explicitly documented |
| Bad write input; backend I/O or flush failure | No cache/offset success commit; indeterminate backend prevents further writes until remount |
| Seek below zero, overflow signed addition or invalid origin | EINVAL/EOVERFLOW as documented; original offset preserved |
| Seek beyond EOF then write | Documented bounded gap zeroing and maximum file size; atomic replacement and correct new offset |
| Remove or rename an open path | Explicitly reject unsupported POSIX lifetime; do not claim unlink-while-open compliance |
| Process fault/exit during descriptor ownership | Every retained open-description reference released once; next process sees no stale descriptor |
| Transfer/duplicate native file capability versus Linux descriptor | Adaptation preserves native rights ceiling; no WRITE gained from an attenuated READ handle |
| Boot fixture writes then reads in a fresh process/VM | Guest-visible bytes match; disk protected region and original image preserved |

Host tests must exhaust small capacities and fuzz valid/invalid descriptor sequences against this model. Guest tests must execute an original static Linux ELF with real syscall numbers, preserve exact binary identity on host Linux and Helix where supported, and validate outputs/error codes rather than only a PASS string. General libc, dynamic linking, signals, permissions, links, PTYs and sockets remain separate contracts.

## Runtime exhaustion and recovery

`python3 tools/runtime_failures.py --timeout 45` runs an independent QEMU instance with a snapshot overlay. It fills the three non-supervisor slots using actual non-yielding native executables, requires NoMemory for a further spawn, checks console/supervisor responsiveness, protects PID 1, kills all children, rejects stale PIDs, reuses a slot with a fresh PID, contains a fresh user page fault and rejects a missing executable. Reports and serial logs stay under `build/runtime-failures/`; source-image SHA256 must remain unchanged. This tests bounded process resources on one CPU, not SMP or every resource class.

Next guest fixtures should fill file/event/channel and anonymous-page capacities, then close/unmap and retry. Each fixture needs generation/stale-handle checks, rights attenuation and output-before-consumption validation. A stopped or failed fixture must not consume another process's authority or poison unrelated objects.

## Blocking waits and network

A blocking-wait fixture must interleave signal-before-wait, wait-before-signal, repeated manual-reset wait, one-consumer auto-reset wake, reset-before-resume, close/kill while blocked and invalid copyout. For channels, add peer-close, full queue, cancellation and handle move rollback. Each case must prove wake result and object ownership; arbitrary sleeps are not sufficient synchronization. Deadline failures must leave no waiter reference after task exit. Cross-core races remain pending until SMP exists.

A TCP test must use a bounded loopback host peer and actual guest packets, with sequence/ack/window/checksum validation. Inject duplicate/out-of-order segments, SYN retry, invalid ACK/RST, short writes, FIN/half-close, peer disappearance and descriptor exhaustion. Fixed-profile echo success alone does not establish sockets, DNS/TLS, congestion control, general interoperability or firewall policy.

## Security and desktop acceptance still missing

Concrete privileged boundary gates include blocked-task cleanup, syscall stack bounds under nested storage operations, stale handles after every slot reuse, IPC transfer rollback on queue/copyout failure, quota exhaustion without unrelated denial of service, package-to-process identity and per-user namespace grants. Production signature trust also requires firmware authentication and protected monotonic generation/revocation storage.

Desktop gates require process output and errors visible without serial access, keyboard-only launch/close/focus recovery, accessible permission/error dialogs, file workflows respecting actual capabilities, and an isolated compositor that can crash/restart without corrupting application buffers. Completed framebuffer images and kernel-hosted panels do not establish these service, accessibility or latency contracts.
