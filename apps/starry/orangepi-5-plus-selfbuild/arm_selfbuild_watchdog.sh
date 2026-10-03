#!/bin/sh
# Arm hardware reset only after the persistent boot selector points to Linux.
set -eu

control=/sys/kernel/debug/selfbuild_watchdog
app_dir=$(dirname "$0")
if "$app_dir/boot_script_is_starry.sh" /boot/boot.scr.tgoskits-backup; then
    echo 'Linux boot backup contains a StarryOS boot command' >&2
    exit 1
else
    [ "$?" -eq 1 ] || exit 1
fi
cmp /boot/boot.scr /boot/boot.scr.tgoskits-backup
sync
[ -e "$control" ]
printf 'arm\n' > "$control"
[ "$(cat "$control")" = armed ]
echo '===STARRY-ORANGEPI5PLUS-SELFBUILD-WATCHDOG-ARMED==='
