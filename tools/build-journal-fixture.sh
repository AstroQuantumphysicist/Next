#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
source tools/env.sh
cargo build --locked --offline -p next-fs --target-dir work/journal-fixture-target
journal_fs=(work/journal-fixture-target/debug/deps/libnext_fs-*.rlib)
rustc --edition 2021 -D warnings tools/journal_fixture.rs --extern "next_fs=${journal_fs[0]}" \
    -L dependency=work/journal-fixture-target/debug/deps -C panic=abort -o work/journal-fixture
