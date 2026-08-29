#!/bin/sh

marker=STARRY-ORANGEPI5PLUS-SELFBUILD
root=/opt/starry-orangepi5plus-selfbuild/rootfs
run_config="$root/etc/starry-selfbuild/run.conf"
guest=/opt/tgoskits/apps/starry/orangepi-5-plus-selfbuild/guest-selfbuild.sh
guest_timeout=9600
restore_boot=/opt/starry-orangepi5plus-selfbuild/restore_linux_boot.sh

fail() {
    echo "===${marker}-FAIL reason=$1==="
    sync 2>/dev/null || true
    reboot -f 2>/dev/null || true
    exit 1
}

# Restore the persistent Linux selector before mounts, clock changes, or the
# compiler workload. A later watchdog reset must return to Linux, not re-enter
# this StarryOS image.
[ -x "$restore_boot" ] || fail linux-boot-restore-script-missing
"$restore_boot" || fail linux-boot-restore

command -v chroot >/dev/null 2>&1 || fail chroot-missing
command -v mount >/dev/null 2>&1 || fail mount-missing
[ -x "$root/bin/bash" ] || fail glibc-rootfs-missing
[ -f "$root$guest" ] || fail guest-runner-missing
[ -f "$run_config" ] || fail run-config-missing

run_id="$(sed -n 's/^run_id=//p' "$run_config")"
profile="$(sed -n 's/^profile=//p' "$run_config")"
build_epoch="$(sed -n 's/^build_epoch=//p' "$run_config")"
case "$run_id" in
    ''|*[!A-Za-z0-9._-]*) fail invalid-run-id ;;
esac
case "$profile" in
    off|stat|record) ;;
    *) fail invalid-profile ;;
esac
case "$build_epoch" in
    ''|*[!0-9]*) fail invalid-build-epoch ;;
esac
[ "$build_epoch" -ge 1609459200 ] || fail invalid-build-epoch
clock_helper=/opt/starry-orangepi5plus-selfbuild/set_guest_clock.sh
[ -x "$clock_helper" ] || fail clock-helper-missing
"$clock_helper" "$marker" "$build_epoch" || fail clock-helper

mkdir -p "$root/proc" "$root/dev" "$root/sys"
mount --bind /proc "$root/proc" 2>/dev/null || true
mount --bind /dev "$root/dev" 2>/dev/null || true
mount --bind /sys "$root/sys" 2>/dev/null || true

echo "===${marker}-CHROOT-BEGIN run=${run_id} profile=${profile} cpu_list=4-7==="
chroot "$root" /usr/bin/timeout --signal=TERM --kill-after=60 "$guest_timeout" \
    /usr/bin/env \
    STARRY_SELFBUILD_MARKER="$marker" \
    STARRY_SELFBUILD_RUN_ID="$run_id" \
    STARRY_SELFBUILD_PROFILE="$profile" \
    /usr/bin/taskset -c 4-7 /bin/bash "$guest"
rc="$?"

if [ "$rc" = "124" ] || [ "$rc" = "137" ]; then
    echo "===${marker}-TIMEOUT==="
elif [ "$rc" != "0" ]; then
    echo "===${marker}-FAIL reason=guest-runner rc=${rc}==="
fi

sync 2>/dev/null || true
systemctl --force --force reboot 2>/dev/null || true
reboot -f 2>/dev/null || true
exit "$rc"
