#!/usr/bin/env bash
set -euo pipefail
NEXT_REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source "$NEXT_REPO_ROOT/tools/env.sh"
cd "$NEXT_REPO_ROOT"
if [[ ! -f build/next-dev.img ]]; then
    echo "Missing development image. Run tools/build.sh first." >&2
    exit 1
fi
NEXT_FIRMWARE_DIR="${NEXT_SYSROOT:-}/usr/share/OVMF"
if [[ ! -f "$NEXT_FIRMWARE_DIR/OVMF_CODE_4M.fd" ]]; then
    NEXT_FIRMWARE_DIR=/usr/share/OVMF
fi
NEXT_QEMU="${NEXT_QEMU:-qemu-system-x86_64}"
NEXT_RAM="${NEXT_RAM:-256M}"
NEXT_SMP="${NEXT_SMP:-1}"
NEXT_DISPLAY="${NEXT_DISPLAY:-vnc=127.0.0.1:0}"
if [[ ! -f "$NEXT_FIRMWARE_DIR/OVMF_CODE_4M.fd" || ! -f "$NEXT_FIRMWARE_DIR/OVMF_VARS_4M.fd" ]]; then
    echo "OVMF firmware is missing. Install OVMF or configure NEXT_SYSROOT." >&2
    exit 1
fi
if ! command -v "$NEXT_QEMU" >/dev/null; then
    echo "QEMU is missing. Install qemu-system-x86_64 or configure NEXT_QEMU." >&2
    exit 1
fi
NEXT_RUN_DIR="$(mktemp -d /tmp/next-run.XXXXXX)"
export TMPDIR="$NEXT_RUN_DIR"
NEXT_QEMU_PID=""
next_cleanup() {
    if [[ -n "$NEXT_QEMU_PID" ]] && kill -0 "$NEXT_QEMU_PID" 2>/dev/null; then
        kill "$NEXT_QEMU_PID" 2>/dev/null || true
        wait "$NEXT_QEMU_PID" 2>/dev/null || true
    fi
    rm -rf "$NEXT_RUN_DIR"
}
trap next_cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
trap 'exit 129' HUP
cp "$NEXT_FIRMWARE_DIR/OVMF_VARS_4M.fd" "$NEXT_RUN_DIR/vars.fd"
echo "Next development image: q35/TCG, software framebuffer, legacy PS/2 keyboard." >&2
echo "This is not Next 1.0; real hardware, SMP scheduling and device drivers remain unverified." >&2
echo "Display backend: $NEXT_DISPLAY (default viewer address 127.0.0.1:5900)." >&2
if [[ "$NEXT_SMP" != 1 ]]; then
    echo "Additional CPUs can be exposed, but this kernel currently uses only the boot CPU." >&2
fi
NEXT_QEMU_DATA=()
for NEXT_DATA in usr/share/qemu usr/share/seabios usr/lib/ipxe/qemu; do
    if [[ -n "${NEXT_SYSROOT:-}" && -d "$NEXT_SYSROOT/$NEXT_DATA" ]]; then
        NEXT_QEMU_DATA+=(-L "$NEXT_SYSROOT/$NEXT_DATA")
    fi
done
"$NEXT_QEMU" "${NEXT_QEMU_DATA[@]}" -machine q35,accel=tcg -m "$NEXT_RAM" -smp "$NEXT_SMP" -vga std \
    -drive "if=pflash,format=raw,readonly=on,file=$NEXT_FIRMWARE_DIR/OVMF_CODE_4M.fd" \
    -drive "if=pflash,format=raw,file=$NEXT_RUN_DIR/vars.fd" \
    -drive "if=none,id=nextdisk,format=raw,snapshot=on,file=$NEXT_REPO_ROOT/build/next-dev.img" \
    -device virtio-blk-pci,drive=nextdisk,disable-modern=on \
    -netdev user,id=nextnet -device virtio-net-pci,netdev=nextnet,disable-modern=on \
    -audiodev "wav,id=nextaudio,path=$NEXT_RUN_DIR/audio.wav,out.frequency=48000,out.channels=2,out.format=s16" -device AC97,audiodev=nextaudio \
    -display "$NEXT_DISPLAY" -serial stdio -no-reboot "$@" <&0 &
NEXT_QEMU_PID=$!
wait "$NEXT_QEMU_PID"
