#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
source tools/env.sh
CARGO_PROFILE_DEV_PANIC=unwind cargo build --locked --offline -p next-fs -p prism --lib --target-dir work/gui-directories-target
next_fs=(work/gui-directories-target/debug/deps/libnext_fs-*.rlib)
prism=(work/gui-directories-target/debug/deps/libprism-*.rlib)
rustc --edition 2021 -D warnings --test tools/gui_directories_tests.rs \
  --extern "next_fs=${next_fs[0]}" --extern "prism=${prism[0]}" \
  -L dependency=work/gui-directories-target/debug/deps -o work/gui-directories-tests
work/gui-directories-tests
