# Appearance and file mutation durability checks

Run `bash tools/test-appearance-journal.sh`. The harness connects the actual
Appearance preferences codec and `save` function to the actual development COW
journal through its required `Persistence` interface. It does not substitute an
in-memory atomic map for the journal.

Five cross-component tests pass alongside the nineteen existing codec/journal
checks. They inject an I/O error before and after each of the 69 real sector-write
and flush operations in an Appearance replacement. At each of the 138 error
boundaries, active preferences stay unchanged, further journal writes are refused
as indeterminate, a simulated crash drops unflushed controller writes, and remount
returns a complete valid old or new preferences record. The unrelated Editor file
and all sectors outside the authorized journal prefix remain intact. Successful
save activates the new preferences only after the final flush and survives the
same simulated crash. Full/read-only stores, invalid preferences checksums,
missing records and oversized records cannot silently repair, truncate or create
preferences during load; their refusal paths perform no media writes.

The model assumes compliant successful flushes. An error after the final flush can
leave new bytes durable even though the application correctly kept old active
settings. The Store becomes indeterminate/read-only, preventing a retry from
clobbering a possibly committed root. A trusted remount/reload must resolve that
uncertainty before permitting further writes; returning a failed save is not a
promise that old media bytes remain unchanged.

The ninth-wave trusted Files adapter separately holds one journal guard across
revision and namespace validation, durable mutation and coherent VFS-cache
publication. Its actual Store tests cover conflicts, out-of-space/read-only
refusal, failed writes, and held inode behavior across rename/unlink/replacement.
Generic Files mutations cannot access `/prefs/appearance`; the dedicated trusted
preferences replacement uses the coherent file cache.

These tests validate the bounded eight-file, 4 KiB-per-file development journal,
not full NextFS. They do not certify dishonest/lost successful flushes, physical
power-cut hardware behavior, general multi-object transactions, persisted inode
identities, filesystem encryption, indexed directories, quotas, snapshots,
production rollback protection, or concurrent AP filesystem mutation. A GUI
preferences save/reboot proof remains a separate guest acceptance gate.
