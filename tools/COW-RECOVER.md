`cow_recover.py` exports a fully verified committed snapshot from an offline
regular scratch image. It supports the bounded eight-file, 4 KiB-per-file
`NXTCOW01` development store. It never repairs or writes the source and does not
recover fragments from unknown/corrupt metadata.

```sh
python3 tools/cow_recover.py export scratch.img NEW_DESTINATION --start-lba 131072 --sectors 132 --protect 0:131072
python3 tools/cow_recover.py verify NEW_DESTINATION
python3 tools/test_cow_recover.py
```

An optional `--generation G` selects an older committed generation only when its
entire root and live file payloads still verify. Unreferenced bytes are ignored,
not silently deleted. Stop the VM/writer before export. The tool hashes the whole
source before and after capture; this detects changes during the read but is not
a lock or authentication of the source.

The destination must be new, inside an existing caller-owned parent that is not
group/world writable. Directory-descriptor traversal rejects symlink components.
The export directory uses mode 0700 and files mode 0600. Contents use generated
`slot-N.bin` filenames; the bounded manifest maps them to original canonical
paths and records source SHA-256, generation, bank, sizes, CRCs, and file SHA-256.
Original names are never interpreted as host extraction paths.

Files and manifest are synchronized before atomic no-replace manifest publication.
Completion requires `manifest.json` present and `INCOMPLETE` absent. Interrupted
exports retain their marker/evidence and are rejected by verification. Verification
is read-only and refuses unexpected files, unsafe names, symlinks, special files,
malformed manifests, duplicates and mismatched payload checksums; it never removes
unknown data. The manifest is not signed: checksums detect accidental corruption,
not coordinated malicious changes to both manifest and contents.

The sixth-wave persistence script includes export/verification while the two-boot
scratch image still exists, then compares its generation/files/source digest with
the independent COW verifier and confirms every scratch byte is unchanged. The
exported manifest/files remain under a new private directory in `build/persistence`.
This live gate is pending a new VM run; prior captured evidence does not prove it.

Host fault tests interrupt every publication checkpoint and actually terminate a
subprocess after manifest publication. They check conservative incomplete-state
rejection, source immutability, old-generation recovery, corruption fallback,
symlink/FIFO safety and no overwrite. This is limited read-only recovery export,
not full NextFS fsck/repair, automated restore, encrypted/incremental backup,
production backup authentication, or physical-device durability certification.
