#!/bin/sh
# Leave the feeder lease to expire after restoring the Linux boot selector.
set -eu

app_dir=/opt/starry-orangepi5plus-selfbuild
"$app_dir/restore_linux_boot.sh"
"$app_dir/arm_selfbuild_watchdog.sh"
echo '===STARRY-WATCHDOG-SMOKE-WAITING-FOR-RESET==='
sleep 75
echo '===STARRY-WATCHDOG-SMOKE-FAIL-NO-RESET===' >&2
exit 1
