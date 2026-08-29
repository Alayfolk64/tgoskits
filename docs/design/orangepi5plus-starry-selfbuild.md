# OrangePi 5 Plus StarryOS self-build

## Problem and success criteria

The existing `macos-selfbuild` application proves that an AArch64 StarryOS
guest can rebuild StarryOS, but its QEMU/HVF rootfs and runner do not describe a
physical OrangePi 5 Plus. The board workflow needs a separate application that
reuses the AArch64 build procedure while keeping board provisioning, recovery,
and measurements explicit.

The first milestone is correctness, not booting the newly built second-stage
kernel. A run succeeds only when all of the following are observable:

- the host opens the direct UART before board Linux atomically selects a
  verified one-time U-Boot script, without racing the U-Boot input window;
- the host seed kernel and guest rebuild use native Cargo commands without a
  `tg-xtask` runner;
- the guest build exits successfully and prints the final PASS marker;
- the resulting AArch64 ELF and raw binary are copied back to the Linux host;
- source metadata and SHA-256 hashes make the input and output reproducible;
- after either success or a forced hang, the board returns to its Linux default
  U-Boot entry without a manual power cycle.

After correctness is stable, Linux and StarryOS use the same source snapshot,
rootfs, toolchain, Cargo cache, CPU affinity, and job count for three cold runs.
The median elapsed time is the baseline. StarryOS `perf` data is collected only
after the full build passes.

## Scope and non-goals

This change adds `apps/starry/orangepi-5-plus-selfbuild`; it does not replace or
delete the macOS AArch64 or x86_64 applications. It does not add KVM/HVF, a
watchdog character device, a syscall/Linux watchdog ABI, automatic boot of the
self-built kernel, or speculative performance fixes copied from another
platform.

## Alternatives

- Reusing the x86_64/KVM application was rejected because the OrangePi is
  AArch64 and the build must execute on the board.
- Modifying `macos-selfbuild` in place was rejected because it would mix a QEMU
  image lifecycle with a physical-board lifecycle and could regress the existing
  app.
- An external USB relay or automated power controller would recover the broadest
  class of failures, but the current three-wire setup only provides UART. The
  on-SoC watchdog is therefore required for kernel deadlocks; command-level
  `timeout` handles userspace livelocks.
- A direct hard-coded MMIO poke from StarryOS was rejected because clock setup,
  timeout rounding, and register ownership belong in reusable driver layers.

## Architecture and ownership

The implementation follows four boundaries:

1. `drivers/watchdog/dw-wdt` owns the Synopsys DesignWare register protocol and
   timeout selection. It is `no_std`, depends on `mmio-api`, and has no FDT,
   scheduler, or StarryOS dependency.
2. `drivers/interface/rdif-watchdog` defines the minimal typed capability used
   by the runtime: arm in reset mode, ping, inspect the programmed timeout, and
   query the remaining time. Errors remain typed.
3. `drivers/ax-driver` owns FDT discovery for `snps,dw-wdt`, resource preparation
   (`tclk`, `pclk`, and resets), MMIO mapping, and RDIF registration. Missing or
   invalid clocks are errors; the driver does not guess a frequency.
4. StarryOS owns the sleepable lease task because `ax-driver` cannot depend on
   `ax-task`. The self-build feature arms the watchdog before PID 1 starts and
   pins the feeder to CPU 0. The compilation workload is pinned to CPUs 4-7.

The board DTB describes the watchdog at `0xfeaf0000` with a `0x100` register
window and named `tclk`/`pclk` clocks. Register semantics and fixed timeout
counts follow the Linux `dw_wdt` driver: reset mode clears the response-mode
bit, `TORR` programs TOP/TOP_INIT, `CRR = 0x76` reloads the counter, and `CR.EN`
starts it. A requested timeout is rounded up to the first representable timeout
and both requested and actual durations are logged.

## Recovery policy

The self-build application requests a 30-second watchdog period and feeds it
every 10 seconds from CPU 0. The lease is 10,200 seconds and the guest command
has a 9,600-second timeout. The lease is the outer bound when shutdown or the
direct serial session can no longer make progress.
If StarryOS deadlocks, the hardware watchdog resets the SoC. If the build
livelocks, `timeout` terminates it and the guest records FAIL. If shutdown then
hangs, the lease expires and resets the board.

The Linux-side provisioning script must verify that U-Boot's default boot target
is Linux before starting the StarryOS run. It may restore a known Linux
`boot.scr.tgoskits-backup` after archiving a recognized Starry script, but it
must not silently rewrite an unknown boot environment. FIT deployment uses a
temporary file, hash check, sync, same-filesystem atomic rename, and a second
hash check. Once the direct UART monitor is ready, Linux temporarily installs
the verified StarryOS script as `/boot/boot.scr` and reboots. The first Starry
shell command restores and syncs the Linux backup before mounts or compiler
work begin. The expected recovery condition is an SSH-reachable Linux system
plus a completed filesystem check/sync path.

Linux, StarryOS, and U-Boot enumerate the eMMC and SD card independently. The
one-time environment therefore selects the Linux root GPT partition by
`PARTUUID` instead of copying `/dev/mmcblkN`, and the OrangePi-specific U-Boot
command uses the serial-observed eMMC index `mmc 1`. Missing partition identity
is an error; the script must not guess a Starry block-device index.

The watchdog is opt-in and fail-closed for this application: if discovery,
clock preparation, registration, or arming fails, boot prints an explicit error
and the self-build run does not start. Generic StarryOS builds remain unchanged.

## Reusable glibc rootfs

Linux prepares one Debian 12 arm64 glibc directory on the board. It contains the
pinned Rust nightly, host build tools, Cargo registry/git caches, the source
archive, source metadata, the guest script, and output/log directories. A
manifest records tool versions and hashes. Provisioning and updating this tree
use SSH/rsync while Linux is running because a compiler rootfs is too large for
serial transfer; boot control and runtime observation remain on the direct UART
session.

The same chroot is used for the Linux baseline and mounted as the StarryOS
rootfs. The guest build runs offline and validates the output with `file`, ELF
headers, and SHA-256 before printing PASS.

## Validation and evidence

| Claim or risk | Lowest useful check | Observable condition |
| --- | --- | --- |
| timeout rounding and MMIO programming | `dw-wdt` unit tests with fake MMIO | correct TOP, kick, reset-mode enable, invalid input rejected |
| capability dispatch | `rdif-watchdog` unit tests | methods reach the wrapped driver and preserve typed errors |
| FDT binding and clock requirements | `ax-driver` tests/build | compatible node registers only with valid MMIO and non-zero `tclk` |
| feature isolation | targeted clippy/build with feature on and off | existing board/apps do not gain watchdog policy |
| board recovery | deliberate missed-feed direct-serial run | watchdog arm marker followed by Linux availability and a changed boot ID |
| self-build correctness | Linux-selected boot observed and driven over direct UART | Linux selector restored before build, PASS marker, fetched ELF/bin, matching metadata and hashes |
| performance baseline | three cold Linux and StarryOS runs | recorded per-run logs and medians; profile linked to a passing run |

The physical-board checks are mandatory before claiming the application is
fully working. Host-only compilation proves integration but not RK3588 clock or
reset behavior.

## Compatibility and rollback

All new behavior is behind application-selected Cargo features. Rolling back is
equivalent to building without the watchdog/self-build features or removing the
new application directory. No on-disk format or userspace ABI is introduced.
The U-Boot Linux-default prerequisite is external board state and must be
recorded in the run log rather than assumed.
