#!/bin/sh
set -eu

marker=STARRY-MACOS-TG-XTASK-PROFILE
profile_duration=300
profile_frequency=49
host_success_settle_seconds=3
source_tar=/opt/tgoskits-src.tar
source_meta=/opt/tgoskits-src.meta
source_dir=/tmp/tgoskits-tg-xtask-profile-src
target_dir=/tmp/tgoskits-tg-xtask-profile-target
self_profile_dir=/tmp/tgoskits-tg-xtask-self-profile
artifact_dir=/opt/starryos-selfbuild-artifacts/tg-xtask-profile
cargo_bin=/opt/cargo-nightly-sysroot

finish_guest() {
    rc="$1"
    sync 2>/dev/null || true
    if command -v poweroff >/dev/null 2>&1; then
        poweroff -f 2>/dev/null || poweroff 2>/dev/null || true
    fi
    exit "$rc"
}

fail() {
    echo "===${marker}-FAIL reason=$1==="
    finish_guest 1
}

for command in gzip perf sha256sum tar tee timeout; do
    command -v "$command" >/dev/null 2>&1 || fail "tool-missing-${command}"
done
[ -x "$cargo_bin" ] || fail cargo-missing
[ -f "$source_tar" ] || fail source-tar-missing

rm -rf "$source_dir" "$target_dir" "$self_profile_dir" "$artifact_dir"
mkdir -p "$source_dir" "$target_dir" "$artifact_dir"
tar -xf "$source_tar" -C "$source_dir"
if [ -f "$source_dir/.cargo/config.toml" ]; then
    sed -i '/^include[[:space:]]*=/d' "$source_dir/.cargo/config.toml"
fi

export PATH=/opt/rust-nightly/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin
export LD_LIBRARY_PATH=/opt/rust-nightly/lib:/usr/lib
export RUSTC=/opt/rustc-nightly-sysroot
export RUSTDOC=/opt/rustdoc-nightly-sysroot
export HOME=/root
export CARGO_HOME=/root/.cargo
export CARGO_TARGET_DIR="$target_dir"
export CARGO_NET_OFFLINE=true
export RUSTC_BOOTSTRAP=1

cargo_config="$CARGO_HOME/config.toml"
mkdir -p "$CARGO_HOME"
touch "$cargo_config"
if ! grep -q 'starry-macos-selfbuild-host-rustflags' "$cargo_config"; then
    cat >> "$cargo_config" <<'EOF'

# starry-macos-selfbuild-host-rustflags
[host]
rustflags = ["-C", "target-feature=-crt-static"]
EOF
fi

cd "$source_dir"
logical_cpus="$(grep -c '^processor' /proc/cpuinfo 2>/dev/null || true)"
echo "===${marker}-BEGIN duration=${profile_duration} frequency=${profile_frequency} parallelism=system-default==="
echo "logical_cpus=$logical_cpus"
echo "cpu_affinity=$(sed -n 's/^Cpus_allowed_list:[[:space:]]*//p' /proc/self/status)"
"$RUSTC" --version --verbose
"$cargo_bin" --version
echo "===${marker}-COMMAND=== cargo build -p tg-xtask"

start="$(date +%s)"
rc_file=/tmp/tg-xtask-profile.rc
rm -f "$rc_file"
profile_backend=perf-cycles
if perf stat -e cycles -- true > "$artifact_dir/pmu-probe.log" 2>&1; then
    echo "profile_backend=$profile_backend"
else
    profile_backend=rustc-self-profile
    echo "profile_backend=$profile_backend"
    cat "$artifact_dir/pmu-probe.log"
    mkdir -p "$self_profile_dir"
    export RUSTFLAGS="-C target-feature=-crt-static -Zself-profile=$self_profile_dir -Zself-profile-events=default"
fi

set +e
if [ "$profile_backend" = perf-cycles ]; then
    (
        timeout --signal=INT --kill-after=30 "$profile_duration" \
            perf record -F "$profile_frequency" -e cycles \
            -o "$artifact_dir/perf.data" -- \
            "$cargo_bin" build -p tg-xtask
        printf '%s\n' "$?" > "$rc_file"
    ) 2>&1 | tee "$artifact_dir/run.log" &
else
    (
        timeout --signal=INT --kill-after=30 "$profile_duration" \
            "$cargo_bin" build -p tg-xtask
        printf '%s\n' "$?" > "$rc_file"
    ) 2>&1 | tee "$artifact_dir/run.log" &
fi
pipeline_pid="$!"
next_progress=60
: > "$artifact_dir/progress.log"
while kill -0 "$pipeline_pid" 2>/dev/null; do
    now="$(date +%s)"
    progress_elapsed="$((now - start))"
    if [ "$progress_elapsed" -ge "$next_progress" ]; then
        compile_units="$(grep -c '^   Compiling ' "$artifact_dir/run.log" 2>/dev/null || true)"
        distinct_crates="$(sed -n 's/^   Compiling \([^ ]*\).*/\1/p' "$artifact_dir/run.log" \
            | sort -u | wc -l | tr -d ' ')"
        echo "progress elapsed=${progress_elapsed} compile_units=${compile_units} distinct_crates=${distinct_crates}" \
            | tee -a "$artifact_dir/progress.log"
        next_progress="$((next_progress + 60))"
    fi
    sleep 5
done
wait "$pipeline_pid"
pipeline_rc="$?"
set -e
[ "$pipeline_rc" = 0 ] || fail "profile-pipeline-${pipeline_rc}"
[ -s "$rc_file" ] || fail profile-rc-missing
profile_rc="$(sed -n '1p' "$rc_file")"
case "$profile_rc" in
    0|124) ;;
    *) fail "profile-command-${profile_rc}" ;;
esac

end="$(date +%s)"
elapsed="$((end - start))"
compile_units="$(grep -c '^   Compiling ' "$artifact_dir/run.log" 2>/dev/null || true)"
distinct_crates="$(sed -n 's/^   Compiling \([^ ]*\).*/\1/p' "$artifact_dir/run.log" \
    | sort -u | wc -l | tr -d ' ')"
echo "progress elapsed=${elapsed} compile_units=${compile_units} distinct_crates=${distinct_crates} final=true" \
    | tee -a "$artifact_dir/progress.log"

if [ "$profile_backend" = perf-cycles ]; then
    [ -s "$artifact_dir/perf.data" ] || fail perf-data-missing
    perf report --stdio --show-nr-samples --sort comm,dso,symbol \
        -i "$artifact_dir/perf.data" > "$artifact_dir/perf-report.txt" \
        || fail perf-report
    [ -s "$artifact_dir/perf-report.txt" ] || fail perf-report-empty
    profile_files=1
else
    profile_files="$(find "$self_profile_dir" -type f -name '*.mm_profdata' | wc -l | tr -d ' ')"
    [ "$profile_files" -gt 0 ] || fail rustc-self-profile-empty
    find "$self_profile_dir" -type f -name '*.mm_profdata' -exec basename {} \; \
        | sort > "$artifact_dir/rustc-self-profile-files.txt"
    tar -czf "$artifact_dir/rustc-self-profile.tar.gz" -C "$self_profile_dir" . \
        || fail rustc-self-profile-archive
fi

if [ -f "$source_meta" ]; then
    cp "$source_meta" "$artifact_dir/source.meta"
fi
{
    echo profile_backend="$profile_backend"
    echo workload=cargo build -p tg-xtask
    echo accelerator=hvf
    echo virtual_cpus="$logical_cpus"
    echo parallelism=system-default
    echo duration_limit_seconds="$profile_duration"
    echo frequency_hz="$profile_frequency"
    echo elapsed_seconds="$elapsed"
    echo command_rc="$profile_rc"
    echo compile_units="$compile_units"
    echo distinct_crates="$distinct_crates"
    echo profile_files="$profile_files"
} > "$artifact_dir/profile.meta"
(
    cd "$artifact_dir"
    if [ "$profile_backend" = perf-cycles ]; then
        sha256sum perf.data perf-report.txt pmu-probe.log profile.meta progress.log run.log \
            ${source_meta:+source.meta} > SHA256SUMS
    else
        sha256sum pmu-probe.log profile.meta progress.log run.log \
            rustc-self-profile-files.txt rustc-self-profile.tar.gz \
            ${source_meta:+source.meta} > SHA256SUMS
    fi
)
sync

echo "===${marker}-ARTIFACT path=${artifact_dir}==="
echo "===${marker}-PASS elapsed=${elapsed} rc=${profile_rc} parallelism=system-default==="
sleep "$host_success_settle_seconds"
finish_guest 0
