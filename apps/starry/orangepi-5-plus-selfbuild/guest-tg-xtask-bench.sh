#!/bin/bash
set -euo pipefail

run_id=${1:?run id is required}
case "$run_id" in
    ''|*[!A-Za-z0-9._-]*) echo "invalid run id" >&2; exit 2 ;;
esac

source_dir=$(readlink -f /opt/tgoskits)
target_dir="/work/targets/$run_id"
run_dir="/output/runs/$run_id"
test -f "$source_dir/Cargo.toml"
test -f "$source_dir/.tgoskits-source-meta"
test ! -e "$target_dir"
test ! -e "$run_dir"
mkdir -p "$target_dir" "$run_dir"

exec > >(tee "$run_dir/run.log") 2>&1
export CARGO_HOME=/root/.cargo
export PATH=/root/.cargo/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin
export CARGO_TARGET_DIR="$target_dir"
export CARGO_NET_OFFLINE=true
export CARGO_BUILD_JOBS=8
export CARGO_INCREMENTAL=0
unset RUSTFLAGS CARGO_ENCODED_RUSTFLAGS

cd "$source_dir"
echo "===TG-XTASK-BENCH-BEGIN run=$run_id==="
cat .tgoskits-source-meta
sha256sum Cargo.lock
rustc --version
cargo --version
echo "logical_cpus=$(nproc --all) available_cpus=$(nproc) jobs=$CARGO_BUILD_JOBS"
echo "target_dir=$target_dir"
echo 'command=cargo build -p tg-xtask'

start=$(date +%s)
set +e
timeout --signal=TERM --kill-after=60 2100 cargo build -p tg-xtask
rc=$?
set -e
end=$(date +%s)
elapsed=$((end - start))
echo "===TG-XTASK-BENCH-BUILD-END run=$run_id rc=$rc elapsed=$elapsed==="

if [ "$rc" -eq 0 ] && [ -x "$target_dir/debug/tg-xtask" ]; then
    sha256sum "$target_dir/debug/tg-xtask"
    stat -c 'artifact_bytes=%s' "$target_dir/debug/tg-xtask"
    printf '%s\n' "$elapsed" > "$run_dir/elapsed-seconds"
    cp .tgoskits-source-meta "$run_dir/source.meta"
    sync
    echo "===TG-XTASK-BENCH-PASS run=$run_id elapsed=$elapsed==="
    exit 0
fi

if [ "$rc" -eq 0 ]; then
    rc=1
    echo "tg-xtask artifact is missing"
fi
sync
echo "===TG-XTASK-BENCH-FAIL run=$run_id rc=$rc==="
exit "$rc"
