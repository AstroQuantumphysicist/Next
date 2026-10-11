# Bounded coherent Trash backend (Source11)

The trusted development-session adapter supports two Trash items, each at most
4 KiB, within the existing eight-object COW store. Source11 exposes
`file::list_trash()` and `file::commit_trash(prism::trash_ui::Intent)` to the kernel
UI. The actual filesystem guard spans snapshot validation, journal transaction
and nonfallible coherent open-inode cache publication. Source11 guest acceptance now confirms the bounded workflow and reboot
restoration below. Source10's frozen image has no Trash feature.

Move uses one non-replacing Rename from a checked `/native/` source to
`/trash/d.ID`, followed by exclusive Create of `/trash/m.ID`. Restore uses one
non-replacing Rename back to an independently checked `/native/` destination and
Remove of the paired metadata. Purge uses two Removes. All operations require an
exact current global generation; stale or forged item fields are refused. No
collision, full Trash or full store condition triggers permanent deletion.

IDs are the checked next committed generation, encoded in 16 lowercase hex
characters; keys are exactly 25 bytes. One metadata object costs one additional
store slot per Trash item. The original native path and held inode identity stay
stable across move/restore. Open offsets and duplicate-description sharing remain
unchanged. Purge makes retained open data anonymous; later writes cannot recreate
either reserved object or the original name.

A held writable descriptor remains usable while its inode is in Trash. Its
private backend path commits Replace(data) and Replace(metadata fingerprint) in
one journal transaction, then ordinary Files.write publishes data/offset changes.
All metadata opens are excluded by the actual `/native/`-only native/Linux/portal
open policy; only data inodes can acquire a Trash link through the trusted move.
Generic kernel/console replace, rename and remove reject `/trash` and `/trash/`.
Uncached trusted developer reads remain diagnostic access, not an application
open grant. Private pairs are excluded from normal Files-panel snapshots and
counts; Trash has a separate validated listing.

The canonical 128-byte record has magic NXTRASH1, version1/length128, zero flags,
trusted owner ID, nonzero item ID, data size/CRC32, native path length and a
31-byte zero-padded path, zero reserved bytes, and CRC32 over bytes0..124 stored
at124. Owner ID1 is bound by the single development-session policy; labels or CRC
never grant authority or establish multi-user isolation. Listing rejects unknown
keys, missing pairs, invalid owner/version/padding, extra records, ID mismatch and
payload fingerprint mismatch without repair. Store reads independently verify
journal file checksums.

`bash tools/test-trash-backend.sh` passes nine actual Store/Files tests, including
414 injected before/after errors across all 69 write/flush boundaries for move,
restore and purge. After errors the live cache retains unconfirmed old names and
bytes and the Store remains indeterminate/read-only. Simulated crash/remount
exposes complete old or new pairs, with protected sectors unchanged. Other tests
cover held writes/fingerprint update and restoration, anonymous purge, collision,
stale/forged items, two-item/eight-object limits, read-only refusal and malformed
or orphaned records. Record codec tests reject every single-bit mutation and
noncanonical encodings. Core cache transaction and journal torn-write tests
provide complementary coverage; successful flushes must be compliant.

No automatic repair, eviction, encrypted or multi-user Trash, directory Trash,
external/network volume Trash, production rollback counter or physical power-cut
certification is claimed. A journal error after the final flush can leave durable
new bytes while the GUI correctly reports failure and keeps old cache state;
trusted remount/reconciliation is required before writes resume.

The retained-descriptor write test adds 138 before/after write/flush failures: live inode bytes and offsets remain unchanged on error, retry is fenced, and remount recovers an old or new data/metadata pair with its matching fingerprint. An open restore destination reports Exists and retains its held inode.

`tools/trash_smoke.py` prepares a Source11-only two-boot guest gate. It never
builds an image and requires a retained Prism source manifest matching the
provided image. The runner uses actual PS/2 GUI Move/Restore/Purge controls and
an original native `trash-probe` application started by the explicit trusted
`runfiles` development command. The application rejects namespace misuse and
holds an open descriptor across the GUI Move before writing/readback. The runner
independently checks the canonical paired record and content, restoration
collision, persisted row text on reboot, cancellation, and both-object purge,
while hashing all bytes outside the authorized 132-sector prefix. Actual Source11 two-boot execution passed on immutable image
`8236ffef6154ef708c37ebe1cf2ba0a750b2c1fc1c543f799fc01060d2a35bee`
with retained compiled-source manifest
`c2b527efc198d5954b5c0d02d503ff68cb2d702c1f274d65128aad1f96b87379`.
Both create and verify boots passed 117 checks, including the actual selected
Trash row rendered from the retained-source font/palette. Source image, source
manifest, helper source and all disk bytes outside the journal prefix remained
unchanged. The final report is `build/eleventh-trash/trash-smoke.json`. Earlier
attempts are retained with their actual failures: Home/Terminal focus assumptions,
a nonexistent Character-T frame-log assumption and an antialias template using
the wrong selected-row background. They were harness errors; no kernel image
was modified. Empty/binary payloads and read-only/full refusal have host coverage,
not an additional guest claim.
