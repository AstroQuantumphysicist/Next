# Owner-signed development GPT journal policy

The GPT guest adapter validates both GPT copies and selects ESP/NEXT_DATA by
unique type GUID. The writable mount requires `config/storage.bin` inside the
owner-signed Application bundle `/system/nextd.elf` in the authenticated initrd.
The verifier checks the configured public key, exact package ID
`org.next.development.native.nextd`, Application kind and `bin/nextd.elf` entry
before parsing the policy member. An initrd CRC, raw policy, unsigned application,
unknown signer or another signed package ID cannot establish storage authority.

The canonical policy is exactly 80 bytes, little endian:

| Offset | Field |
|---|---|
| 0 | Magic `NXTSTOR1`, 8 bytes |
| 8 | Version u16 = 1 |
| 10 | Length u16 = 80 |
| 12 | Flags u32: bit0 allows writes, all other bits rejected |
| 16 | Disk GUID, GPT on-disk byte order, 16 bytes |
| 32 | Data partition unique GUID, GPT on-disk byte order, 16 bytes |
| 48 | Data partition first LBA, u64 |
| 56 | Full data partition block count, u64 |
| 64 | Probed device capacity in 512-byte sectors, u64 |
| 72 | Logical block bytes, u32 = 512 |
| 76 | Reserved u32 = 0 |

Both GUIDs must be nonzero. Full partition geometry and capacity bind exactly to
validated GPT metadata; the type must be NEXT_DATA and NoBlockIO is refused.
The GPT read-only attribute (bit60) and a cleared policy write bit independently
attenuate the mount to read-only. GUID binding identifies the declared layout;
it does not prove hardware identity or prevent a complete disk clone.

`AuthenticatedExtent` has no public constructor. Its mount transaction permits
only the first 132 sectors of that selected partition, protecting all media
outside this fixed journal prefix, including the ESP, GPT headers, other
partitions and unused data sectors. The kernel device adapter separately checks
the same write bounds and read-only state before performing hardware I/O.
`Store::mount` reads an existing journal; zero or corrupt unknown data never
triggers format or repair. Recovery fallback remains read-only. Policy attenuation
is reapplied on each mount; it is not recorded as persistent journal metadata.

For an owned fixture, `bash tools/build-journal-fixture.sh` builds a host producer.
`work/journal-fixture START_LBA CAPACITY_SECTORS NEW_BLOB` formats only a memory
device, verifies it by remounting, and exclusively creates a new 67,584-byte blob.
It never opens an existing image for mutation. The GPT fixture builder may copy
the blob into its newly created scratch image before booting it.

`tools/test-storage-policy.sh` checks the actual signed member/verifier and mount
transaction, including tampered content/signatures, unknown signer, wrong package,
malformed schema, layout mismatches, read-only attenuation, zero/corrupt refusal
without writes, and confinement of successful commits. Fixture producer tests
independently verify its bytes and reject existing files, symlinks and FIFOs.

This is a bounded development storage profile with eight 4 KiB files, not the
complete NextFS or an installer. Trusted owner signatures authenticate policy;
protected monotonic rollback state, production key management and hardware disk
authentication require separate acceptance.

`source tools/env.sh; python3 tools/gpt_mount_smoke.py --timeout 150` prepares
exclusive new 70 MiB GPT scratch images from the authenticated development image.
It preserves the exact kernel and all unrelated Application bundles, signs the
supervisor policy and deployment, and preformats only a new memory-produced
journal prefix before boot. The actual guest mounts that existing journal with
`formatted=false`. Two full desktop boots verify an Editor save at
`/native/persistent`, durable bytes after reboot, and unchanged SHA-256 hashes for
every sector outside the 132-sector prefix. The eighth-wave run passed all eight
cases: writable persistence, valid-CRC backup GUID disagreement, unsigned
supervisor, tampered policy, mismatched layout, blank journal, signed read-only
policy and GPT read-only attribute. The seven refusal/attenuation cases preserved
whole-image hashes. Reports are under `build/gpt-mount/`.

This proves clean VM reboot persistence, not power-loss recovery. Writable OVMF
was observed repairing an invalid backup GPT CRC before the kernel read it; that
failed test is retained under `build/gpt-mount/first-firmware-repair-observation/`.
The final writable corruption gate instead uses individually valid headers with
different GUIDs and checks `CopiesDisagree` rejection without firmware repair.
The earlier read-only GPT gate covers invalid backup CRC refusal when firmware
cannot write. Kernel refusal never calls format or repair; this does not claim
that firmware never modifies unknown disks. The first Editor path protocol
failure is also retained separately; `/persistent` lies outside the intentional
Editor save namespace, while the corrected test uses `/native/persistent`.
