#!/bin/bash
set -euo pipefail

marker="${STARRY_SELFBUILD_MARKER:-STARRY-ORANGEPI5PLUS-SELFBUILD}"
run_id="${STARRY_SELFBUILD_RUN_ID:-manual}"
profile="${STARRY_SELFBUILD_PROFILE:-off}"
jobs="${JOBS:-4}"
rustc_threads="${RUSTC_THREADS:-2}"
source_dir=/opt/tgoskits
build_config=apps/starry/orangepi-5-plus-selfbuild/build-aarch64-unknown-none-softfloat.toml
target_dir="/work/targets/${run_id}"
run_dir="/output/runs/${run_id}"
run_log="$run_dir/run.log"

fail() {
    printf '===%s-FAIL reason=%s===\n' "$marker" "$1"
    exit 1
}

case "$run_id" in
    ''|*[!A-Za-z0-9._-]*) fail invalid-run-id ;;
esac
case "$profile" in
    off|stat|record) ;;
    *) fail invalid-profile ;;
esac
case "$jobs" in
    ''|*[!0-9]*|0) fail invalid-jobs ;;
esac
case "$rustc_threads" in
    ''|*[!0-9]*|0) fail invalid-rustc-threads ;;
esac

[ -f "$source_dir/Cargo.toml" ] || fail source-missing
[ -f "$source_dir/.tgoskits-source-meta" ] || fail source-meta-missing
[ -f "$source_dir/$build_config" ] || fail build-config-missing

mkdir -p "$run_dir" "$target_dir"
exec > >(tee -a "$run_log") 2>&1

export HOME=/root
export CARGO_HOME=/root/.cargo
export PATH="$CARGO_HOME/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin"
export CARGO_TARGET_DIR="$target_dir"
export CARGO_BUILD_JOBS="$jobs"
export CARGO_INCREMENTAL=0
export CARGO_NET_OFFLINE=true
export RAYON_NUM_THREADS="${RAYON_NUM_THREADS:-2}"
export AXBUILD_STARRY_KALLSYMS_AUTO_INSTALL=0
export AX_ARCH=aarch64
export AX_TARGET=aarch64-unknown-none-softfloat
export AX_MODE=release
export CC_aarch64_unknown_none_softfloat="${CC_aarch64_unknown_none_softfloat:-aarch64-linux-gnu-gcc}"
export AR_aarch64_unknown_none_softfloat="${AR_aarch64_unknown_none_softfloat:-aarch64-linux-gnu-ar}"
export CC_AARCH64_UNKNOWN_NONE_SOFTFLOAT="$CC_aarch64_unknown_none_softfloat"
export AR_AARCH64_UNKNOWN_NONE_SOFTFLOAT="$AR_aarch64_unknown_none_softfloat"

cd "$source_dir"

for command in \
    aarch64-linux-gnu-gcc cargo rustc rustup rust-nm rust-objcopy gen_ksym \
    file python3 readelf sha256sum; do
    command -v "$command" >/dev/null 2>&1 || fail "tool-missing-${command}"
done

build_settings="$(python3 - "$build_config" <<'PY'
import pathlib
import sys
import tomllib

config = tomllib.loads(pathlib.Path(sys.argv[1]).read_text())
print(
    config["target"],
    ",".join(config["features"]),
    config["log"].lower(),
    config["max_cpu_num"],
    sep="|",
)
PY
)"
IFS='|' read -r build_target build_features build_log build_smp <<EOF
$build_settings
EOF
[ "$build_target" = "aarch64-unknown-none-softfloat" ] \
    || fail "unexpected-build-target-${build_target}"
case "$build_log" in off|error|warn|info|debug|trace) ;; *) fail invalid-build-log ;; esac
case "$build_smp" in ''|*[!0-9]*|0) fail invalid-build-smp ;; esac
export AX_TARGET="$build_target"
export AX_LOG="$build_log"
export SMP="$build_smp"

echo "===${marker}-BEGIN run=${run_id} jobs=${jobs} profile=${profile}==="
echo "===${marker}-SOURCE-META-BEGIN==="
cat .tgoskits-source-meta
echo "===${marker}-SOURCE-META-END==="
rustc --version --verbose
cargo --version
rustup show active-toolchain
echo "cpu_affinity=$(taskset -pc $$ | sed 's/.*: //')"

probe_source=apps/starry/orangepi-5-plus-selfbuild/tests/aarch64_unaligned_access_probe.c
probe_binary="$target_dir/aarch64-unaligned-access-probe"
aarch64-linux-gnu-gcc -std=gnu11 -O2 -Wall -Wextra -Werror \
    "$probe_source" -o "$probe_binary" \
    || fail aarch64-unaligned-access-probe-build
"$probe_binary" || fail aarch64-unaligned-access-probe-run

target_rustflags="[\"-Crelocation-model=pic\",\"-Clink-args=-pie\",\"-Clink-args=--gc-sections\",\"-Clink-args=-znorelro\",\"-Clink-args=-znostart-stop-gc\",\"-Clink-args=-Tlinker.x\",\"-Clink-args=-u _head\",\"-Zthreads=${rustc_threads}\"]"
build_command=(cargo build -p starryos --bin starryos)
build_command+=("--target" "$build_target" "--release")
build_command+=("-Z" "build-std=core,alloc")
build_command+=("--features" "$build_features")
build_command+=("--config" "target.${build_target}.rustflags=${target_rustflags}")
case "$profile" in
    off)
        command=("${build_command[@]}")
        ;;
    stat)
        command=(perf stat -d -d -d -o "$run_dir/perf-stat.txt" -- "${build_command[@]}")
        ;;
    record)
        command=(perf record -F 99 -g -o "$run_dir/perf.data" -- "${build_command[@]}")
        ;;
esac

printf '===%s-COMMAND===' "$marker"
printf ' %q' "${command[@]}"
printf '\n'
start="$(date +%s)"
set +e
"${command[@]}"
rc="$?"
set -e
end="$(date +%s)"
elapsed="$((end - start))"
echo "===${marker}-BUILD-END run=${run_id} rc=${rc} elapsed=${elapsed}==="
if [ "$rc" != "0" ]; then
    fail "build rc=${rc}"
fi

artifact="$target_dir/aarch64-unknown-none-softfloat/release/starryos"
artifact_bin="$artifact.bin"
[ -s "$artifact" ] || fail artifact-elf-missing
KERNEL_ELF="$artifact" AXBUILD_STARRY_KALLSYMS_AUTO_INSTALL=0 \
    sh apps/starry/macos-selfbuild/starry-kallsyms.sh \
    || fail kallsyms-generation
rust-objcopy --strip-all -O binary "$artifact" "$artifact_bin" \
    || fail artifact-bin-generation
[ -s "$artifact_bin" ] || fail artifact-bin-missing
file "$artifact" | grep -q 'ELF 64-bit.*ARM aarch64' || fail artifact-elf-architecture
readelf -h "$artifact" | grep -q 'Machine:.*AArch64' || fail artifact-readelf-architecture

cp "$artifact" "$run_dir/starryos.elf"
cp "$artifact_bin" "$run_dir/starryos.bin"
cp .tgoskits-source-meta "$run_dir/source.meta"
printf '%s\n' "$elapsed" > "$run_dir/elapsed-seconds"
(
    cd "$run_dir"
    sha256sum starryos.elf starryos.bin source.meta > SHA256SUMS
)
if [ "${KEEP_TARGET:-0}" != "1" ]; then
    case "$target_dir" in
        /work/targets/*)
            echo "===${marker}-TARGET-CLEANUP path=${target_dir}==="
            rm -rf "$target_dir"
            ;;
        *) fail unsafe-target-cleanup-path ;;
    esac
fi
printf '%s\n' "$run_id" > /output/latest-run
sync

echo "===${marker}-ARTIFACT elf=$run_dir/starryos.elf bin=$run_dir/starryos.bin==="
echo "===${marker}-PASS run=${run_id} jobs=${jobs} elapsed=${elapsed}==="
