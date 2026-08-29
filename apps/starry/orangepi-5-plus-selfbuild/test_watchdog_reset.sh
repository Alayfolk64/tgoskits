#!/bin/bash
set -euo pipefail

app_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "$app_dir/../../.." && pwd)"
linux_user="${BOARD_LINUX_USER:-orangepi}"
ssh_port="${BOARD_SSH_PORT:-22}"
host=""
serial="${BOARD_SERIAL:-/dev/serial/by-id/usb-1a86_USB_Serial-if00-port0}"
state_dir="$repo_root/target/starry-orangepi5plus-selfbuild/watchdog-reset/$(date -u +%Y%m%dT%H%M%SZ)"
state_file="$state_dir/pre-reset-boot-id"
serial_log="$state_dir/serial.log"
ready_file="$state_dir/serial.ready"

usage() {
    cat <<'USAGE'
Usage:
  test_watchdog_reset.sh --host BOARD_IP

Stages the missed-feed image, opens the direct UART, boots from Linux without
interrupting U-Boot, restores the Linux selector at the Starry shell, and then
verifies the hardware reset through both serial evidence and a changed boot ID.
USAGE
}

while [ "$#" -gt 0 ]; do
    case "$1" in
        --host) host="${2:-}"; shift 2 ;;
        -h|--help) usage; exit 0 ;;
        *) echo "unknown argument: $1" >&2; usage >&2; exit 2 ;;
    esac
done
[[ "$host" =~ ^[A-Za-z0-9._-]+$ ]] || { echo "invalid --host" >&2; exit 2; }
[[ "$linux_user" =~ ^[A-Za-z_][A-Za-z0-9_-]*$ ]] \
    || { echo "invalid BOARD_LINUX_USER" >&2; exit 2; }
if ! [[ "$ssh_port" =~ ^[0-9]+$ ]] \
    || ((10#$ssh_port < 1 || 10#$ssh_port > 65535)); then
    echo "invalid BOARD_SSH_PORT" >&2
    exit 2
fi

remote="${linux_user}@${host}"
ssh_args=(-p "$ssh_port")

[ -c "$serial" ] || { echo "serial device is unavailable: $serial" >&2; exit 1; }
python3 -c 'import serial' || { echo "Python package pyserial is required" >&2; exit 1; }
mkdir -p "$state_dir"
previous_boot_id=$(ssh "${ssh_args[@]}" "$remote" cat /proc/sys/kernel/random/boot_id)
printf '%s\n' "$previous_boot_id" > "$state_file"
grep -Eq '^[0-9a-f-]{36}$' "$state_file" || { echo "invalid Linux boot ID" >&2; exit 1; }

"$app_dir/stage_starry_boot.sh" --host "$host" \
    --config apps/starry/orangepi-5-plus-selfbuild/watchdog-reset-test.toml

python3 "$app_dir/serial_watchdog_reset.py" \
    --serial "$serial" --log "$serial_log" --ready-file "$ready_file" &
serial_pid=$!
cleanup_serial() {
    if kill -0 "$serial_pid" 2>/dev/null; then
        kill "$serial_pid" 2>/dev/null || true
        wait "$serial_pid" 2>/dev/null || true
    fi
}
trap cleanup_serial EXIT
for _ in $(seq 1 50); do
    [ -s "$ready_file" ] && break
    kill -0 "$serial_pid" 2>/dev/null || { echo "serial monitor exited early" >&2; exit 1; }
    sleep 0.1
done
[ -s "$ready_file" ] || { echo "serial monitor did not become ready" >&2; exit 1; }
"$app_dir/boot_starry_once.sh" --host "$host"
wait "$serial_pid"
trap - EXIT

linux_ready=0
for _ in $(seq 1 90); do
    if ssh -o ConnectTimeout=3 "${ssh_args[@]}" "$remote" true; then
        linux_ready=1
        break
    fi
    sleep 5
done
[ "$linux_ready" = 1 ] || { echo "Linux did not return after watchdog reset" >&2; exit 1; }
current_boot_id=$(ssh "${ssh_args[@]}" "$remote" cat /proc/sys/kernel/random/boot_id)
[ "$current_boot_id" != "$previous_boot_id" ] || {
    echo "watchdog reset test failed: Linux did not reboot" >&2
    exit 1
}
ssh "${ssh_args[@]}" "$remote" sudo -n \
    cmp -s /boot/boot.scr /boot/boot.scr.tgoskits-backup || {
        echo "watchdog reset test failed: Linux boot selector was not restored" >&2
        exit 1
    }
grep -q 'self-build watchdog reset test armed' "$serial_log" || {
    echo "watchdog reset test failed: armed marker is missing" >&2
    exit 1
}
grep -q 'LINUX-BOOT-RESTORED' "$serial_log" || {
    echo "watchdog reset test failed: Linux restore marker is missing" >&2
    exit 1
}

echo "watchdog_reset_previous_boot_id=$previous_boot_id"
echo "watchdog_reset_current_boot_id=$current_boot_id"
echo "watchdog_reset_serial_log=$serial_log"
echo "===STARRY-ORANGEPI5PLUS-WATCHDOG-RESET-PASS==="
