# Bounded explicit directories in the development COW store

`next_fs::journal::directories` adds a checked directory namespace to the
existing eight-object, 4 KiB-per-object COW journal. It is a storage-layer API:
the caller must hold the trusted store guard, supply the trusted owner identity,
and use the listing generation for mutations. It does not grant authority from
paths or checksummed metadata.

`/native` is the sole implicit directory. Each nested directory is represented
by one checksummed 96-byte record stored at `/dirs/m.<immutable-id>`. Its record
binds the trusted owner, immutable id, and canonical path. Directory markers
share the eight object slots with files, Trash metadata, and preferences. A
listing validates all markers, owners, paths, payload checksums, and explicit
parents before returning sorted immediate children. It excludes private
metadata objects from child rows. A regular file containing the marker magic
remains a file.

Mutators require an exact store generation and preflight the complete namespace
before committing one journal transaction. `mkdir_at` and `rename_snapshot`
add snapshot identity checks; receipts contain the previous and committed
generations. Deletion and rename require an empty directory. A failed/indeterminate
store transaction preserves the journal's read-only write fence. The new module
does not alter the on-disk journal format or recovery protocol.

This is not a POSIX VFS directory implementation. The 31-byte canonical path,
eight-object capacity, and 132-sector store bounds still apply. Existing
unmarked nested files do not create directories and are rejected during a
namespace scan. The kernel Files cache, open-inode lifetime, create/rename entry
points, Trash restoration, Linux/NT adapters, and Prism navigation still require
trusted adapters before this API can safely drive user-visible file operations.
There is no automatic migration, links, directory ACL model, or atomic rename of
a non-empty tree. See the module contract in
[`directories.rs`](../crates/next-fs/src/journal/directories.rs).

The host suite exercises remount, exact snapshots, collision/corruption,
read-only/full behavior, and 552 injected before/after write/flush failures
across nested mkdir, empty-directory rename, rmdir, and nested file create.
These tests do not establish physical power-loss durability beyond the block
device flush contract.
