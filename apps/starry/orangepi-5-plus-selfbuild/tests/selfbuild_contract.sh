#!/bin/bash
set -euo pipefail

app_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
repo_root="$(cd "$app_dir/../../.." && pwd)"

fail() {
    echo "selfbuild contract: $*" >&2
    exit 1
}

shell_number() {
    local file=$1
    local name=$2
    local value
    value="$(sed -n "s/^${name}=\([0-9][0-9]*\)$/\1/p" "$file")"
    case "$value" in
        ''|*[!0-9]*) fail "cannot read $name from $file" ;;
    esac
    printf '%s\n' "$value"
}

timeout_after_kill_margin() {
    local file=$1
    local value
    value="$(awk '
        {
            for (i = 1; i < NF; i++) {
                if ($i == "--kill-after=60") {
                    print $(i + 1)
                    exit
                }
            }
        }
    ' "$file")"
    value="${value%\\}"
    case "$value" in
        ''|*[!0-9]*) fail "cannot read command timeout from $file" ;;
    esac
    printf '%s\n' "$value"
}

guest_timeout="$(shell_number "$app_dir/init.sh" guest_timeout)"
serial_timeout="$(shell_number "$app_dir/run_selfbuild.sh" serial_timeout)"
linux_timeout="$(timeout_after_kill_margin "$app_dir/run_linux_remote.sh")"
board_timeout="$(sed -n 's/^timeout = \([0-9][0-9]*\)$/\1/p' \
    "$app_dir/board-orangepi-5-plus-selfbuild.toml")"
watchdog_lease="$(sed -n \
    '/const SELFBUILD_WATCHDOG_LEASE:/s/.*from_secs(\([0-9_][0-9_]*\)).*/\1/p' \
    "$repo_root/os/StarryOS/kernel/src/entry.rs" | tr -d '_')"

case "$board_timeout" in
    ''|*[!0-9]*) fail "cannot read board timeout" ;;
esac
case "$watchdog_lease" in
    ''|*[!0-9]*) fail "cannot read watchdog lease" ;;
esac

# The first physical-board cold target run exceeded 9,600 seconds while Cargo
# was still compiling dependencies. Keep a six-hour command budget, followed
# by ten minutes for shutdown recovery and another ten minutes for the host
# serial/Linux-return path.
[ "$guest_timeout" -ge 21600 ] \
    || fail "guest timeout is below the measured cold-build budget"
[ "$linux_timeout" -ge "$guest_timeout" ] \
    || fail "Linux baseline timeout is shorter than the guest timeout"
[ "$watchdog_lease" -ge "$((guest_timeout + 600))" ] \
    || fail "watchdog lease lacks the shutdown recovery margin"
[ "$serial_timeout" -ge "$((watchdog_lease + 600))" ] \
    || fail "serial timeout lacks the Linux-return margin"
[ "$board_timeout" -ge "$serial_timeout" ] \
    || fail "board timeout is shorter than the direct-serial workflow"

grep -q 'aarch64-linux-musl-gcc' "$app_dir/guest-selfbuild.sh" \
    || fail "guest preflight does not check the compiler invoked by lwprintf-rs"
grep -q 'aarch64-linux-musl-gcc' "$app_dir/provision_rootfs_remote.sh" \
    || fail "rootfs provisioning does not provide the lwprintf-rs compiler name"
grep -q "rustup run.*rustc --version" "$app_dir/provision_rootfs_remote.sh" \
    || fail "rootfs provisioning does not reuse an installed pinned toolchain"
grep -q "rustup component list.*--installed" "$app_dir/provision_rootfs_remote.sh" \
    || fail "rootfs provisioning does not verify installed Rust components"

# Linux and StarryOS must use the same scheduler-visible CPU set and Cargo's
# default parallelism. The guest first builds the host runner, then invokes the
# resulting binary for the StarryOS kernel build.
if grep -q 'taskset -c' "$app_dir/init.sh" "$app_dir/run_linux_remote.sh"; then
    fail "self-build entrypoints pin the build to a subset of CPUs"
fi
if grep -Eq 'CARGO_BUILD_JOBS|RAYON_NUM_THREADS|-Zthreads' \
    "$app_dir/guest-selfbuild.sh" "$app_dir/build_seed.sh"; then
    fail "self-build overrides Cargo or rustc default parallelism"
fi
grep -q 'cargo build -p tg-xtask)' "$app_dir/guest-selfbuild.sh" \
    || fail "guest does not build the tg-xtask host runner first"
grep -q 'starry build --config' "$app_dir/guest-selfbuild.sh" \
    || fail "guest does not build StarryOS through the tg-xtask binary"
if grep -Eq 'xtask_command=.*--release|build_command=.*--arch' \
    "$app_dir/guest-selfbuild.sh"; then
    fail "guest adds unnecessary parameters to the two build commands"
fi
grep -q 'PROGRESS phase=' "$app_dir/guest-selfbuild.sh" \
    || fail "guest does not emit periodic compile-unit progress markers"
grep -q 'kernel-target-not-cold' "$app_dir/guest-selfbuild.sh" \
    || fail "guest does not reject a warm StarryOS target directory"
grep -q 'kernel_target_dir/\$build_target/release/starryos' \
    "$app_dir/guest-selfbuild.sh" \
    || fail "guest does not collect the tg-xtask StarryOS artifact"

# Profiling is a bounded diagnostic of the first command.  The current StarryOS
# PMU sampler emits flat instruction-pointer samples but does not implement
# PERF_SAMPLE_CALLCHAIN, so `perf record -g` would be rejected by the kernel.
profile_duration="$(shell_number "$app_dir/guest-selfbuild.sh" profile_duration)"
[ "$profile_duration" -eq 300 ] \
    || fail "tg-xtask profiling window is not the maintained five-minute default"
perf_stat_line="$(grep -n 'perf stat .*xtask_command' "$app_dir/guest-selfbuild.sh" \
    | head -n 1 | cut -d: -f1)"
perf_record_line="$(grep -n 'perf record .*xtask_command' "$app_dir/guest-selfbuild.sh" \
    | head -n 1 | cut -d: -f1)"
starry_command_line="$(grep -n 'build_command=.*starry build --config' \
    "$app_dir/guest-selfbuild.sh" | head -n 1 | cut -d: -f1)"
case "$perf_stat_line:$perf_record_line:$starry_command_line" in
    *[!0-9:]*|:*|*::*) fail "cannot locate first-phase profiling commands" ;;
esac
[ "$perf_stat_line" -lt "$starry_command_line" ] \
    || fail "perf stat is not attached to the tg-xtask build"
[ "$perf_record_line" -lt "$starry_command_line" ] \
    || fail "perf record is not attached to the tg-xtask build"
if grep -Eq 'perf record .* (-g|--call-graph)( |$)' "$app_dir/guest-selfbuild.sh"; then
    fail "perf record requests unsupported callchain samples"
fi
grep -q 'PROFILE-PASS run=' "$app_dir/guest-selfbuild.sh" \
    || fail "bounded profiling has no explicit completion marker"
grep -q 'profile.meta' "$app_dir/fetch_artifacts.sh" \
    || fail "host artifact fetch does not recognize profiling-only runs"
grep -q 'PROFILE-TARGET-RETAINED' "$app_dir/guest-selfbuild.sh" \
    || fail "perf record target is not retained for symbol resolution"
grep -q 'PROFILE-TARGET-CLEANUP' "$app_dir/fetch_artifacts.sh" \
    || fail "host fetch does not clean the retained perf record target"

echo "orangepi5plus_selfbuild_contract=PASS"
