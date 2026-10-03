#!/bin/bash
set -euo pipefail

run_id=${1:?run id is required}
rootfs=/opt/starry-orangepi5plus-selfbuild/rootfs
mounted_proc=0
mounted_dev=0
mounted_sys=0

cleanup() {
    if [ "$mounted_sys" = 1 ]; then umount -R "$rootfs/sys" || true; fi
    if [ "$mounted_dev" = 1 ]; then umount -R "$rootfs/dev" || true; fi
    if [ "$mounted_proc" = 1 ]; then umount "$rootfs/proc" || true; fi
}
trap cleanup EXIT

if ! mountpoint -q "$rootfs/proc"; then
    mount --bind /proc "$rootfs/proc"
    mount --make-rslave "$rootfs/proc"
    mounted_proc=1
fi
if ! mountpoint -q "$rootfs/dev"; then
    mount --rbind /dev "$rootfs/dev"
    mount --make-rslave "$rootfs/dev"
    mounted_dev=1
fi
if ! mountpoint -q "$rootfs/sys"; then
    mount --rbind /sys "$rootfs/sys"
    mount --make-rslave "$rootfs/sys"
    mounted_sys=1
fi

sync
echo 3 > /proc/sys/vm/drop_caches
chroot "$rootfs" /bin/bash /guest-tg-xtask-bench.sh "$run_id"
