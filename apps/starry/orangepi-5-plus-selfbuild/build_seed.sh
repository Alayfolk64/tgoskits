#!/bin/bash
set -euo pipefail

app_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "$app_dir/../../.." && pwd)"
build_config="$app_dir/build-aarch64-unknown-none-softfloat.toml"
rustc_threads="${RUSTC_THREADS:-2}"

usage() {
    cat <<'USAGE'
Usage:
  apps/starry/orangepi-5-plus-selfbuild/build_seed.sh [--config BUILD_CONFIG]

Build the OrangePi 5 Plus StarryOS seed kernel directly with Cargo, generate
kallsyms, and emit the raw AArch64 boot image. This command does not use xtask.
USAGE
}

while [ "$#" -gt 0 ]; do
    case "$1" in
        --config) build_config="${2:-}"; shift 2 ;;
        -h|--help) usage; exit 0 ;;
        *) echo "unknown argument: $1" >&2; usage >&2; exit 2 ;;
    esac
done

case "$build_config" in
    /*) ;;
    *) build_config="$repo_root/$build_config" ;;
esac
[ -f "$build_config" ] || { echo "build config is missing: $build_config" >&2; exit 2; }
case "$rustc_threads" in
    ''|*[!0-9]*|0) echo "invalid RUSTC_THREADS: $rustc_threads" >&2; exit 2 ;;
esac

for command in cargo file gen_ksym python3 readelf rust-nm rust-objcopy rust-objdump; do
    command -v "$command" >/dev/null 2>&1 || {
        echo "$command not found" >&2
        exit 1
    }
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
[ "$build_target" = "aarch64-unknown-none-softfloat" ] || {
    echo "unexpected build target: $build_target" >&2
    exit 2
}
case "$build_log" in off|error|warn|info|debug|trace) ;; *) echo "invalid build log" >&2; exit 2 ;; esac
case "$build_smp" in ''|*[!0-9]*|0) echo "invalid max_cpu_num" >&2; exit 2 ;; esac

export CARGO_INCREMENTAL=0
export CARGO_TARGET_DIR="$repo_root/target"
export AXBUILD_STARRY_KALLSYMS_AUTO_INSTALL=0
export AX_ARCH=aarch64
export AX_TARGET="$build_target"
export AX_MODE=release
export AX_LOG="$build_log"
export SMP="$build_smp"
export CC_aarch64_unknown_none_softfloat="${CC_aarch64_unknown_none_softfloat:-aarch64-linux-gnu-gcc}"
export AR_aarch64_unknown_none_softfloat="${AR_aarch64_unknown_none_softfloat:-aarch64-linux-gnu-ar}"
export CC_AARCH64_UNKNOWN_NONE_SOFTFLOAT="$CC_aarch64_unknown_none_softfloat"
export AR_AARCH64_UNKNOWN_NONE_SOFTFLOAT="$AR_aarch64_unknown_none_softfloat"

target_rustflags="[\"-Crelocation-model=pic\",\"-Clink-args=-pie\",\"-Clink-args=--gc-sections\",\"-Clink-args=-znorelro\",\"-Clink-args=-znostart-stop-gc\",\"-Clink-args=-Tlinker.x\",\"-Clink-args=-u _head\",\"-Zthreads=${rustc_threads}\"]"
build_command=(cargo build -p starryos --bin starryos)
build_command+=("--target" "$build_target" "--release")
build_command+=("-Z" "build-std=core,alloc")
build_command+=("--features" "$build_features")
build_command+=("--config" "target.${build_target}.rustflags=${target_rustflags}")

printf 'seed_build_command='
printf ' %q' "${build_command[@]}"
printf '\n'
(
    cd "$repo_root"
    "${build_command[@]}"
)

artifact="$repo_root/target/$build_target/release/starryos"
artifact_bin="$artifact.bin"
[ -s "$artifact" ] || { echo "seed kernel ELF is missing: $artifact" >&2; exit 1; }
(
    cd "$repo_root"
    KERNEL_ELF="$artifact" sh apps/starry/macos-selfbuild/starry-kallsyms.sh
)
rust-objcopy --strip-all -O binary "$artifact" "$artifact_bin"
[ -s "$artifact_bin" ] || { echo "seed kernel raw image is missing: $artifact_bin" >&2; exit 1; }
file "$artifact" | grep -q 'ELF 64-bit.*ARM aarch64'
readelf -h "$artifact" | grep -q 'Machine:.*AArch64'

has_sctlr_alignment_clear() {
    local symbol=$1
    rust-objdump -dC "$artifact" | awk -v symbol="$symbol" '
        index($0, "<" symbol ">:") {
            in_symbol = 1
            next
        }
        in_symbol && /^[[:xdigit:]]+ <.*>:/ {
            in_symbol = 0
        }
        in_symbol && index($0, "#0xfffffffffffffffd") {
            clear_seen = 1
        }
        in_symbol && clear_seen \
            && tolower($0) ~ /msr[[:space:]]+sctlr_el1/ {
            committed = 1
        }
        END {
            exit committed ? 0 : 1
        }
    '
}

has_sctlr_alignment_clear 'someboot::arch::paging::enable_mmu' || {
    echo "seed kernel primary MMU path does not clear SCTLR_EL1.A" >&2
    exit 1
}
has_sctlr_alignment_clear 'someboot::arch::paging::init_mmu_secondary' || {
    echo "seed kernel secondary MMU path does not clear SCTLR_EL1.A" >&2
    exit 1
}

echo "seed_kernel_elf=$artifact"
echo "seed_kernel_bin=$artifact_bin"
echo "seed_kernel_sctlr_alignment_clear=primary,secondary"
echo "seed_kernel_elf_sha256=$(sha256sum "$artifact" | awk '{ print $1 }')"
echo "seed_kernel_bin_sha256=$(sha256sum "$artifact_bin" | awk '{ print $1 }')"
