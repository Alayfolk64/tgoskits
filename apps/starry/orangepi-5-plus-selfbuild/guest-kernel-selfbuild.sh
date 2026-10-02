#!/bin/bash
# Measure only the kernel build with an already prepared task tool.
set -euo pipefail

marker=STARRY-ORANGEPI5PLUS-SELFBUILD
run_id=${1:-kernel-cold}
source_dir=$(readlink -f /opt/tgoskits)
build_config=apps/starry/orangepi-5-plus-selfbuild/build-aarch64-unknown-none-softfloat.toml
xtask=/usr/local/bin/tg-xtask
run_dir=/output/runs/$run_id

fail() {
    printf '===%s-FAIL reason=%s===\n' "$marker" "$1"
    exit 1
}

case "$run_id" in
    ''|*[!A-Za-z0-9._-]*) fail invalid-run-id ;;
esac
[ -f "$source_dir/Cargo.toml" ] || fail source-missing
[ -f "$source_dir/.tgoskits-source-meta" ] || fail source-meta-missing
[ -f "$source_dir/$build_config" ] || fail build-config-missing
[ -x "$xtask" ] || fail prepared-xtask-missing
target_mode=$(sed -n 's/^target_mode=//p' /etc/starry-selfbuild/run.conf)
target_mode=${target_mode:-cold}
target_dir=$(sed -n 's/^target_dir=//p' /etc/starry-selfbuild/run.conf)
if [ -z "$target_dir" ]; then
    target_dir=$source_dir/target
else
    case "$target_dir" in
        /opt/tgoskits-sources/*/target) ;;
        *) fail invalid-target-dir ;;
    esac
fi
artifact=$target_dir/aarch64-unknown-none-softfloat/release/starryos
case "$target_mode" in
    cold)
        [ ! -e "$target_dir" ] || fail kernel-target-not-cold
        cold_target=true
        ;;
    reuse)
        [ -d "$target_dir" ] || fail kernel-target-cache-missing
        rm -f "$artifact" "$artifact.bin"
        cold_target=false
        ;;
    *) fail invalid-target-mode ;;
esac
mkdir -p -- "${run_dir%/*}"
mkdir -- "$run_dir" || fail run-directory-already-exists
exec > >(tee "$run_dir/run.log") 2>&1

export CARGO_HOME=/root/.cargo
export PATH="$CARGO_HOME/bin:/usr/local/bin:/usr/bin:/bin"
export CARGO_NET_OFFLINE=true
if [ "$target_dir" = "$source_dir/target" ]; then
    unset CARGO_TARGET_DIR
else
    export CARGO_TARGET_DIR="$target_dir"
fi
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

printf '===%s-BEGIN run=%s workload=starry cold_target=%s===\n' "$marker" "$run_id" "$cold_target"
printf 'target_dir=%s\n' "$target_dir"
start=$(date +%s)
set +e
"$xtask" starry build --config "$build_config"
rc=$?
set -e
elapsed=$(( $(date +%s) - start ))
printf '===%s-STARRY-BUILD-END run=%s rc=%s elapsed=%s===\n' "$marker" "$run_id" "$rc" "$elapsed"
[ "$rc" -eq 0 ] || fail "starry-build-rc-$rc"

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
    sha256sum -c SHA256SUMS
)
printf '%s\n' "$run_id" > /output/latest-run
sync
printf '===%s-PASS run=%s workload=starry cold_target=%s elapsed=%s===\n' "$marker" "$run_id" "$cold_target" "$elapsed"
