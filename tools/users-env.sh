#!/usr/bin/env bash
# High-address native executables use large-code sections and the native linker.
# Prebuilt freestanding core has a kernel code model, so LLVM LTO cannot merge
# its module flags with the application's large code model.
NEXT_USER_FLAGS="-C no-redzone=yes -C relocation-model=static -C code-model=large -C linker=rust-lld -C link-arg=-Tcrates/next-user/user.ld -C link-arg=--build-id=none -C link-arg=-z -C link-arg=max-page-size=4096"
