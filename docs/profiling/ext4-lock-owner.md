# ext4 锁持有者测量设计

## 问题与当前证据

状态：L 正常结束后运行确定性 RED，再实现事件；4 项真实锁回归与 5 项绘图
测试已全部 GREEN，含 profile 的 ax-fs-ng 全套 102 项通过。M 窗口已正常结束，
五项哈希校验与离线 fsck（退出 0）通过。

当前 J 窗口的 ext4 锁占 mutex 等待 86.04%，最长一次 rustc 在
`fstatat → lookup → Ext4Filesystem::lock` 等待约 15.41 秒。但这描述的是
等待者，不证明持锁者正在执行 metadata 或 lookup。

当前 `fs/ax-fs-ng/src/fs/ext4/rsext4/fs.rs` 的 Ext4 事件在获取锁前开始，
随 Ext4Guard 释放而结束，包含等待、持锁操作和解锁成本；Ext4LockWait
只测获取等待。两者使用不同抽样倍率，不能直接相减得到精确持锁分布。
此外 `sync_to_disk`、`shutdown_filesystem` 直接使用 inner mutex，没有
经过 Ext4Guard。这是决定下一个锁拆分优化之前需要补齐的观察缺口。

参考 MOSS `moss-smp-scheduler-optimization` 的
`docs/profiling/riscv-buildstorm-latest-bottleneck-report-20260724.md`：
该报告对应样本提交 `97196f4195e6`，明确区分 AddrSpace 等待者和持有者，
要求先测 owner-side hold time 再改锁策略。当前 StarryOS 使用 rsext4，
且工作负载与该报告不同，不能直接迁移其 ext4/AddrSpace 优先级或时间比例。

## 所选边界与替代方案

保留现有事件编号和 inclusive 含义，新增 ext4 锁持有时间事件，而不是
改写旧事件语义或用独立抽样总和相减。持锁 scope 在获取 inner 后开始，
在 inner 解锁前结束；这必须依靠明确的 guard 字段释放顺序，并由测试验证。
sync/shutdown 复用同一个 Ext4Guard 入口，避免遗漏主要同步持有者。

同时可独立添加 Ext4Disk::flush 延迟事件：已有 BlockRead/BlockWrite
不能覆盖设备 flush，不能把提交屏障时间都算成普通写请求。若实施该项，
为其分配单独编号和折叠栈，不能与 write 图直接相加当墙钟比例。

相比只读现有等待图，新事件能回答谁长时间阻塞其它任务；相比直接拆锁，
它不改变事务、inode 一致性、磁盘格式或同步语义。事件会增加少量采样成本，
需要记录倍率、丢样和开关条件；无 profile feature 时不增加 scope 或计时。

本轮使用 8=`Ext4LockHold`、9=`BlockFlush`，两项均先按倍率 1 记录，以便
定位少见的长持锁/屏障延迟；旧 Ext4 inclusive 仍为 1/16，含义不变。
新增事件对应的诊断窗口不能把活跃率变化直接归因于写回优化，需单独记录
额外观测成本和 dropped_wait/dropped_pending。事件表容量随最大事件编号扩展。

## 实施前和验收门槛

- 跨层诊断接口涉及 ax-sync 事件枚举、ax-fs-ng guard、Starry profiler
  数组边界/抽样率和 host renderer，按共享接口变化处理，合入前需要边界审核。
- 不改变旧 1–7 号事件。新增编号必须扩展 profiler event_ticks，防止越界。
- 回归用真实锁和 hook 验证：hold begin 时锁已持有、hold end 时尚未解锁；
  inclusive end 在解锁后；失败和提前返回也结束 scope。
- renderer 对新记录必须输出独立 ns 总量、最大延迟和火焰图，旧记录继续解析。
- 运行相关 cargo fmt/clippy 和默认、profile、host-test 配置测试。
- 独立 600 秒 QEMU 窗口检查所有 CPU、丢样、原始记录和离线 fsck；保持 tg-xtask
  和源码归档复用，不在正在测量的窗口中编译宿主内核。
- 最终按持锁累计时间及最大值选择 metadata/read/write/create/unlink/sync
  中的目标，再设计锁拆分或 I/O 合并；不把等待方的宽框当作持有者证据。

## 2026-09-10 实施记录

`Ext4Guard` 按 hold scope → inner mutex guard → inclusive scope 的字段顺序
释放。hold 在锁获取后创建，sync/shutdown 改走相同 lock 入口；未修改其写盘
顺序、屏障或错误传播。Ext4Disk::flush 的新 scope 覆盖底层成功和错误返回。
profiler 事件表按新增末尾枚举值扩展，原始头报告两项倍率 1；renderer 保留旧图，
分别导出 ext4-lock-hold、block-flush-sync 的 ns 总量、最大值、每 CPU 和折叠栈。

先运行 `cargo test -p ax-fs-ng --features host-test,ext4,vfs,profile --lib profile_tests -- --nocapture`：
退出 101、4/4 失败，普通 lock 只有 inclusive 首尾，sync/shutdown/失败 sync
的锁事件为空。真实锁通过 try_lock 确认每个 hook 时刻的占用状态，不解析源码文本。
实施后相同 4 项通过；全套 `cargo test -p ax-fs-ng --features host-test,ext4,vfs,profile --lib`
102/102 通过，包含 5 项真实 ext4 镜像/fsck 回归。

`python3 apps/starry/macos-selfbuild/tests/test_render_kernel_profile.py` 先退出 1，
新测试因缺少 ext4-lock-hold 失败，加入事件映射后 5/5 通过。
`cargo xtask clippy --package ax-fs-ng --package ax-sync` 13/13 通过，fmt 通过。
以上设置项目 TMPDIR；原始失败输出已完整展示，没有在活动 QEMU 窗口中运行宿主构建。

这是诊断覆盖扩展，未改变 syscall 返回值、持久化格式、锁所有权或事务模型；
不构成一项新的用户态 ABI 优化，也不以主机测试代替实际 QEMU 采样验收。

组合测试 clippy 曾因固定长度 `chunks_exact(2)` 返回 101，完整诊断已展示；
按工具链要求改用 `as_chunks::<2>()`，没有添加 allow。相同 4 项回归与
`cargo clippy -p ax-fs-ng --features host-test,ext4,vfs,profile --tests --no-deps -- -D warnings`
均通过。ax-sync 的 host-test/profile 配置 7 项单元、2 项集成、6 项文档测试通过。
实际 aarch64 guest-profile/smp 内核 clippy 和固定配置 `cargo xtask starry build`
通过；新 ELF/BIN 在 kallsyms 刷新结束后独立保存。

诊断窗口 M：`target/profiling/arceos-helloworld/starry/ext4-owner-window-600/`。
ELF SHA-256 `451a654304cf97ef9eb67038375b288ea2d4521ff80c63130ecb56dc4048074c`，
BIN SHA-256 `fbed4ea793fb3fa7ac42604deab87c978ab1b325dac12a3471e25c95f3962924`。
沿用 L 正常关闭、fsck 返回 0 的同一根文件系统及冻结 tg-xtask/源码归档。

## M 实测结果

606 秒窗口正常返回预期 rc=124，启动 39 个编译单元、36 个不同 crate；未完成
整个构建。采样覆盖 606515324128 ns，47406 个 CPU 样本中 14136 个活跃，
活跃率 29.8190%；所有 CPU/wait/pending/skipped 丢样为 0。额外诊断探针与
连续运行磁盘状态不同，不能把高于 L 的活跃率/进度当作一项优化的确定收益。

全部 mutex 折叠栈累计 2403415882864 ns，其中 ext4 锁叶节点
1867249098880 ns（77.6915%），CachedFile::read_at 364383423152 ns
（15.1611%）。用户缺页调用链 597329052544 ns（24.8533%），与前两类重叠。

event 8 实际持锁 135667 次、累计 485306695744 ns，按直接持锁调用方聚合：

| 持锁调用方 | 累计秒 | 占持锁时间 |
| --- | ---: | ---: |
| sync_to_disk | 144.485168 | 29.7719% |
| FileNodeOps::read_at | 135.096192 | 27.8373% |
| Inode::lookup_locked | 99.695761 | 20.5428% |
| set_len | 36.766890 | 7.5760% |
| metadata | 27.907210 | 5.7504% |
| create（不含随后独立 sync） | 22.226224 | 4.5798% |

用户缺页路径持锁共 102703534080 ns（21.1626%），与上表 read_at 重叠。
单个最宽持锁栈是 rustc 文件缺页 → populate_page_window → Inode::read_at，
79183814480 ns（16.3162%）；紧随其后是 cargo openat → create → sync_to_disk，
75651290672 ns（15.5883%）。因此缺页优化重要，但不是唯一或过半的持锁来源。

最长 ext4 持锁 7.609398048 秒来自 opt cgu.02：openat → open_file → create →
sync_to_disk → Ext4Filesystem::lock。最长 mutex 等待 19.191633728 秒则来自
cargo：sys_openat → with_fs → fs_context.lock，不是 ext4 锁本身。
两者已用本窗口 ELF 和 raw 记录地址符号化，不能仅按最大值将其混为一类。

设备 flush 9168 次，累计 17462273040 ns、最长 150030256 ns；块读
332918407824 ns、块写 102864697920 ns。sync 持锁远高于 flush 本身，
需要追踪同步内部的数据/元数据读写，不能把全部 sync 时间归咎于 flush 屏障。
这些都是累计并可能嵌套的时长，不能相加作为墙钟比例。

已实际打开检查 ext4-lock-hold、block-flush-sync、cpu-active、mutex-wait
四张 PNG；SVG 与全量折叠栈保存在 M 的 rendered/。debugfs 导出只有属主修改
权限提示且退出 0，完整原文已展示；导出五项 SHA256SUMS 通过，e2fsck -fn 退出 0。

CPU 活跃图的叶节点另行统计：用户态 5197/14136（36.7643%）；主要内核叶节点为
exit_preemption 1191（8.4253%）、memset 1097（7.7603%）、find_free_area 987
（6.9822%）、spin_release 819（5.7937%）、页表 unmap 627（4.4355%）、map 497
（3.5158%）。这些以活跃样本为分母，不包含 idle，不能替代等待时间图来解释
约 70% 的 CPU 空闲，也不能与缺页的 inclusive 栈百分比直接相加。
