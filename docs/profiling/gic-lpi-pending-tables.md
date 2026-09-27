# GICv3 每核 LPI pending table 地址重叠

## 原现场证据

S（ext4 锁外读）在 600 秒窗口内停滞，没有分钟进度、采样导出或窗口成功
标记，不能作为性能结果。原 QEMU PID 491862 的现场保存在
`target/profiling/arceos-helloworld/starry/ext4-read-concurrency-window-600/`：
`host-gdb.log`、`guest-gdb.log`、`stall.vmcore`（完整 8 GiB guest RAM）。
内存快照在连接调试器后采集，包含调试暂停，不能用于计时比较。

7 个核位于 run_idle；CPU 6 多次停在中断入口/返回，ELR_EL1 始终为
`0xffffffff80345848`，即 WaitQueue::wait_timeout 恢复 IRQ 后的下一条指令。
实际被中断任务是 gc_entry。分发编号为 domain=9、hwirq=7、cpu=6，处理器
为 NvmeBlockIrqHandler::ack，GIC 原始 LPI INTID 为 `0x2007`。

按 QEMU info mtree 确认 GICR 起点 `0x80a0000`、每核跨度 `0x20000`。
读取所有八核的 PROPBASER/PENDBASER，分别全部为
`0x070000023fc6038f` / `0x070000023fc70380`；pending 物理地址全部为
`0x23fc70000`。读取 `0x23fc70400`（LPI 8192 起的 pending 字节）为零。

## 因果链和修复边界

somehal ITS 使用 16 个 ID 位，pending 字节数为 8192。旧实现只把总分配
首地址按 65536 对齐，每核 stride 却按 4096 对齐，仍为 8192。
arm-gic-driver 编码 PENDBASER 时右移 16 位，导致八核地址全部重叠。
这是当前 base 已有的平台缺陷，不是 S 新增的文件系统锁顺序。

本地 Linux `980ab36ae5972c83f683b939e50c469c4947229e` 的
drivers/irqchip/irq-gic-v3-its.c:66 用 ALIGN(..., SZ_64K) 确定 pending
分配大小，its_allocate_pending_table 为每核分配独立表；其
include/linux/irqchip/arm-gic-v3.h:200 定义地址位为 51:16。

[QEMU v10.2.1 Redistributor 实现](https://github.com/qemu/qemu/blob/v10.2.1/hw/intc/arm_gicv3_redist.c#L823)
在 pending 位已为目标状态时直接返回，不刷新该核缓存的最高优先级 LPI。
因此共享 pending 表会使别的核清位与本核缓存不一致；这与现场“pending
字节为零但持续投递 0x2007”吻合。修复后仍须原场景验证，不能只用推断
宣称全部卡顿已解决。

修复复用既有表分配和硬件接口，不改 IRQ 亲和性、NVMe、调度策略或 ext4。
每核 stride 和总分配首地址统一遵守 driver 的 64 KiB 对齐常量。
Gic::init_lpi_tables 在任何寄存器写入前拒绝不对齐、零 stride、地址运算
溢出和超出 PENDBASER 编码范围的布局。8 核 pending 分配从 64 KiB 增至
512 KiB，多用 448 KiB，这是隔离每核状态的必要开销。

不选择减少 CPU、切换 virtio-blk、屏蔽 NVMe 中断、手工清 pending 位或
增加重试/看门狗；这些不能修复 pending table 所有权重叠。

## 验证记录

直接编译执行 AArch64 arm-gic-driver 生产 Gic::init_lpi_tables，使用内存
寄存器 fixture，通过 qemu-aarch64 执行单元测试，不解析源码文本。
旧实现 3 项中 2 失败（退出 101），原文分别为：
`8 KiB stride aliases all eight pending tables` 和
`PENDBASER silently discards address bits 15:0`。
修复后同一测试 3/3 通过，完整 driver AArch64 单元测试 21/21 通过。
初次测试构建暴露原有尺寸测试的四个 E0603 私有导入错误，现已把这些测试
移至 GICv3 子模块，未扩大生产类型可见性。

cargo xtask clippy --package arm-gic-driver --package somehal：10/10 通过。
driver 的 AArch64 lib+tests clippy 通过。另行检查 somehal AArch64
mmu/uspace 组合发现已有 unnecessary_cast，删除同类型转换后重验。
重验通过；固定 Starry 配置构建通过，完成 13045 个 kallsyms 与 BIN 刷新。
本仓库 clippy 通过 metadata 选择包，不存在旧 skill 提到的
scripts/test/clippy_crates.csv，因此未创建第二套白名单。

S 故障根盘已移至 tmp/axbuild/rootfs/rootfs-profile-ext4-read-stall.img，
没有删除或修复；只读 e2fsck -fn 退出 0，但提示跳过 journal recovery，
不把这次非正常退出等同于干净卸载。

## 修复后寄存器验证与测量隔离

2026-09-09 23:25 UTC 的 ext4-read-gic-alignment-window-600 仅用于启动
诊断，没有发送编译 runner，不是 600 秒性能结果。该次启动与根盘哈希计算
发生重叠，即使哈希等于冻结值也不把它作为受控测量。boot-gdb.log 记录
全部八核 PENDBASER 的物理地址依次为 0x23fc80000、0x23fc90000、
0x23fca0000、0x23fcb0000、0x23fcc0000、0x23fcd0000、0x23fce0000、
0x23fcf0000；每核独立且间隔 64 KiB，PROPBASER 仍共享 0x23fc60000。
GDB 脱离后通过 guest sync、poweroff -f 退出 QEMU，诊断盘保存为
tmp/axbuild/rootfs/rootfs-profile-gic-boot-check.img。

正式测量使用 ext4-read-gic-alignment-measure-600，新恢复冻结根盘并在
启动前等待全量哈希完成，不启用调试服务器。保存 ELF SHA-256：
857616b842a3271fcd31ac3309af9e7b853c63d8307cd4347adf6f91a559dacd；BIN：
842a9e5caaeb9604ca905a9c5735671bae27a7b3a0a8c09753d2004a4acdf0f0。
候选包含 S 锁外读和本次 T 中断表修复，因此相对 R 的差异不能全部归因于 S。
正式窗口已正常结束：611 秒、40 单元 / 37 crate、CPU 活跃 26.4154%，
所有 dropped/skipped 为 0，五项 SHA-256 与离线 e2fsck -fn 通过。
本轮未复现 IRQ 循环，但不代表所有停滞已修复，也没有完整编译加速证据。
详细热点与 R 对照见 [锁外读取实测](ext4-read-concurrency.md)。

补充零 stride、乘法/加法溢出和 52 位边界回归后，AArch64 单元测试 22/22。
clippy 曾报告测试中 u64::MAX 的冗余按位与（identity_op，退出 101），
移除冗余表达式后 AArch64 lib+tests clippy 和格式检查通过，未压制 lint。

run-profile-qemu.py 的调试开关回归先验证旧实现失败，再实现并通过 4/4：
默认不启用调试、显式启用仅使用运行目录内 Unix socket、不暂停启动、拒绝
非法开关和已有日志；默认路径仍传播 QEMU 退出码，不打开 TCP 监听。
