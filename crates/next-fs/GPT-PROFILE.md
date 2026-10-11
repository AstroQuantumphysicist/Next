The independent `gpt` module is read-only partition discovery. It does not write
GPT metadata, install an OS, infer filesystem types from partition names, grant
writable extent authority, repair a damaged copy, or format any device.

`BlockReader` supplies trusted logical geometry and exact block reads. The parser
supports 512-byte and 4096-byte logical blocks, revision 1.0/92-byte headers,
128-byte entries, and at most 128 partition entries. Header extensions, other
entry dimensions and nonzero header/table padding are unsupported. These bounds
are a development profile, not general GPT compatibility certification.

Discovery requires a nonhybrid protective MBR, valid primary and backup headers,
matching disk GUID/usable bounds/entry dimensions, valid table CRC, and identical
table copies. Header and entry/table ranges use checked arithmetic and cannot
intersect usable data. Used partitions require nonzero unique GUIDs, usable
nonoverlapping inclusive ranges, valid UTF-16 names with canonical padding, and
zero reserved attribute bits. Empty entries must be entirely zero. Unknown type
GUIDs remain enumerated and untouched; unknown names never select a filesystem.
A failed copy fails discovery rather than granting writable recovery authority.

`next_layout` selects unique ESP and experimental Next Data GUIDs, plus optional
unique Recovery. Duplicate candidate types are ambiguous and fail closed. A/B
System partitions are enumerated separately; selection does not implement A/B
boot or updates. Required selected partitions with the GPT NoBlockIO attribute
are rejected. High attribute bits are type-specific metadata; the caller must
apply any additional supported-type policy before mounting or authorizing writes.
The experimental Next GUID constants match `tools/gpt_plan.py`; they are not
claims of a registered production partition format.

Run `tools/test-gpt-parser.sh` for fixtures produced by the actual host planner's
plan/encode/inspect functions and hostile Rust tests. Both logical block sizes,
CRC/copy damage, valid-CRC malicious fields, hybrid MBR, overlap, duplicate GUID,
UTF-16 corruption, attributes, ambiguous types, truncation, device errors and
256 seeded mutations are checked. The reader trait exposes no write method and
positive fixture bytes remain unchanged. Guest GPT boot/discovery and user
disk protection require separate live acceptance; host parser tests do not prove
an installer, filesystem probe, recovery service or hardware support.

`helix-kernel/src/volumes.rs` supplies the independent guest read-only adapter.
Its 512-byte block reader validates GPT before constructing an ESP-relative FAT
device; every translated read checks the selected extent and current device
capacity. It reads `/NEXT.TXT` and returns validated ESP/data/recovery extents,
without exposing formatting or write authority. Any GPT or FAT error is returned
without fallback. It must run on the boot CPU under storage's DMA/ownership
contract, outside IRQ stacks. The guest adapter holds its roughly 38 KiB GPT
output and read buffers in static storage under an exclusive atomic guard;
caller-owned `discover_with_workspace` avoids large parser stack objects.
The existing 65 MiB development superfloppy path remains a separate caller policy.

Run `tools/test-gpt-volume.sh` for a sparse scratch GPT encoded by the actual host
planner with a real FAT32 ESP. Adapter regressions cover the partition-relative
note read, overflow/escape/media-size guards, corrupted GPT refusal, an oversized
BPB, missing files, small outputs and backend errors. The main guest path invokes
read-only discovery on protective-MBR or non-FAT media and then stops before
filesystem mutation. Live alternate-image proof is pending; this scratch fixture
is not a boot image. Storage marks any MBR 0xEE entry as protective, and the
development COW authorization refuses such media even with a forged FAT BPB/tag.
