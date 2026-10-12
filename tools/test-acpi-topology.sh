#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
source tools/env.sh
mkdir -p build
rustc --edition=2021 --crate-name next_abi --crate-type lib \
  crates/next-abi/src/lib.rs -o build/libnext_abi-acpi-test.rlib
rustc --edition=2021 --test crates/helix-kernel/src/acpi.rs \
  --extern next_abi=build/libnext_abi-acpi-test.rlib \
  -o build/acpi-topology-tests
build/acpi-topology-tests
