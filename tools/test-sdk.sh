#!/usr/bin/env bash
# Host-only SDK validation never executes the privileged/native syscall gate.
set -euo pipefail
cd "$(dirname "$0")/.."
source tools/env.sh
CARGO_PROFILE_DEV_PANIC=unwind cargo build --locked --offline -p next-user --lib --target-dir work/sdk-target
sdk_abi_libs=(work/sdk-target/debug/deps/libnext_abi-*.rlib)
sdk_core_libs=(work/sdk-target/debug/deps/libhelix_core-*.rlib)
rustc --edition=2021 --test crates/next-user/src/lib.rs \
    --extern "next_abi=${sdk_abi_libs[0]}" \
    --extern "helix_core=${sdk_core_libs[0]}" \
    -L dependency=work/sdk-target/debug/deps -o work/sdk-tests
work/sdk-tests
