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
and limits the guest command to 9,600 seconds. The Starry shell restores the
verified Linux boot script before starting the workload, so a later reset
returns to Linux. The build workload is pinned to CPUs 4-7 and directly builds
the `starryos` package instead of rebuilding the host-side runner. The seed
kernel is also built directly with Cargo by `build_seed.sh`; neither path
invokes `tg-xtask`.
The one-time boot selects the Linux root partition by GPT `PARTUUID`; Linux,
StarryOS, and U-Boot do not share stable MMC device numbers. The end-to-end
entry drives the UART, waits for Linux to return, and fetches and verifies the
output artifacts before reporting success.
