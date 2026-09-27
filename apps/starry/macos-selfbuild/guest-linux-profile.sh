#!/bin/sh
# Zero-argument entry inside the prepared, disposable Linux QEMU guest.
set -eu

artifact_dir=/opt/linux-profile-artifacts
profile_root=/opt/tgoskits-profile
source_dir=/opt/tgoskits-profile/source
tg_xtask_bin=/opt/tgoskits-profile/bin/tg-xtask
stage=preflight
profile_pid=

report_exit() {
    rc="$?"
    # Cleanup failures must remain visible without suppressing the original error.
    set +e
    if [ -n "$profile_pid" ]; then
        kill -INT "$profile_pid"
        wait "$profile_pid"
    fi
    if [ "$rc" -ne 0 ]; then
        echo "LINUX_PROFILE_FAIL stage=$stage exit=$rc"
        echo "Cause: the named stage failed; original command output follows."
        for log in bpftrace.stderr kernel-profile.raw run.log; do
            if [ -f "$artifact_dir/$log" ]; then
                cat "$artifact_dir/$log"
            fi
        done
    fi
    sync
    exit "$rc"
}
trap report_exit EXIT

[ "$(uname -s)" = Linux ]
[ "$(uname -m)" = aarch64 ]
grep -q 'root=/dev/nvme0n1' /proc/cmdline
[ "$(grep -c '^processor' /proc/cpuinfo)" = 8 ]
[ -x "$tg_xtask_bin" ]
[ -f "$source_dir/Cargo.toml" ]
[ -f /opt/guest-linux-profile.bt ]
[ -f /opt/guest-linux-profile-workload.sh ]
stage=mount-kernel-tracepoints
if [ ! -f /sys/kernel/tracing/events/sched/sched_switch/format ]; then
    mount -t tracefs tracefs /sys/kernel/tracing
fi
if [ ! -f /sys/kernel/tracing/events/sched/sched_switch/format ]; then
    echo 'required sched:sched_switch tracepoint is absent after mounting tracefs'
    exit 1
fi
if [ -e "$artifact_dir" ]; then
    echo "refusing to overwrite previous profile: $artifact_dir"
    exit 1
fi
mkdir "$artifact_dir"
mkdir -p "$source_dir/tmp"
export TMPDIR="$source_dir/tmp"
export PATH=/opt/rust-nightly/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin
export LD_LIBRARY_PATH=/opt/rust-nightly/lib:/usr/lib
export RUSTC=/opt/rustc-nightly-sysroot
export RUSTDOC=/opt/rustdoc-nightly-sysroot
export CARGO_HOME=/root/.cargo
export CARGO_NET_OFFLINE=true
export RUSTC_BOOTSTRAP=1
# [host].rustflags is ignored unless both nightly Cargo gates are enabled.
export CARGO_UNSTABLE_HOST_CONFIG=true
export CARGO_UNSTABLE_TARGET_APPLIES_TO_HOST=true
unset CARGO_TARGET_DIR CARGO_BUILD_JOBS RUSTFLAGS

stage=verify-frozen-inputs
archive_checksum="$(sha256sum /opt/tgoskits-src.tar)"
archive_hash="${archive_checksum%% *}"
[ "$archive_hash" = 7de46545454e3f5562d78a1a065bf2ed1a736cf807f4cbf05b30d98ab2725147 ]
[ "$(sed -n '1p' "$profile_root/source.tar.sha256")" = "$archive_hash" ]
binary_checksum="$(sha256sum "$tg_xtask_bin")"
[ "${binary_checksum%% *}" = e6823ab3b6a6d944266fc31d5e54814f463466f88949849ba139b6c575f7e6f1 ]
[ "$(sed -n '1p' "$profile_root/tg-xtask.source.sha256")" = c5e7b4e0ea49661387a3bcd107aada00e74fa33c9ce3b3c5c71d232cc2f1762b ]

stage=clean-disposable-build-output
# These are generated build directories in the independent experiment rootfs.
# Preserve the frozen source, reusable tg-xtask and its own target directory.
[ ! -L "$source_dir" ]
rm -rf /opt/tgoskits-profile/source/target /opt/tgoskits-profile/source/tmp/axbuild

stage=record-environment
{
    uname -a
    bpftrace --version
    "$RUSTC" --version --verbose
    /opt/cargo-nightly-sysroot --version
    cat /proc/cmdline
    cat /proc/mounts
    cat /root/.cargo/config.toml
    echo "cpu_affinity=$(sed -n 's/^Cpus_allowed_list:[[:space:]]*//p' /proc/self/status)"
    echo "cargo_host_config=$CARGO_UNSTABLE_HOST_CONFIG"
    echo "cargo_target_applies_to_host_gate=$CARGO_UNSTABLE_TARGET_APPLIES_TO_HOST"
} > "$artifact_dir/environment.log"
cat "$artifact_dir/environment.log"
{
    echo profile_backend=linux-guest-bpftrace
    echo virtual_cpus=8
    echo accelerator=tcg
    echo measurement=full-build
    echo timeout_seconds=0
    echo frequency_hz=10
    echo parallelism=system-default
    echo tg_xtask_reused=true
    echo "source_archive_sha256=$archive_hash"
    echo "tg_xtask_sha256=${binary_checksum%% *}"
    echo workload="$tg_xtask_bin arceos build --package arceos-helloworld --arch aarch64"
} > "$artifact_dir/profile.meta"

stage=kernel-profile-and-compilation
bpftrace -f json /opt/guest-linux-profile.bt \
    -o "$artifact_dir/kernel-profile.raw" 2> "$artifact_dir/bpftrace.stderr" &
profile_pid="$!"
ready=false
attempt=0
while [ "$attempt" -lt 60 ]; do
    if [ -f "$artifact_dir/kernel-profile.raw" ] \
        && grep -q LINUX_KERNEL_PROFILE_BEGIN "$artifact_dir/kernel-profile.raw"; then
        ready=true
        break
    fi
    if ! kill -0 "$profile_pid"; then
        echo 'bpftrace exited before publishing its ready marker'
        profile_pid=
        exit 1
    fi
    attempt="$((attempt + 1))"
    sleep 1
done
if [ "$ready" != true ]; then
    echo 'bpftrace did not attach within the 60-second startup bound'
    exit 1
fi
# Do not use bpftrace -c: v0.24.1 confines profile probes to the child PID,
# although the unfiltered sched tracepoint still sees other guest tasks.
/bin/sh /opt/guest-linux-profile-workload.sh
kill -INT "$profile_pid"
wait "$profile_pid"
profile_pid=
cat "$artifact_dir/bpftrace.stderr"
grep -q LINUX_KERNEL_PROFILE_END "$artifact_dir/kernel-profile.raw"
if grep -Fq '"@cpu": {}' "$artifact_dir/kernel-profile.raw"; then
    echo 'bpftrace produced an empty CPU map; this is not a valid profiling window'
    exit 1
fi
stage=verify-workload-result
workload_rc="$(sed -n '1p' "$artifact_dir/workload.rc")"
case "$workload_rc" in
    0)
        arceos_binary="$source_dir/target/aarch64-unknown-linux-musl/release/arceos-helloworld"
        if [ ! -s "$arceos_binary" ]; then
            echo "workload returned 0 but its arceos-helloworld binary is missing: $arceos_binary"
            exit 1
        fi
        sha256sum "$arceos_binary" > "$artifact_dir/arceos-helloworld.sha256"
        ;;
    *)
        echo "workload command failed with exit=$workload_rc"
        exit 1
        ;;
esac
cat "$artifact_dir/profile.meta"
cat "$artifact_dir/run.log"
cd "$artifact_dir"
sha256sum kernel-profile.raw bpftrace.stderr profile.meta progress.log run.log \
    environment.log workload.rc cpu-before.stat cpu-after.stat \
    disk-before.stat disk-after.stat > SHA256SUMS
if [ "$workload_rc" = 0 ]; then
    sha256sum arceos-helloworld.sha256 >> SHA256SUMS
fi
sync
stage=clean-rootfs-shutdown
mount -o remount,ro /
echo LINUX_PROFILE_WINDOW_PASS
trap - EXIT
poweroff -f
