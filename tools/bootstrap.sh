#!/usr/bin/env bash
# Installs only below /workspace/toolchains; no root and no system-file edits.
set -euo pipefail
TASK_REPO_ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
TASK_TOOLS=/workspace/toolchains
mkdir -p "$TASK_TOOLS/downloads" "$TASK_TOOLS/cargo" "$TASK_TOOLS/rustup"
export CARGO_HOME="$TASK_TOOLS/cargo"
export RUSTUP_HOME="$TASK_TOOLS/rustup"
export PATH="$CARGO_HOME/bin:$PATH"
if [ ! -x "$CARGO_HOME/bin/rustup" ]; then
    cd "$TASK_TOOLS/downloads"
    curl --fail --location --proto '=https' --proto-redir '=https' --tlsv1.2 https://static.rust-lang.org/rustup/dist/x86_64-unknown-linux-gnu/rustup-init -o rustup-init
    curl --fail --location --proto '=https' --proto-redir '=https' --tlsv1.2 https://static.rust-lang.org/rustup/dist/x86_64-unknown-linux-gnu/rustup-init.sha256 -o rustup-init.sha256
    sha256sum -c rustup-init.sha256
    chmod +x rustup-init
    ./rustup-init -y --no-modify-path --profile minimal --default-toolchain 1.90.0 --target x86_64-unknown-uefi --target x86_64-unknown-none
fi
rustup toolchain install 1.90.0 --profile minimal --component rustfmt --component clippy --target x86_64-unknown-uefi --target x86_64-unknown-none
TASK_SYSROOT="$TASK_TOOLS/sysroot"
if [ ! -x "$TASK_SYSROOT/usr/bin/qemu-system-x86_64" ] || [ ! -x "$TASK_SYSROOT/usr/sbin/mkfs.fat" ] || [ ! -x "$TASK_SYSROOT/usr/bin/mcopy" ] || [ ! -f "$TASK_SYSROOT/usr/share/OVMF/OVMF_CODE_4M.fd" ]; then
    mkdir -p "$TASK_TOOLS/apt/lists/partial" "$TASK_TOOLS/apt/cache/archives/partial" "$TASK_SYSROOT"
    cat > "$TASK_TOOLS/apt/sources.list" <<'SOURCES'
deb https://deb.debian.org/debian trixie main
deb https://security.debian.org/debian-security trixie-security main
SOURCES
    TASK_APT_OPTIONS=(-o Debug::NoLocking=1 -o "Dir::Etc::sourcelist=$TASK_TOOLS/apt/sources.list" -o Dir::Etc::sourceparts=- -o "Dir::State::lists=$TASK_TOOLS/apt/lists" -o "Dir::Cache=$TASK_TOOLS/apt/cache" -o "Dir::Cache::archives=$TASK_TOOLS/apt/cache/archives")
    /usr/bin/apt-get "${TASK_APT_OPTIONS[@]}" update
    /usr/bin/apt-get "${TASK_APT_OPTIONS[@]}" --download-only --no-install-recommends -y install qemu-system-x86 ovmf mtools dosfstools
    # apt verifies repository signatures and package hashes before extraction.
    for TASK_PACKAGE in "$TASK_TOOLS/apt/cache/archives/"*.deb; do
        dpkg-deb -x "$TASK_PACKAGE" "$TASK_SYSROOT"
    done
fi
rustc +1.90.0 --version
LD_LIBRARY_PATH="$TASK_SYSROOT/usr/lib/x86_64-linux-gnu${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}" "$TASK_SYSROOT/usr/bin/qemu-system-x86_64" --version

# Fetch locked cryptographic dependencies through the configured HTTPS policy.
cd "$TASK_REPO_ROOT"
cargo fetch --locked
