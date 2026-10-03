#!/bin/sh
# Run immediately before the one-time StarryOS boot, after Linux NTP sync.
set -eu

rootfs=/opt/starry-orangepi5plus-selfbuild/rootfs
clock_file=$rootfs/etc/starry-selfbuild/linux-clock-epoch
[ "$(id -u)" -eq 0 ]
cmp /boot/boot.scr /boot/boot.scr.tgoskits-backup
[ -x "$rootfs/usr/local/bin/tg-xtask" ]
[ -x "$rootfs/usr/local/bin/busybox" ]
linux_epoch=$(date -u +%s)
case "$linux_epoch" in
    ''|*[!0-9]*) exit 2 ;;
esac
mkdir -p "$(dirname "$clock_file")"
printf '%s\n' "$linux_epoch" > "$clock_file.new"
sync "$clock_file.new"
mv "$clock_file.new" "$clock_file"
sync "$(dirname "$clock_file")"
echo "===ARCEOS-HELLOWORLD-BENCH-LINUX-CLOCK epoch=$linux_epoch boot_id=$(cat /proc/sys/kernel/random/boot_id)==="
