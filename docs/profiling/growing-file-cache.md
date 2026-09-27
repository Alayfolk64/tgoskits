# 随文件长度维护缓存保留目标

## 问题、范围与选择

重构后 [600 秒窗口](ext4-background-window.md) 的最大单条 ext4 持锁栈
来自前台脏页淘汰（76.899106 秒）。当前 `CachedFileShared::new` 仅按打开时
长度设置缓存目标，新建输出增长后仍为 512 页。较早的
[大文件缓存实验](large-file-cache.md) 明确将动态增长列为非目标，故这里不是
已有能力的重复实现，也不把其遗漏包装成原先已经承诺的 ABI 修复。

成功标准：同一磁盘文件在无内存压力下，从空文件连续写入超过 2 MiB，
其完整工作集在既有上限以内时，不因打开时长度而触发前台脏页淘汰；显式
writeback 后每字节内容一致，失败的长度变更不发布错误容量，truncate 及
内存文件行为不变。最终仍需同基盘 QEMU 窗口验证整机收益。

本轮复用现有 512 页下限、256 MiB 上限、按需分配、后台回写和 clean-page
reclaim；只让保留目标随已确认的共享文件长度更新。不新增 API、依赖、
磁盘格式、锁种类或 worker，不推迟 backing `set_len`，不改变 stat 可见大小。

| 方案 | 取舍 |
| --- | --- |
| 保持固定打开时容量 | 继续逐页同步淘汰新输出；已由调用栈确认该路径昂贵 |
| 统一提高全部文件容量 | 小文件也增加可保留量，且不表达文件增长的实际边界 |
| 随共享长度维护现有容量策略 | 采用；维护已有策略，不引入新的缓存或所有权 |
| 全局脏页预算和按压力回写 | Linux 的完整方向；还需 dirty accounting、节流与唤醒设计，不与本轮混合 |
| 将全部数据写 I/O 拆出 ext4 锁 | 可继续降低后台持锁，但需要新的映射/事务所有权协议，不是修复本项的前提 |

这是涉及内存保留与性能的高风险候选，设计需在合入前独立审查；本地实施
不等于已完成合入或所有内存规模验收。增大的工作集会增加脏页驻留；当前
全局回收只回收干净页，本候选不提供 Linux 等价的全局脏页硬预算。

## 内外部来源

已检查 `cache/mod.rs`、`pages.rs`、`populate.rs`、`resize.rs`、`reclaim.rs`、
现有 eviction/writeback 测试，以及本路径最近 5 个提交；历史
`ed3d4a1e6` 的构造器使用固定 512 页。2026-09-10 查询开放 cache PR/issue，
相关 [PR #2015](https://github.com/rcore-os/tgoskits/pull/2015) 的 head 为
`6d5cc09f45a073680a270ae0b6047b24fd9eaff5`：文件清单局限 rsext4，未修改
ax-fs-ng 页缓存容量；PR 说明已撤销延后 set_len 的方案，因为影响 stat。
本轮保持 backing set_len 时序，不移植其被撤销的语义。
[issue #2206](https://github.com/rcore-os/tgoskits/issues/2206) 面向共享 block cache，
不是本轮文件页缓存的长度策略；[issue #2209](https://github.com/rcore-os/tgoskits/issues/2209)
属于 ext4/JBD2 能力矩阵，不声称本轮完成该矩阵。

Linux 源码固定 `980ab36ae5972c83f683b939e50c469c4947229e`：

- [`generic_perform_write`](https://github.com/torvalds/linux/blob/980ab36ae5972c83f683b939e50c469c4947229e/mm/filemap.c#L4328-L4360)
  在写入循环中调用脏页平衡，而非按打开文件时的长度选固定缓存容量。
- [`balance_dirty_pages_ratelimited_flags`](https://github.com/torvalds/linux/blob/980ab36ae5972c83f683b939e50c469c4947229e/mm/page-writeback.c#L2025-L2115)
  按写回域与脏页计数触发检查、回写和必要等待。
- [`shrink_folio_list`](https://github.com/torvalds/linux/blob/980ab36ae5972c83f683b939e50c469c4947229e/mm/vmscan.c#L1367-L1393)
  遇到普通文件 LRU 上的脏 folio（一个或多个页的缓存单位）先标记回收后再激活，
  此分支不直接逐页同步写盘。
- [官方 VM sysctl 文档](https://docs.kernel.org/admin-guide/sysctl/vm.html#dirty-background-bytes)
  区分后台脏页回写阈值与写入进程参与回写阈值。

借鉴的是避免按陈旧容量强制前台逐页写回，不声称实现了 Linux 的脏页节流。
参考源码 commit 与本次 Linux QEMU 发行版 6.18.35 不相同，不混作同一版本。

## 状态与验证门槛

共享长度仍由已有 `io_lock` / update owner 保护变更；在成功发布增长、
截断或失败恢复后的实际长度时同步维护容量。锁顺序保持 `io_lock → page_cache`。
容量变更只改策略，不通过 LRU resize 隐式销毁 frame；truncate 仍走退休页和
映射失效确认。内存文件保持无界策略，不因增长被误改成磁盘容量。

先添加并运行真实 CachedFile 的确定性失败回归，再修改实现。所有当前
生产重构已有三轮检查；新实现完成后重新执行三轮：

1. 状态、失败恢复、页所有权审查；带测试目标的 host Clippy。
2. 项目 `cargo xtask clippy --package ax-fs-ng` 的目标/feature 矩阵及锁序审查。
3. 最终源码 diff、fmt、目标 Starry 内核构建和调用方复核。

三轮完成前不执行修改后的测试；Linux 正在采样时不进行编译或测试。
回归需覆盖连续 write/append、显式扩展/截断、长度变更失败、无额外初始分配、
既有脏页/映射失败保留规则。门槛后的实际性能结果见下文。

## 实施与三轮静态检查记录

2026-09-10 22:01 在此前重构三轮放行的生产版本上运行新回归：5 项中
3 项失败、2 项通过，退出 101。增长 write/append 在第 513 页各触发
1 次前台写回；显式扩展后目标仍为 512，而非已有上限 65536。
原始完整输出保存为 `tmp/ext4-final-static.ryo14j/growth-red.log`；
改动前缓存源码快照为同目录 `growth-before.tar.gz`。

22:02 实现共享长度发布后更新保留目标：构造、增长、truncate 和失败恢复
复用同一计算；仅磁盘文件参与。`CachedPages::set_reclaim_target` 只改目标，
不调用有隐式逐出的 LRU resize；原映射失效/脏页恢复协议不变。

| 轮次 | 时间（Asia/Shanghai） | 检查与结果 |
| --- | --- | --- |
| 1 | 22:02:31 | `cargo fmt --package ax-fs-ng`；host-test,ext4,vfs,profile 的 lib/tests Clippy，`-D warnings`，退出 0。复核边界、容量计算与真实回归接线 |
| 2 | 22:02:53 | `cargo xtask clippy --package ax-fs-ng`，8/8，退出 0。检索全部长度写者，均遵循已有 IO owner→page_cache；不在已有 cache guard 内递归加锁 |
| 3 | 22:04:01 | `cargo fmt --all --check`、`git diff --check`、实际 AArch64 SMP=8 Starry profiling 内核构建，均退出 0。检查 truncate/rollback/内存文件路径和调用方，无新生产修改 |

第三轮 release 构建 11.98 秒，注入 14050 个 kallsyms。
ELF SHA-256：`1710b2fe4aaa85991cf9d6348e51556ef565bb6f50f19a3077da414f576e8fa8`；
BIN：`668db3bfd0bc05176bfcd8de7603198fe2a17fa377a150f14d615821bac541e7`。
保留 core/memchr 的 future-incompatibility 提示，未通过 allow 屏蔽。
22:04:01 三轮全部完成后，才放行修改后行为测试；此时尚无性能结果。

门槛后相同 5 项回归全部通过，完整 ax-fs-ng lib 测试 218/218、无忽略，
均退出 0。包括既有脏页写回失败、忙映射保留、truncate 失效重试和长度回滚。
`growth-green.log` 保存同一回归成功侧，`growth-clippy.log` 保存两项 Clippy 完整输出。

最终源码快照 `tmp/ext4-final-static.ryo14j/source-growing-cache.tar.gz`，
SHA-256 `eff2e89b3a9826f7d054d42bb6f793df1806b1e119c2f70e96687bd866ff96c6`，
tar 全量比较退出 0。增长前缓存快照 SHA-256：
`2a59c4dd0cda0394e5bc3b080d66bf6f0160fa39cd997124e6d9c18247661469`。
新运行目录 `target/profiling/arceos-helloworld/starry/growing-cache-window-600`；
启动前完整根盘 SHA 与同一冻结基盘 `9968004595dec398480417e210d64f9ed509801d4325d4ab22f43951983f5ea3`
一致，只读 fsck 退出 0。上一轮结束盘保存在
`tmp/axbuild/rootfs/rootfs-profile-ext4-background-window.img`，没有删除。

## 600 秒窗口结果

2026-09-10 22:18 正常结束，窗口实际 611 秒，`rc=124` 是预定 timeout，
`build_completed=false`、`tg_xtask_reused=true`。8c8g、TCG multi、Cortex-A53、
NVMe、冻结工作负载与前一轮一致。启动 55 个编译单元、52 个不同 crate；
这不是完成数量，也没有完成整个 arceos-helloworld 编译。

| 指标 | 前一轮后台重构 | 本轮增长缓存 |
| --- | ---: | ---: |
| 实际窗口 | 607 秒 | 611 秒 |
| 已启动单元 / 不同 crate | 52 / 49 | 55 / 52 |
| 非 idle CPU 样本比例 | 38.8718% | 42.1067% |
| ext4 累计持锁 | 249.764290 秒 | 164.411787 秒 |
| ext4 累计等锁 | 943.763106 秒 | 396.234723 秒 |
| 最长单次持锁 | 12.850828 秒 | 5.198240 秒 |
| 前台 page_or_insert → write_locked 持锁栈合计 | 79.967705 秒 | 未记录到 |

本轮 47382 个 CPU 样本中 19951 个非 idle；丢样、pending 丢失及 skipped 均为 0。
累计持锁降低 34.17%，累计等锁降低 58.02%，CPU 活跃比例提高 3.23 个百分点。
后台页写回的主要持锁栈仍为 23.068652 秒。单窗口进度略增，尚无重复测量、
完整编译时间或稳定整体加速倍数结论。新增缓存驻留的内存代价仍需压力验证。

最大 ext4 持锁来源变为 `Inode::lookup_locked`：88.787315 秒，占 54.00%；
其次 `write_locked` 为 23.078986 秒、`read_inode` 为 21.930866 秒。
lookup 中最大单栈是 busybox 的 fstatat 路径（22.905055 秒）。现有采样不记录
查找成功/NotFound 结果，不能据此直接断言全部 lookup 都是重复负查找。

原始、folded、SVG 与 summary 均在上述运行目录；导出后 SHA256SUMS 5/5 通过，
只读 `e2fsck -fn` 退出 0。导出 debugfs 有 8 条因宿主非 root 无权恢复属主的
EPERM，已原文展示，导出内容全量哈希通过；fsck 保留 extent tree 可缩窄提示，
未写盘。49 条 IllegalInstruction 与既有 Cargo/OpenSSL 指令能力探测一致，
未观察到新的内核崩溃。当前固定工作盘保存本轮结束状态，后续冷窗口不能直接复用。

同硬件最新 Linux 窗口见 [Linux 对照](linux-qemu-comparison.md)：604 秒、
177 个已启动单元 / 171 个 crate，CPU 活跃 71.8212%。双方都未编译完成；
不同 OS 根盘准备内容以及 Linux 未展开用户栈的限制仍保留，不能转成完整编译速度比。
