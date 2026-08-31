#!/bin/sh
set -eu

app_dir="$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)"
runner="$app_dir/guest-tg-xtask-profile.sh"
config="$app_dir/qemu-aarch64-profile.toml"

fail() {
    echo "macos tg-xtask profile contract: $1" >&2
    exit 1
}

[ -f "$runner" ] || fail "profile guest runner is missing"
[ -f "$config" ] || fail "profile QEMU config is missing"
grep -q 'cargo_bin.*build -p tg-xtask' "$runner" \
    || fail "profile workload is not cargo build -p tg-xtask"
grep -q 'profile_duration=300' "$runner" \
    || fail "profile window is not 300 seconds"
grep -q 'perf record .*cycles' "$runner" \
    || fail "cycle sampling command is missing"
grep -q 'perf stat .*cycles' "$runner" \
    || fail "profile runner does not probe whether the guest PMU is available"
grep -q -- '-Zself-profile=' "$runner" \
    || fail "profile runner has no compiler self-profile fallback for HVF without PMU"
grep -q 'profile_backend=rustc-self-profile' "$runner" \
    || fail "profile metadata cannot identify the compiler self-profile fallback"
grep -q 'host_success_settle_seconds=3' "$runner" \
    || fail "profile runner does not keep the guest alive after printing the host success marker"
grep -Fq 'sleep "$host_success_settle_seconds"' "$runner" \
    || fail "profile runner powers off before the host can commit the success match"
if grep -Eq 'perf record .* (-g|--call-graph)( |$)' "$runner"; then
    fail "profile requests unsupported guest callchains"
fi
if grep -Eq 'CARGO_BUILD_JOBS|RAYON_NUM_THREADS|RUSTC_THREADS|-Zthreads|taskset' "$runner"; then
    fail "profile runner limits build parallelism or affinity"
fi
grep -q 'STARRY_MACOS_SELFBUILD_MODE' "$app_dir/prebuild.sh" \
    || fail "prebuild cannot select the profile runner"
grep -Eq '^[[:space:]]+perf$' "$app_dir/prepare_toolchain_overlay.sh" \
    || fail "toolchain overlay does not contain perf"
grep -q '"hvf"' "$config" || fail "QEMU profile does not use HVF"
grep -q '"host"' "$config" || fail "QEMU profile does not use the host CPU"
grep -q '"8"' "$config" || fail "QEMU profile does not expose eight vCPUs"
grep -q 'TG-XTASK-PROFILE-PASS' "$config" \
    || fail "QEMU profile has no completion marker"

echo "macos_tg_xtask_profile_contract=PASS"
