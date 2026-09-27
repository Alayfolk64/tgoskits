#!/bin/sh
# Called by the guest runner only after BPF's ready marker has been observed.
set -eu

artifact_dir=/opt/linux-profile-artifacts
source_dir=/opt/tgoskits-profile/source
tg_xtask_bin=/opt/tgoskits-profile/bin/tg-xtask

report_progress() {
    counts="$(awk '/^   Compiling / { units++; crates[$2] = 1 }
        END { for (crate in crates) distinct++;
              printf "compile_units=%d distinct_crates=%d", units, distinct }' "$artifact_dir/run.log")"
    echo "progress elapsed=$1 $counts"
    echo "progress elapsed=$1 $counts" >> "$artifact_dir/progress.log"
}

cd "$source_dir"
cat /proc/stat > "$artifact_dir/cpu-before.stat"
cat /proc/diskstats > "$artifact_dir/disk-before.stat"
start="$(date +%s)"
echo "LINUX_WORKLOAD_BEGIN epoch=$start"
(
    set +e
    "$tg_xtask_bin" arceos build --package arceos-helloworld --arch aarch64
    printf '%s\n' "$?" > "$artifact_dir/workload.rc"
) > "$artifact_dir/run.log" 2>&1 &
workload_pid="$!"
next_progress=60
while [ ! -f "$artifact_dir/workload.rc" ]; do
    now="$(date +%s)"
    elapsed="$((now - start))"
    if [ "$elapsed" -ge "$next_progress" ]; then
        report_progress "$elapsed"
        next_progress="$((next_progress + 60))"
    fi
    sleep 5
done
wait "$workload_pid"
end="$(date +%s)"
cat /proc/stat > "$artifact_dir/cpu-after.stat"
cat /proc/diskstats > "$artifact_dir/disk-after.stat"
elapsed="$((end - start))"
report_progress "$elapsed"
rc="$(sed -n '1p' "$artifact_dir/workload.rc")"
echo "elapsed_seconds=$elapsed" >> "$artifact_dir/profile.meta"
echo "command_rc=$rc" >> "$artifact_dir/profile.meta"
echo "LINUX_WORKLOAD_END elapsed=$elapsed rc=$rc"
case "$rc" in
    0) echo build_completed=true >> "$artifact_dir/profile.meta" ;;
    *)
        echo build_completed=false >> "$artifact_dir/profile.meta"
        echo "workload failed: command exit=$rc; original output follows"
        cat "$artifact_dir/run.log"
        exit "$rc"
        ;;
esac
