#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
source tools/env.sh
export CARGO_PROFILE_RELEASE_LTO=false
source tools/users-env.sh
export RUSTFLAGS="$NEXT_USER_FLAGS"
cargo build --offline --release --target x86_64-unknown-none -p next-user --bins
mkdir -p build/user
for app in nextd probe fault spin fault-supervisor fault-rx fault-nx fault-guard fault-ud context wait-negative fault-avx fault-fsgsbase clock event-wait service-worker vfs-probe portal-probe udp-probe udp-block dns-probe surface-probe client-input-probe trash-probe client-close-probe client-input-delayed client-reconfigure-probe timer-probe; do
    cp "target/x86_64-unknown-none/release/$app" "build/user/$app.elf"
done
