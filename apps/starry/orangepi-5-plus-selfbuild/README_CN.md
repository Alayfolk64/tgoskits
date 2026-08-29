# OrangePi 5 Plus 上的 StarryOS 自举编译

这个应用从 `macos-selfbuild` 的 AArch64 编译闭环演化而来，但使用真实的
OrangePi 5 Plus、直连串口和 Debian 12 arm64 glibc chroot。物理板启动不经过
OSTool/`cargo xtask ... board`，也不抢 U-Boot 的 0 秒输入窗口；主机先打开串口
监控，再由板载 Linux 临时选择已经校验的一次性 StarryOS 启动脚本。
它是 `apps/starry/orangepi-5-plus-selfbuild` 下的独立应用，不替换也不删除
macOS AArch64 或 x86_64 适配，不使用 KVM/HVF。

当前正确性目标是“编译闭环”：StarryOS 内完整编译 `starryos`，产出 AArch64
ELF 和 raw binary，回到 Linux 后取回产物并验证 SHA-256。暂不把第二代内核
自动设为下一次启动内核。

## 恢复模型

应用单独开启 `selfbuild-watchdog`：RK3588 的 `snps,dw-wdt` 请求 30 秒超时，
硬件按 TOP 粒度向上取整；CPU 0 每 10 秒喂狗。喂狗租约为 10,200 秒，chroot
里的编译命令由 `timeout` 限制为 9,600 秒。因此内核死锁由硬件 watchdog
复位，用户态活锁由 `timeout` 终止；正常结束后主动 reboot。U-Boot 必须默认
进入 Linux：若当前 `/boot/boot.scr` 是已知的
Starry 脚本且存在经过内容检查的 `boot.scr.tgoskits-backup`，准备脚本会先保留
当前脚本再恢复 Linux 备份；遇到未知 `bootcmd` 或无法验证的备份时会停止，
绝不猜测和覆盖。

`stage_starry_boot.sh` 从板端当前 Linux 的 `/sys/firmware/fdt` 取得实际 DTB，
与主机编译的内核组成带 SHA-256 节点的 FIT，并反向提取 FIT 中的 kernel/DTB
与输入逐字节比较。部署先写 `.new`、校验、`sync`，再在相同文件系统原子改名为
`/image.fit`、`/boot/boot-starryos-emmc.scr` 和 `/boot/starryEnv.txt`；该阶段不修改
`/boot/boot.scr`。完整入口在串口监控就绪后才临时替换 `/boot/boot.scr` 并重启，
StarryOS shell 出现后执行的第一步是恢复并校验 Linux 备份，然后才开始编译。
一次性环境使用 Linux 根分区的 GPT `PARTUUID` 作为 StarryOS 的 `root=`；不能
复用 Linux 的 `/dev/mmcblkN` 编号，因为本板上 Linux、StarryOS 与 U-Boot 的
MMC 枚举顺序不同。OrangePi 5 Plus 的 eMMC 在当前 U-Boot 中实测为 `mmc 1`。

另有一次性 missed-feed 验证：测试内核只启动 watchdog、不创建 feeder；只有
串口确实出现 armed 日志且随后 Linux SSH 恢复，测试才通过。

## 一次性准备可复用 glibc rootfs

先让板卡进入默认 Linux，并从 Linux 控制台得到 IP。自动入口需要独占直连串口，
因此运行前只检查设备存在，不能让 `picocom`、`tio`、`minicom` 或其他进程保持
打开：

```bash
test -c /dev/serial/by-id/usb-1a86_USB_Serial-if00-port0
```

另一个终端执行：

```bash
apps/starry/orangepi-5-plus-selfbuild/provision_rootfs.sh \
  --host <BOARD_IP>
```

脚本在板端创建并复用
`/opt/starry-orangepi5plus-selfbuild/rootfs`，其中包含：

- Debian 12 arm64 glibc 用户态；
- 仓库锁定的 Rust nightly、`rust-src`、LLVM tools、`cargo-binutils` 和
  `gen_ksym`；
- C/C++、Clang/LLVM、CMake、binutils、U-Boot tools、`perf` 等构建工具；
- Cargo registry/git cache，以及 `-Z build-std=core,alloc` 所需的 nightly
  sysroot crate；
- 当前工作区精确快照及 commit/ref/dirty/toolchain/SHA-256 元数据；
- `/work/targets` 和 `/output/runs` 持久目录。

源码按 archive SHA-256 存在版本目录中，rootfs 和旧架构应用都不会被删除。
相同 rootfs 后续只更新工具链、Cargo cache 和源码快照。

## 跑完整正确性闭环

```bash
apps/starry/orangepi-5-plus-selfbuild/run_selfbuild.sh \
  --host <BOARD_IP> \
  --skip-provision
```

也可以省略 `--skip-provision`，让入口先幂等检查 rootfs。主运行命令会独占打开
1,500,000 波特率串口，原子切换一次启动项、自动发送 guest 命令、等待 Linux
恢复并取回产物。如果板载 Linux 安装了一次性启动脚本却没有实际进入重启，主机
会立即恢复 Linux 默认启动脚本并失败退出。

入口默认通过 `build_seed.sh` 使用原生 Cargo 构建 StarryOS 种子内核、生成
kallsyms 并输出 raw AArch64 boot image；主机种子构建和板端自举编译都不调用
`tg-xtask`。已有经过验证的种子内核时可加 `--skip-boot-build`。

StarryOS 内直接执行 `cargo build -p starryos`，不会在每轮冷 target 中重新编译
`tg-xtask` host runner。编译固定到 Cortex-A76 大核 CPU 4–7，默认 `JOBS=4`、
`RUSTC_THREADS=2`、`RAYON_NUM_THREADS=2`，内核日志为 Warn；watchdog armed、
FAIL/PASS 等验收信号仍可见。完整成功需要先在串口看到 guest PASS：

```text
===STARRY-ORANGEPI5PLUS-SELFBUILD-PASS run=... jobs=4 elapsed=...===
```

板卡随后自动回到 Linux；完整入口会自行取回并验证产物。也可以单独复核：

```bash
apps/starry/orangepi-5-plus-selfbuild/fetch_artifacts.sh \
  --host <BOARD_IP> --run-id <RUN_ID>
```

主机最后应输出：

```text
===STARRY-ORANGEPI5PLUS-SELFBUILD-HOST-PASS run=...===
```

取回目录为 `target/starry-orangepi5plus-selfbuild/artifacts/<run-id>/`，包含
`starryos.elf`、`starryos.bin`、`SHA256SUMS`、源码元数据、耗时和完整日志。

## 验证 watchdog 复位

板卡处于 Linux 时先准备 missed-feed 镜像：

```bash
apps/starry/orangepi-5-plus-selfbuild/test_watchdog_reset.sh \
  --host <BOARD_IP>
```

该测试不会运行编译，而是故意不创建 feeder。脚本会自动恢复 Linux 启动项，
随后同时验证 armed 串口证据、Linux SSH 恢复和 boot ID 已变化。正常自举流程
不使用本测试的 missed-feed 镜像。
最终通过标志为：

```text
===STARRY-ORANGEPI5PLUS-WATCHDOG-RESET-PASS===
```

## Linux 基线、三轮中位数和 profiling

Linux 单次基线使用同一 chroot、源码、工具链、cache、CPU 4–7 和并行度：

```bash
apps/starry/orangepi-5-plus-selfbuild/run_linux_baseline.sh \
  --host <BOARD_IP> --skip-provision
```

在正确性和 watchdog 恢复均通过后，交替执行三轮 Linux/StarryOS 冷构建并取
中位数。每轮 StarryOS 构建复用上述自动串口和 Linux 恢复流程：

```bash
apps/starry/orangepi-5-plus-selfbuild/benchmark.sh --host <BOARD_IP>
```

结果位于 `target/starry-orangepi5plus-selfbuild/benchmarks/`。每轮使用新的
Cargo target 目录；Linux 轮先 sync/drop_caches，StarryOS 轮通过重新启动清空
内核缓存。成功轮在复制 ELF/bin 后清理其 app 专属 target 目录，失败轮保留
target 便于诊断，避免六轮构建耗尽板端存储。

只有完整编译已经 PASS 后才开始 profiling：

```bash
# 低开销总体计数
apps/starry/orangepi-5-plus-selfbuild/run_selfbuild.sh \
  --host <BOARD_IP> --skip-provision --profile stat

# 99 Hz 调用栈采样
apps/starry/orangepi-5-plus-selfbuild/run_selfbuild.sh \
  --host <BOARD_IP> --skip-provision --profile record
```

`perf-stat.txt` 或 `perf.data` 与对应的 PASS 日志和产物保存在同一 run 目录。
先用测量定位最大瓶颈，再评估 MOSS BuildStorm 中出现过的缺页、分配器、page
cache、ext4 或 TLB 假设；这些优化不会在正确性阶段被盲目移植。
