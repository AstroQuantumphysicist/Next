#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
source tools/env.sh
mkdir -p work build/esp/EFI/NEXT
NEXT_SIGNING_SEED="${NEXT_SIGNING_SEED:-$PWD/work/package-dev.seed}"
export NEXT_DEV_PUBLIC_KEY="${NEXT_DEV_PUBLIC_KEY:-$PWD/work/package-dev.pub}"
cargo run --locked --offline -p next-package --features host-tools -- keygen "$NEXT_SIGNING_SEED" "$NEXT_DEV_PUBLIC_KEY"
if [[ "${1:-}" == --key-only ]]; then exit 0; fi
cargo run --locked --offline -p next-package --features host-tools -- pack "$NEXT_SIGNING_SEED" org.next.development 0.1.0 0 deployment boot/helix.elf build/esp/EFI/NEXT/SYSTEM.NXP boot/helix.elf=build/esp/EFI/NEXT/HELIX.ELF boot/initrd.nxi=build/esp/EFI/NEXT/INITRD.NXI
cargo run --locked --offline -p next-package --features host-tools -- verify "$NEXT_DEV_PUBLIC_KEY" build/esp/EFI/NEXT/SYSTEM.NXP
mkdir -p build/esp/EFI/NEXT/A build/esp/EFI/NEXT/B
for slot in A B; do
    cp build/esp/EFI/NEXT/HELIX.ELF "build/esp/EFI/NEXT/$slot/HELIX.ELF"
    cp build/esp/EFI/NEXT/INITRD.NXI "build/esp/EFI/NEXT/$slot/INITRD.NXI"
done
cp build/esp/EFI/NEXT/SYSTEM.NXP build/esp/EFI/NEXT/A/SYSTEM.NXP
cargo run --locked --offline -p next-package --features host-tools -- pack "$NEXT_SIGNING_SEED" org.next.development 0.1.1 0 deployment boot/helix.elf build/esp/EFI/NEXT/B/SYSTEM.NXP boot/helix.elf=build/esp/EFI/NEXT/B/HELIX.ELF boot/initrd.nxi=build/esp/EFI/NEXT/B/INITRD.NXI
cargo run --locked --offline -p next-package --features host-tools -- verify "$NEXT_DEV_PUBLIC_KEY" build/esp/EFI/NEXT/A/SYSTEM.NXP
cargo run --locked --offline -p next-package --features host-tools -- verify "$NEXT_DEV_PUBLIC_KEY" build/esp/EFI/NEXT/B/SYSTEM.NXP
