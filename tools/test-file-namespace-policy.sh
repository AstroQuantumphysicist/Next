#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
source tools/env.sh
CARGO_PROFILE_DEV_PANIC=unwind cargo build --locked --offline -p next-abi --lib --target-dir work/file-namespace-policy-target
namespace_abi=(work/file-namespace-policy-target/debug/deps/libnext_abi-*.rlib)
rustc --edition 2021 -D warnings --test crates/helix-kernel/src/file_namespace_policy.rs \
  --extern "next_abi=${namespace_abi[0]}" -L dependency=work/file-namespace-policy-target/debug/deps \
  -o work/file-namespace-policy-tests
work/file-namespace-policy-tests
