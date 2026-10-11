#!/usr/bin/env bash
# Freestanding ELF uses freestanding Rust core, bundled LLD, and no host runtime.
NEXT_KERNEL_FLAGS="-C no-redzone=yes -C relocation-model=static -C linker=rust-lld -C link-arg=-Tcrates/helix-kernel/kernel.ld -C link-arg=--build-id=none -C link-arg=-z -C link-arg=max-page-size=4096"
