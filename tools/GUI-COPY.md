# Bounded conditional Copy foundation

`crates/helix-kernel/src/gui_copy.rs` was included only as unwired source in
the ninth-wave snapshot; its immutable guest image has no Copy action. Source10
connects `file::copy_gui` and `Intent::Copy` to `copy` inside
`filesystem::with_gui_store`, holding that same journal guard through validation,
checksummed source read, durable destination replacement and coherent open-inode
cache publication. It does not grant an application authority. The Prism Copy
button/dialog integration is implemented in Source10; guest validation is pending.

The request contains a backend-issued source Snapshot, destination Path, and
optional exact target Snapshot from trusted overwrite confirmation. Both paths
must be canonical `/native/` paths and distinct. A present destination with no
confirmation is refused; source and confirmed target generations must match the
current journal generation, and the confirmed target must still exist at the
same path. Any unrelated mutation invalidates this global revision token.

Copy reads at most 4 KiB of source bytes, then commits one destination replacement
through `Files::replace` and `Store::put`. Source data and open offsets remain
unchanged. An existing destination's open inode stays coherent and retains its
identity and offset while observing the committed replacement content. There is
no truncate-then-write interval. Namespace validation repeats immediately before
the backend mutation, under the same caller-held guard.

Run `bash tools/test-gui-copy.sh`. Five actual Store/Files host tests pass: new
copy and confirmed overwrite with held source/target handles; unconfirmed,
stale, wrong-path and same-path refusal; full/read-only refusal; device I/O failure
without cache publication and with poisoned journal; exact empty and maximum
4 KiB binary copies with a single generation increment and stale replay refusal.
These are host backend tests, not evidence of a working guest Copy workflow.
The dialog model also tests default-cancel overwrite, exact snapshot forwarding,
stale completion refusal and preservation of source/input after a conflict.
General directories, ACL propagation, large/streamed copies, multi-file copy
transactions and physical power-cut certification remain outside this profile.
