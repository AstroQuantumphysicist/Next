#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
source tools/env.sh
bash tools/build-package.sh --key-only
export NEXT_DEV_PUBLIC_KEY="${NEXT_DEV_PUBLIC_KEY:-$PWD/work/package-dev.pub}"
# Freestanding core disables the red zone, including its precompiled routines.
# No libc/startup/interpreter is linked; Rust's bundled LLD works offline.
source tools/kernel-env.sh
RUSTFLAGS="$NEXT_KERNEL_FLAGS" cargo build --locked --offline --release -p helix-kernel --target x86_64-unknown-none
bash tools/build-users.sh
bash tools/build-linux.sh
bash tools/build-nt.sh
mkdir -p build/esp/EFI/BOOT build/esp/EFI/NEXT
cp target/x86_64-unknown-none/release/helix-kernel build/esp/EFI/NEXT/HELIX.ELF
bash tools/build-apps.sh
python3 tools/initrd.py
bash tools/build-package.sh
export NEXT_DEV_PUBLIC_KEY="${NEXT_DEV_PUBLIC_KEY:-$PWD/work/package-dev.pub}"
cargo build --locked --offline --release -p next-boot --target x86_64-unknown-uefi
cp target/x86_64-unknown-uefi/release/next-boot.efi build/esp/EFI/BOOT/BOOTX64.EFI
python3 tools/image.py

python3 tools/spec_coverage.py
