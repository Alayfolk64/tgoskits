#!/bin/bash
# Measure only the kernel build with an already prepared task tool.
set -euo pipefail

marker=STARRY-ORANGEPI5PLUS-SELFBUILD
run_id=${1:-kernel-cold}
source_dir=$(readlink -f /opt/tgoskits)
build_config=apps/starry/orangepi-5-plus-selfbuild/build-aarch64-unknown-none-softfloat.toml
xtask=/usr/local/bin/tg-xtask
run_dir=/output/runs/$run_id
kernel_profile=${STARRY_KERNEL_PROFILE:-off}

fail() {
    printf '===%s-FAIL reason=%s===\n' "$marker" "$1"
    exit 1
}

case "$run_id" in
    ''|*[!A-Za-z0-9._-]*) fail invalid-run-id ;;
esac
case "$kernel_profile" in
    off|on) ;;
    *) fail invalid-kernel-profile ;;
esac
[ -f "$source_dir/Cargo.toml" ] || fail source-missing
[ -f "$source_dir/.tgoskits-source-meta" ] || fail source-meta-missing
[ -f "$source_dir/$build_config" ] || fail build-config-missing
[ -x "$xtask" ] || fail prepared-xtask-missing
[ ! -e "$source_dir/target" ] || fail kernel-target-not-cold
[ ! -e "$run_dir" ] || fail run-directory-already-exists
mkdir -p "$run_dir"
exec > >(tee "$run_dir/run.log") 2>&1

export CARGO_HOME=/root/.cargo
export PATH="$CARGO_HOME/bin:/usr/local/bin:/usr/bin:/bin"
export CARGO_NET_OFFLINE=true
unset CARGO_TARGET_DIR
cd "$source_dir"

sysroot=$(rustc --print sysroot)
host=$(rustc -vV)
host=${host#*host: }
host=${host%%$'\n'*}
objcopy=$sysroot/lib/rustlib/$host/bin/llvm-objcopy
# The llvm-tools component depends on the matching Rust LLVM shared library.
# Use the installed sysroot; a missing library still makes this preflight fail.
export LD_LIBRARY_PATH="$sysroot/lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
"$objcopy" --version || fail llvm-objcopy-preflight
rustc --version --verbose
cargo --version
sha256sum "$xtask" "$objcopy"
cat .tgoskits-source-meta
printf 'available_cpus=%s\n' "$(nproc)"
sed -n '/^Cpus_allowed_list:/p' /proc/self/status
for policy in /sys/devices/system/cpu/cpufreq/policy*; do
    [ -d "$policy" ] || continue
    for setting in affected_cpus scaling_cur_freq scaling_max_freq scaling_governor; do
        [ -f "$policy/$setting" ] || continue
        printf '%s/%s=' "$policy" "$setting"
        cat "$policy/$setting"
    done
done

profile_pid=
if [ "$kernel_profile" = on ]; then
    [ -r /proc/starry_profile ] && [ -w /proc/starry_profile ] \
        || fail kernel-profile-unavailable
    printf 'reset\n' > /proc/starry_profile
    printf 'start\n' > /proc/starry_profile
    profile_status=$(sed -n '1p' /proc/starry_profile)
    case "$profile_status" in
        'STARRY_PROFILE_V1 '*'enabled=true phase=1 '*) ;;
        *) fail kernel-profile-start ;;
    esac
    # End sampling after five minutes while the complete build keeps running.
    python3 - "$run_dir" <<'PY' &
from pathlib import Path
from collections import Counter
import json
import sys
import time

run = Path(sys.argv[1])
control = Path("/proc/starry_profile")
started = time.monotonic()
next_report = 60
recorded = False
while True:
    elapsed = time.monotonic() - started
    done = (run / "profile-build.rc").exists()
    if not recorded and (elapsed >= 300 or done):
        control.write_text("stop\n")
        snapshot = control.read_text()
        lines = snapshot.splitlines()
        if not lines or not lines[0].startswith("STARRY_PROFILE_V1 "):
            raise SystemExit("kernel profile header missing")
        if "enabled=false phase=0 " not in lines[0]:
            raise SystemExit("kernel profile did not stop")
        if not any(line.startswith(("STARRY_CPU ", "STARRY_WAIT ")) for line in lines):
            raise SystemExit("kernel profile has no samples")
        (run / "kernel-profile.raw").write_text(snapshot)
        print(lines[0], flush=True)
        print(f"kernel_profile_window_complete={elapsed:.1f}", flush=True)
        recorded = True
    if done:
        break
    if elapsed >= next_report:
        processes = []
        for process in Path("/proc").iterdir():
            if not process.name.isdecimal():
                continue
            try:
                stat = (process / "stat").read_text()
            except (FileNotFoundError, ProcessLookupError):
                # A process can exit between enumeration and reading stat.
                continue
            name = stat[stat.index("(") + 1:stat.rindex(")")]
            if name not in {"rustc", "cargo", "tg-xtask", "rust-lld", "ld.lld"}:
                continue
            fields = stat[stat.rindex(")") + 2:].split()
            thread_fields = []
            try:
                for thread in (process / "task").iterdir():
                    try:
                        thread_stat = (thread / "stat").read_text()
                    except (FileNotFoundError, ProcessLookupError):
                        continue
                    thread_fields.append(thread_stat[thread_stat.rindex(")") + 2:].split())
            except (FileNotFoundError, ProcessLookupError):
                continue
            processes.append({"pid": int(process.name), "comm": name,
                              "state": fields[0], "utime_ticks": int(fields[11]),
                              "stime_ticks": int(fields[12]), "threads": int(fields[17]),
                              "rss_pages": int(fields[21]), "cpu": int(fields[36]),
                              "thread_ticks": sum(int(t[11]) + int(t[12]) for t in thread_fields),
                              "thread_states": dict(Counter(t[0] for t in thread_fields)),
                              "runnable_thread_cpus": [int(t[36]) for t in thread_fields if t[0] == "R"]})
        print(f"kernel_profile_progress elapsed={elapsed:.1f} "
              f"processes={json.dumps(processes)}", flush=True)
        next_report = elapsed + 60
    time.sleep(5)
PY
    profile_pid=$!
fi

printf '===%s-BEGIN run=%s workload=starry cold_target=true===\n' "$marker" "$run_id"
start=$(date +%s)
set +e
"$xtask" starry build --config "$build_config"
rc=$?
set -e
elapsed=$(( $(date +%s) - start ))
printf '===%s-STARRY-BUILD-END run=%s rc=%s elapsed=%s===\n' "$marker" "$run_id" "$rc" "$elapsed"
if [ -n "$profile_pid" ]; then
    printf '%s\n' "$rc" > "$run_dir/profile-build.rc"
    wait "$profile_pid" || fail kernel-profile-record
    [ -s "$run_dir/kernel-profile.raw" ] || fail kernel-profile-empty
fi
[ "$rc" -eq 0 ] || fail "starry-build-rc-$rc"

artifact=$source_dir/target/aarch64-unknown-none-softfloat/release/starryos
[ -s "$artifact" ] || fail artifact-elf-missing
[ -s "$artifact.bin" ] || fail artifact-bin-missing
python3 - "$artifact" <<'PY'
import pathlib
import sys

with pathlib.Path(sys.argv[1]).open("rb") as elf:
    header = elf.read(20)
if header[:6] != b"\x7fELF\x02\x01" or int.from_bytes(header[18:20], "little") != 183:
    raise SystemExit("expected a little-endian ELF64 AArch64 kernel")
PY
cp "$artifact" "$run_dir/starryos.elf"
cp "$artifact.bin" "$run_dir/starryos.bin"
cp .tgoskits-source-meta "$run_dir/source.meta"
printf '%s\n' "$elapsed" > "$run_dir/starry-build-elapsed-seconds"
(
    cd "$run_dir"
    sha256sum starryos.elf starryos.bin source.meta > SHA256SUMS
    if [ "$kernel_profile" = on ]; then
        sha256sum kernel-profile.raw >> SHA256SUMS
    fi
    sha256sum -c SHA256SUMS
)
printf '%s\n' "$run_id" > /output/latest-run
sync
printf '===%s-PASS run=%s workload=starry cold_target=true elapsed=%s===\n' "$marker" "$run_id" "$elapsed"
