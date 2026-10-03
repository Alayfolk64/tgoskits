#!/bin/sh
# Restore the Linux selector before arming the bounded benchmark watchdog.
set -eu

app_dir=/opt/starry-orangepi5plus-selfbuild
rootfs=$app_dir/rootfs
run_id=${1:?run id is required}

"$app_dir/restore_linux_boot.sh"
"$app_dir/arm_selfbuild_watchdog.sh"

build_epoch=$(sed -n 's/^build_epoch=//p' "$rootfs/etc/starry-selfbuild/run.conf")
case "$build_epoch" in
    ''|*[!0-9]*) exit 2 ;;
esac
"$app_dir/set_guest_clock.sh" TG-XTASK-BENCH "$build_epoch"

for directory in proc dev sys; do
    [ -d "$rootfs/$directory" ]
    if ! mountpoint -q "$rootfs/$directory"; then
        mount --bind "/$directory" "$rootfs/$directory"
    fi
done

set +e
chroot "$rootfs" /bin/bash /guest-tg-xtask-bench.sh "$run_id"
rc=$?
set -e
sync
echo "===TG-XTASK-BENCH-STARRY-EXIT run=$run_id rc=$rc==="
reboot -f
exit "$rc"
