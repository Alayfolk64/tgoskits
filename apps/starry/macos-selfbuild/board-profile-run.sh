#!/bin/bash
set -u

run_id=arceos-inflight-page-io-profile-20260920-9
source_dir=/opt/arceos-helloworld-linux/source
result_dir=/opt/arceos-helloworld-linux/starry-inflight-page-io-profile-result-9
tg_xtask=/opt/arceos-helloworld-linux/bin/tg-xtask
binary=/opt/arceos-helloworld-linux/source/target/aarch64-unknown-linux-musl/release/arceos-helloworld
profile_control=/proc/starry_profile

fail() {
    echo "===STARRY-ORANGEPI5PLUS-SELFBUILD-FAIL reason=$1==="
    exit 1
}

[ -x "$tg_xtask" ] || fail tg-xtask-missing
[ -f "$source_dir/Cargo.toml" ] || fail source-missing
[ ! -e "$source_dir/target" ] || fail workload-target-not-clean
[ -w "$profile_control" ] || fail kernel-profile-control-missing
[ ! -e "$result_dir" ] || fail result-directory-already-exists

mkdir -p "$source_dir/tmp"
mkdir -p "$result_dir"

export HOME=/root
export CARGO_HOME=/root/.cargo
export CARGO_NET_OFFLINE=true
export TMPDIR="$source_dir/tmp"
export PATH=/root/.cargo/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin
export LD_LIBRARY_PATH=/root/.rustup/toolchains/nightly-2026-07-15-aarch64-unknown-linux-gnu/lib
unset CARGO_TARGET_DIR
unset CARGO_BUILD_JOBS
unset RUSTFLAGS

cd "$source_dir" || fail source-directory-unavailable

uname -a > "$result_dir/environment.txt"
rustc --version --verbose >> "$result_dir/environment.txt"
cargo --version >> "$result_dir/environment.txt"
rustup show active-toolchain >> "$result_dir/environment.txt"
nproc >> "$result_dir/environment.txt"

start_seconds="$(date +%s)"
start_nanoseconds="$(date +%s%N)"
echo "STARRY-ARCEOS-HELLOWORLD-BEGIN epoch=$start_seconds"
echo "Command: $tg_xtask arceos build --package arceos-helloworld --arch aarch64"

printf 'reset\n' > "$profile_control" || fail kernel-profile-reset
printf 'phase=prebuild\n' > "$profile_control" || fail kernel-profile-start
echo "Kernel profiling: direct /proc/starry_profile sampling enabled"

/usr/bin/time -v -o "$result_dir/time.txt" "$tg_xtask" arceos build --package arceos-helloworld --arch aarch64
command_rc="$?"

printf 'stop\n' > "$profile_control" || fail kernel-profile-stop
cat "$profile_control" > "$result_dir/kernel-profile.raw" || fail kernel-profile-read
[ -s "$result_dir/kernel-profile.raw" ] || fail kernel-profile-empty

end_seconds="$(date +%s)"
end_nanoseconds="$(date +%s%N)"
elapsed_seconds="$((end_seconds - start_seconds))"
elapsed_nanoseconds="$((end_nanoseconds - start_nanoseconds))"

echo "command_rc=$command_rc" > "$result_dir/result.meta"
echo "elapsed_seconds=$elapsed_seconds" >> "$result_dir/result.meta"
echo "elapsed_nanoseconds=$elapsed_nanoseconds" >> "$result_dir/result.meta"
echo "logical_cpus=$(nproc)" >> "$result_dir/result.meta"
echo "source_archive_sha256=7de46545454e3f5562d78a1a065bf2ed1a736cf807f4cbf05b30d98ab2725147" >> "$result_dir/result.meta"
echo "tg_xtask_sha256=e6823ab3b6a6d944266fc31d5e54814f463466f88949849ba139b6c575f7e6f1" >> "$result_dir/result.meta"
echo "kernel_variant=scheduler+ext4-owned-io+vma-gap+cqhci+global-page-cache+bounded-extent-reap+bounded-dma-pool+private-file-cache-cow+mmap-readahead+inflight-page-io+direct-profile-v2" >> "$result_dir/result.meta"
echo "kernel_bin_sha256=0dd3b1a1f0bbf200b0184508905e3bd832513bacae6974ecff2d60656bdfe63a" >> "$result_dir/result.meta"
echo "profile_backend=starry-kernel-fp-v2" >> "$result_dir/result.meta"

if [ "$command_rc" -ne 0 ]; then
    echo "===STARRY-ORANGEPI5PLUS-SELFBUILD-FAIL reason=workload-exit-$command_rc==="
    echo "Cause: tg-xtask returned a non-zero exit status; the original compiler output is printed above."
    exit "$command_rc"
fi

[ -s "$binary" ] || fail arceos-helloworld-missing
sha256sum "$binary" > "$result_dir/arceos-helloworld.sha256"
sync
echo "===STARRY-ORANGEPI5PLUS-SELFBUILD-PASS run=$run_id parallelism=system-default elapsed=$elapsed_seconds==="
