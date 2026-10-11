#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
source tools/env.sh
bash tools/build-package.sh --key-only
export NEXT_DEV_PUBLIC_KEY="${NEXT_DEV_PUBLIC_KEY:-$PWD/work/package-dev.pub}"
cargo fmt --all -- --check
bash tools/build-users.sh
bash tools/build-linux.sh
bash tools/build-nt.sh
bash tools/build-apps.sh
python3 tools/test_apps.py
cargo test --locked --offline --workspace --exclude next-boot --exclude helix-kernel --exclude next-user
cargo test --locked --offline -p next-package --features host-tools installer::tests
cargo clippy --locked --offline --workspace --exclude next-boot --exclude helix-kernel --exclude next-user --all-targets -- -D warnings
cargo clippy --locked --offline -p next-package --features host-tools --all-targets -- -D warnings
cargo clippy --locked --offline -p next-boot --target x86_64-unknown-uefi -- -D warnings
source tools/kernel-env.sh
RUSTFLAGS="$NEXT_KERNEL_FLAGS" cargo clippy --locked --offline -p helix-kernel --target x86_64-unknown-none -- -D warnings
source tools/users-env.sh
CARGO_PROFILE_RELEASE_LTO=false RUSTFLAGS="$NEXT_USER_FLAGS" cargo clippy --locked --offline -p next-user --target x86_64-unknown-none --bins -- -D warnings
mkdir -p build
rustc --edition=2021 --test crates/helix-kernel/src/storage.rs -o build/storage-tests
build/storage-tests
rustc --edition=2021 --test tools/linux_profile_tests.rs -o build/linux-profile-tests
build/linux-profile-tests
python3 -m unittest discover -s tools -p 'test_*.py'
python3 tools/spec_audit.py --self-test
python3 tools/spec_audit.py

python3 tools/spec_coverage.py --self-test

bash tools/test-sdk.sh

python3 tools/gpt_plan.py --self-test
bash tools/test-gpt-parser.sh
bash tools/test-gpt-volume.sh
bash tools/test-storage-policy.sh
bash tools/test-appearance-journal.sh

bash tools/test-gui-copy.sh

bash tools/test-gui-files-integration.sh

bash tools/test-trash-backend.sh

bash tools/test-trash-undo.sh

bash tools/test-files-query.sh
bash tools/test-gui-directories.sh
python3 tools/files_directory_smoke.py --self-test
