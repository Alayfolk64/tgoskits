#!/bin/sh
set -u

duration=10
omit=2
block_size=128K
rounds=3
result_dir=/tmp/starry-iperf3-bench
summary_file=$result_dir/summary

fail() {
    printf '\niperf3-bench: %s\n' "$1" >&2
    echo STARRY_IPERF3_BENCH_FAILED
    exit 1
}

to_mbps() {
    awk -v rate="$1" 'BEGIN { printf "%.3f", rate / 1000000 }'
}

read_rate() {
    awk -v field="$1" '
        $0 ~ "\"" field "\"[[:space:]]*:" && /\{/ {
            in_field = 1
            next
        }
        in_field && /"bits_per_second"[[:space:]]*:/ {
            rate = $0
            sub(/^[^:]*:[[:space:]]*/, "", rate)
            sub(/,.*/, "", rate)
            gsub(/[[:space:]]/, "", rate)
            print rate
            exit
        }
    ' "$2"
}

print_command() {
    printf 'Command: iperf3 -c %s -t %s -O %s -P %s -l %s' \
        "$server_ip" "$duration" "$omit" "$case_streams" "$block_size"
    case "$case_mode" in
        rx) printf ' -R' ;;
        bidir) printf ' --bidir' ;;
    esac
    printf '\n\n'
}

run_iperf() {
    case "$case_mode" in
        tx)
            iperf3 -c "$server_ip" -t "$duration" -O "$omit" \
                -P "$case_streams" -l "$block_size" -J
            ;;
        rx)
            iperf3 -c "$server_ip" -t "$duration" -O "$omit" \
                -P "$case_streams" -l "$block_size" -R -J
            ;;
        bidir)
            iperf3 -c "$server_ip" -t "$duration" -O "$omit" \
                -P "$case_streams" -l "$block_size" --bidir -J
            ;;
    esac
}

record_rate() {
    record_direction=$1
    record_field=$2
    record_rate=$(read_rate "$record_field" "$round_result")
    [ -n "$record_rate" ] || round_failed "$round_result"

    record_mbps=$(to_mbps "$record_rate")
    echo "$record_mbps" >>"$result_dir/$case_id-$record_direction.samples"
    printf 'Run %s  %-6s %10s Mbps\n' \
        "$round" "DUT $record_direction:" "$record_mbps"
}

round_failed() {
    cat "$1" >&2
    fail "$case_id round $round did not produce a complete result"
}

run_round() {
    round=$1
    round_result=$result_dir/$case_id-$round.json

    run_iperf >"$round_result" 2>&1 || round_failed "$round_result"
    if grep -q '"error"[[:space:]]*:' "$round_result" ||
        ! grep -q '"end"[[:space:]]*:' "$round_result"; then
        round_failed "$round_result"
    fi

    case "$case_mode" in
        tx) record_rate TX sum_received ;;
        rx) record_rate RX sum_received ;;
        bidir)
            record_rate TX sum_received
            record_rate RX sum_received_bidir_reverse
            ;;
    esac
}

summarize_direction() {
    summary_direction=$1
    case "$summary_direction" in
        TX) summary_direction_lower=tx ;;
        RX) summary_direction_lower=rx ;;
    esac
    summary_samples=$result_dir/$case_id-$summary_direction.samples
    summary_run_1=$(sed -n '1p' "$summary_samples")
    summary_run_2=$(sed -n '2p' "$summary_samples")
    summary_run_3=$(sed -n '3p' "$summary_samples")
    summary_median=$(sort -n "$summary_samples" | sed -n '2p')

    printf '\nMedian %-6s %10s Mbps\n' "DUT $summary_direction:" "$summary_median"
    printf '%s|%s|%s|%s|%s|%s|%s\n' \
        "$case_id" "$case_label" "$summary_direction" \
        "$summary_run_1" "$summary_run_2" "$summary_run_3" "$summary_median" \
        >>"$summary_file"
    printf 'STARRY_IPERF3_BENCH_RESULT case=%s direction=%s median_mbps=%s\n' \
        "$case_id" "$summary_direction_lower" "$summary_median"
}

run_case() {
    case_id=$1
    case_label=$2
    case_mode=$3
    case_streams=$4

    : >"$result_dir/$case_id-TX.samples"
    : >"$result_dir/$case_id-RX.samples"

    printf '\n============================================================\n'
    printf '%s  %s\n' "$case_id" "$case_label"
    printf '============================================================\n\n'
    print_command

    round=1
    while [ "$round" -le "$rounds" ]; do
        run_round "$round"
        round=$((round + 1))
    done

    case "$case_mode" in
        tx) summarize_direction TX ;;
        rx) summarize_direction RX ;;
        bidir)
            summarize_direction TX
            summarize_direction RX
            ;;
    esac
}

print_summary() {
    printf '\n============================================================\n'
    printf 'iperf3 benchmark summary (Mbps)\n'
    printf '============================================================\n\n'
    printf '%-4s %-29s %-4s %10s %10s %10s %10s\n' \
        Case Scenario Dir Run1 Run2 Run3 Median
    printf '%-4s %-29s %-4s %10s %10s %10s %10s\n' \
        ---- ----------------------------- ---- ---------- ---------- ---------- ----------

    while IFS='|' read -r summary_case summary_label summary_direction \
        summary_run_1 summary_run_2 summary_run_3 summary_median; do
        printf '%-4s %-29s %-4s %10s %10s %10s %10s\n' \
            "$summary_case" "$summary_label" "$summary_direction" \
            "$summary_run_1" "$summary_run_2" "$summary_run_3" "$summary_median"
    done <"$summary_file"

    printf '\nSTARRY_IPERF3_BENCH_PASSED\n'
}

main() {
    if [ "$#" -ne 1 ] || [ -z "$1" ]; then
        fail "usage: $0 <server-ip>"
    fi
    command -v iperf3 >/dev/null 2>&1 || fail "iperf3 is not installed"

    server_ip=$1
    mkdir -p "$result_dir" || fail "cannot create $result_dir"
    : >"$summary_file"

    printf '\niperf3 benchmark\n'
    printf 'Server: %s:5201\n' "$server_ip"
    printf 'Profile: 10 seconds, 2-second omit, 128K block, 3 rounds\n'

    run_case T01 "Single-stream DUT TX" tx 1
    run_case T02 "Single-stream DUT RX" rx 1
    run_case T03 "Single-stream bidirectional" bidir 1
    run_case T04 "2-stream DUT TX" tx 2
    run_case T05 "4-stream DUT TX" tx 4
    run_case T06 "8-stream DUT TX" tx 8
    run_case T07 "4-stream DUT RX" rx 4

    print_summary
}

main "$@"
