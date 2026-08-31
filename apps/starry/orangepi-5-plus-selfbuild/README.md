# StarryOS self-build on OrangePi 5 Plus

This independent board application reuses the AArch64 self-build flow without
changing the existing macOS or x86_64 applications. It runs on a physical
OrangePi 5 Plus and does not use KVM/HVF. Physical-board control uses the direct
1,500,000-baud UART and a Linux-selected, verified one-time U-Boot script; it
does not invoke the OSTool board runner or interrupt U-Boot autoboot.

The correctness milestone is a compile closure: StarryOS builds an AArch64
`starryos` ELF and raw binary inside a reusable Debian 12 arm64 glibc chroot,
then Linux retrieves and verifies both artifacts and their SHA-256 hashes.
Booting the self-built second-generation kernel is intentionally out of scope.

See [README_CN.md](README_CN.md) for the complete provisioning, recovery,
benchmark, and profiling workflow. The end-to-end entry is:

```bash
apps/starry/orangepi-5-plus-selfbuild/run_selfbuild.sh \
  --host <BOARD_IP>
```

The application enables the RK3588 DesignWare hardware watchdog only for this
build. It requests a 30-second reset timeout, feeds from CPU 0 every 10 seconds,
and limits the guest command to 21,600 seconds. The feeder lease is 22,200
seconds, leaving ten minutes for timeout recovery. The Starry shell restores the
verified Linux boot script before starting the workload, so a later reset
returns to Linux. The guest uses the system-default CPU affinity and
parallelism: it first builds the debug `tg-xtask` host runner with plain
`cargo build -p tg-xtask`, then invokes that exact binary to build
StarryOS from the application build config. It emits minute-level compile-unit
progress markers for Linux/StarryOS comparison. The seed kernel remains a
separate native-Cargo bootstrap built by `build_seed.sh`.
Profiling is deliberately bounded to the first command: `--profile stat` or
`--profile record` measures at most 300 seconds of `cargo build -p tg-xtask`
and exits without starting the StarryOS build. `record` uses flat 49 Hz cycle
samples because the current StarryOS perf ABI does not support call-chain
samples. The Linux and StarryOS runs keep the same system-default parallelism.
The one-time boot selects the Linux root partition by GPT `PARTUUID`; Linux,
StarryOS, and U-Boot do not share stable MMC device numbers. The end-to-end
entry drives the UART, waits for Linux to return, and fetches and verifies the
output artifacts before reporting success.
