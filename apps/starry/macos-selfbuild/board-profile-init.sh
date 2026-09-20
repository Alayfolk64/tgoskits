#!/bin/sh
set -u

marker=STARRY-ORANGEPI5PLUS-SELFBUILD
root=/opt/starry-orangepi5plus-selfbuild/rootfs
restore_boot=/opt/starry-orangepi5plus-selfbuild/restore_linux_boot.sh
guest=/opt/arceos-helloworld-linux/run-starry.sh

fail() {
    echo "===${marker}-FAIL reason=$1==="
    sync
    reboot -f
    exit 1
}

[ -x "$restore_boot" ] || fail linux-boot-restore-script-missing
"$restore_boot" || fail linux-boot-restore
[ -x "$root$guest" ] || fail guest-runner-missing

ln -sfn bash "$root/bin/sh" || fail prepare-chroot-shell
[ "$(readlink "$root/bin/sh")" = bash ] || fail chroot-shell-is-not-bash

mkdir -p "$root/proc"
mkdir -p "$root/dev"
mkdir -p "$root/sys"
mount --bind /proc "$root/proc" || fail mount-proc
mount --bind /dev "$root/dev" || fail mount-dev
mount --bind /sys "$root/sys" || fail mount-sys

echo "===${marker}-CHROOT-BEGIN run=arceos-inflight-page-io-profile-20260920-9 cpu_policy=system-default==="
chroot "$root" "$guest"
command_rc="$?"

ln -sfn dash "$root/bin/sh" || fail restore-chroot-shell
[ "$(readlink "$root/bin/sh")" = dash ] || fail chroot-shell-restore-mismatch
sync
umount "$root/sys" || fail unmount-sys
umount "$root/dev" || fail unmount-dev
umount "$root/proc" || fail unmount-proc
sync

if [ "$command_rc" -ne 0 ]; then
    echo "===${marker}-FAIL reason=guest-runner-exit-$command_rc==="
fi

reboot -f
exit "$command_rc"
