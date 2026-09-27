#!/bin/sh
set -eu

marker=STARRY-ARCEOS-HELLOWORLD-PROFILE
profile_frequency=10
host_success_settle_seconds=3
source_tar=/opt/tgoskits-src.tar
source_meta=/opt/tgoskits-src.meta
profile_root=/opt/tgoskits-profile
source_dir="$profile_root/source"
source_stamp=/opt/tgoskits-profile/source.tar.sha256
tg_xtask_target="$profile_root/tg-xtask-target"
tg_xtask_bin=/opt/tgoskits-profile/bin/tg-xtask
tg_xtask_stamp=/opt/tgoskits-profile/tg-xtask.source.sha256
artifact_dir=/opt/starryos-selfbuild-artifacts/arceos-helloworld-profile
cargo_bin=/opt/cargo-nightly-sysroot
profile_control=/proc/starry_profile
workload="$tg_xtask_bin arceos build --package arceos-helloworld --arch aarch64"

finish_guest() {
    rc="$1"
    sync
    poweroff -f
    exit "$rc"
}

fail() {
    sync
    echo "===${marker}-FAIL reason=$1==="
    if [ -f "$artifact_dir/run.log" ]; then
        cat "$artifact_dir/run.log"
    fi
    finish_guest 1
}

report_progress() {
    counts="$(awk '/^   Compiling / { units++; crates[$2] = 1 }
        END { for (crate in crates) distinct++;
              printf "compile_units=%d distinct_crates=%d", units, distinct }' "$artifact_dir/run.log")"
    progress="progress elapsed=$1 $counts"
    echo "$progress"
    echo "$progress" >> "$artifact_dir/progress.log"
}

for command in awk cat chmod cp date find grep sed sha256sum sort tar poweroff; do
    command -v "$command" || fail "tool-missing-${command}"
done
[ -x "$cargo_bin" ] || fail cargo-missing
[ -f "$source_tar" ] || fail source-tar-missing
[ -w "$profile_control" ] || fail kernel-profile-missing

rm -rf "$artifact_dir"
mkdir -p "$artifact_dir" "$profile_root/bin" "$tg_xtask_target"
archive_checksum="$(sha256sum "$source_tar")"
source_archive_hash="${archive_checksum%% *}"
source_reused=false
if [ -f "$source_dir/Cargo.toml" ] && [ -f "$source_stamp" ] \
    && [ "$(sed -n '1p' "$source_stamp")" = "$source_archive_hash" ]; then
    source_reused=true
else
    rm -rf "$source_dir"
    mkdir -p "$source_dir"
    tar -xf "$source_tar" -C "$source_dir"
    printf '%s\n' "$source_archive_hash" > "$source_stamp"
    sync
fi
export PATH=/opt/rust-nightly/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin
export LD_LIBRARY_PATH=/opt/rust-nightly/lib:/usr/lib
export RUSTC=/opt/rustc-nightly-sysroot
export RUSTDOC=/opt/rustdoc-nightly-sysroot
export CARGO_HOME=/root/.cargo
export CARGO_NET_OFFLINE=true
export RUSTC_BOOTSTRAP=1
# Build scripts load libclang on the guest; enable the [host] flags below.
export CARGO_UNSTABLE_HOST_CONFIG=true
export CARGO_UNSTABLE_TARGET_APPLIES_TO_HOST=true

cargo_config="$CARGO_HOME/config.toml"
cargo_config_tmp="$CARGO_HOME/.config.toml.profile.$$"
mkdir -p "$CARGO_HOME"
cat > "$cargo_config_tmp" <<'EOF'
target-applies-to-host = false

[net]
git-fetch-with-cli = true
offline = true

# starry-macos-selfbuild-host-rustflags
[host]
rustflags = ["-C", "target-feature=-crt-static"]
EOF
mv "$cargo_config_tmp" "$cargo_config"

cd "$source_dir"
mkdir -p "$source_dir/tmp"
export TMPDIR="$source_dir/tmp"
fingerprint_manifest="$profile_root/fingerprint.manifest"
fingerprint_paths="$profile_root/fingerprint.paths"
{
    for path in Cargo.toml Cargo.lock rust-toolchain.toml; do
        [ ! -f "$path" ] || sha256sum "$path"
    done
    for tree in xtask scripts/axbuild; do
        find "$tree" -type f -print > "$fingerprint_paths"
        sort -o "$fingerprint_paths" "$fingerprint_paths"
        while IFS= read -r path; do
            sha256sum "$path"
        done < "$fingerprint_paths"
    done
} > "$fingerprint_manifest"
fingerprint_checksum="$(sha256sum "$fingerprint_manifest")"
source_fingerprint="${fingerprint_checksum%% *}"

tg_xtask_reused=false
if [ -x "$tg_xtask_bin" ] && [ -f "$tg_xtask_stamp" ] \
    && [ "$(sed -n '1p' "$tg_xtask_stamp")" = "$source_fingerprint" ]; then
    tg_xtask_reused=true
else
    echo "===${marker}-TG-XTASK-BUILD-BEGIN fingerprint=${source_fingerprint}==="
    export CARGO_TARGET_DIR="$tg_xtask_target"
    "$cargo_bin" build --locked -p tg-xtask
    cp "$tg_xtask_target/debug/tg-xtask" "$tg_xtask_bin"
    chmod 0755 "$tg_xtask_bin"
    printf '%s\n' "$source_fingerprint" > "$tg_xtask_stamp"
    unset CARGO_TARGET_DIR
    sync
    echo "===${marker}-TG-XTASK-BUILD-END==="
fi

[ -x "$tg_xtask_bin" ] || fail tg-xtask-missing
rm -rf "$source_dir/target" "$source_dir/tmp/axbuild"

logical_cpus="$(grep -c '^processor' /proc/cpuinfo)"
echo "===${marker}-BEGIN measurement=full-build frequency=${profile_frequency} parallelism=system-default==="
echo "logical_cpus=$logical_cpus"
echo "cpu_affinity=$(sed -n 's/^Cpus_allowed_list:[[:space:]]*//p' /proc/self/status)"
echo "tg_xtask_reused=$tg_xtask_reused"
echo "source_reused=$source_reused"
echo "source_archive_sha256=$source_archive_hash"
echo "source_fingerprint=$source_fingerprint"
"$RUSTC" --version --verbose
"$cargo_bin" --version
echo "===${marker}-COMMAND=== $workload"

profile_backend=starry-kernel-fp
printf 'reset\n' > "$profile_control"
printf 'phase=prebuild\n' > "$profile_control"
profile_start_status="$(sed -n '1p' "$profile_control")"
echo "$profile_start_status"
case "$profile_start_status" in
    *'enabled=true phase=1'*) ;;
    *) fail kernel-profile-start ;;
esac
echo "profile_backend=$profile_backend"

start="$(date +%s)"
rc_file="$artifact_dir/workload.rc"
set +e
(
    "$tg_xtask_bin" arceos build --package arceos-helloworld --arch aarch64
    printf '%s\n' "$?" > "$rc_file"
) > "$artifact_dir/run.log" 2>&1 &
workload_pid="$!"
next_progress=60
: > "$artifact_dir/progress.log"
while [ ! -f "$rc_file" ]; do
    now="$(date +%s)"
    progress_elapsed="$((now - start))"
    if [ "$progress_elapsed" -ge "$next_progress" ]; then
        report_progress "$progress_elapsed"
        next_progress="$((next_progress + 60))"
    fi
    sleep 5
done
wait "$workload_pid"
workload_wrapper_rc="$?"
set -e
end="$(date +%s)"
elapsed="$((end - start))"

printf 'stop\n' > "$profile_control"
cat "$profile_control" > "$artifact_dir/kernel-profile.raw" \
    || fail kernel-profile-read
[ -s "$artifact_dir/kernel-profile.raw" ] || fail kernel-profile-empty
grep -q '^STARRY_PROFILE_V1 ' "$artifact_dir/kernel-profile.raw" \
    || fail kernel-profile-header
[ "$workload_wrapper_rc" = 0 ] || fail "profile-wrapper-${workload_wrapper_rc}"
[ -s "$rc_file" ] || fail profile-rc-missing
profile_rc="$(sed -n '1p' "$rc_file")"
case "$profile_rc" in
    0) build_completed=true ;;
    *) build_completed=false ;;
esac

report_progress "$elapsed"
compile_units="$(awk '/^   Compiling / { count++ } END { print count+0 }' "$artifact_dir/run.log")"
distinct_crates="$(awk '/^   Compiling / { crates[$2] = 1 }
    END { for (crate in crates) count++; print count+0 }' "$artifact_dir/run.log")"
cat "$artifact_dir/run.log"

profile_records="$(grep -Ec '^STARRY_(CPU|WAIT) ' "$artifact_dir/kernel-profile.raw")"
[ "$profile_records" -gt 0 ] || fail kernel-profile-no-records

arceos_binary="$source_dir/target/aarch64-unknown-linux-musl/release/arceos-helloworld"
if [ "$build_completed" = true ]; then
    [ -s "$arceos_binary" ] || fail arceos-helloworld-missing
    sha256sum "$arceos_binary" > "$artifact_dir/arceos-helloworld.sha256"
fi
if [ -f "$source_meta" ]; then
    cp "$source_meta" "$artifact_dir/source.meta"
fi
{
    echo profile_backend="$profile_backend"
    echo workload="$workload"
    echo accelerator=tcg
    echo virtual_cpus="$logical_cpus"
    echo parallelism=system-default
    echo measurement=full-build
    echo timeout_seconds=0
    echo frequency_hz="$profile_frequency"
    echo elapsed_seconds="$elapsed"
    echo command_rc="$profile_rc"
    echo build_completed="$build_completed"
    echo tg_xtask_reused="$tg_xtask_reused"
    echo cargo_host_config="$CARGO_UNSTABLE_HOST_CONFIG"
    echo source_reused="$source_reused"
    echo source_archive_sha256="$source_archive_hash"
    echo tg_xtask_source_fingerprint="$source_fingerprint"
    echo compile_units="$compile_units"
    echo distinct_crates="$distinct_crates"
    echo profile_records="$profile_records"
} > "$artifact_dir/profile.meta"
(
    cd "$artifact_dir"
    sha256sum kernel-profile.raw profile.meta progress.log run.log \
        ${source_meta:+source.meta} > SHA256SUMS
    if [ "$build_completed" = true ]; then
        sha256sum arceos-helloworld.sha256 >> SHA256SUMS
    fi
)
sync

echo "===${marker}-ARTIFACT path=${artifact_dir}==="
[ "$build_completed" = true ] || fail "profile-command-${profile_rc}"
echo "===${marker}-WINDOW-PASS elapsed=${elapsed} rc=${profile_rc} build_completed=${build_completed} tg_xtask_reused=${tg_xtask_reused}==="
sleep "$host_success_settle_seconds"
finish_guest 0
