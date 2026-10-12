# Development COW data store, format 1

This bounded store implements durable file transactions for development. It is not the complete NextFS filesystem, a POSIX VFS, a production deployment layout, or a certified primary filesystem. It supports eight case-sensitive file names and at most 4 KiB per file. CRC32 detects accidental corruption; it provides no authentication, encryption, or package trust.

The development disk retains its original 64 MiB FAT32 boot volume. The separate writable extent begins at LBA 131072 and uses 132 sectors (67,584 bytes). It is outside the entire FAT32 volume. This trailing extent is a development convention, not a GPT partition claim. The store itself can operate on another explicitly authorized extent through `Partition::new`; that constructor checks device bounds, integer overflow, and overlap with every supplied protected extent. Partition metadata and authority must come from a trusted caller.

All sectors are 512 bytes. File slots are 4 KiB. All integer fields are little endian, reserved bytes must be zero, and unknown format versions fail closed. Sector CRC32 covers bytes 0 through 507 inclusive; its four-byte value occupies bytes 508 through 511. The polynomial is the standard reflected CRC32 polynomial `0xedb88320`, initialized to all ones and complemented at the end.

| Relative sector | Contents |
| --- | --- |
| 0 | Commit superblock A |
| 1 | Commit superblock B |
| 2–65 | Bank A: eight consecutive 4 KiB file slots |
| 66 | Bank A metadata root |
| 67–130 | Bank B: eight consecutive 4 KiB file slots |
| 131 | Bank B metadata root |

Each root entry's index selects the same-index file slot in its bank. A file's CRC covers its declared contents. Bytes after the declared length through the end of a used file slot must be zero. Empty files are valid; unused root entries are entirely zero. There are no extent trees or dynamic allocation: a transaction reserves the inactive bank implicitly, and every file always has a complete fixed slot.

| Superblock byte offset | Field |
| --- | --- |
| 0 | Eight-byte magic `NXTCOW01` |
| 8 | `u32` format version, 1 |
| 12 | `u32` sector size, 512 |
| 16 | `u64` committed generation, nonzero |
| 24 | `u32` bank, 0 or 1 |
| 28 | `u32` file-slot count, 8 |
| 32 | `u64` absolute extent starting LBA |
| 40 | `u64` extent sector count |
| 48 | `u32` expected metadata-root CRC |
| 52 | 456 zero reserved bytes |
| 508 | `u32` superblock CRC |

| Root byte offset | Field |
| --- | --- |
| 0 | Eight-byte magic `NXTRT001` |
| 8 | `u32` format version, 1 |
| 12 | Four zero reserved bytes |
| 16 | `u64` generation, matching its commit superblock |
| 24 | `u32` occupied entry count |
| 28 | Four zero reserved bytes |
| 32 | Eight 56-byte entries |
| 480 | 28 zero reserved bytes |
| 508 | `u32` root CRC |

Each entry contains a 32-byte zero-padded name, `u32` content length, `u32` content CRC, and 16 zero reserved bytes. Names are absolute ASCII paths of 2–31 bytes, using letters, digits, `/`, `.`, `_`, and `-`. Empty components, `.` and `..` components, unterminated names, duplicate names, and nonzero name padding are rejected. Slash-separated names are keys; this format does not implement directory objects, links, permissions, ownership, Unicode normalization, or mount namespaces.

Formatting is explicit and destructive. It first invalidates both superblocks and flushes, clears both banks, installs and flushes the empty generation-1 root in bank A, then writes and separately flushes both initial superblocks pointing to that root. Mount never formats an unknown or corrupt volume.

A normal transaction follows this sequence:

1. Validate names, limits, and available file slots before issuing writes.
2. Copy the active snapshot into the inactive bank with the requested content or namespace change. Write all 64 data sectors.
3. Flush the data writes.
4. Write the new CRC-protected metadata root with the next generation.
5. Flush the root.
6. Write the CRC-protected commit superblock corresponding to the new bank.
7. Flush the commit superblock, then publish the new in-memory state.

The previous active bank remains intact throughout the transaction. A successful operation means the device reported all flush barriers successful. Devices that cannot flush must return an error; the driver cannot substitute a fake successful flush. Controller caches, dishonest firmware, and hardware that violates its advertised durability contract remain limitations.

`put` creates or replaces a whole file. `rename` atomically replaces an existing destination with the source name and contents. `remove` atomically removes a name. After a crash, a valid committed generation exposes the complete old or complete new state. A transaction error can mean either state reached disk, so the mounted writer becomes indeterminate and rejects further changes until remount. Generation exhaustion returns an error before issuing writes.

Mount examines both superblocks, tries valid generations in descending order, checks the matching root, validates every occupied entry and payload, and selects only a complete snapshot. It can recover the previous snapshot read-only when newer metadata or data is corrupt or unreadable. An invalid nonblank superblock or unreadable superblock also forces read-only recovery. If no complete snapshot survives, mount fails. Mount and recovery issue no writes. This is bounded verification and snapshot selection, not a general `fsck`, salvage tool, or journal log replay.

Host tests inject cuts before and after every one of the 69 transaction write/flush operations, using both cached and write-through device models. They also exercise torn data/root/superblock writes, atomic rename/delete cuts, failed flushes, unreadable sectors, corrupted metadata/data, full file tables, oversized files, generation wrap, protected-extent preservation, and metadata mutations with recomputed CRCs. These tests establish behavior under the modeled compliant-device contract, not arbitrary hardware power-loss certification.

`tools/persistence_smoke.py` adds a separate integration check: it boots two fresh QEMU instances using the same writable scratch copy, writes and flushes `/persistent` in the first instance, reads its expected contents in the second instance, then durably removes it. It compares the protected FAT32 extent byte for byte by SHA256 and verifies the original generated image remained unchanged. This tests clean VM reboot persistence; fault-injection host tests supply the crash-boundary coverage. Neither test implies complete NextFS, real hardware certification, snapshots exposed to users, encryption, quotas, or production update/rollback support.

The d4f7b7da7ef8 persistence milestone additionally runs the read-only host verifier against the actual two-boot guest-written journal, selects generation 94, and checks durable removal of `/persistent`. This cross-check validates the development serialization and clean-reboot workflow; it is not a repair operation or physical power-loss certification.

The separately gated archive codec and one-transaction journal restore are
described in [BACKUP-ARCHIVE.md](BACKUP-ARCHIVE.md). That feature is a bounded
host/test slice, not an integrated backup service.
