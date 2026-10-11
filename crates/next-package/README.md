# Signed development bundles

This allocation-free verifier authenticates bounded native application or OS
boot bundles using `sha2` 0.10.9 and `ed25519-dalek` 2.2.0, with default features
disabled. Their exact transitive versions and registry checksums are pinned in
Cargo.lock. It does not extract files, install packages, grant permissions,
resolve dependencies, manage repository expiry/revocation delivery, or implement
UEFI Secure Boot. Callers provide authoritative trust anchors and revocations.

The canonical format is little endian, schema 1, x86-64, native ABI 0.1:

| Offset | Header field |
| --- | --- |
| 0 | Eight-byte `NXTAPP01` magic |
| 8 | u16 schema, u16 architecture (62), u32 header size (256) |
| 16 | u64 flags (zero), u64 complete bundle size |
| 32 | u32 file count, u32 executable index |
| 40 | u64 requested permissions |
| 48 | Three u32 numeric version components, four zero reserved bytes |
| 64 | SHA-256 of the signer's 32-byte Ed25519 public key |
| 96 | Canonical package ID in a 64-byte zero-padded string |
| 160 | u16 native ABI major/minor, u16 kind (1 app, 2 deployment) |
| 166 | Reserved zero bytes through offset 255 |

Each 128-byte entry contains a zero-padded 64-byte relative path, u64 payload
position and size, SHA-256 digest (32 bytes), and 16 reserved zero bytes.
Entries are sorted lexically, uniquely named, and followed immediately by their
contiguous payloads, then a 64-byte Ed25519 signature. The signature covers the
entire canonical header and entry table, binding the full content tree through
its digests. Every payload is hashed before a `VerifiedPackage` is returned.
No mutable metadata or unchecked archive extension is exposed after verification.

Limits are 32 files, 8 MiB per file, and 16 MiB for a whole bundle. Files are
regular payloads only; no links or special file kinds exist. Relative paths use
lowercase ASCII, digits, slash, period, underscore and hyphen; traversal,
empty components, trailing periods, DOS device aliases and duplicate names are
rejected. App executables must be under `bin/`; deployment executables under
`boot/`. Application entries end in `.elf` or `.exe`; deployment entries end in `.elf`. The caller still validates the executable ABI.

Permission bits are log (1), user-selected files (2), network (4), clipboard
(8), process launch (16), and IPC (32). These are requests, not grants. A trust
anchor's capability and package-kind ceilings further restrict authentication;
installation and runtime policy remain separate. Unknown bits fail closed.

The host CLI supports:

```
next-package keygen PRIVATE_SEED PUBLIC_KEY
next-package verify PUBLIC_KEY BUNDLE
next-package pack PRIVATE_SEED ID MAJOR.MINOR.PATCH PERMISSIONS_HEX \
  app|deployment ENTRY_PATH OUTPUT PATH=SOURCE ...
```

Development key generation reads the host OS randomness source, creates a
private seed with mode 0600, and reuses an existing matching key pair. Neither
private keys nor production trust roots are embedded in this crate. Host key
storage is a development facility, not a production signing service or HSM.
Boot builds select a public trust root with `NEXT_DEV_PUBLIC_KEY`; authenticating
kernel/init-image bytes does not authenticate an unsigned EFI loader itself.

Tests include SHA-256 known-answer vectors, RFC 8032 Ed25519 vectors, altered
messages/signatures/digests, weak keys and R, noncanonical S/key encodings,
revocation, unknown signers, permissions, path aliases, reserved fields,
malicious offsets, every truncated prefix, and bounded byte mutations.

## Host development installer

The optional `host-tools` feature provides `installer::Installer` and CLI
`install`, `update`, `uninstall`, `rollback`, and `list`. This runs on the build
host's filesystem; it is not yet a kernel package service or installer for the
bounded development COW volume.

```
mkdir -m 700 work/apps
next-package install work/package-dev.pub work/apps example.nextapp
next-package update work/package-dev.pub work/apps new-example.nextapp
next-package rollback work/package-dev.pub work/apps org.example.app
next-package list work/apps
next-package uninstall work/apps org.example.app
```

The caller owns an existing private root and must protect its ancestors from
hostile renaming. Roots writable by another user/group, symlinks, special-file
payloads, unsafe paths, conflicting package IDs, and corrupt metadata fail
closed. Writers serialize with an exclusive lock file. Signature and complete
content verification occur before creating locks, staging files, or registry
state. No package scripts run and declared permissions remain requests.

Each deployment is named by the SHA-256 of its complete authenticated bundle.
Files live inside that deployment's private namespace, so another application's
same relative filename cannot overwrite it. Payloads, directories, and bundle
bytes are synchronized before activation. Installation/update/uninstallation
commit with one atomic rename of a bounded registry containing at most 16
installed application IDs. Reopening the installer reads the committed state.
Updates retain the preceding content digest; rollback rechecks its signature,
trust/revocation policy, and extracted contents before activation.

Precommit failures leave active registry entries unchanged. A synchronization
failure after registry rename is explicitly `Indeterminate`: the caller must
reopen and inspect the registry, rather than assume the commit did not happen.
Persistence still depends on the host filesystem and compliant hardware honoring
synchronization requests. A process crash can leave a stale lock or inactive
staging directory; these are not automatically erased or treated as trusted.
Owner-directed recovery is needed. Uninstall removes registry activation but
retains immutable deployments; garbage collection is not implemented.

This prototype has no dependency solver, repository expiry/rollback protection,
user-data migration hooks, package garbage collector, or runtime permission UI.
It does not defend against an attacker who already controls the private root or
its parent directories. Kernel installation and application execution must still
validate native executable ABI and enforce process capabilities.

Eight installer tests use real temporary directories and cover fresh install,
update/reopen, rollback, uninstall, conflicting IDs, isolated file ownership,
invalid signatures before mutation, two injected precommit failure boundaries,
corrupt extracted files, corrupt rollback deployments, symlinks, malformed registry
state, revoked rollback, and wrong-key rejection before destination access.

## Authenticated trust-root rotation

The allocation-free `trust` module verifies owner-authorized key transitions
and chains. A transition uses exactly 208 bytes:

| Offset | Field |
| --- | --- |
| 0 | Eight-byte `NXTKEY01` domain marker |
| 8 | u16 schema (1), u16 record size (208), four zero reserved bytes |
| 16 | u64 next generation |
| 24 | Old signing-key ID (SHA-256 of its public key) |
| 56 | New 32-byte Ed25519 public key |
| 88 | New signing-key ID |
| 120 | u64 permission ceiling |
| 128 | u8 package-kind ceiling; reserved zero bytes through 143 |
| 144 | Old key's Ed25519 signature over bytes 0..144 |

`Transition::verify` requires a caller-protected `TrustState`: current
generation, anchors, and revoked IDs. The old key must remain trusted and
unrevoked. The next generation must equal the current generation plus one,
without wrapping. The replacement must be canonical, non-weak, not already
trusted or revoked, and cannot request wider permission/kind ceilings than its
signing predecessor.

`verify_chain` accepts at most 32 records, maintains up to 32 active anchors and
64 revoked/retired IDs, and changes caller output only after every transition
passes. Retired keys cannot reappear later in the chain. Its result contains the
new generation and retired IDs; the caller must commit these together with the
resulting anchors in protected durable storage.

This provides cryptographic transition verification, not a production monotonic
counter. Replaying both an old chain and an old caller state remains possible
unless the platform protects the persisted generation and revocation state.
The EFI development loader currently compiles one owner-selected public key;
rotation is not automatically connected to firmware enrollment or boot-state
storage. Six tests cover authenticated chains, exact generation, escalation,
revocation, retired-key reentry, malformed lengths, weak replacement keys,
altered signatures, and unchanged output after a failed chain.


Development application image pipeline

`tools/build-apps.sh` builds the host package CLI once and signs a separate
Application envelope for every raw Native/Linux/NT fixture. Raw build outputs
remain unchanged for executable parser tests. Mirrors live under `build/apps`
and keep each `.elf`/`.exe` archive filename; their bytes are signed NXP bundles,
not bare executable images. Entry paths are `bin/<filename>`, version is 0.1.0,
and IDs are `org.next.development.<native|linux|nt>.<stem>`. The profile component
prevents collisions between fixtures with the same stem.

These owner-signed development manifests declare the current 0x3f ceiling.
Declarations do not grant authority: the runtime separately applies its explicit
development launch policy and capability attenuation. This broad development
policy is not a production permission broker or user-consent implementation.
One intentional `/system/untrusted.elf` negative fixture has correct signed
metadata and payload but a corrupted application signature; the outer initrd
CRC/deployment signature remain valid. It must be rejected before task creation.
The current 31 positive fixtures plus that negative use all 32 archive slots.

`tools/initrd.py` takes only these Application mirrors and rejects raw/deployment
payloads. Runtime verification still authenticates each envelope independently
before extracting its verified executable bytes. `tools/test_apps.py` verifies
all positive envelopes, their exact raw payloads and metadata, and rejection of
the negative using the owner's public key, without reading key seed contents.
