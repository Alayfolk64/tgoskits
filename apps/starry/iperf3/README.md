# StarryOS iperf3 benchmark

This board app runs a fixed TCP benchmark matrix on Orange Pi 5 Plus. It covers
single-stream TX/RX, bidirectional traffic, 2/4/8-stream TX, and 4-stream RX.
Every scenario runs three times and prints the samples, median, and a final
summary table. JSON results remain on the board under
`/tmp/starry-iperf3-bench/` for later inspection.

Run the complete benchmark from the repository root:

```bash
./apps/starry/iperf3/run-board.sh
```

The wrapper starts or reuses the host iperf3 server and delegates deployment to
the board runner. The board session provides both the host address and the
script URL, so the command does not contain a fixed IP address.

The benchmark profile is intentionally fixed: 10 seconds, a 2-second omit,
128K application blocks, and three rounds. `native-network-smoke` remains the
short CI-oriented connectivity check.
