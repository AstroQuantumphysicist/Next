#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
source tools/env.sh
CARGO_PROFILE_DEV_PANIC=unwind cargo build --locked --offline -p helix-core -p next-fs -p prism --lib --target-dir work/trash-target
trash_core=(work/trash-target/debug/deps/libhelix_core-*.rlib)
trash_fs=(work/trash-target/debug/deps/libnext_fs-*.rlib)
trash_prism=(work/trash-target/debug/deps/libprism-*.rlib)
rustc --edition 2021 -D warnings --test tools/trash_undo_tests.rs \
 --extern "helix_core=${trash_core[0]}" --extern "next_fs=${trash_fs[0]}" \
 --extern "prism=${trash_prism[0]}" -L dependency=work/trash-target/debug/deps -o work/trash-undo-tests
work/trash-undo-tests
