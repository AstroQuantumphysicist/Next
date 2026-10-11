#!/usr/bin/env bash
# Actual render audit. Current source only; never attributes results to an image.
set -euo pipefail
cd "$(dirname "$0")/.."
source tools/env.sh
mkdir -p work/client-surface-layout
rustc --edition=2021 --crate-type rlib --crate-name prism crates/prism/src/lib.rs -o work/client-surface-layout/libprism.rlib
rustc --edition=2021 tools/client_surface_layout.rs --extern prism=work/client-surface-layout/libprism.rlib -o work/client-surface-layout/audit
work/client-surface-layout/audit
