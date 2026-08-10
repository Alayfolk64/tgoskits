#!/bin/sh
set -eu

case_dir=$(unset CDPATH; cd -- "$(dirname -- "$0")" && pwd)
repo_root=$(git -C "$case_dir" rev-parse --show-toplevel)
server_log=$(mktemp "${TMPDIR:-/tmp}/starry-iperf3-server.XXXXXX")
server_pid=

cleanup() {
    if [ -n "$server_pid" ]; then
        kill "$server_pid" 2>/dev/null || true
        wait "$server_pid" 2>/dev/null || true
    fi
    rm -f "$server_log"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

command -v iperf3 >/dev/null 2>&1 || {
    echo "iperf-bench: host iperf3 is not installed" >&2
    exit 1
}

iperf3 -s >"$server_log" 2>&1 &
server_pid=$!
sleep 1

if ! kill -0 "$server_pid" 2>/dev/null; then
    wait "$server_pid" 2>/dev/null || true
    server_pid=
    if ! command -v ss >/dev/null 2>&1 ||
        ! ss -H -ltnp '( sport = :5201 )' | grep -q 'iperf3'; then
        cat "$server_log" >&2
        echo "iperf-bench: port 5201 is unavailable" >&2
        exit 1
    fi
    echo "iperf-bench: using the existing host server on port 5201"
fi

cd "$repo_root"
cargo xtask starry test board \
    -c native-network-smoke \
    --board orangepi-5-plus
