# Source12 bounded Trash undo foundation

`crates/helix-kernel/src/trash_undo.rs` implements a caller-owned, single-operation
session receipt using actual coherent Files/Store transactions. It is tested as
an independent host adapter. Live Source12 file::commit_trash now uses it under
the existing filesystem/cache guard, and file::trash_undo_available plus
file::undo_trash expose the exact receipt to the trusted UI. Root main now
acknowledges the exact UI request before refreshing authoritative availability;
guest GUI proof is still pending.

The caller must hold the same trusted filesystem/cache guard across both
`History::commit` and `History::undo`. A successful Move stores an inverse
Restore to the original path. A successful Restore stores its original canonical
128-byte Trash record, enabling one inverse Rename+Create transaction that
preserves the exact original item id and original path even when the user chose
a different restore destination. Held descriptions keep inode identity, bytes,
and offsets across both directions. There is no overwrite path.

All fallible receipt construction precedes the forward commit. Failed
validation does not replace the previous receipt. Undo requires the exact
receipt generation and current Store generation plus the checksummed payload
size/fingerprint. An intervening mutation, including a retained-descriptor
write, rejects undo. An indeterminate I/O failure leaves the existing Store
read-only fence intact; retries issue no writes. A successful undo consumes the
receipt once. Purge clears the receipt and has no undo because removed bytes
are not retained elsewhere.

Receipts are session-only and grant no authority. A new History after reboot
has no receipt; this is not a durable undo log, redo stack, multi-operation
history, or undo of overwrite. The receipt adds no persistent objects and does
not change the journal format. Within the current eight-object/4KiB store,
a durable history retaining deleted/overwritten bytes would need separate
reserved storage and transactional capacity accounting, not optimistic recovery
from the inactive bank.

`bash tools/test-trash-undo.sh` runs six actual RAM Store/Files tests. They cover
exact alternative-restore metadata preservation, held inode/offset lifetime,
single consumption, stale generation, intervening writes, Purge, reboot loss,
read-only refusal, and failed forward restoration preserving the previous
receipt with zero writes and unchanged full media. The cross-component actual
Shell/Store test exercises a held-write race, Conflict completion before receipt
retirement, and stale callback replay rejection without changing newer bytes.
The fault matrix injects
before/after errors at all 69 write/flush boundaries for each inverse (276
cases), verifying unchanged live cache/offsets, read-only retry fences, complete
old/new recovered namespace pairs, and protected-sector preservation.

The file adapter clears stale/read-only receipts while querying under the
Store guard. UI integration must only offer Undo while its receipt generation equals
the authoritative current Store generation, pass that generation back on
activation, refresh from the confirmed backend result, and clear session state
at owner/session changes. A menu label alone is not this integration.
