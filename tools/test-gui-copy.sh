#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
source tools/env.sh
CARGO_PROFILE_DEV_PANIC=unwind cargo build --locked --offline -p helix-core -p next-fs -p prism --lib --target-dir work/gui-copy-target
copy_core=(work/gui-copy-target/debug/deps/libhelix_core-*.rlib)
copy_fs=(work/gui-copy-target/debug/deps/libnext_fs-*.rlib)
copy_prism=(work/gui-copy-target/debug/deps/libprism-*.rlib)
rustc --edition 2021 -D warnings --test tools/gui_copy_tests.rs \
 --extern "helix_core=${copy_core[0]}" --extern "next_fs=${copy_fs[0]}" \
 --extern "prism=${copy_prism[0]}" -L dependency=work/gui-copy-target/debug/deps -o work/gui-copy-tests
work/gui-copy-tests
