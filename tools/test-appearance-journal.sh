#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
source tools/env.sh
rustc --edition 2021 -D warnings --test tools/appearance_journal_tests.rs -o work/appearance-journal-tests
work/appearance-journal-tests
