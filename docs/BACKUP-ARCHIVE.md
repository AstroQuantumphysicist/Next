# Development COW backup archive

The current implementation provides a bounded, allocation-free archive codec
and a test/host-gated bridge to the development COW store. It exercises
complete-namespace export and restore; it is not yet a user-facing backup
service or a replacement for the specification's backup contract.

The binary archive uses a 16-byte header followed by a versioned manifest and
the file contents packed in manifest order. The archive header is `NXTBKA01`,
a little-endian `u16` version (`1`), two zero reserved bytes, and a little-endian
`u32` manifest length. The manifest begins with `NXTBKP01`, version `1`, an
entry count, and four zero reserved bytes. Each entry contains a little-endian
path length, two zero reserved bytes, a little-endian `u64` content length, a
SHA-256 digest, and the path bytes. File payloads follow without gaps. Parsers
require exact lengths, known versions, canonical paths, unique entries, and
zero reserved fields.

The archive matches the present journal limits: eight files, 4 KiB per file,
31-byte ASCII paths, 32 KiB total payload, a 616-byte maximum manifest, and a
33,400-byte maximum archive. The digests detect changed bytes but do not
authenticate the archive's author or encrypt its contents.

`export_archive` reads the current journal snapshot into fixed-size buffers
before writing the caller's output buffer. `restore_archive` validates the
complete archive and every digest into fixed staging buffers, rejects paths
reserved by the journal, then replaces the whole namespace with one COW
transaction. Rejected input issues no device writes. Fault-injection tests cut
before and after all 69 write/flush operations and verify recovery exposes the
complete old or new namespace. A device error still requires remount before
the store is written again; durable flush behavior depends on the device.

The Rust API is available to tests and builds selecting the `next-fs`
`host-backup` feature. It does not select a destination, read or write an
archive file, offer snapshots or incremental history, encrypt or sign backups,
mount external media, or present a Prism workflow. It has no independent
rollback if a generic restore target partially fails; the journal bridge
avoids that limitation by staging and committing the complete snapshot once.

Validate the bounded bridge with:

```sh
source tools/env.sh
cargo test --locked --offline -p next-fs --features host-backup
cargo clippy --locked --offline -p next-fs --features host-backup --all-targets -- -D warnings
```

The broader journal format and durability limits are documented in
[FS-FORMAT.md](FS-FORMAT.md).
