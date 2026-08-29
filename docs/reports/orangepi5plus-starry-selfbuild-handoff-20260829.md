# OrangePi 5 Plus StarryOS 自举编译任务交接

更新时间：2026-08-29 10:06（Asia/Shanghai）

## 1. 当前结论

本任务使用独立 worktree 和分支开发，没有在另一个对话正在使用的主工作树中修改代码：

- worktree：`/home/wuxun/Projects/tgoskits-orangepi5plus-selfbuild`
- branch：`feat/orangepi5plus-starry-selfbuild`
- base：`f96452ce892e2916a0c5bfe5aa9e7908b8085c06`（当时的 `upstream/dev`）
- 板卡：OrangePi 5 Plus，8 GiB，Linux 地址 `192.168.88.2`
- 串口：`/dev/serial/by-id/usb-1a86_USB_Serial-if00-port0`，1,500,000 baud

真正导致长时间自举编译失败的根因已经定位并修复：AArch64 PTE 的
`AttrIndx[2:0]` 位没有被 `bitflags` 命名，旧代码使用
`A64DescriptorAttr::from_bits_truncate` 查询 PTE 属性时会丢掉这些位，因而把 Normal
内存错误报告为 Device 内存。`mremap`/页迁移随后复用查询到的 flags 建立目标 PTE，
使普通匿名堆页真的被重映射成 Device 内存。glibc `memmove` 对该页执行非对齐
`ldp q4, q5, [x4, #-64]` 时便触发 alignment fault。

修复位于 `components/axcpu/src/aarch64/paging.rs`：查询 PTE 属性时改用
`from_bits_retain`，保留由数值字段解码的 AttrIndx 位。

截至本文更新时间，第一次包含最终修复的实板冷自举仍在运行，不能宣称任务已经完成。
第二次从干净 target 的复现也尚未开始。

## 2. 当前实板运行状态

正在运行：

- run ID：`starry-pte-green-1-20260829T020000Z`
- 源码归档 SHA-256：
  `62c582c46dc5da4e2d3d694984bd2a6a53da2bb366a5b651a4cf7f203704dc87`
- seed ELF SHA-256：
  `25f89bff3ea2fa3048b22b2e6640a5d5faf7a52718d4ce490b7058b5bc4e22df`
- seed BIN SHA-256：
  `a10818fc0d2c4d38e65d22172da3b8c7631017eff3604b12f578c4df9b6f01e9`
- FIT SHA-256：
  `179310373a582ac51a1e26c9c23c581c7e4c0992ecff161d51bb0ac1433608f6`
- FDT SHA-256：
  `30d62cc6cd7961b1c2c050142b407d33146304e19fcc5c69ff3b5f1480f032c2`
- host log：`tmp/board-starry-pte-green-1-20260829T020000Z-host.log`
- serial log：
  `target/starry-orangepi5plus-selfbuild/board-runs/starry-pte-green-1-20260829T020000Z/serial.log`

本次运行已经完成以下安全动作：

1. 从 Linux 原子部署并校验本次 FIT，未中断 U-Boot 零秒自动启动。
2. StarryOS shell 出现后，`init.sh` 的第一项动作已把 `/boot/boot.scr` 恢复为
   `/boot/boot.scr.tgoskits-backup`。
3. DesignWare watchdog 已启用：请求 30 秒，硬件实际周期 44.739242666 秒，
   每 10 秒喂狗，租期 10,200 秒。
4. chroot 冷构建前置探针已全部通过：

```text
aarch64_unaligned_access=PASS
aarch64_unaligned_ldp_q_stack=PASS
aarch64_unaligned_ldp_q_file=PASS
aarch64_unaligned_ldp_q_file_mmap=PASS
aarch64_unaligned_ldp_q_mremap=PASS
```

不要启动第二个串口程序，也不要杀死当前 `serial_selfbuild.py`。先用以下命令判断现有流程是否还活着：

```bash
ps -eo pid,etimes,args | rg 'run_selfbuild|serial_selfbuild|starry-pte-green-1'
tail -n 80 target/starry-orangepi5plus-selfbuild/board-runs/\
starry-pte-green-1-20260829T020000Z/serial.log
```

成功终态必须同时出现：

```text
===STARRY-ORANGEPI5PLUS-SELFBUILD-PASS run=starry-pte-green-1-20260829T020000Z ...===
===STARRY-ORANGEPI5PLUS-SELFBUILD-END-TO-END-PASS run=starry-pte-green-1-20260829T020000Z===
```

随后还必须确认 Linux SSH 恢复、boot ID 已变化、`boot.scr` 与备份一致、ext4 为 `rw`。

## 3. 根因证据链

旧的真实失败具有稳定特征：

- 失败运行：`starry-final-2-20260828T191318Z`、
  `starry-alignment-diag-20260829T013042Z` 等。
- 用户 PC 始终为 `0x1e2d220`，ESR 为 `0x92000021`。
- 目标 Debian glibc Build ID：
  `6f268ff546c56d7f14e9501a55698cc2d0ab4732`。
- `0x1e2d220` 对应 glibc 基址 `0x1d90000` 加 `0x9d220`；反汇编为非对齐
  `ldp q4, q5, [x4, #-64]`。
- 故障时 `SCTLR_EL1.A=0`，排除了全局 alignment check。
- 故障 VMA flags 是 `READ | WRITE | USER`，但实际 PTE 查询结果是
  `READ | WRITE | USER | DEVICE`。
- 普通栈、文件段、动态文件 mmap 和每个 CPU 上的相同非对齐指令都能通过，
  排除了 CPU 局部 SCTLR、一般 SIMD 和一般非对齐访问问题。
- `AddrSpace::move_pages` 会执行“查询源 PTE flags，再用这些 flags 建立目标 PTE”，
  与长时间 Cargo 构建中的内存迁移场景吻合。

Armv8-A 内存模型规定，`SCTLR_EL1.A=0` 时 Normal memory 可执行普通非对齐访问，
而 Device memory 的非对齐访问仍会 fault。参考：
[Armv8-A memory model guide](https://developer.arm.com/-/media/Arm%20Developer%20Community/PDF/Learn%20the%20Architecture/Armv8-A%20memory%20model%20guide.pdf?revision=58b1dd0a-3800-4218-b21a-f95a0332034c)。

## 4. 已完成验证

页表根因修复：

- 修复前新增测试必然失败：Normal PTE 查询结果多出 `DEVICE`。
- 修复后 `cargo test -p ax-cpu --test paging`：7/7 通过。
- `cargo xtask clippy --package ax-cpu`：29/29 check 通过。
- 新回归不仅测试 PTE round-trip，还模拟源 PTE flags 被查询后用于目标 PTE 的迁移路径。

真实 StarryOS QEMU 路径：

```bash
cargo xtask starry test qemu --arch aarch64 \
  -c qemu/system/test-aarch64-cpu-feat
```

结果：1/1 case 通过，测试内部 7/7 通过；其中包括实际
`mremap(MREMAP_MAYMOVE | MREMAP_FIXED)` 后执行与 glibc 故障形状相同的非对齐
`LDP Q`。完整日志：`tmp/qemu-aarch64-mremap-pte-green.log`。

此前已完成并仍适用于当前改动的验证：

- `cargo test -p dw-wdt`：7/7 通过。
- `cargo test -p rdif-watchdog`：1/1 通过。
- `cargo test -p rockchip-soc`：93 个单元测试和 doctest 通过。
- `cargo test -p axfs-ng-vfs --features host-test path_from...`：通过。
- `cargo xtask clippy --package someboot`：通过。
- watchdog、driver、VFS 和 axcpu 的目标 Clippy：通过。
- `cargo fmt --all`、ShellCheck、脚本 smoke、`git diff --check`：通过。
- AArch64 `getcwd` QEMU 回归：5/5 通过。

全量 Starry feature 的手工 Clippy 曾只被不属于本分支改动的既有
`platforms/somehal/src/arch/aarch64/gic/v3.rs` unnecessary-cast warning阻断；没有为绕过它
修改无关代码。一次更宽的 driver/Starry xtask matrix 还曾被 `aic8800-wifi` 网络依赖等待
阻断；定向相关 crate 已全部通过。

## 5. 主要改动分组

- `components/axcpu`：AArch64 启动寄存器初始化、PTE AttrIndx 修复及回归测试。
- `platforms/someboot`：生产启动路径显式清除 `SCTLR_EL1.A`。
- `drivers/interface/rdif-watchdog`、`drivers/watchdog/dw-wdt`、`drivers/ax-driver`：
  可复用 watchdog capability、DesignWare watchdog 和 OS glue。
- `drivers/soc/rockchip/rockchip-soc`：RK3588 watchdog/CRU 所需支持与验证。
- `os/StarryOS`：自举 watchdog 接入，以及板上自举所需功能组合。
- `fs/axfs-ng-vfs` 和 `os/StarryOS/kernel/src/syscall/fs/ctl.rs`：`getcwd` 路径语义修复。
- `test-suit/starryos/qemu/system`：AArch64 CPU/PTE 迁移和 `getcwd` 回归。
- `apps/starry/orangepi-5-plus-selfbuild`：可复用 rootfs、源码快照、seed/FIT 构建、
  原子部署、串口驱动、Linux 恢复、看门狗、产物回收与脚本测试。
- `docs/design/orangepi5plus-starry-selfbuild.md`：高风险设计、替代方案与安全不变量。
- `.claude/skills/arch-platform-porting/SKILL.md`：本次启动路径经验更新。

没有保留临时的 alignment-fault 内核诊断函数；最终代码只保留根因修复和稳定回归。

## 6. 完成任务的剩余步骤

### 6.1 等待第一次运行完成

不要重复启动当前命令。等待现有 `run_selfbuild.sh` 自行结束并检查退出码、两个 PASS
marker、产物 SHA-256、Linux 恢复和文件系统健康。

### 6.2 第二次干净 target 复现

第一次成功并确认 Linux 健康后，复用同一源码快照和 seed/FIT，运行：

```bash
export TMPDIR="$PWD/tmp"
apps/starry/orangepi-5-plus-selfbuild/run_selfbuild.sh \
  --host 192.168.88.2 \
  --run-id starry-pte-green-2-20260829TXXXXXXZ \
  --skip-provision \
  --skip-boot-build
```

每个 run ID 使用独立 `/work/targets/<run-id>`，成功后脚本会删除该 target。因此第二次仍是
冷 target 构建，同时复用完全相同的源码目录，适合比较最终 ELF/BIN SHA-256。

### 6.3 最终验收

只有以下条件全部成立才可把原任务标记为完成：

1. 两次不同 run ID 的冷自举都出现 guest PASS 和 end-to-end PASS。
2. 两次都自动返回 Linux，boot ID 变化。
3. `/boot/boot.scr` 与 `/boot/boot.scr.tgoskits-backup` 的 SHA-256 相同。
4. 根文件系统保持 ext4 `rw`，`dmesg` 无新的 ext4/mmc I/O error。
5. 两次产物的 `SHA256SUMS` 和 `source.meta` 已回收到主机并比较。
6. 把两次实测结果和全部复现命令写入最终总结报告。

## 7. 板卡安全约束

- 始终从板载 Linux 中转启动；不要抢 U-Boot 的零秒窗口。
- 不要手工输入 U-Boot `fdt`、`cp.b`、`booti` 命令，不反复 reset 或断电。
- 每次重启前必须先独占打开串口。
- 必须校验本次 FIT 内嵌 kernel 与当前 seed BIN 完全一致。
- Starry shell 出现后必须先恢复 Linux `boot.scr`，再运行负载。
- 测试后使用 `systemctl --force --force reboot` 返回 Linux，并实际检查 SSH、启动脚本和 ext4。
- 当前板端 Linux 默认启动脚本和备份的已知 SHA-256 均为：
  `d47fa003c0210128b863a04301e17ec56b7957cb3b3b2c80c1d467ee99c965e9`。
- 根分区 PARTUUID：`372c3b35-bab3-4069-acc1-8bce9be63ebf`；不要改成猜测的
  `/dev/mmcblk*` 名称。
- 持久 rootfs 只能复用
  `/opt/starry-orangepi5plus-selfbuild/rootfs`，不要创建第二份。

如果一次运行失败但 Linux 已恢复，只做只读健康检查并保留失败 target/log。若 Linux 尚未
恢复，先观察现有独占串口和 watchdog；不要再开第二个串口进程，也不要临时发明 U-Boot
恢复命令。
