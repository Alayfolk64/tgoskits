#!/bin/sh
# Install the tg-xtask binary built once in the board's Linux rootfs.
set -eu

rootfs=/opt/starry-orangepi5plus-selfbuild/rootfs
guest_tool=${1:?guest path to Linux-built tg-xtask is required}
case "$guest_tool" in
    /*) ;;
    *) exit 2 ;;
esac

[ "$(id -u)" -eq 0 ]
cmp /boot/boot.scr /boot/boot.scr.tgoskits-backup
[ "$(systemctl show --property=RuntimeWatchdogUSec --value)" = 20s ]
fuser -s /dev/watchdog0

source=$rootfs$guest_tool
destination=$rootfs/usr/local/bin/tg-xtask
[ -x "$source" ]
mkdir -p "$rootfs/usr/local/bin"
install -m 0755 "$source" "$destination.new"
sync "$destination.new"
mv "$destination.new" "$destination"
sync "$rootfs/usr/local/bin"
cmp "$source" "$destination"
sha256sum "$destination"
echo '===ARCEOS-HELLOWORLD-BENCH-LINUX-TOOL-READY==='
