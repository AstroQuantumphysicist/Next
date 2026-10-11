# Native development directories

The current kernel exposes the bounded directory layer only on the development
Store. `/native` is its implicit root; every nested folder needs a checked
`/dirs/m.<id>` record. Listing and mutations run under the same journal guard as
file streams and the Files cache. UI actions must carry the exact folder
snapshot and generation they observed, then publish a refreshed listing only
after the journal receipt commits.

The one-volume image build supplies a domain-separated owner identity from its
mount policy. Marker bytes and UI paths never supply authority. Private marker
objects are omitted from Files snapshots, search results, normal file reads,
and generic mutations. File reads and writes validate the complete persisted
ancestry, and exclusive nested creation goes through the checked directory
transaction. Rename and remove remain file-only in generic syscall adapters;
empty-directory rename and removal use snapshot-bound trusted UI actions.

The development Store still has eight shared file slots, 4 KiB per file, and a
31-byte ASCII path limit. Directory creation consumes one slot. Existing flat
`/native/<name>` files remain valid. An orphan nested path, malformed marker,
missing parent, file/directory collision, or unsupported cross-namespace move
fails closed. No migration of existing opaque paths, multi-volume ownership,
links, open directory handles, or general POSIX directory semantics is claimed.

## Actual guest acceptance

`tools/files_directory_smoke.py` runs two serialized QEMU boots from an immutable
development image using actual QMP pointer and PS/2 keyboard input. The first boot
creates `/native/alpha` and `/native/beta`, creates a nested folder, renames it in
place, moves it across parents, verifies a non-empty removal is rejected, removes
the child and then the empty folders. A separately compiled helper is bound to the
retained Prism, NextFS and kernel producer sources; while QEMU is paused it checks
the committed explicit-directory listing and generation without writing the image.
The second boot remounts the same scratch image and checks that the remaining nested
folders survived restart. Raw serial, input timeline, completed guest screenshots,
image identity and source-binding hashes are retained under `build/files-directory-smoke/`.

Run it only after freezing and building Source16, with other QEMU acceptance jobs
idle:

```sh
source tools/env.sh
python3 tools/files_directory_smoke.py \
  --image work/sixteenth-wave/frozen-src/build/next-dev.img \
  --source-manifest work/sixteenth-wave/source-manifest.json \
  --frozen-source-root work/sixteenth-wave/frozen-src \
  --output-dir build/files-directory-smoke \
  --cpu qemu64 --store-start 131072
```

The validator never builds or changes the input image; it mutates only a temporary
copy below `work/`. Its scope is the bounded single-profile development volume and
empty-directory move/remove behavior, not generic POSIX directory semantics.
