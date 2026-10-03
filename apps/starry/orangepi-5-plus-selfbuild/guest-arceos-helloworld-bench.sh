#!/bin/bash
set -euo pipefail

run_id=${1:?run id is required}
case "$run_id" in
    ''|*[!A-Za-z0-9._-]*) echo "invalid run id" >&2; exit 2 ;;
esac

source_dir=$(readlink -f /opt/tgoskits)
target_dir="/work/targets/$run_id"
run_dir="/output/runs/$run_id"
tool=/usr/local/bin/tg-xtask
test -f "$source_dir/Cargo.toml"
test -f "$source_dir/.tgoskits-source-meta"
test -x "$tool"
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
sysroot=$(rustc --print sysroot)
export LD_LIBRARY_PATH="$sysroot/lib"
echo "===ARCEOS-HELLOWORLD-BENCH-BEGIN run=$run_id==="
cat .tgoskits-source-meta
sha256sum Cargo.lock
sha256sum "$tool"
rustc --version
cargo --version
echo "logical_cpus=$(nproc --all) available_cpus=$(nproc) jobs=$CARGO_BUILD_JOBS"
echo "target_dir=$target_dir"
echo 'command=/usr/local/bin/tg-xtask arceos build --package arceos-helloworld --target aarch64-unknown-none-softfloat'

start=$(date +%s)
set +e
timeout --signal=TERM --kill-after=60 2100 "$tool" arceos build \
    --package arceos-helloworld --target aarch64-unknown-none-softfloat
rc=$?
set -e
end=$(date +%s)
elapsed=$((end - start))
echo "===ARCEOS-HELLOWORLD-BENCH-BUILD-END run=$run_id rc=$rc elapsed=$elapsed==="

artifact="$target_dir/aarch64-unknown-linux-musl/release/arceos-helloworld"
if [ "$rc" -eq 0 ] && [ -s "$artifact" ]; then
    sha256sum "$artifact"
    stat -c 'artifact_bytes=%s' "$artifact"
    printf '%s\n' "$elapsed" > "$run_dir/elapsed-seconds"
    cp .tgoskits-source-meta "$run_dir/source.meta"
    sync
    echo "===ARCEOS-HELLOWORLD-BENCH-PASS run=$run_id elapsed=$elapsed==="
    exit 0
fi

if [ "$rc" -eq 0 ]; then
    rc=1
    echo "arceos-helloworld artifact is missing: $artifact"
fi
sync
echo "===ARCEOS-HELLOWORLD-BENCH-FAIL run=$run_id rc=$rc==="
exit "$rc"
