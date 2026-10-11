# Bounded atomic journal transactions

`Store::transaction(expected_generation, operations)` accepts one or two ordered
`journal::transaction::Mutation` values: Create, Replace, Rename, or Remove.
Create is exclusive; Replace requires an existing file; Rename requires explicit
`replace=true` to overwrite another path. Paths follow the existing canonical
journal rules, and supplied data is limited to 4 KiB per file. The exact global
generation must match a trusted caller snapshot. This primitive supplies neither
permissions nor a Trash service.

Operations stage against the preceding operation's private namespace result.
All count, generation, path, existence, size, capacity and read-only checks occur
before any sector write. A bounded 4 KiB scratch buffer then checksums every
current file, including files scheduled for removal/replacement, so a transaction
cannot hide corruption by deleting its entry. Preflight errors issue zero writes
and checksum/media failure attenuates the mounted Store to read-only.

One existing inactive-bank transaction writes all data, flushes, writes the root,
flushes, advances one superblock generation, and flushes. The narrow commit
extension supports borrowed replacement payloads in two slots while preserving
existing put/remove/rename behavior and the exact on-disk format. No allocation
or unsafe code was added. The receipt reports one old/new generation pair.
I/O failures during commit retain the existing indeterminate state and forbid a
retry until trusted remount. Successful flushes must be honored by the device.

Six actual Store tests pass, including 276 before/after failure cases across all
69 write/flush boundaries in write-back and write-through models, and 396 torn
sector cases. Rename-plus-metadata-create and its inverse publish in one commit;
recovery exposes a complete old or new namespace with matching metadata, never
a renamed file alone. Other tests cover two separate payload replacements,
create-then-rename and slot reuse, validation/count/conflict errors, full/read-only
refusal and post-mount checksum corruption. Existing golden format and crash
tests, Appearance/journal cross-tests and Copy backend tests still pass.

This API is not wired into kernel VFS or GUI mutations. Before a Trash/restore
feature can use it, a trusted adapter must reserve metadata/path authority, stage
all affected open-inode updates, commit under the same journal/cache guard, and
publish those cache updates without a post-commit allocation failure. Calling
Store transactions directly while named files are cached would bypass the
coherent Files adapter. Eight total file slots limit simultaneous user and
metadata files; reservation/quota and restore-conflict policy remain required.
No directories, general transaction count, retained historical snapshots,
encrypted metadata or physical power-cut certification is claimed.
