#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
source tools/env.sh
cargo build --locked --offline -p helix-core -p next-fs -p prism --lib --target-dir work/gui-files-target
core=(work/gui-files-target/debug/deps/libhelix_core-*.rlib)
fs=(work/gui-files-target/debug/deps/libnext_fs-*.rlib)
prism=(work/gui-files-target/debug/deps/libprism-*.rlib)
rustc --edition 2021 -C panic=abort -D warnings tools/gui_files_integration.rs \
  --extern "helix_core=${core[0]}" --extern "next_fs=${fs[0]}" \
  --extern "prism=${prism[0]}" -L dependency=work/gui-files-target/debug/deps \
  -o work/gui-files-integration
work/gui-files-integration
