#!/bin/sh
# Restore the Linux selector before arming the bounded benchmark watchdog.
set -eu

app_dir=/opt/starry-orangepi5plus-selfbuild
rootfs=$app_dir/rootfs
run_id=${1:?run id is required}

"$app_dir/restore_linux_boot.sh"
"$app_dir/arm_selfbuild_watchdog.sh"

linux_epoch=$(cat "$rootfs/etc/starry-selfbuild/linux-clock-epoch")
case "$linux_epoch" in
    ''|*[!0-9]*) exit 2 ;;
esac
"$rootfs/usr/local/bin/busybox" date -u -s "@$linux_epoch"
current_epoch=$(date -u +%s)
echo "===ARCEOS-HELLOWORLD-BENCH-CLOCK linux_epoch=$linux_epoch current_epoch=$current_epoch==="

for directory in proc dev sys; do
    [ -d "$rootfs/$directory" ]
    if ! mountpoint -q "$rootfs/$directory"; then
        mount --bind "/$directory" "$rootfs/$directory"
    fi
done

set +e
chroot "$rootfs" /bin/bash /guest-arceos-helloworld-bench.sh "$run_id"
rc=$?
set -e
sync
echo "===ARCEOS-HELLOWORLD-BENCH-STARRY-EXIT run=$run_id rc=$rc==="
reboot -f
exit "$rc"
