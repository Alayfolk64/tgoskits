# munmap 范围扫描优化

## 问题与成功标准

`scheduler-ready-window-600` 的非 idle 样本共 8,581 个。其中 rustc 的
MemorySet::unmap、AddrSpace::unmap 本体和 memfd::on_aspace_unmap_range 三个
叶子栈分别为 512、400、399 个样本，合计 15.28%。这三层都扫描全部 VMA
（虚拟内存区域），即使 munmap 只撤销其中一个。文件缺页的 I/O 锁等待仍是
更大的 off-CPU 问题；本改动不声称解决该等待。

目标是在真实 MemorySet 上把局部解除映射和重叠查询限制在相关地址范围，
保留边界分裂、页表回收、RSS/VM 统计、memfd 可写共享映射计数和错误行为。
以计数地址类型的比较次数验证不再线性扫描，再运行相同 QEMU 600 秒窗口。
不更改地址分配策略、页缓存、锁顺序、用户 ABI 或编译并行度。

## 设计与替代方案

沿用 MemorySet 现有的非重叠 BTreeMap，新增借用式 `iter_overlapping`，先查
左边界的唯一可能前驱，再按起始地址访问半开范围。空范围不返回任何区域。
AddrSpace 和 memfd 复用该查询，不暴露 BTreeMap 或复制 backend。
完整区域删除使用 BTreeMap::extract_if 的地址范围；边界收缩沿用原算法。
不会增加内存分配、索引、unsafe、锁或第二份映射状态。

保持全表扫描浪费 CPU；仅在循环末尾 break 仍需扫描左侧前缀；维护第二份
interval tree 或 gap 索引超出本轮需要。复用当前 BTreeMap 的有界遍历最小。
内部已检查 MemorySet::unmap/unmap_metadata、AddrSpace::areas_in_range、
memfd::on_aspace_unmap_range 及相关历史；现有 areas_in_range 会全表扫描并复制
backend，不能直接用于无分配计数。已有的区域顺序和不重叠不变量足以定位。

外部依据为 [Rust BTreeMap 文档](https://doc.rust-lang.org/std/collections/struct.BTreeMap.html#method.extract_if)，
查阅版本 1.98.1（48a229cea，2026-09-01）；实际编译以项目固定的
nightly-2026-07-15 为准。extract_if 只访问指定范围，必须完整消费迭代器。
这不是 Linux VMA 算法移植，不改变 syscall 返回值或 flags，不新增硬件能力。
共享接口变更需要对应边界审核；本地实验不代表已满足合入审核要求。

## 验证

先在旧全表遍历上运行 4,096 个区域的确定性计数回归，记录失败；再修复，
验证比较次数、边界邻居、空区间和页表后置状态。随后执行 MemorySet 全套测试、
对应 crate clippy、Starry 构建及 QEMU 冷构建窗口。

已实测：旧实现两种 unmap 各 6,193 次比较，线性重叠查询 10,243 次比较，
三项回归全部失败；有界实现同一测试全部通过（各少于 200 次）。MemorySet
共 11 个测试及 1 个文档测试通过。Starry 构建通过，保存 ELF SHA-256
`0f22fde8b8a80e076fe111605eb906b7ddebbdf2eb34549c7fa3fc8d91e9f014`，
BIN `30a1c18e557c3721be05f666b02947f180cd5c3a966afd9e3115c4b4bbd8d5da`。

`cargo xtask clippy --package ax-memory-set --package starry-kernel` 前 22 项通过，
第 23 项 sg2002-wifi 卡在 AIC8800 固件下载，显式终止，退出 143；未完成完整矩阵。
针对实际配置执行
`cargo clippy --no-deps -p starry-kernel --target aarch64-unknown-none-softfloat --features smp,guest-profile -- -D warnings`
通过；`--no-deps` 与 xtask 使用方式相同，只对当前 crate 报 lint。
此前未带此参数的一次检查在未修改的 somehal gic/v3.rs:164 遇到 unnecessary_cast。

宿主内核测试限制：旧 std_tests:: 过滤器选中了 0 项，不能算验证；完整默认测试
SIGSEGV，GDB 定位为 tty 测试经 ax-task 锁进入真实 cpu_local 抢占路径，宿主未初始化。
启用 `ax-task/host-test` 后运行 166 项，159 通过、7 失败。失败涉及未初始化
Network service、AreaNotInstalled、waitpid(i32::MIN) 错误期望、PID/进程拓扑状态；
本轮不宣称完整内核宿主测试通过，也不修改无关测试去掩盖这些问题。
曾尝试加 `ax-sync/host-test`，但它不是 starry-kernel 直接 feature 路径，Cargo
返回 101，随后移除该无效参数。所有原始错误均在执行记录直接输出。
定向执行 `cargo test -p starry-kernel --lib --features ax-task/host-test mm::`：
实际选择并通过 17 项内存子系统测试。

## 600 秒窗口结果与下一热点

`vma-range-window-600` 正常结束，实际内核采样 611.698762208 秒，退出 124，
进入 11 个编译单元，未完成编译。冻结源码、tg-xtask、runner、8 vCPU 和冷清理
与上一轮相同；本轮还包含前述 rsext4 空闲 inode 标志修复。5 个证据文件的
SHA-256 全部通过，正常退出后 `e2fsck -fn` 返回 0，无 garbage inode。

CPU 48,467 个样本中，idle 41,306（85.2250%），非 idle 7,161，约 1.18 核。
因此不能宣称该候选已改善整体吞吐：上一轮是 14 个编译单元、82.2468% idle。
不过优化目标的三个 unmap 叶子栈合计仅 7 个样本（MemorySet 3、AddrSpace 2、
memfd 2），已从上一轮的 1,311 个样本降下来。rustc 用户态 1,551 个样本，
比上一轮 1,141 个更多，但用户态工作量与编译单元数不是同一指标。

mutex 累计等待 2,872.066 秒，动态链接器缺页读取 1,193.224 秒、rustc 同路径
1,034.624 秒，两者合计 77.5695%。全任务 `FileBackend::read_at` 同一锁调用点
`0xffffffff802400a4` 合计 2,459.689 秒。累计等待可以跨线程重叠，不是墙钟。
已实际查看本轮 mutex 和去除 idle 的 CPU 火焰图，保存在实验目录的 `rendered/`。

新的 CPU 最大内核叶子是 find_free_area：全任务 1,280 个样本，其中 rustc
1,278（非 idle 的 17.8467%）。sys_mmap 使用 `.or(fallback_search())`，即使
hint 搜索成功，仍无条件再次从 base 搜索；mremap 同文件 helper 已使用惰性
`or_else`。下一步先修复这次无效扫描，不修改映射地址选择和 errno。
