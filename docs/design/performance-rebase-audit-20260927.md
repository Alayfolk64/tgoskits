# 性能优化变基迁移核对

## 1. 源码与实际生效范围

冻结检查点 `82e869b6a71e0c7a7e95f9e4539d62f13812c830` 确实缺少四项生产效果。后续迁移已经将这些消费者接入当前 `VmaRoot`、`MappingSlot`、`PageObject`、文件缓存端点及延迟回收机制，并补齐旧接口在新版中的对应关系。接入状态、功能验证和性能验收分别记录；源码迁移不能替代完整编译或约十五分钟的实测目标。

### 1.1 冻结来源与远端基线

旧性能备份为 `snapshot/performance-before-rebase-20260927`，提交 `3e99d0cc28c0aef6d40cca21f1feee513618a290`；其实验基线为 `affddc3fecec02b31df94573063e4840433c9ebe`。当前工作树位于 `/home/wuxun/Projects/tgoskits-performance-dev-20260927`，分支为 `integration/performance-dev-20260927`，已包含本次变基基线 `4e2e62f471e93aba5d4021eb3398b9657d07a5d2`。

2026-09-27 再次获取远端后，`upstream/dev` 为 `212ba669c2385b4a7b5394bedfdb6a40c9a65308`，比原基线新增一个 `feat(axbuild): support external Starry QEMU runs (#2509)` 提交。已将集成分支的十九个提交变基到该提交，迁移提交为 `ed1e26bfd`；`git merge-base --is-ancestor upstream/dev HEAD` 返回 0。唯一冲突是 procfs 的覆盖率导出与 profiling 入口，合并后保留各自条件编译与节点；没有 Cargo.lock 冲突或手工合并锁文件。变基前完整改动保存在 `snapshot/performance-port-before-latest-dev-20260927`，提交 `8d7ee75bc`。新增 dev 内容发生在已测运行基线之外，不能用它解释已测耗时。

对旧实验的全部 616 个变化路径进行了 Git 对象比较：412 个与备份完全一致，50 个与旧实验基线一致，154 个采用不同的当前表示。这是路径与对象分类，包含移动和删除；不能换算成优化迁移百分比。

### 1.2 按生产消费者核对

下面按原定十二组记录当前生产消费者及保留的新版机制。新增迁移代码尚未执行编译、静态检查或运行测试；本阶段按用户要求先完成接入和变基，再开始验证。旧实验的接口没有机械恢复为第二套所有者。

| 组 | 当前状态 | 源码依据与缺口 |
| --- | --- | --- |
| P01 空闲区间索引 | Starry 消费者已接入 | [`VmaNode` / `FreeAreaSearch`](../../os/StarryOS/kernel/src/mm/aspace/vma.rs) 在持久 AVL 路径复制和旋转时维护 `first_start`、`last_end`、`max_gap`，`VmaMap::find_free_area` 剪枝并保留原 first-fit、对齐及溢出规则。`MemorySet` 继续使用现有 `GapIndex`。 |
| P02 抢占退出与放置 | 使用新版等价及替代机制 | [`PreemptionState::finish`](../../components/cpu-local/src/preempt.rs) 保留无 pending 快速退出；[`exit_lock_preempt`](../../os/arceos/modules/axruntime/src/guard/mod.rs) 仅在最终 pending 退出关中断。新线程进入 [`stage_new_thread`](../../components/ax-task/src/sched/system/task_system/delivery/admission.rs)，由当前容量/需求放置策略选 CPU。旧无条件 previous-CPU 唤醒策略被新版亲和性及负载策略取代。 |
| P03 缺失页表发布 | 使用新版等价机制 | [`plan_map_page` / `PageTableMapPlan::prepare` / `try_map_page_with`](../../memory/page-table-generic/src/table.rs) 分别捕获无分配计划、锁外初始化未发布页表后缀、重走并验证 root/parent/空项后一次发布。失败返回 move-only deposit，在临界区外释放，不恢复旧安装调用。 |
| P04 私有缺页锁外准备 | 当前消费者保留 | [`prepare_page_fault`](../../os/StarryOS/kernel/src/mm/aspace/mod.rs) 与 [`CowBackend::prepare_fault`](../../os/StarryOS/kernel/src/mm/aspace/backend/cow.rs) 继续使用准备、重验证、发布与取消协议。非缓存页保留有界文件读取、`MaybeUninit` 目的缓冲区和只清未读尾部；裸 exec 准备仍批量复制。 |
| P05 共享匿名零页 | 当前消费者已接入 | [`ZeroPage`](../../os/StarryOS/kernel/src/mm/aspace/backend/zero.rs) 由内核镜像永久拥有只读 4 KiB RAM；源内 PageObject 由 COW 索引保留，匿名基本页读缺页不分配帧、不计 RSS。用户写与强制内核写均先 COW；THP 和共享匿名分支保持现有行为。零页不进入 `MADV_FREE` 的独占页回收。 |
| P06 并行填充及锁外写回 | 当前消费者已接入 | [`FillOwner::prepare`](../../fs/ax-fs-ng/src/file/cache/fill/mod.rs) 保留锁外填充；[`WritebackPages`](../../fs/ax-fs-ng/src/file/cache/writeback/pages.rs) 固定有限页集合、pin 和写回状态；[`WritebackBatch`](../../fs/ax-fs-ng/src/file/cache/writeback/batch.rs) 在短锁内快照，在锁外执行有界连续 I/O。显式、后台、周期、全局及退休入口共用 `writeback_lock`。 |
| P07 私有文件缓存页共享 | 当前消费者已接入 | [`CachedPageBacking`](../../fs/ax-fs-ng/src/file/page.rs) 独立保留物理分配；[`pin_read_page` / `with_current_read_backing`](../../fs/ax-fs-ng/src/file/cache/mapping.rs) 分离物理生命周期与当前 EOF、epoch、缓存身份的发布许可。COW 接入现有 [`FilePageDomain`](../../os/StarryOS/kernel/src/mm/aspace/backend/file.rs)，不新增全局登记表；读页只读，首次写始终复制。移植 `with_start` 的文件坐标位移；写回保护用 occupied-leaf 查询处理 `PROT_NONE`。 |
| P08 ext4 锁外读取 | 当前消费者保留 | [`read_admitted_inode`](../../fs/ax-fs-ng/src/fs/ext4/rsext4/fs/read.rs) 分离准备、锁外 `execute` 和 inode/mount 重验证；[`Inode::read_at`](../../fs/ax-fs-ng/src/fs/ext4/rsext4/inode/io.rs) 保留内容读取许可，不支持的形态进入显式序列化路径。 |
| P09 日志提交所有权 | 当前消费者保留 | [`PreparedCommit`](../../fs/rsext4/src/blockdev/journal/detached.rs) 与 [`execute_writeback`](../../fs/ax-fs-ng/src/fs/ext4/rsext4/fs/writeback.rs) 保留拥有式提交、完成校验和锁外 abort 持久化；错误不丢失原始设备原因。 |
| P10 独立写入与 inode 预读 | 当前消费者保留 | [`write_extent_inode`](../../fs/ax-fs-ng/src/fs/ext4/rsext4/fs/write.rs) 运行拥有式 extent 写；[`sync_with_commit_gate`](../../fs/ax-fs-ng/src/fs/ext4/rsext4/fs/writeback.rs) 在锁外执行 inode table 预读，再验证并 seal 有限提交前缀，保留 checkpoint 重试及新版关闭许可。 |
| P11 目录与元数据缓存 | 当前消费者保留 | [`DirNode` 缓存](../../fs/axfs-ng-vfs/src/node/dir/cache.rs) 保留正缓存及有界负缓存，权威变更推进 generation，查找结果发布前重验证。ext4 [`lookup_entry`](../../fs/ax-fs-ng/src/fs/ext4/rsext4/inode/directory/mod.rs) 使用已选父 inode 的生命周期对象；目录游标仅在用户复制成功后提交。 |
| P12 批量解除映射 | 当前消费者已接入 | [`unmap_range_deferred`](../../memory/page-table-generic/src/unmap.rs) 复用现有 range walker，一次遍历并转移有界页表 owner，不自行刷新或释放。已发布 COW 解除映射进入该接口，再由 [`MappingMutationContext`](../../os/StarryOS/kernel/src/mm/aspace/backend/mod.rs) 和当前 tagged TLB 回执完成回收；回滚保留原有清理协议。 |

上述状态是消费者级核对结果，不是按 Git 路径估算的迁移比例。P06 保留当前 `dev` 的 16 页/64 KiB 写回上限、后台水位及映射端点协调，不恢复旧 256 页/1 MiB 批次或“所有增长均不自动写回”的策略。P12 保留当前多核确认和隔离所有者，只补充 range walk；没有绕过或并行运行第二套 TLB 协议。

### 1.3 物理所有权与失败路径

共享零页和缓存页复用都需要适配当前用户内存生命周期，不能只在读缺页入口替换物理地址。`MappingSlot`、退休 PageObject 和 `FrameLease` provider 在 fork、失败回滚、移动、解除映射和延迟 TLB 确认期间共同保持真实物理 owner。零页不交给分配器；缓存页仅由最后一个 `CachedPageFrame` 释放给文件系统页提供者。

私有缓存页只适用于已注册 MM、完整且对齐的基本页读/执行缺页。直接文件、未注册 exec 地址空间、ELF 前后缀、部分 EOF、首次写及非基本页继续进入原有复制路径。最终 PTE 发布在持有 MM 元数据时短暂取得缓存索引，校验更新屏障、EOF、epoch 和 backing 身份；页表 deposit 已在此前准备。失效端点在缓存锁外查找 MM，取消与失败释放瞬时 pin，退休回执继续保留物理 owner。

`AddrSpace::write` 改为独占 MM 借用，在实际复制前处理 COW，包括只读 VMA 的强制写。缓存页 COW 源复制取得页级字节锁，并以 volatile 读取避免在共享用户映射可并发写入时新增普通源切片；不宣称整个页的原子快照。历史 `PageCache::data` 接口仍沿用现有内核映射契约，本阶段没有宣称完成所有旧共享内存 unsafe 边界的独立审查。

### 1.4 旧模块与测试对应

旧实验中未由当前模块声明的文件不能算启用状态。保守模块图之后逐项检查实际声明、内联测试模块和 `include!("root.rs")`，避免把测试入口或内核根模块误判成未接入。保留的旧文件仅作 Git 可追溯来源；PR 整理时删除已失去当前所有者的实现。

| 旧表示 | 当前所有者与功能证明入口 |
| --- | --- |
| `cache/inode_index`、`registry`、`retirement` | `CACHED_FILE_BY_INODE`、`cache/reclaim.rs` 和 `resize.rs` 的完整批次恢复；现有硬链接、unlink、退休失败重试和部分失效回归。 |
| `cache/populate.rs`、旧 listener eviction 测试 | `fill/mod.rs`、`with_page_or_insert`、映射端点及现有 pageout、拒绝 truncate、LRU 身份、部分批次恢复测试。不能恢复旧 listener 列表。 |
| 旧 `growth.rs` | 当前有界 retention、水位与映射文件增长回归；增强 `disk_cache_capacity_is_bounded_without_allocating_data_pages`，覆盖实际 resize、失败长度保持和 tmpfs 无界目标。 |
| 旧 `writeback_owner.rs` 与孤立的 batch/progress 文件 | 当前已声明 `writeback/{pages,batch}` 与 `tests/{writeback_batches,writeback_progress}`，覆盖各写回入口、锁外读取进展、红脏重写、短写、错误及轮次释放。 |
| `FsBlockReader` 与 `clean_cache.rs` | `FsBlockDevice::fork_io` 提供同一设备及 coherent cache domain 的独立端点；`BufferedBlockDevice` 替代独立 clean-cache，保持分区坐标和所有权。 |
| 旧 ext4 `sync.rs`、`profile_tests`、inode/metadata 测试 | 当前 `fs/writeback.rs`、`fs/tests/inode_writeback.rs`、`profile.rs`、`persistence.rs`、`sync_policy.rs`、`namespace` 等真实文件系统入口；设备号证明位于 rsext4 当前公开 API 测试。旧 write-sequence 饱和场景由新版 staged sync/提交票据协议取代。 |
| 旧 `MemorySetGapMutationGuard` | 当前 `MemorySet` 预先准备元数据并在后端成功后更新索引，失败保留原有持久状态。 |
| 旧 `cow/`、`fault_tests/` 和 `backend/unmap.rs` | 当前 COW backend 与 aspace 事务测试；新增零页及缓存页真实 axtest，保留已有 fork、THP、回滚、失效回执证明。 |
| 旧空 chroot 的 getrandom 测试 | 已接入 `qemu/system/test-getrandom-chroot` 的根 CMake 分组发现及安装入口；直接系统调用验证脱离 `/dev`、多块读取和错误优先级。 |

硬件清零已移动至 [`ax_cpu::cache::try_zero_page`](../../components/axcpu/src/arch/aarch64/memory.rs)，由 `frame_zero::clear_owned` 在防迁移 guard 内调用。GIC LPI 的独立 64 KiB pending-table stride 及其配置消费者仍接入当前 ITS 初始化。块缓存 pending read、目录读取 owner、元数据快照、随机源、profiling 与板级自编准备保留当前生产入口；现场配置、固件及图片未纳入本次提交。

### 1.5 验证阶段边界

截至迁移提交 `ed1e26bfd`，本次新增测试和修改后的功能测试均尚未运行。源码接入、调用链及失败路径核对、格式/差异检查、最新 dev 变基已经完成；本阶段据此完成用户规定的迁移检查点，随后才能启动项目 `cargo xtask` 编译、静态检查和必要功能验证。完整冷自编、约十五分钟性能目标、PR 拆分和兼容性结论仍是后续交付项，不能用冻结检查点的成功结果替代。格式化使用固定 nightly 的 `cargo fmt`，对 `include!("root.rs")` 下的修改文件补充直接 rustfmt；`git diff --check` 及本次设计文档本地链接检查均返回 0。

## 2. 耗时证据与验收缺口

已测结果证明当前内核可以自编，但尚未达到截图约十五分钟的水平。需要分别核对运行内核、被编译源码、工具链和编译策略，不能把不同工作量的总时间直接当作运行内核回归。

### 2.1 两种构建工作量

前两轮使用相同的当前正常运行内核；第三轮加入诊断采样。每轮均在构建前移走 `target`，保留旧目录以供追溯；准备好的 `tg-xtask` 不在计时区间内编译。

| 运行 | 被编译源码与工具链 | release LTO | Cargo / 全流程 | 实际终态 |
| --- | --- | --- | --- | --- |
| 旧冻结源码对照 | `b609e4f8…`；nightly-2026-07-15 | `false` | 19m07s / 19m45s | 自编、产物校验、返回 Linux 均完成 |
| 当前源码冷编译 | `81e736bd…`；nightly-2026-09-04 | `fat` | 34m11s / 35m19s | 自编、产物校验、返回 Linux 均完成 |
| 当前源码诊断运行 | 相同 `81e736bd…` 与 nightly-2026-09-04 | `fat` | 33m56s / 35m04s | `tg-xtask` 返回 0；随后 Bash 段错误，运行器返回 139，未生成 PASS |

当前源码的最终 `starryos` 单元墙钟跨度约 709.87 秒。该跨度包含等待，不能等同于纯 CPU 时间或全部 LTO 时间。源码、依赖、目标配置和工具链也发生变化；没有同条件 Linux 对照或受控 A/B 时，不能把约十六分钟差额全部归因于 LTO 或某个尚未迁移的优化。截图的缓存状态没有独立证据，不能将其直接标作已确认的冷编译基线。

### 2.2 原生采样和运行器失败

诊断运行保存了前 301.5 秒的 `kernel-profile.raw`，记录未丢失，使用匹配的运行内核 ELF `a0605158…` 解析。采样落在缺页、分配器、块读取、缓存和锁路径。多个最高频分配器 PC 紧接 `msr daifclr, #2`；定时中断可能在关中断期间延迟，因此这些比例不能作为无偏 CPU 时间占比，更不能外推到后面的最终链接阶段。等待事件在不同线程和嵌套操作间重叠，不能加总为墙钟耗时。

编译结束后的原始失败为 `/usr/bin/timeout: the monitored command dumped core`、`Segmentation fault (core dumped)`，并输出 `STARRY-ORANGEPI5PLUS-SELFBUILD-FAIL rc=139`。串口记录显示 Python 采样助手已返回 0，随后 Bash 在等待结束附近收到地址 `0x8` 的 SIGSEGV；根因尚未确认。编译本身完成，取回的 ELF 与 BIN 经同版 LLVM 全量转换比较一致，但整轮运行器未通过，不能标作验收成功。

板子已正常返回 Linux，SSH 可用，boot ID 为 `8c6e65c2-66eb-497d-b045-88e84eac0d3c`。启动检查重放日志、优化 extent tree，并修复 inode bitmap 尾部 padding，fsck 返回 1；这些文件系统修改保留在原始记录中。Linux 正常启动不代替最终迁移后的持久性回归。

后续在消费者迁移及最新 dev 变基完成后，先通过必要功能验证，再进行同条件完整性能复测。当前不能声明十五分钟目标达成或 PR 已准备好。
