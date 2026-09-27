# 编译低 CPU 利用率：任务放置实验

## 问题和范围

2026-09-09 的 AArch64 QEMU TCG、8 vCPU、8 GiB 实验中，StarryOS 使用根文件系统内
可复用的 tg-xtask 编译 arceos-helloworld。600 秒上限的旧内核窗口实际采样
615.843949328 秒，记录 49,065 个 CPU 样本，其中 43,185 个为 idle；没有丢样本。
宿主 QEMU 的一个 vCPU 线程累计接近 95% CPU，其余约 2%–3%。

这轮先验证新任务/唤醒任务的 CPU 放置，不修改页缓存容量、文件系统锁、显式迁移、
Cargo 并行度或 QEMU 加速器。成功标准是相同冷构建窗口中多核实际执行、编译进展
和等待形态改善，且启动、CPU affinity 和上下文切换约束不被破坏。

## 当前证据

短窗口原始结果在
`target/profiling/arceos-helloworld/starry/baseline/window-600/artifacts/arceos-helloworld-profile/`，
同级 `rendered/` 保存 CPU、非 idle CPU、mutex、off-CPU 和 I/O 火焰图。
结果文件的 `SHA256SUMS` 已全部验证。

| 观察口径 | 最大热点 | 解释 |
| --- | --- | --- |
| CPU 样本 | idle 43,185 / 49,065（88.0%） | 约 0.96 核非 idle 执行，不能用宿主整机 CPU 百分比替代 |
| mutex 累计等待 | rustc 缺页 → CowBackend::populate → FileBackend::read_at：2,151.207 秒（66.44%） | 内联的 CachedFile::read_at 持有共享 io_lock，涉及多个任务的累计等待 |
| page-cache 累计区间 | 同一 rustc 缺页读取路径：2,405.345 秒（82.07%） | 区间包含锁和下层 I/O，不能与 mutex 数值相加 |
| 非 idle CPU 栈 | rustc 用户态、find_free_area、munmap 系列 | 减少空闲后再判断地址空间扫描的优先级 |
| off-CPU | ctrl-c 线程长期 futex 睡眠 | 正常后台等待，不能因图宽就认定为编译瓶颈 |

CPU 栈中的用户态被聚合成 `[user-space]`，这里没有 rustc/LLVM 用户态函数级解析。
`compile_units=10` 表示日志中已启动的单元，不代表完成 10 个单元或线性速度。

## 设计与替代方案

内部依据是 ax-task 的 `select_run_queue`、`select_wake_run_queue`、
`ax-runtime::mp` 启动次序和 `ax-ipi::is_cpu_ready` 的发布语义。
此前 MOSS 的 `docs/smp-scheduler-optimization-migration.md` 提醒：idle 既可能是
队列空而任务全阻塞，也可能是任务分布失衡，必须结合等待栈和实际 CPU 分布判断。
这不是 Linux 调度算法的移植，不新增 Linux ABI；任务允许 CPU 集合保持原约束。

| 方案 | 判断 |
| --- | --- |
| 保持创建者 CPU/唤醒者 CPU 优先 | 便宜但可把所有后代和 I/O 完成后任务集中到一个队列 |
| 增加 Cargo jobs、固定 CPU affinity | 改变工作负载且不能修复内核放置问题，本轮不采用 |
| 空闲核主动窃取任务 | 引入更多跨队列锁和运行中任务迁移，本轮不采用 |
| 新任务在就绪 CPU 间轮转，唤醒优先原 CPU | 复用现有选择器和队列边界，作为本轮候选 |

新任务必须取 `affinity ∩ spawn_ready_cpus`，不能直接轮转编译时 CPU_CAPACITY。
设备后台任务在次核启动前就会创建。第一次候选在 `AxRunQueueRef::add_task` 读空地址，
说明此前的静态 `MaybeUninit` 数组不具备安全的远程就绪查询能力。

用同一张 AtomicPtr 运行队列表表达“未发布/永久有效”，不另建重复的 online 状态：
所有者构造完运行队列后 Release 发布，远端 Acquire 读取；实际队列操作仍由既有锁保护。
IPI 构建还要求远端 `is_cpu_ready`，当前 CPU 则允许接收本地 bootstrap 任务。
没有可用 affinity 交集属于既有 infallible spawn API 的调用不变量失败，不越过 affinity
选择猜测的 CPU。此设计不提供 CPU hotplug；若引入下线，必须先关闭远程准入再撤销发布。

## 验证与可恢复性

- 新增 `new_tasks_skip_secondary_cpus_before_scheduler_publication`，先验证旧选择器
  稳定选择 CPU 1 而非唯一就绪 CPU 0，再应用交集修复；同一测试转绿。
- `cargo test -p ax-task --features host-test,multitask,irq,preempt,smp,sched-rr --lib`：59 通过。
  使用 native Cargo 是因为当前 xtask 的测试入口不提供此 host-test feature 组合。
- `cargo fmt --package ax-task` 和 `cargo xtask clippy --package ax-task`：70 项检查全部通过。
- StarryOS 保持应用配置，通过 `cargo xtask starry build` 构建，保存匹配 ELF/BIN。
- 正常进入 QEMU Starry shell 后，再启动 600 秒工作负载；早期启动失败不算性能结果。

旧窗口正常 sync/poweroff 后，e2fsck 仍发现 7 个异常 inode；原因待单独核查。
先导出并验证结果，再保留
`tmp/axbuild/rootfs/rootfs-profile-before-inode-repair.img`，随后仅修复实验镜像，复查通过。
修复删除的目录项为本轮 5 个结果文件和 2 个临时编译文件，不影响已导出的证据。

后续 guest runner 保持相同冻结源码、tg-xtask、冷清理和 600 秒上限，移除了 shell 管道
以及隐藏错误的重定向。输出先写入 run.log，定期直接打印编译进展，结束后打印原始日志。
这个观测脚本变化必须记录为 A/B 限制；重复基线应使用同一新版 runner。

## 就绪发布候选结果

`scheduler-ready-window-600` 实际采样 610.635920896 秒，无 CPU/wait/pending 丢样。
CPU 样本 48,335，idle 39,754（82.2468%），非 idle 8,581，约 1.42 核在执行。
相比旧窗口的 88.0159% idle，方向有改善，但 CPU 低利用率尚未解决。
8 个核均有活跃任务，CPU 2 非 idle 占比 63.57%，其它核 7.80%–16.58%。
窗口进入 14 个编译单元，rc=124，未编译完成；不能解释为整体加速 40%。

新 mutex 等待合计 2,781.857 秒，rustc 缺页读取路径 1,045.263 秒（37.57%），
动态链接器同路径 993.774 秒（35.72%），合计 73.30%。page-cache 区间合计
3,501.603 秒，其中两条相同路径占 84.62%。这些均为多任务累计，不相加为墙钟时间。
去掉 idle 后，三个 munmap 全表扫描叶子栈占 1,311 / 8,581（15.28%）。
后续局部优化设计见 [vma-range-scans.md](vma-range-scans.md)。

按 mutex 原始调用地址（不按任务拆分）聚合，`0xffffffff800745e4` 占
2,387.594 秒，即所有 mutex 等待的 85.8274%。匹配本轮 ELF 的反汇编表明，
该位置位于 FileBackend::read_at 内联的 CachedFile::read_at 循环中，紧接
profile_scope，随后调用 mutex_acquire；对应 cache/mod.rs 的 `shared.io_lock`。
缓存命中也必须先获取此锁。它还串行化写入和截断，不能仅凭等待占比直接移除；
尤其要保持截断失败回滚时不暴露中间缓存状态。

## 根文件系统重复使用问题

候选正常退出后再次发现 inode garbage。现已用批量删除回归确认根因：
rsext4::free_inode 将 i_block 清零，却保留 EXTENTS 标志；e2fsck 1.47.2 对
空闲 inode 也验证 extent 头，同块多数无效时会误报仍有效的相邻 inode。
依据为 [e2fsprogs v1.47.2 inode.c](https://github.com/tytso/e2fsprogs/blob/v1.47.2/lib/ext2fs/inode.c)，
check_inode_block_sanity / extent_head_looks_insane / ext2fs_get_next_inode_full。
不是数据块已损坏的证据；不应对这种现场直接自动清除所有被报告的有效 inode。

已修复释放时清除 EXTENTS 标志，真实 Linux mkfs 镜像上创建 64 个文件、删除 48 个，
旧实现稳定让 15 个有效邻居被 fsck 报错；修复后 fsck 通过，重挂载验证剩余内容。
`cargo test -p rsext4 --features host-test`：193 通过、1 个已有外部镜像用例忽略；
`cargo xtask clippy --package rsext4`：3 组通过。测试使用与 xtask std 相同
host-test feature；native Cargo 用于定向选择当前 crate 和回归。

本轮先完整备份到 `tmp/axbuild/rootfs/rootfs-profile-scheduler-ready-preserved.img`。
验证 inode 位图空闲、链接数/长度/块数全零、flags 恰为 0x80000 后，仅清除
24 个空闲 inode 的布局标志（74834、74837–74838、74843–74848、74946–74960）。
没有删除有效文件；随后 `e2fsck -fn` 返回 0，8 个原被误报的有效目录项全部保留。
更早旧窗口的自动清理已发生，但原始镜像备份与导出数据仍保留，可恢复。
