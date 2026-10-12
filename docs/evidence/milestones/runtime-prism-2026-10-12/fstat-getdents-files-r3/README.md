# `fstat` / `getdents64` and Prism Files evidence

This source-bound run uses signed development image SHA-256
`1b226f5ee89a59656cf74a673ea381f7d56e9dbe764bb852c75d224a902d6c6b`. The
qemu64 q35/TCG/OVMF smoke passed 128 checks on 2026-10-12. The report and serial
trace are retained here; `capture-manifest.json` binds each committed PNG in
`docs/images/` to the SHA-256 of its original 1280×800 guest PPM.

The guest ran the original static Linux `fstat.elf` and `getdents.elf` fixtures
with exit statuses 54 and 55. The directory fixture covers the bounded
`getdents64` record ABI, shared duplicate cursor, independent opens, EOF,
descriptor errors, and copyout bounds. The `fstat` fixture covers the bounded
144-byte x86-64 record, file identity/type/size and descriptor/copyout errors.
Host Linux also ran both original binaries successfully through
`bash tools/build-linux.sh`; the host contract harness passed all 15 tests.

Prism Files now opens at 960×600 when the viewport permits, supports two
independent panes with up to four directory tabs per pane, and retains each
pane's path across navigation in the other. The smoke checked all four
navigation completions against their pane request-ID ranges. The gallery also
records root and nested tab states, the editable-field I-beam, and a 16-frame
overview. Narrow-window row-label clipping and compact four-tab markers have
host pixel/layout regression coverage in the 230-test Prism suite.

The complete `source tools/env.sh && bash tools/test.sh` run passed, followed by
`source tools/env.sh && export NEXT_DEV_PUBLIC_KEY="$PWD/work/package-dev.pub" && bash tools/build.sh`.

This is scoped development evidence, not Next 1.0 acceptance. The VM used one
configured vCPU, APs remained stopped, and scheduler work ran on the BSP. The
directory listing is a bounded open-time snapshot with synthetic
path-derived inode values. No SMP scheduling or physical-hardware certification
is established. The requirement ledger remains at 74 pending, 121 partial, and
0 verified requirements.
