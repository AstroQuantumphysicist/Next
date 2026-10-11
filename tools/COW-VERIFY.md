`cow_verify.py` validates the current bounded `NXTCOW01` development COW store.
It opens a regular scratch image read-only and never repairs, formats, salvages,
or writes. Extent geometry and protected ranges must come from a trusted caller;
the tool does not discover a partition table or authorize an extent.

For the generated 65 MiB development image:

```sh
python3 tools/cow_verify.py scratch.img --start-lba 131072 --sectors 132 --protect 0:131072
python3 tools/test_cow_verify.py
```

The JSON result reports both commit candidates, root and live-file checksum
results, the newest mountable generation, and the kernel mount algorithm's
read-only fallback decision. An older unselected snapshot can be corrupt while
a newer mount remains writable; this damage is still reported. Unknown nonblank
metadata, conflicting equal-generation commits, invalid canonical names,
truncated media, and protected overlaps fail closed. No valid snapshot exits 1.
SHA-256 before and after verification detects changes to the source during the
read. CRC detects accidental corruption and provides no authentication.

This verifier is specific to the two-bank, eight-file development store. It is
not full NextFS fsck, a repair tool, a power-loss certification, or evidence that
a physical controller honors flush barriers. Unreferenced payload slots are not
live files; their bytes are ignored consistently with the kernel mount contract.
