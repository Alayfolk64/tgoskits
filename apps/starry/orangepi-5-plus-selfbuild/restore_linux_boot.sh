#!/bin/sh
set -eu

marker=STARRY-ORANGEPI5PLUS-SELFBUILD
linux_boot=/boot/boot.scr.tgoskits-backup
starry_boot=/boot/boot-starryos-emmc.scr
active_boot=/boot/boot.scr

fail() {
    echo "===${marker}-LINUX-BOOT-RESTORE-FAIL reason=$1==="
    exit 1
}

[ -s "$linux_boot" ] || fail linux-boot-backup-missing
[ -s "$starry_boot" ] || fail starry-boot-script-missing
if cmp -s "$active_boot" "$linux_boot"; then
    echo "===${marker}-LINUX-BOOT-RESTORED state=already-linux==="
    exit 0
fi
cmp -s "$active_boot" "$starry_boot" || fail active-boot-script-unknown
cp "$linux_boot" "$active_boot.new" || fail linux-boot-copy
cmp -s "$active_boot.new" "$linux_boot" || fail linux-boot-copy-verify
sync || fail linux-boot-copy-sync
mv -f "$active_boot.new" "$active_boot" || fail linux-boot-activate
sync || fail linux-boot-activate-sync
cmp -s "$active_boot" "$linux_boot" || fail linux-boot-activate-verify
echo "===${marker}-LINUX-BOOT-RESTORED state=switched==="
