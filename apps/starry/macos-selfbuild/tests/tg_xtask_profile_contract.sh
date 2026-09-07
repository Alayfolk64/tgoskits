#!/bin/sh
set -eu

app_dir="$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)"
runner="$app_dir/guest-tg-xtask-profile.sh"
config="$app_dir/qemu-aarch64-profile.toml"
profile_proc="$app_dir/../../../os/StarryOS/kernel/src/pseudofs/proc.rs"
profile_api="$app_dir/../../../os/arceos/modules/axsync/src/profile.rs"
page_cache="$app_dir/../../../fs/ax-fs-ng/src/file/cache/mod.rs"
ext4_fs="$app_dir/../../../fs/ax-fs-ng/src/fs/ext4/rsext4/fs.rs"
ext4_disk="$app_dir/../../../fs/ax-fs-ng/src/fs/ext4/rsext4/mod.rs"

sh "$app_dir/tests/toolchain_overlay_contract.sh"

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
grep -q 'profile_control=/proc/starry_profile' "$runner" \
    || fail "profile runner does not control the in-guest kernel profiler"
grep -q "printf 'phase=prebuild" "$runner" \
    || fail "profile runner does not mark the build phase"
grep -q 'enabled=true phase=1' "$runner" \
    || fail "profile runner does not verify that kernel sampling started"
grep -q 'profile_backend=starry-kernel-fp' "$runner" \
    || fail "profile metadata does not identify the kernel stack profiler"
grep -q 'kernel-profile.raw' "$runner" \
    || fail "profile runner does not save raw kernel samples"
grep -q 'host_success_settle_seconds=3' "$runner" \
    || fail "profile runner does not keep the guest alive after printing the host success marker"
grep -Fq 'sleep "$host_success_settle_seconds"' "$runner" \
    || fail "profile runner powers off before the host can commit the success match"
if grep -Eq 'CARGO_BUILD_JOBS|RAYON_NUM_THREADS|RUSTC_THREADS|-Zthreads|taskset' "$runner"; then
    fail "profile runner limits build parallelism or affinity"
fi
grep -q 'STARRY_MACOS_SELFBUILD_MODE' "$app_dir/prebuild.sh" \
    || fail "prebuild cannot select the profile runner"
grep -q 'guest-profile' "$app_dir/build-aarch64-unknown-none-softfloat.toml" \
    || fail "Starry kernel profiler feature is not enabled"
grep -q 'struct StarryProfileFile' "$profile_proc" \
    || fail "profile proc node does not preserve direct control-write semantics"
grep -q 'SpecialFsFile::new_regular_with_perm' "$profile_proc" \
    || fail "profile proc node is still backed by SimpleFile"
[ -f "$profile_api" ] || fail "MOSS-style latency event API is missing"
grep -q 'ProfileEvent::PageCache' "$page_cache" \
    || fail "page-cache latency is not instrumented"
grep -q 'ProfileEvent::Ext4' "$ext4_fs" \
    || fail "ext4 latency is not instrumented"
grep -q 'ProfileEvent::BlockRead' "$ext4_disk" \
    || fail "ext4 block-read latency is not instrumented"
grep -q 'BACKTRACE = "y"' "$app_dir/build-aarch64-unknown-none-softfloat.toml" \
    || fail "profile build does not force frame pointers"
grep -q '"hvf"' "$config" || fail "QEMU profile does not use HVF"
grep -q '"host"' "$config" || fail "QEMU profile does not use the host CPU"
grep -q '"8"' "$config" || fail "QEMU profile does not expose eight vCPUs"
grep -q 'TG-XTASK-PROFILE-PASS' "$config" \
    || fail "QEMU profile has no completion marker"

echo "macos_tg_xtask_profile_contract=PASS"
