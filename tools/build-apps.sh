#!/usr/bin/env bash
# Owner-signed development application envelopes; declarations are not grants.
set -euo pipefail
cd "$(dirname "$0")/.."
source tools/env.sh
NEXT_SIGNING_SEED="${NEXT_SIGNING_SEED:-$PWD/work/package-dev.seed}"
NEXT_APP_PUBLIC_KEY="${NEXT_DEV_PUBLIC_KEY:-$PWD/work/package-dev.pub}"
[[ -f "$NEXT_SIGNING_SEED" && -f "$NEXT_APP_PUBLIC_KEY" ]] || {
    echo 'Generate owner development keys with tools/build-package.sh --key-only first' >&2
    exit 1
}
# One host build, then direct invocations rather than a cargo run per executable.
env -u RUSTFLAGS cargo build --locked --offline -p next-package --features host-tools --bin next-package
NEXT_PACKAGE_TOOL="$PWD/target/debug/next-package"
NEXT_APPS_DESTINATION="${NEXT_APPS_DESTINATION:-build/apps}"
NEXT_SERVICE_ACTIVE_MASK="${NEXT_SERVICE_ACTIVE_MASK:-1}"
NEXT_APPS_STAGE="$(mktemp -d "$PWD/build/.apps.XXXXXXXX")"
cleanup_apps() {
    # Restore the prior mirror if publishing the complete staged mirror fails.
    if [[ -d "$NEXT_APPS_STAGE.old" && ! -d "$NEXT_APPS_DESTINATION" ]]; then
        mv "$NEXT_APPS_STAGE.old" "$NEXT_APPS_DESTINATION"
    fi
    rm -rf "$NEXT_APPS_STAGE"
}
trap cleanup_apps EXIT
mkdir -p "$NEXT_APPS_STAGE/system" "$NEXT_APPS_STAGE/compat/linux" "$NEXT_APPS_STAGE/compat/nt"
python3 tools/service_config.py --active-mask "$NEXT_SERVICE_ACTIVE_MASK" --output "$NEXT_APPS_STAGE/services.bin"
NEXT_APP_COUNT=0
shopt -s nullglob
for NEXT_APP_PROFILE in native linux nt; do
    case "$NEXT_APP_PROFILE" in
        native) NEXT_APP_FILES=(build/user/*.elf); NEXT_APP_OUTPUT=system ;;
        linux) NEXT_APP_FILES=(build/compat/linux/*.elf); NEXT_APP_OUTPUT=compat/linux ;;
        nt) NEXT_APP_FILES=(build/compat/nt/*.exe); NEXT_APP_OUTPUT=compat/nt ;;
    esac
    [[ ${#NEXT_APP_FILES[@]} -gt 0 ]] || { echo "Missing $NEXT_APP_PROFILE fixtures" >&2; exit 1; }
    for NEXT_APP_SOURCE in "${NEXT_APP_FILES[@]}"; do
        NEXT_APP_NAME="${NEXT_APP_SOURCE##*/}"
        NEXT_APP_STEM="${NEXT_APP_NAME%.*}"
        [[ "$NEXT_APP_STEM" =~ ^[a-z0-9_-]+$ ]] || { echo 'Noncanonical fixture stem' >&2; exit 1; }
        NEXT_APP_ID="org.next.development.$NEXT_APP_PROFILE.$NEXT_APP_STEM"
        NEXT_APP_ENTRY="bin/$NEXT_APP_NAME"
        NEXT_APP_MEMBERS=("$NEXT_APP_ENTRY=$NEXT_APP_SOURCE")
        if [[ "$NEXT_APP_PROFILE" == native && "$NEXT_APP_NAME" == nextd.elf ]]; then
            NEXT_APP_MEMBERS+=("config/services.bin=$NEXT_APPS_STAGE/services.bin")
        fi
        "$NEXT_PACKAGE_TOOL" pack "$NEXT_SIGNING_SEED" "$NEXT_APP_ID" 0.1.0 3f app \
            "$NEXT_APP_ENTRY" "$NEXT_APPS_STAGE/$NEXT_APP_OUTPUT/$NEXT_APP_NAME" \
            "${NEXT_APP_MEMBERS[@]}"
        NEXT_APP_COUNT=$((NEXT_APP_COUNT + 1))
    done
done
[[ -f "$NEXT_APPS_STAGE/system/nextd.elf" && -f "$NEXT_APPS_STAGE/system/probe.elf" ]] || {
    echo 'Signed nextd/probe application fixtures are required' >&2; exit 1;
}
# This intentionally untrusted app keeps a valid outer initrd CRC/deployment
# signature: only its application signature is invalid. Runtime must reject it.
"$NEXT_PACKAGE_TOOL" pack "$NEXT_SIGNING_SEED" org.next.development.native.untrusted 0.1.0 3f app \
    bin/untrusted.elf "$NEXT_APPS_STAGE/system/untrusted.elf" bin/untrusted.elf=build/user/probe.elf
python3 - "$NEXT_APPS_STAGE" <<'PY'
from pathlib import Path
import sys
stage = Path(sys.argv[1])
bundle = bytearray((stage / 'system/untrusted.elf').read_bytes())
bundle[-64] ^= 1
(stage / 'system/untrusted.elf').write_bytes(bundle)
PY
NEXT_APP_COUNT=$((NEXT_APP_COUNT + 1))
rm "$NEXT_APPS_STAGE/services.bin"
[[ "$NEXT_APP_COUNT" -le 64 ]] || { echo 'Application fixtures exceed 64 initrd entries' >&2; exit 1; }
# Only replace the mirror after every application was successfully signed.
if [[ -d "$NEXT_APPS_DESTINATION" ]]; then mv "$NEXT_APPS_DESTINATION" "$NEXT_APPS_STAGE.old"; fi
mv "$NEXT_APPS_STAGE" "$NEXT_APPS_DESTINATION"
rm -rf "$NEXT_APPS_STAGE.old"
trap - EXIT
echo "Built $NEXT_APP_COUNT application envelopes (one deliberately invalid negative fixture)"
