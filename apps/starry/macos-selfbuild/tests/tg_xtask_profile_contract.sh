#!/bin/sh
set -eu

app_dir="$(CDPATH='' cd -- "$(dirname "$0")/.." && pwd)"
runner="$app_dir/guest-tg-xtask-profile.sh"
config="$app_dir/qemu-aarch64-profile.toml"
linux_runner="$app_dir/guest-linux-profile.sh"
linux_workload="$app_dir/guest-linux-profile-workload.sh"
linux_bpftrace="$app_dir/guest-linux-profile.bt"
renderer="$app_dir/render-kernel-profile.py"
workspace_tmp="$app_dir/../../../tmp"
profile_proc="$app_dir/../../../os/StarryOS/kernel/src/pseudofs/proc.rs"
profile_api="$app_dir/../../../os/arceos/modules/axsync/src/profile.rs"
page_cache="$app_dir/../../../fs/ax-fs-ng/src/file/cache/mod.rs"
ext4_guard="$app_dir/../../../fs/ax-fs-ng/src/fs/ext4/rsext4/fs/guard.rs"
ext4_disk="$app_dir/../../../fs/ax-fs-ng/src/fs/ext4/rsext4/mod.rs"

sh "$app_dir/tests/toolchain_overlay_contract.sh"
python3 "$app_dir/tests/test_render_kernel_profile.py"

fail() {
    echo "macos tg-xtask profile contract: $1" >&2
    exit 1
}

[ -f "$runner" ] || fail "profile guest runner is missing"
[ -f "$config" ] || fail "profile QEMU config is missing"
[ -f "$linux_runner" ] || fail "Linux guest comparison runner is missing"
[ -f "$linux_workload" ] || fail "Linux guest workload entry is missing"
[ -f "$linux_bpftrace" ] || fail "Linux comparison bpftrace program is missing"
[ -x "$renderer" ] || fail "Starry kernel profile renderer is missing or not executable"

mkdir -p "$workspace_tmp"
renderer_test_dir="$(mktemp -d "$workspace_tmp/tg-xtask-profile-contract.XXXXXX")"
trap 'rm -rf "$renderer_test_dir"' EXIT
mkdir -p "$renderer_test_dir/artifacts/arceos-helloworld-profile"
cp /bin/true "$renderer_test_dir/starryos.elf"
cat > "$renderer_test_dir/artifacts/arceos-helloworld-profile/kernel-profile.raw" <<'EOF'
STARRY_PROFILE_V1 enabled=false phase=0 phase1_ns=100 skipped_cpu=0
STARRY_CPU phase=1 cpu=0 samples=3 task=69646c65 stack=user
STARRY_CPU phase=1 cpu=0 samples=7 task=7275737463 stack=user
STARRY_CPU phase=1 cpu=1 samples=10 task=69646c65 stack=user
EOF
"$renderer" "$renderer_test_dir"
python3 - "$renderer_test_dir/rendered/summary.json" <<'PY'
import json
import sys

with open(sys.argv[1], encoding="utf-8") as source:
    cpu = json.load(source)["cpu"]

assert cpu["by_cpu_active_samples"] == {"0": 7, "1": 0}
assert cpu["by_cpu_idle_samples"] == {"0": 3, "1": 10}
assert cpu["by_cpu_by_task"] == {
    "0": {"idle": 3, "rustc": 7},
    "1": {"idle": 10},
}
PY
grep -Fq 'STARRY_CPU phase=' "$renderer" \
    || fail "Starry renderer does not consume the direct kernel profile format"
grep -Fq 'workload="$tg_xtask_bin arceos build --package arceos-helloworld --arch aarch64"' "$runner" \
    || fail "profile workload is not tg-xtask arceos-helloworld"
grep -Fq 'tg_xtask_bin=/opt/tgoskits-profile/bin/tg-xtask' "$runner" \
    || fail "profile tg-xtask is not reusable from the rootfs"
grep -Fq 'tg_xtask_stamp=/opt/tgoskits-profile/tg-xtask.source.sha256' "$runner" \
    || fail "profile tg-xtask has no source fingerprint"
grep -Fq 'source_stamp=/opt/tgoskits-profile/source.tar.sha256' "$runner" \
    || fail "profile source checkout has no persistent archive fingerprint"
grep -Fq 'source_reused=true' "$runner" \
    || fail "profile source checkout cannot be reused from the rootfs"
if grep -Fq 'sed -i' "$runner"; then
    fail "profile runner mutates immutable source configuration before every build"
fi
grep -Fq 'cargo_config_tmp="$CARGO_HOME/.config.toml.profile.$$"' "$runner" \
    || fail "profile runner does not stage its reusable Cargo configuration atomically"
grep -Fq 'mv "$cargo_config_tmp" "$cargo_config"' "$runner" \
    || fail "profile runner does not publish a complete Cargo configuration atomically"
if grep -Fq 'cat >> "$cargo_config"' "$runner"; then
    fail "profile runner appends in place to persistent Cargo configuration"
fi
grep -Fq 'rm -rf "$source_dir/target" "$source_dir/tmp/axbuild"' "$runner" \
    || fail "profile runner retains generated Cargo or axbuild state between cold builds"
for guest_workload in "$runner" "$linux_workload"; do
    if grep -Eq 'timeout --signal|124\)' "$guest_workload"; then
        fail "full-build profiling must not time-limit compilation or accept timeout: $guest_workload"
    fi
done
for guest_runner in "$runner" "$linux_runner"; do
    grep -Fq 'echo measurement=full-build' "$guest_runner" \
        || fail "profile metadata does not declare a full build: $guest_runner"
    grep -Fq 'echo timeout_seconds=0' "$guest_runner" \
        || fail "profile metadata still declares a measurement deadline: $guest_runner"
done
grep -Fq '[ "$build_completed" = true ] || fail "profile-command-${profile_rc}"' "$runner" \
    || fail "Starry full-build profiling accepts an incomplete compilation"
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
if grep -Eq 'xtask starry perf|qperf|perf record|perf stat' "$runner"; then
    fail "profile runner delegates profiling outside the Starry kernel"
fi
grep -Fq '"$tg_xtask_bin" arceos build --package arceos-helloworld --arch aarch64' "$linux_workload" \
    || fail "Linux comparison does not use the same tg-xtask workload"
grep -Fq 'tg_xtask_bin=/opt/tgoskits-profile/bin/tg-xtask' "$linux_runner" \
    || fail "Linux comparison does not reuse the rootfs tg-xtask binary"
grep -Fq 'cd "$source_dir"' "$linux_workload" \
    || fail "Linux comparison does not use the frozen workload checkout"
grep -Fq '/bin/sh /opt/guest-linux-profile-workload.sh' "$linux_runner" \
    || fail "Linux comparison does not invoke its guest workload"
grep -Fq 'LINUX_KERNEL_PROFILE_BEGIN' "$linux_runner" \
    || fail "Linux comparison does not wait for kernel probe readiness"
grep -Fq 'mount -o remount,ro /' "$linux_runner" \
    || fail "Linux comparison does not cleanly shut down its rootfs"
for guest_runner in "$runner" "$linux_runner"; do
    grep -Fq 'export TMPDIR="$source_dir/tmp"' "$guest_runner" \
        || fail "guest temporary files are outside the workload workspace: $guest_runner"
    grep -Fq 'export CARGO_UNSTABLE_HOST_CONFIG=true' "$guest_runner" \
        || fail "guest Cargo host configuration gate is missing: $guest_runner"
    grep -Fq 'export CARGO_UNSTABLE_TARGET_APPLIES_TO_HOST=true' "$guest_runner" \
        || fail "guest Cargo target-applies-to-host gate is missing: $guest_runner"
done
grep -Fq 'profile:hz:10' "$linux_bpftrace" \
    || fail "Linux comparison CPU sampling is not aligned at 10 Hz"
grep -Fq 'tracepoint:sched:sched_switch' "$linux_bpftrace" \
    || fail "Linux comparison does not collect off-CPU intervals"
if grep -Eq 'perf record|perf stat|qperf' "$linux_runner"; then
    fail "Linux comparison uses perf/qperf instead of the aligned bpftrace probes"
fi
grep -q 'host_success_settle_seconds=3' "$runner" \
    || fail "profile runner does not keep the guest alive after printing the host success marker"
grep -Fq 'sleep "$host_success_settle_seconds"' "$runner" \
    || fail "profile runner powers off before the host can commit the success match"
awk '/^fail\(\)/,/^}/ { if ($0 == "    sync") flushed = 1 }
    END { exit !flushed }' "$runner" \
    || fail "profile runner publishes its fail marker before flushing the reusable rootfs"
if grep -Eq 'CARGO_BUILD_JOBS|RAYON_NUM_THREADS|RUSTC_THREADS|-Zthreads|taskset' "$runner"; then
    fail "profile runner limits build parallelism or affinity"
fi
grep -q 'STARRY_MACOS_SELFBUILD_MODE' "$app_dir/prebuild.sh" \
    || fail "prebuild cannot select the profile runner"
grep -Fq 'task_tmp_dir="$workspace/tmp"' "$app_dir/prebuild.sh" \
    || fail "prebuild does not keep temporary files inside the workspace"
grep -Fq 'mktemp -p "$task_tmp_dir" starry-tar-flags.XXXXXX' "$app_dir/prebuild.sh" \
    || fail "prebuild tar probing can create temporary files outside the workspace"
grep -q -- '--exclude docs/node_modules' "$app_dir/prebuild.sh" \
    || fail "profile source archive includes docs/node_modules"
grep -q -- '--exclude docs/build' "$app_dir/prebuild.sh" \
    || fail "profile source archive includes generated documentation"
grep -q 'arceos-helloworld-profile-source.tar' "$app_dir/prebuild.sh" \
    || fail "prebuild does not preserve a fixed profile workload snapshot"
grep -q 'prepare_profile_source_overlay' "$app_dir/prebuild.sh" \
    || fail "prebuild cannot repair or seed the reusable source checkout from the host"
grep -q 'prepare_profile_tg_xtask' "$app_dir/prebuild.sh" \
    || fail "prebuild does not prepare tg-xtask on the host"
grep -q 'prepare_profile_source_host_cache' "$app_dir/prebuild.sh" \
    || fail "prebuild does not preserve a host checkout of the frozen profile source"
grep -Fq 'cd "$profile_source_host"' "$app_dir/prebuild.sh" \
    || fail "prebuild does not build tg-xtask from the frozen profile source"
grep -q -- '--target aarch64-unknown-linux-musl' "$app_dir/prebuild.sh" \
    || fail "prebuild does not cross-compile the reusable AArch64 tg-xtask"
grep -Fq 'opt/tgoskits-profile/bin/tg-xtask' "$app_dir/prebuild.sh" \
    || fail "prebuild does not inject tg-xtask into the reusable rootfs path"
grep -Fq 'opt/tgoskits-profile/tg-xtask.source.sha256' "$app_dir/prebuild.sh" \
    || fail "prebuild does not inject the tg-xtask source fingerprint"
grep -Fq 'for tree in xtask scripts/axbuild' "$runner" \
    || fail "guest and host tg-xtask fingerprints do not cover both implementation trees"
grep -q 'guest-profile' "$app_dir/build-aarch64-unknown-none-softfloat.toml" \
    || fail "Starry kernel profiler feature is not enabled"
grep -q 'struct StarryProfileFile' "$profile_proc" \
    || fail "profile proc node does not preserve direct control-write semantics"
grep -q 'SpecialFsFile::new_regular_with_perm' "$profile_proc" \
    || fail "profile proc node is still backed by SimpleFile"
grep -Fq 'STARRY_CPU phase={} cpu={} samples={}' "$app_dir/../../../os/StarryOS/kernel/src/profiler.rs" \
    || fail "Starry kernel CPU records do not preserve their vCPU identity"
grep -Fq 'STARRY_WAIT phase={} cpu={} event={}' "$app_dir/../../../os/StarryOS/kernel/src/profiler.rs" \
    || fail "Starry kernel wait records do not preserve their vCPU identity"
[ -f "$profile_api" ] || fail "MOSS-style latency event API is missing"
grep -q 'ProfileEvent::PageCache' "$page_cache" \
    || fail "page-cache latency is not instrumented"
grep -q 'ProfileEvent::Ext4' "$ext4_guard" \
    || fail "ext4 latency is not instrumented"
grep -q 'ProfileEvent::BlockRead' "$ext4_disk" \
    || fail "ext4 block-read latency is not instrumented"
grep -q 'BACKTRACE = "y"' "$app_dir/build-aarch64-unknown-none-softfloat.toml" \
    || fail "profile build does not force frame pointers"
grep -q '"tcg,thread=multi"' "$config" || fail "QEMU profile does not use multi-threaded TCG"
grep -q '"cortex-a53"' "$config" || fail "QEMU profile does not use a portable AArch64 CPU"
grep -q '"8"' "$config" || fail "QEMU profile does not expose eight vCPUs"
grep -q 'rootfs-aarch64-arceos-helloworld-profile.img' "$config" \
    || fail "QEMU profile does not use a dedicated reusable rootfs"
grep -q 'STARRY-ARCEOS-HELLOWORLD-PROFILE-WINDOW-PASS' "$config" \
    || fail "QEMU profile has no completion marker"
grep -Fq 'timeout = 0' "$config" \
    || fail "full-build QEMU profiling still has a host deadline"
grep -Fq 'rc=0 build_completed=true' "$config" \
    || fail "QEMU profiling accepts an incomplete compilation"

echo "macos_tg_xtask_profile_contract=PASS"
