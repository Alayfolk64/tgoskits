#!/bin/sh
set -u

RESULT_DIR=/tmp/starry-iperf-bench

main() {
    if [ "$#" -ne 1 ]; then
        fail "usage: $0 <server-ip>"
    fi

    SERVER_IP=$1
    if [ -z "$SERVER_IP" ]; then
        fail "server IP is empty"
    fi
    command -v iperf3 >/dev/null 2>&1 || fail "iperf3 is not installed"
    mkdir -p "$RESULT_DIR" || fail "cannot create $RESULT_DIR"

    echo "iperf-bench: server=$SERVER_IP:5201"
    echo "iperf-bench: TCP single stream, 3 rounds per direction, -t 10 -O 2 -l 128K"
    run_benchmark tx
    run_benchmark rx
    echo STARRY_IPERF_BENCH_PASSED
}

fail() {
    echo "iperf-bench: $1"
    echo STARRY_IPERF_BENCH_FAILED
    exit 1
}

run_benchmark() {
    direction=$1
    samples="$RESULT_DIR/$direction.samples"
    : >"$samples"

    for round in 1 2 3; do
        run_round "$direction" "$round"
    done

    median_bits_per_second=$(sort -n "$samples" | sed -n '2p')
    median_mbps=$(to_mbps "$median_bits_per_second")
    printf 'STARRY_IPERF_BENCH_RESULT direction=%s median_mbps=%s\n' \
        "$direction" "$median_mbps"
}

run_round() {
    direction=$1
    round=$2
    result="$RESULT_DIR/$direction-$round.json"

    if [ "$direction" = "rx" ]; then
        run_rx >"$result" 2>&1 || round_failed "$result"
    else
        run_tx >"$result" 2>&1 || round_failed "$result"
    fi

    if grep -q '"error"[[:space:]]*:' "$result"; then
        round_failed "$result"
    fi
    if ! grep -q '"end"[[:space:]]*:' "$result"; then
        round_failed "$result"
    fi

    bits_per_second=$(read_rate "$result")
    if [ -z "$bits_per_second" ]; then
        round_failed "$result"
    fi

    echo "$bits_per_second" >>"$RESULT_DIR/$direction.samples"
    printf 'STARRY_IPERF_BENCH_SAMPLE direction=%s round=%s mbps=%s\n' \
        "$direction" "$round" "$(to_mbps "$bits_per_second")"
}

run_tx() {
    iperf3 -c "$SERVER_IP" -t 10 -O 2 -l 128K -J
}

run_rx() {
    iperf3 -c "$SERVER_IP" -t 10 -O 2 -l 128K -R -J
}

round_failed() {
    result=$1
    cat "$result"
    fail "iperf3 did not produce a complete result"
}

read_rate() {
    awk '
        /"sum_received"[[:space:]]*:[[:space:]]*\{/ {
            reading_sum = 1
            next
        }
        reading_sum && /"bits_per_second"[[:space:]]*:/ {
            rate = $0
            sub(/^[^:]*:[[:space:]]*/, "", rate)
            sub(/,.*/, "", rate)
            gsub(/[[:space:]]/, "", rate)
            print rate
            exit
        }
    ' "$1"
}

to_mbps() {
    awk -v rate="$1" 'BEGIN { printf "%.3f", rate / 1000000 }'
}

main "$@"
