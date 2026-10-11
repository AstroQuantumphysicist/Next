#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
source tools/env.sh
python3 tools/gpt_fixture.py
export NEXT_DEV_PUBLIC_KEY="${NEXT_DEV_PUBLIC_KEY:-$PWD/work/package-dev.pub}"
CARGO_PROFILE_DEV_PANIC=unwind cargo build --locked --offline -p next-checks --lib --target-dir work/storage-policy-target
storage_abi=(work/storage-policy-target/debug/deps/libnext_abi-*.rlib)
storage_fs=(work/storage-policy-target/debug/deps/libnext_fs-*.rlib)
storage_package=(work/storage-policy-target/debug/deps/libnext_package-*.rlib)
NEXT_SOURCE_ROOT="$PWD" rustc --edition 2021 -D warnings --test tools/storage_policy_tests.rs \
    --extern "next_abi=${storage_abi[0]}" --extern "next_fs=${storage_fs[0]}" \
    --extern "next_package=${storage_package[0]}" -L dependency=work/storage-policy-target/debug/deps \
    -o work/storage-policy-tests
work/storage-policy-tests
