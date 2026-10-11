#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
source tools/env.sh
python3 tools/gpt_volume_fixture.py
rustc --edition 2021 -D warnings --test tools/gpt_volume_tests.rs -o work/gpt-volume-tests
work/gpt-volume-tests
