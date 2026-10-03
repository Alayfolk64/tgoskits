#!/bin/sh
# Let Linux PID 1 own the hardware watchdog before long board workloads.
set -eu

app_dir=$(dirname "$0")
config_dir=/etc/systemd/system.conf.d
config=$config_dir/90-tgoskits-board-watchdog.conf
temporary=$config.new
previous=$config.previous
trap 'rm -f "$temporary" "$previous"' EXIT

[ "$(id -u)" -eq 0 ]
[ -c /dev/watchdog0 ]
if "$app_dir/boot_script_is_starry.sh" /boot/boot.scr.tgoskits-backup; then
    echo 'Linux boot backup contains a StarryOS boot command' >&2
    exit 1
else
    [ "$?" -eq 1 ]
fi
cmp /boot/boot.scr /boot/boot.scr.tgoskits-backup

runtime=$(systemctl show --property=RuntimeWatchdogUSec --value)
if [ "$runtime" = 0 ] && fuser -s /dev/watchdog0; then
    echo '/dev/watchdog0 is owned outside systemd' >&2
    exit 1
fi

mkdir -p "$config_dir"
printf '[Manager]\nRuntimeWatchdogSec=20s\nRebootWatchdogSec=20s\n' > "$temporary"
if [ -e "$config" ]; then
    if ! cmp -s "$config" "$temporary"; then
        printf '[Manager]\nRuntimeWatchdogSec=30s\nRebootWatchdogSec=30s\n' > "$previous"
        cmp "$config" "$previous"
        sync "$temporary"
        mv "$temporary" "$config"
        sync "$config_dir"
    fi
else
    sync "$temporary"
    mv "$temporary" "$config"
    sync "$config_dir"
fi

if [ "$runtime" != 20s ]; then
    systemctl daemon-reexec
fi
[ "$(systemctl show --property=RuntimeWatchdogUSec --value)" = 20s ]
fuser -s /dev/watchdog0
echo '===ORANGEPI5PLUS-LINUX-WATCHDOG-ACTIVE requested=20s feed=systemd-half-time==='
