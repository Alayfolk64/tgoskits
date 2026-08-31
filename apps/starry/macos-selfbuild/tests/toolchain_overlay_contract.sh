#!/bin/sh
set -eu

app_dir="$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)"
toolchain_overlay="$app_dir/prepare_toolchain_overlay.sh"

fail() {
    echo "macos toolchain overlay contract: $1" >&2
    exit 1
}

grep -Eq 'for path in .* fs( |;)' "$toolchain_overlay" \
    || fail "offline Cargo prefetch omits the workspace fs directory"
grep -Fq 'cargo fetch --locked --manifest-path "$prefetch_manifest"' \
    "$toolchain_overlay" \
    || fail "offline Cargo prefetch does not fetch the locked workspace for all targets"
if grep -q 'starry-selfbuild-extra-fetch-workspace' "$toolchain_overlay"; then
    fail "offline Cargo prefetch re-resolves locked packages through a synthetic workspace"
fi

echo "macos_toolchain_overlay_contract=PASS"
