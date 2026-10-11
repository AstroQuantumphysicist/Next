#!/usr/bin/env bash
# Source this file from the repository or any working directory.
if [ -d /workspace/toolchains/cargo/bin ]; then
    export CARGO_HOME=/workspace/toolchains/cargo
    export RUSTUP_HOME=/workspace/toolchains/rustup
    export PATH="/workspace/toolchains/cargo/bin:$PATH"
fi
if [ -d /workspace/toolchains/sysroot/usr ]; then
    export NEXT_SYSROOT=/workspace/toolchains/sysroot
    export LD_LIBRARY_PATH="${NEXT_SYSROOT}/usr/lib/x86_64-linux-gnu${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
    export PATH="${NEXT_SYSROOT}/usr/bin:${NEXT_SYSROOT}/usr/sbin:$PATH"
fi
