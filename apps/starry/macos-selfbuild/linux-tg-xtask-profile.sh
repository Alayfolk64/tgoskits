#!/usr/bin/env bash
set -euo pipefail

app_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
workspace="$(cd "$app_dir/../../.." && pwd)"
output_dir="$workspace/target/profiling/arceos-helloworld/linux/formal"
source_tar="$workspace/target/starry-macos-selfbuild/arceos-helloworld-profile-source.tar"
source_dir="$output_dir/source"
tg_xtask_target="$output_dir/tg-xtask-target"
tg_xtask="$tg_xtask_target/debug/tg-xtask"
profile_program="$app_dir/linux-tg-xtask-profile.bt"
cpu_list=0-7
workload="tg-xtask arceos build --package arceos-helloworld --arch aarch64"
profile_pid=
workload_pid=
start_gate=
old_kptr_restrict=
old_perf_event_paranoid=

restore_host_profile_settings() {
    if [[ -n "$profile_pid" ]] && kill -0 "$profile_pid" 2>/dev/null; then
        sudo -n kill -INT "$profile_pid" 2>/dev/null || true
        wait "$profile_pid" 2>/dev/null || true
    fi
    if [[ -n "$workload_pid" ]] && kill -0 "$workload_pid" 2>/dev/null; then
        kill -TERM "$workload_pid" 2>/dev/null || true
        wait "$workload_pid" 2>/dev/null || true
    fi
    if [[ -n "$start_gate" ]]; then
        rm -f "$start_gate"
    fi
    if [[ -n "$old_kptr_restrict" ]]; then
        sudo -n sysctl -q -w "kernel.kptr_restrict=$old_kptr_restrict" >/dev/null || true
    fi
    if [[ -n "$old_perf_event_paranoid" ]]; then
        sudo -n sysctl -q -w "kernel.perf_event_paranoid=$old_perf_event_paranoid" >/dev/null || true
    fi
}
trap restore_host_profile_settings EXIT INT TERM

profile_attached() {
    grep -Eq '^Attach(ed|ing) [0-9][0-9]* probes' "$output_dir/kernel-profile.raw"
}

[[ -f "$source_tar" ]] || {
    echo "missing fixed profile workload source: $source_tar" >&2
    exit 1
}
command -v bpftrace >/dev/null
command -v taskset >/dev/null
sudo -n true

mkdir -p "$output_dir"
rm -rf "$source_dir"
mkdir -p "$source_dir"
tar -xf "$source_tar" -C "$source_dir"

(cd "$source_dir" && CARGO_TARGET_DIR="$tg_xtask_target" cargo build --locked -p tg-xtask)
[[ -x "$tg_xtask" ]]

old_kptr_restrict="$(sudo -n cat /proc/sys/kernel/kptr_restrict)"
old_perf_event_paranoid="$(sudo -n cat /proc/sys/kernel/perf_event_paranoid)"
sudo -n sysctl -q -w kernel.kptr_restrict=0 >/dev/null
sudo -n sysctl -q -w kernel.perf_event_paranoid=-1 >/dev/null

source_hash="$(sha256sum "$source_tar" | sed 's/[[:space:]].*$//')"
start_gate="$output_dir/workload.start"
rm -f "$start_gate"
mkfifo "$start_gate"
(
    read -r _ <"$start_gate"
    started_ns="$(date +%s%N)"
    set +e
    (
        cd "$source_dir"
        /usr/bin/time -v -o "$output_dir/workload.time.txt" \
            taskset -c "$cpu_list" "$tg_xtask" \
            arceos build --package arceos-helloworld --arch aarch64
    ) 2>&1 | tee "$output_dir/run.log"
    command_rc="${PIPESTATUS[0]}"
    set -e
    stopped_ns="$(date +%s%N)"
    printf '%s\n' "$started_ns" >"$output_dir/workload.started_ns"
    printf '%s\n' "$stopped_ns" >"$output_dir/workload.stopped_ns"
    printf '%s\n' "$command_rc" >"$output_dir/workload.rc"
) &
workload_pid=$!

: >"$output_dir/kernel-profile.raw"
sudo -n env BPFTRACE_MAX_MAP_KEYS=65536 \
    bpftrace -B none "$profile_program" "$workload_pid" \
    >"$output_dir/kernel-profile.raw" 2>&1 &
profile_pid=$!
for _ in $(seq 1 120); do
    profile_attached && break
    kill -0 "$profile_pid" 2>/dev/null || {
        cat "$output_dir/kernel-profile.raw" >&2
        exit 1
    }
    sleep 0.25
done
profile_attached
sleep 2
kill -0 "$profile_pid"

printf 'start\n' >"$start_gate"
wait "$workload_pid" || true
workload_pid=
rm -f "$start_gate"
start_gate=

sudo -n kill -INT "$profile_pid" 2>/dev/null || true
wait "$profile_pid" 2>/dev/null || true
profile_pid=

started_ns="$(sed -n '1p' "$output_dir/workload.started_ns")"
stopped_ns="$(sed -n '1p' "$output_dir/workload.stopped_ns")"
command_rc="$(sed -n '1p' "$output_dir/workload.rc")"
elapsed_ns="$((stopped_ns - started_ns))"
compile_units="$(grep -c '^   Compiling ' "$output_dir/run.log" || true)"
binary="$source_dir/target/aarch64-unknown-linux-musl/release/arceos-helloworld"
[[ "$command_rc" == 0 ]]
[[ -s "$binary" ]]
sha256sum "$binary" >"$output_dir/arceos-helloworld.sha256"
{
    echo profile_backend=linux-bpftrace
    echo workload="$workload"
    echo cpu_list="$cpu_list"
    echo logical_cpus=8
    echo frequency_hz=10
    echo source_archive_sha256="$source_hash"
    echo elapsed_ns="$elapsed_ns"
    echo command_rc="$command_rc"
    echo compile_units="$compile_units"
    echo kernel="$(uname -srvm)"
    echo bpftrace="$(bpftrace --version)"
} >"$output_dir/profile.meta"

echo "LINUX-ARCEOS-HELLOWORLD-PROFILE-PASS elapsed_ns=$elapsed_ns rc=$command_rc"
