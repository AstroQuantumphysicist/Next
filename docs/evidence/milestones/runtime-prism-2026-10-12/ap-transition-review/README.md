# AP transition-stage review

This is a host-only audit of the compiled AP trampoline and its source-built
descriptor tables. It does not start an AP, establish a platform NMI
quiescence guarantee, or authorize scheduler participation. The AP activation
guard remains closed.

The disassembly and audit manifest identify these stages from the actual
compiled instruction addresses:

- Real mode installs a private stack before its 256-entry IVT, then replaces
  it with an NMI-only bridge immediately before enabling protected mode.
- Protected mode enters with that bridge active, installs its private stack,
  then loads the serialized 256-entry IDT at `0x82d46`. The serializer uses
  selector `0x08` and the copied 32-bit emergency park handler. It later
  restores the vector-2 bridge before enabling paging and long mode.
- Long-mode entry switches to the private stack before loading the current
  AP mailbox's private IDT. Until that IDT is loaded, the bridge covers only
  vector 2.

The remaining startup blocker is before and during these transitions. An NMI
can arrive before the AP executes its first instruction, the initial real-mode
IVT is active while the AP discovers its record and stack, bridge windows do
not handle synchronous exceptions, and SMI/INIT are not IDT vectors. BSP-side
route observations also cannot constrain VMM event injection or prove AP reset
LINT state. APs therefore remain stopped and the scheduler remains BSP-only.

## Reproduction

Build an isolated audit ELF and retain fresh output under `work/`:

```sh
source tools/env.sh
source tools/kernel-env.sh
NEXT_DEV_PUBLIC_KEY="$PWD/work/package-dev.pub" \
  CARGO_TARGET_DIR="$PWD/work/ap-transition-target-replay" \
  RUSTFLAGS="$NEXT_KERNEL_FLAGS -C link-dead-code=yes" \
  cargo build --locked --offline --release -p helix-kernel --target x86_64-unknown-none
python3 tools/ap_startup_audit.py \
  --elf work/ap-transition-target-replay/x86_64-unknown-none/release/helix-kernel \
  --output-dir work/ap-transition-audit-replay
python3 tools/ap_startup_failures.py \
  --elf work/ap-transition-target-replay/x86_64-unknown-none/release/helix-kernel \
  --output-dir work/ap-transition-mutation-replay
```

The recorded build passed. The decoded 644-byte trampoline has SHA-256
`e66be1e4fd10829fb1e2fc24d1b16e828ac68ebe2f673f0f1a82b0c05aff23cf`. The
host audit passed, and the hostile-byte profile rejected all 16 mutations,
including removal and misdirection of the full real, protected, and long-mode
IDT loads and wrong NMI-bridge offsets. A direct host harness for
`ap_transition.rs` passed all 8 tests.
These results verify compiled order, instruction decoding, source-level table
construction, and fail-closed mutation handling only.

`audit.json` contains exact stage landmarks, source digests, and each
unsupported window. `failures.json` records all hostile mutations. The three
`.txt` files preserve decoded real16, protected32, and long64 instructions;
`startup.bin` preserves the audited trampoline bytes. The separate
[two-vCPU QEMU profile](../smp-two-vcpu/README.md) continues to prove topology
discovery and BSP workload behavior with APs stopped. Neither artifact
establishes AP execution or cross-CPU scheduling.
