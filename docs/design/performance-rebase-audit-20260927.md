# 性能优化变基迁移核对

## 1. 源码与实际生效范围

冻结检查点 `82e869b6a71e0c7a7e95f9e4539d62f13812c830` 确实缺少四项生产效果。后续迁移已经将这些消费者接入当前 `VmaRoot`、`MappingSlot`、`PageObject`、文件缓存端点及延迟回收机制，并补齐旧接口在新版中的对应关系。接入状态、功能验证和性能验收分别记录；源码迁移不能替代完整编译或约十五分钟的实测目标。

### 1.1 冻结来源与远端基线

旧性能备份为 `snapshot/performance-before-rebase-20260927`，提交 `3e99d0cc28c0aef6d40cca21f1feee513618a290`；其实验基线为 `affddc3fecec02b31df94573063e4840433c9ebe`。当前工作树位于 `/home/wuxun/Projects/tgoskits-performance-dev-20260927`，分支为 `integration/performance-dev-20260927`，已包含本次变基基线 `4e2e62f471e93aba5d4021eb3398b9657d07a5d2`。

2026-09-27 再次获取远端后，`upstream/dev` 为 `212ba669c2385b4a7b5394bedfdb6a40c9a65308`，比原基线新增一个 `feat(axbuild): support external Starry QEMU runs (#2509)` 提交。已将集成分支的十九个提交变基到该提交，迁移提交为 `ed1e26bfd`；`git merge-base --is-ancestor upstream/dev HEAD` 返回 0。唯一冲突是 procfs 的覆盖率导出与 profiling 入口，合并后保留各自条件编译与节点；没有 Cargo.lock 冲突或手工合并锁文件。变基前完整改动保存在 `snapshot/performance-port-before-latest-dev-20260927`，提交 `8d7ee75bc`。新增 dev 内容发生在已测运行基线之外，不能用它解释已测耗时。

初次迁移对旧实验的全部 616 个变化路径进行了 Git 对象比较：412 个与备份完全一致，50 个与旧实验基线一致，154 个采用不同的当前表示。补齐写回、普通唤醒及缓存身份后，在 `28ea96565` 再次全量核对：396 个与备份一致、171 个采用当前表示、49 个旧路径在旧基线和当前树中均不存在。后一类都是旧分支新增、当前已移动或由现有所有者接管的路径，不是保留旧基线实现的生产文件。这是路径与对象分类，包含移动和删除；不能换算成优化迁移百分比。

### 1.2 按生产消费者核对

下面按原定十二组记录当前生产消费者及保留的新版机制。迁移提交形成时尚未执行编译、静态检查或运行测试；按用户要求先完成接入和变基，再开始验证。后续回归记录在 1.6 节。旧实验的接口没有机械恢复为第二套所有者。

| 组 | 当前状态 | 源码依据与缺口 |
| --- | --- | --- |
| P01 空闲区间索引 | Starry 消费者已接入 | [`VmaNode` / `FreeAreaSearch`](../../os/StarryOS/kernel/src/mm/aspace/vma.rs) 在持久 AVL 路径复制和旋转时维护 `first_start`、`last_end`、`max_gap`，`VmaMap::find_free_area` 剪枝并保留原 first-fit、对齐及溢出规则。`MemorySet` 继续使用现有 `GapIndex`。 |
| P02 抢占退出与放置 | 旧普通唤醒顺序已补齐 | [`PreemptionState::finish`](../../components/cpu-local/src/preempt.rs) 保留无 pending 快速退出；[`exit_lock_preempt`](../../os/arceos/modules/axruntime/src/guard/mod.rs) 仅在最终 pending 退出关中断。新线程进入 [`stage_new_thread`](../../components/ax-task/src/sched/system/task_system/delivery/admission.rs)，由当前容量/需求放置策略选 CPU。[`select_fair_wake_cpu`](../../components/ax-task/src/sched/system/task_system/dispatch/wake/placement.rs) 恢复普通唤醒的 previous CPU、waker CPU、可用 CPU 顺序，保留亲和性及 active 检查；同步唤醒、RT/Deadline 规则和独立的空闲核拉取继续使用当前协议。 |
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

上述状态是消费者级核对结果，不是按 Git 路径估算的迁移比例。首轮 P02 将新版负载放置误称为旧 previous-CPU 策略的等价替代，实际不等价；1.11 节记录补齐及独立验证。首轮 P06 曾保留当前 `dev` 的 16 页/64 KiB 批次和固定写回水位，这没有完整保留旧优化的性能效果；1.9 节记录后续补齐。P12 保留当前多核确认和隔离所有者，只补充 range walk；没有绕过或并行运行第二套 TLB 协议。

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

### 1.6 迁移后的本地回归

2026-09-27 的回归使用相同集成工作树与 nightly-2026-09-04。`cargo xtask clippy --package page-table-generic --package ax-fs-ng --package starry-kernel` 的 101 项功能及目标组合全部通过，`cargo xtask clippy --package axbuild` 的一项检查通过。`cargo xtask test --since upstream/dev` 重跑后所选 68 个软件包全部通过，包含 axbuild 的 421 个测试。AArch64 内核测试通过 `cargo xtask ktest qemu --package starry-kernel --test axtest_kernel --arch aarch64 --target-dir target/performance-axtest-aarch64` 实际运行 224 项，输出 `AXTEST_SUMMARY pass=224 fail=0 skip=0 total=224`。板级脚本 smoke 通过；直接系统调用回归和板上冷自编尚在后续验证阶段。

首轮失败保留在 `tmp/performance-migration-validation-20260927/` 的原始日志。静态检查修复了显式自动解引用、定长分块、未使用导入和 `MaybeUninit` 目的指针类型。`VmaMap` 测试最初使用半页碎片，被 `CowBackend::for_extent` 的基本页对齐要求拒绝；现改为两个多页 VMA 的页对齐挖空和权限拆分，仍对四个持久快照逐地址比较 first-fit。缺页发布测试改用写缺页以验证独占页 RSS，匿名读零页不再产生匿名 RSS。axbuild 的两个迁移用例漏带 `command_args`、`command_env` 和测试配置访问，现已补齐测试辅助代码；这一遗漏不能归因于最新 dev。

固件下载进程曾挂起；核对上游固定提交及全部十个 SHA-256 后，补齐被忽略的固件目录，终止本轮下载进程并重跑项目入口，未修改驱动实现。共享产物目录的一次内核构建被宿主测试的 future-incompatibility 记录污染，现将内核回归隔离到独立 target，保留构建门禁。epoll 的现有测试以 1024 次 yield 作为工作线程完成窗口，曾在本轮提前失败；改为等待工作线程实际发布的原子状态和通知，五秒上限仅负责失败退出。其生产队列未改动，修改后的内核回归全部通过。

直接系统调用用例 `qemu/system/test-private-cache-backing` 暴露了两个实际衔接问题。`CowBackend::clone_map` 拒绝仍有物理所有者的 `PROT_NONE` 叶项，fork 返回 `EINVAL`；克隆、slot 发布校验及未发布克隆回滚现使用 occupied-leaf 身份，访问许可仍由 VMA 与 PTE flags 决定。该修复让用例继续执行到截断，但退出子进程的缓存页 rmap 尚在退休队列中，截断返回 `EBUSY`。诊断分别捕获 `Retired/users=0/pins=0/activations=0` 与 `Retiring/users=0/pins=0/activations=1`，证明不能只等待已经进入 `Retired` 的 MM。

[`pin_mm_for_cache_invalidation`](../../os/StarryOS/kernel/src/mm/aspace/lifecycle.rs) 为失效端点等待退休结果，保留普通 rmap 的非阻塞查找。最后一个内核 pin 已归零时，新 pin 不能在 `Retiring` 状态取得，等待者才可等待 CPU 切换和后台回收；仍有内核 pin 或进入 `NeedsRepair` 时返回冲突，避免等待缺页 continuation。`reclaim_done` 在发布 `Freed` 或 `NeedsRepair`、离开生命周期锁后通知；只有完成回收或确认已注销 MM 的精确 rmap key 消失，端点才跳过旧映射。第一次只处理 `Retired` 的修复仍失败，日志保留；补充 CPU 切换窗口后的同一完整用例已通过 AArch64 八核 QEMU，外层返回 0。同一正式 C 用例在板载 Linux `6.1.115-vendor-rk35xx` 上也输出 `PRIVATE_CACHE_BACKING_PASSED`。

最终生产修复后的内核回归再次输出 `AXTEST_SUMMARY pass=224 fail=0 skip=0 total=224`，增强原有退休测试以确认 `NeedsRepair` 与仍有 kernel pin 的退出 continuation 不被等待。`test-anonymous-zero-backing` 与 `test-getrandom-chroot` 均在板载 Linux 和精确选择的 Starry AArch64 QEMU 入口通过。隔离 target 的标准库重跑首次因 axbuild 配置测试继承 `CARGO_TARGET_DIR` 失败，实际目录优先级符合预期，测试夹具没有隔离环境；现按相邻已有测试使用独立子进程清除该变量，生产配置解析未改。最终 68 个软件包全部通过，axbuild 为 421 项通过。

板级构建与 `cargo xtask clippy` 并行使用同一个 target 时，构建报告含宿主 `bitmaps@3.2.1`，被现有 AArch64 门禁拒绝；另一入口等待报告锁超时，两个非零结果和原报告均保存。顺序重跑板级项目入口后，报告仅包含已批准的 `core@0.0.0` 与 `memchr@2.8.3`，构建返回 0；没有放宽包名单或屏蔽门禁。顺序执行的内核静态检查 92/92 项通过，隔离 target 的 axbuild 静态检查 1/1 项通过，两个入口均返回 0。该轮板级 ELF 为 `0a7f19f29576f46c30aa69ae8918dee2889fa28af28ac0bec46034a6536caa2f`，BIN 为 `a1e88b9cedb8401e70875720cd77813d09f958fb2236ce06e338656624d1cf71`，打包 FIT 为 `7f05911ef465f4da745a6bdbdb9e4fa6290ed7e0a5f1f80098d9ddb8759a8dd6`。部署入口核对 FIT 内嵌 BIN、板载 Linux DTB、远端文件哈希及根 PARTUUID；这些哈希属于后续弱引用修复前的镜像，不能代表当前源码产物。

### 1.7 缓存页查找生命周期

板上迁移后的首次冷编译在 `llvm-objcopy --version` 预检查中遇到读缺页 `BadState`，输出 `llvm-objcopy-preflight` 失败标记，尚未创建构建 target。正常返回 Linux 后，诊断内核进一步捕获 `FilePageDomain::reserve_page` 失败以及 rustc 的 SIGSEGV。迁移完整接入并不保证运行正确，这两轮失败不能用作成功耗时或十五分钟验收。

[`FilePageIndex`](../../os/StarryOS/kernel/src/mm/aspace/backend/file.rs) 原先先扫描整棵索引、升级每个弱引用判断存活，再在目标条目上第二次升级。最后一个映射或退休页 owner 可以在两次升级之间释放，导致第二次返回 `None` 并被转换为 `BadState`。现由 `retain_page` 对目标条目只升级一次，并保留取得的 `Arc` 到校验及发布结束；过期条目按访问删除，整个 domain 析构负责剩余弱引用。epoch、物理地址和发布状态的检查仍保留。删除每次缺页的全索引扫描也改变了热路径成本，但尚无板上数据证明其性能收益。

增强既有 `independent_file_backends_share_page_object` 内核用例，使用真实 `CachedFile`、缓存页 pin、`MappingSlot` 发布和 detach，再在查找交接时确定性释放最后一个外部 owner。错误实现的同一用例在 `file.rs:1331:77` 因 `BadState` 失败，项目入口返回 1；修复后仍检查物理页身份和完整页字节内容，AArch64 内核测试输出 `AXTEST_SUMMARY pass=224 fail=0 skip=0 total=224`，返回 0。红绿日志分别为 `kernel-axtest-cache-lookup-red.log` 与 `kernel-axtest-cache-lookup-green.log`，保存在本轮验证目录。

移除临时诊断输出后的生产修复再次通过 `cargo xtask clippy --package starry-kernel`，共 92/92 项；隔离 target 的 `tg-xtask test --since upstream/dev` 覆盖 68 个软件包并全部通过，包含 axbuild 的 421 项测试。曾尝试 `--since HEAD`，它不包含未提交差异、选择了零个软件包，该次返回 0 不计入通过证据。`tg-xtask starry test qemu --arch aarch64 -c qemu/system/test-private-cache-backing` 的直接系统调用回归在八核 QEMU 输出 `PRIVATE_CACHE_BACKING_PASSED` 和分组成功标记，外层返回 0；完整日志为 `system-private-cache-aarch64-cache-lookup-fixed.log`。随后板上冷编译在首个 Cargo 编译单元出现前触发内核页错误，不能记作自编成功。

### 1.8 重命名与目录扩容

八线程创建、重命名并回读同一目录的文件，在 Linux 上完成，在 StarryOS 上曾返回 `EUCLEAN` 或 `ENOENT`。[`rename_replace`](../../fs/rsext4/src/file/rename.rs) 在插入目标目录项之前捕获源目录项的物理位置。同目录插入可能将线性目录转换为 HTree，或分裂、重排已有叶块；后续删除不能继续使用原 inode 映射及源记录位置。提交 `2260f074f` 在同目录且目标原先不存在时重新读取父 inode、按名称定位源项并确认 inode 身份，保持同一次命名空间排他操作和元数据事务。

增强现有 `test_file_rename`，通过 128 个长文件名反复重命名、检查旧名称消失和完整内容不变，并确认目录进入 HTree。错误实现返回 `Corrupted`，操作为 `directory:delete_record`；修复后通过。现有系统用例 [`bugfix-bug-renameat2-noreplace-flags`](../../test-suit/starryos/qemu/system/bugfix-bug-renameat2-noreplace-flags/src/main.c) 同时覆盖 AArch64 的 `renameat`、`renameat2(flags=0)` 和 `RENAME_NOREPLACE`。八核 QEMU 修复前为 44 项通过、3 项失败，返回 1；修复后与同板 Linux 均为 47 项通过、0 项失败，返回 0。最终 rsext4 静态检查 3/3 项通过，所选 14 个标准库软件包全部通过。

修复后的板级 ELF SHA-256 为 `7661e40e45f88d231ffb8a693cf478df4a1f458bdf8d924a1980b5f1bd730d65`，BIN 为 `5bc66c524bd91e101cf85e378023dca15d82401877583af670c91a8804911aa7`，FIT 为 `f2fbe88f5289e468626b002dcf2c0507985bfd0090ef99c6840a500286317a96`。同一八线程板上场景完成 1,024 个文件，全部名称及内容核对通过，耗时 3.520 秒，退出码 0；日志为 `rename-fixed-concurrent-probe.serial.log`。返回 Linux 后 fsck 完成五遍检查、重放日志并优化一棵 extent tree，返回 1，正常启动至 SSH；该轮没有 inode bitmap padding 修复。此结果只证明文件操作及该轮恢复，不能证明前述哈希表页错误已解决。

### 1.9 补齐写入缓存策略

用户明确要求完整迁移已有性能优化，不再设计新的优化方案。重新核对旧快照后确认 [`WritebackBatch`](../../fs/ax-fs-ng/src/file/cache/writeback/batch.rs) 的 256 页/1 MiB 上限没有保留；文件容量虽随长度增长，`balance_dirty_pages` 仍在每次写页后扫描脏页并按固定水位写回，破坏了旧分支的增长缓存策略。提交 `1600ddf0e` 恢复旧批次上限，增长写入在尚未超过实际 retention target 时跳过固定水位扫描和提前写回。已有覆盖写入水位、容量回收、映射端点、周期写回和显式同步协议继续生效。

复用现有缓存容量测试，加入连续增长写入和 append 后的完整内容核对，确认显式同步前没有 backing 写入、同步后内容全部持久化。现有第二批写回失败测试改为跨越原定 256 页边界，并继续检查未写尾部、pin 释放和重试结果。错误实现的这两个测试均失败；修复后 `tg-xtask test --since 2260f074f` 所选 13 个软件包全部通过，日志为 `std-writeback-migration-green.log`。`tg-xtask clippy --package ax-fs-ng` 的 7/7 项组合通过。真实板上冷自编及最终 17 分钟目标尚未取得通过证据。

### 1.10 缺页取消后的缓存身份

补齐写回策略后的完整冷编译在 232 秒内启动 118 个编译单元，随后 `core` 的读/执行缺页返回 `BadState`，rustc 收到 SIGSEGV，构建返回 1。该轮没有通过标记，不能作为成功耗时。诊断内核上四份相同的 `core` 编译全部失败，合计 161.025 秒；同板 Linux 上相同命令全部通过，合计 29.950 秒。诊断捕获的是 COW 索引的缓存页身份冲突，不是新的 `FilePageDomain::reserve_page` 失败。

[`FilePageIndex::cancel_publication`](../../os/StarryOS/kernel/src/mm/aspace/backend/file.rs) 在取消最后一个准备 pin 且尚无映射时移除了身份条目。然而取消清理仍可持有原 PageObject 的强引用；此时另一个缺页重试会为同一物理缓存页创建第二个 PageObject，现有 COW 索引拒绝这个仍存活的身份替换。提交 `8e918cc82` 在取消时保留不拥有物理资源的弱身份，直到最后一个强 owner 消失；重试复用同一 PageObject，过期身份仍由 `retain_page` 按访问清理。

增强既有 `private_cache_reads_fork_cow_and_truncate_keep_exact_owners`，通过真实文件缓存、缺页准备、取消及重试确定性保留旧 owner，继续核对同一 PageObject、页字节及 fork/truncate 生命周期。错误实现因 `BadState` 失败，内核入口返回 1；修复后八核 AArch64 QEMU 输出 `AXTEST_SUMMARY pass=224 fail=0 skip=0 total=224`，返回 0。所选 starry-kernel 标准库测试通过，静态检查 92/92 项通过。日志为 `kernel-axtest-cache-cancel-{red,green}.log`、`std-cache-cancel-fixed.log` 和 `clippy-cache-cancel-fixed.log`。修复后的八核 AArch64 直接系统回归 `qemu/system/test-private-cache-backing` 输出 `PRIVATE_CACHE_BACKING_PASSED` 和分组成功标记，外层返回 0，日志为 `system-private-cache-cancel-wake-fixed.log`。板上四份相同 core 编译命令随后全部返回 0，合计 157.867 秒；日志为 `core-cancel-wake-fixed-probe.serial.log`。完整冷编译结果记录在 2.4 节。

### 1.11 普通唤醒的处理器亲和性

旧分支的 `select_wake_run_queue_index` 优先选择线程上次运行且符合亲和性的 CPU，随后选择唤醒者 CPU，再选择其他 CPU。首轮迁移保留了当前 Fair 负载放置，可能在上次 CPU 仍可用时直接迁移到其他 CPU，因此没有完整迁移原策略。提交 `281b75594` 在 [`select_fair_wake_cpu`](../../components/ax-task/src/sched/system/task_system/dispatch/wake/placement.rs) 补回普通唤醒顺序；在线状态与亲和性检查仍由当前 owner 事务负责，新增线程分布、同步唤醒、实时调度和空闲核拉取继续使用现有机制。

复用 [`fair_wake_idle_sibling`](../../test-suit/arceos/rust/src/task/fair_wake_idle_sibling.rs) 的真实四核等待/唤醒功能证明，保留 SCHED_IDLE 和 SCHED_BATCH 的有界进展及同步唤醒的空闲核选择。普通唤醒阶段限制到两个保持运行的候选 CPU，并增加源 CPU 的工作量，使负载选择与旧亲和策略产生不同结果；这样独立的空闲核拉取不会掩盖唤醒选择。错误实现选 CPU1，断言预期 CPU0，项目入口返回 1；恢复旧策略后同一用例通过，返回 0。对应日志为 `wake-previous-migration-{red,green}-isolated.log`。首次测试允许空闲 CPU 拉取，修复前后均最终运行在 CPU2；该结果不算普通唤醒选择的通过证据，原日志保留。ax-task 与 ArceOS 测试套件的静态检查 51/51 项通过，所选 15 个软件包的标准库测试全部通过。真实四核空闲核拉取及跨核等待唤醒分别通过，八核 AArch64 内核测试仍为 224 项通过、0 项失败；日志为 `clippy-wake-previous-migration.log`、`std-wake-previous-migration.log`、`wake-migration-idle-pull.log`、`wake-migration-remote-wait.log` 及 `kernel-axtest-wake-migration.log`。

## 2. 耗时证据与验收缺口

迁移补齐前的内核曾完成自编；补齐后的当前源码冷编译已通过，但尚未达到最终 17 分钟目标。需要分别核对运行内核、被编译源码、工具链和编译策略，不能把不同工作量的总时间直接当作运行内核回归。

### 2.1 两种构建工作量

前两轮使用相同的当前正常运行内核；第三轮加入诊断采样。每轮均在构建前移走 `target`，保留旧目录以供追溯；准备好的 `tg-xtask` 不在计时区间内编译。

| 运行 | 被编译源码与工具链 | release LTO | Cargo / 构建计时 | 实际终态 |
| --- | --- | --- | --- | --- |
| 旧冻结源码对照 | `b609e4f8…`；nightly-2026-07-15 | `false` | 19m07s / 19m45s | 自编、产物校验、返回 Linux 均完成 |
| 当前源码冷编译 | `81e736bd…`；nightly-2026-09-04 | `fat` | 34m11s / 35m19s | 自编、产物校验、返回 Linux 均完成 |
| 当前源码诊断运行 | 相同 `81e736bd…` 与 nightly-2026-09-04 | `fat` | 33m56s / 35m04s | `tg-xtask` 返回 0；随后 Bash 段错误，运行器返回 139，未生成 PASS |
| 板载 Linux 同工作量冷编译 | 相同 `81e736bd…` 与 nightly-2026-09-04 | `fat` | 5m12s / 5m18s | `tg-xtask` 返回 0，ELF、BIN 与源码元数据校验通过，输出 PASS |
| 完整接入后的缺页诊断运行 | 相同 `81e736bd…` 与 nightly-2026-09-04 | `fat` | 未完成 / 30m07s | 278 个编译单元启动，出现缓存页预留失败和 rustc SIGSEGV，最终构建返回 1 |
| 补齐写回策略后的冷编译 | 相同 `81e736bd…` 与 nightly-2026-09-04 | `fat` | 未完成 / 3m52s | 118 个编译单元启动，core 缓存页身份冲突及 SIGSEGV，最终构建返回 1 |

当前源码的最终 `starryos` 单元墙钟跨度约 709.87 秒。该跨度包含等待，不能等同于纯 CPU 时间或全部 LTO 时间。源码、依赖、目标配置和工具链也发生变化，不能把两种源码之间约十六分钟的差额全部归因于 LTO 或某个尚未迁移的优化。截图的缓存状态没有独立证据，不能将其直接标作已确认的冷编译基线。

Linux 对照运行 `linux-frozen81-kernel-cold-20260927` 使用相同实体板卡、持久 Debian chroot、准备好的 `tg-xtask`、冻结源码、工具链、离线依赖与构建配置。构建前将旧 target 改名保留，执行 `sync` 与 Linux 页缓存清理，计时排除任务工具编译；由正式 [`guest-kernel-selfbuild.sh`](../../apps/starry/orangepi-5-plus-selfbuild/guest-kernel-selfbuild.sh) 拒绝残留 target 并执行构建和产物校验。Linux governor 为 `ondemand`，频率仍动态变化，不能宣称两个 OS 的频率和页缓存策略完全相同。完整日志和产物回收到 `tmp/performance-migration-validation-20260927/linux-frozen81-kernel-cold/`，本机再次执行 SHA256SUMS 校验全部通过。这个对照表明相同编译工作量存在运行系统差距；迁移后内核的板上性能尚未验证。

### 2.2 原生采样和运行器失败

诊断运行保存了前 301.5 秒的 `kernel-profile.raw`，记录未丢失，使用匹配的运行内核 ELF `a0605158…` 解析。采样落在缺页、分配器、块读取、缓存和锁路径。多个最高频分配器 PC 紧接 `msr daifclr, #2`；定时中断可能在关中断期间延迟，因此这些比例不能作为无偏 CPU 时间占比，更不能外推到后面的最终链接阶段。等待事件在不同线程和嵌套操作间重叠，不能加总为墙钟耗时。

编译结束后的原始失败为 `/usr/bin/timeout: the monitored command dumped core`、`Segmentation fault (core dumped)`，并输出 `STARRY-ORANGEPI5PLUS-SELFBUILD-FAIL rc=139`。串口记录显示 Python 采样助手已返回 0，随后 Bash 在等待结束附近收到地址 `0x8` 的 SIGSEGV；根因尚未确认。编译本身完成，取回的 ELF 与 BIN 经同版 LLVM 全量转换比较一致，但整轮运行器未通过，不能标作验收成功。

板子已正常返回 Linux，SSH 可用，boot ID 为 `8c6e65c2-66eb-497d-b045-88e84eac0d3c`。启动检查重放日志、优化 extent tree，并修复 inode bitmap 尾部 padding，fsck 返回 1；这些文件系统修改保留在原始记录中。Linux 正常启动不代替最终迁移后的持久性回归。

后续在消费者迁移及最新 dev 变基完成后，先通过必要功能验证，再进行同条件完整性能复测。当前不能声明 17 分钟目标达成或 PR 已准备好。

### 2.3 迁移后原生失败

弱引用修复前的诊断运行出现 16 次缓存页预留失败和 14 份 rustc 回溯，最后 lwprintf 构建助手收到 SIGSEGV，随后报 `limits.h` 不存在。返回 Linux 后两个 Clang 头文件路径均存在，相同头文件的最小编译通过；不能把错误直接归因于 rootfs 缺少软件包。完整日志为 `performance-migrated-cache-fault-diag-20260928.serial.log`，失败 target 保留为 `target-after-cache-fault-diag-20260928`。

弱引用修复后的运行在首个编译单元出现前发生 `Unhandled Page Fault @ 0xffffffff803544a0`，读取地址为 `0xffff000200000018`。使用精确匹配的 ELF `558988c9325ceb8d986383c8edb6553154e682d54e3979370aca8c9b60477e65` 解析，故障 PC 位于 `foldhash::hash_bytes_long`，调用点属于目录缓存 `HashMap` 的重哈希路径。真实原因尚未确认，目录缓存的短时扫描及键长度诊断没有捕获非法键。硬件 watchdog 在 panic 后停止喂狗并约 44 秒复位，这是 panic 恢复，尚未达到 22,200 秒的 feeder lease。Linux 随后正常启动；该轮不能用于性能比较。

### 2.4 当前迁移源码冷编译

补齐缓存取消身份与普通唤醒策略后，正式板级构建、八核内核测试和直接系统回归均通过。运行内核 BIN SHA-256 为 `20600f90e37cdedfab86b8a7dad9b3a5dcbeffbb4fe5053539e6306a5d9b1b96`，FIT 为 `ba28f2d4c77269720a5eeed6da167d60024b567619e6e51a3184efcf459561b4`。没有加入新的性能优化或诊断 Rust 代码。

本轮 `performance-complete-current-cold-20260928` 被编译源码为干净提交 `28ea965658ecc7a3ca14c883fa2d9964cfcf4948`，归档 SHA-256 为 `2dae544eca2d24ccfe6f77557fdca2622d51847a5cbc37adb7a1b9dec7be18b0`，使用 nightly-2026-09-04、八核、离线依赖与默认 `release.lto="fat"`。正式 [`guest-kernel-selfbuild.sh`](../../apps/starry/orangepi-5-plus-selfbuild/guest-kernel-selfbuild.sh) 在构建前确认 target 不存在，仅复用已准备的任务工具。Cargo 输出 `Finished release profile ... in 29m 58s`，任务工具及二进制转换完整计时为 1,863 秒，即 31 分 03 秒，退出码 0；产物校验与串口运行器均输出 PASS。日志为 `performance-complete-current-cold-20260928.serial.log`。

取回的 ELF SHA-256 为 `c641e9f84a157d43f1052eeb346d963312093f2ea3a531e0ef8088519997bf4d`，BIN 为 `2dac7153124efe28eabf2ff07c1a4e2076211c862a0d730ab33056d9b523f8e3`。本机再次检查全部 SHA256SUMS，并用匹配的 LLVM 23.1.1 将 ELF 完整转换；转换后的 BIN 与板上 BIN 逐字节一致。这证明本轮成功生成产物，不证明达到 17 分钟。该源码的 Cargo.lock SHA-256 为 `f1dad97448a5f8c1f2c216a27e61fa1e45f8a4bb45a399ad8d6df0211c03d0b2`，与冻结 `81e736bd…` 的锁文件不同，不能直接将历史 35 分 19 秒与本轮的差额归因于迁移效果。

正常重启后 Linux 与 SSH 均可用，boot ID 为 `f79823f6-b47d-4120-9c65-1298afabd90a`，当前启动脚本和 Linux 备份的 SHA-256 均为 `d47fa003c0210128b863a04301e17ec56b7957cb3b3b2c80c1d467ee99c965e9`。fsck 重放日志、优化 extent tree，并报告 `Padding at end of inode bitmap is not set. Fix? yes`，返回 1，继续正常启动；不能记作无需修复的文件系统状态。运行末尾还出现一次 `ext4 periodic writeback failed: ResourceBusy`，没有使构建失败。inode 分配源码与旧性能快照完全一致；当前没有将这些现象定位为迁移遗漏，保留原始记录而不扩展修改范围。

只读复核确认 [`CpuRemote::charge_busy_runtime`](../../components/ax-task/src/sched/system/cpu/remote/owner.rs) 在非 idle 调度计费时推进累计运行时间；[`timer_irq_handler`](../../os/arceos/modules/axruntime/src/clock_event_runtime.rs) 的非空闲周期 tick 会执行这条计费链。Fair 单独运行时停用的是 slice deadline，不是非空闲 CPU 的周期 tick。因此没有依据认定调频输入遗漏持续负载；实际送达频率仍需独立运行证据。旧性能快照和它的基线均使用 `release.lto=false`，该设置不是旧性能分支新增的优化；当前保留 dev 的 `fat`，没有将降低构建工作量计作优化迁移。

为避免使用不同锁文件的历史对照，本轮随后在同板 Linux 上运行相同归档、工具链、任务工具、锁文件和 `fat` 配置。先将 Starry 构建目录保留为 `target-after-complete-current-starry-cold-20260928`，执行 `sync` 和 Linux 页缓存清理，再用相同正式 guest 脚本进行无 target 的构建。`linux-current-complete-cold-20260928` 的 Cargo 时间为 5 分 21 秒，完整构建 327 秒，即 5 分 27 秒，退出 0；产物校验及取回均通过。Linux ELF SHA-256 为 `842dc297d0e94735232738e4c45349de8ba356b917aeade6fea5caf10976d56f`，BIN 为 `e471fb2572e82120ef426aca2b87a846b07d7c61a49112e85cf3b3c97779a1d1`。Starry 完整构建耗时约为此对照的 5.70 倍；Linux 与 Starry 的调频策略和送达频率仍未完全对齐，不能将全部差距归因于单个软件机制。原始日志为 `linux-current-complete-cold.log`。

在保留默认 `fat` 结果与仓库配置的前提下，另运行 `performance-current-old-lto-cold-20260928`，通过现有 Cargo 环境参数 `CARGO_PROFILE_RELEASE_LTO=false` 临时采用旧分支构建设置。源码和工具链保持相同，Linux target 已移走保留，正式脚本再次确认冷构建。Cargo 用时 19 分 31 秒，完整构建用时 1,221 秒，即 20 分 21 秒，返回 0，产物与运行器均输出 PASS，仍未达到 17 分钟。两个 `fat` 构建的 Cargo profile 哈希均为 `17647372896918672692`，该轮为 `854319013279132213`；同一源码和工具链的构建记录确认环境设置已生效。该轮是构建配置对照，不是新性能优化，不能计作默认 `fat` 的 17 分钟验收。

该轮 ELF SHA-256 为 `95f18f6acb63aa09db20db63c0df960f3d95b2ed2b865ae862de3ab44875175f`，BIN 为 `20cb208e8ca41c6edaa446a2ae2534911e096490934efcaa2072522f0e36da81`；取回后校验通过，匹配 LLVM 的完整转换再次逐字节一致。正常返回 Linux 后 boot ID 为 `d29fad0e-af7b-465e-a60b-8413a47f29ba`，启动脚本与 Linux 备份哈希仍相同。fsck 重放日志、优化 extent tree，返回 1 并正常启动；该轮没有 bitmap padding 修复。原始构建、恢复与 profile 记录为 `performance-current-old-lto-cold-20260928.serial.log`、`linux-after-current-old-lto.log` 和 `reference-workload-cold-preparation.log`。

### 2.5 原始固定工作量

截图使用 `b609e4f8…` 归档与 nightly-2026-07-15，和当前源码、锁文件及 nightly-2026-09-04 不同。板上保存的 `tgoskits-src.tar` 全量 SHA-256 与该归档一致；已从该归档重新解包正式板端源码，保留原目录及其 target，确认新目录没有 target。旧锁文件 SHA-256 为 `e05882da57abb9d874c2d49d4ac38441ddd10241ee3718c4ab4831372c551306`，原始 `release.lto=false` 保持不变，离线依赖及 LLVM 22.1.8 工具预检通过。没有将旧源码的性能实现部署为运行内核。

固定输入对照使用与 2.4 节相同的迁移后 FIT，仅更换被编译的输入，以核对原始工作量的耗时。它不能替代最新源码自编的 17 分钟验收；结果与当前源码分别记录。预检日志为 `reference-workload-inventory.log`、`reference-workload-cold-preparation.log` 和 `reference-workload-offline-preflight.log`。

首次 `performance-reference-b609-cold-20260928` 在 9 秒后失败，未开始 Rust 编译：当前任务工具请求旧源码不存在的 `scripts/targets/bare/aarch64-unknown-none-softfloat.json`，Cargo 返回 101，任务工具与运行器返回 1。该时间不能计作自编成功或性能结果。板子随后正常返回 Linux，fsck 重放日志并完成五遍检查，返回 1，没有 extent 优化或 bitmap padding 修复。

板上保存的旧工具 `tg-xtask-nightly-20260715` SHA-256 为 `d68bb3780d4537c9b06a3bb96357d91db631bf2ed0668c70774ae261f965e6e9`，与原始旧源码对照记录完全相同。已保留当前工具 `c2130b54…` 的副本，临时恢复兼容工具；没有给旧源码添加新 target 配置。`performance-reference-b609-cold-20260928-2` 再次确认 target 不存在、锁文件不变后启动。Cargo 用时 17 分 22 秒，完整构建 1,072 秒，即 17 分 52 秒，返回 0；310 个编译单元启动，产物与串口运行器均输出 PASS。该轮距 17 分钟仍差 52 秒，不能记作目标通过。

ELF SHA-256 为 `ba21135d9861303aad9dafeecfe5fabb8428660e98e4bbf004d51a4709d6a611`，BIN 为 `474dc99ecb1139f75c97464c58324b9fb49b9ce2a2128695692e0dafce955f87`。正式取回入口再次校验全部 SHA256SUMS；宿主机使用匹配的 LLVM 22.1.8 完整转换 ELF，生成的 BIN 与板上 BIN 逐字节一致。源码元数据仍为原始 `048c1dda…` 的 dirty 归档，锁文件哈希未变；不能将该固定输入称为最新迁移源码。原始日志为 `performance-reference-b609-cold-20260928-2.serial.log` 和 `fetch-reference-complete.log`。

正常返回 Linux 后，boot ID 为 `db8f0f0b-5776-404e-95a7-856ca6ca6566`，SSH 可用，Linux 启动脚本与备份哈希相同。fsck 重放日志、优化 extent tree，并修复 inode bitmap 尾部 padding，返回 1 后正常启动；不能记作无修复的文件系统状态。本轮有 31 条周期写回 `ResourceBusy` 日志，没有造成构建失败。恢复记录为 `return-linux-after-reference-complete.serial.log` 和 `linux-after-reference-complete.log`。

取回证据后，正式 `install_source_link.sh` 已将板端 `/opt/tgoskits` 恢复到 `2dae544e…` 当前迁移源码，任务工具恢复为 `c2130b54…`。之前当前源码的 LTO 对照 target 已改名保存为 `target-after-current-old-lto-cold-20260928`，当前 target 不存在；原始固定源码及其产物继续保留。`restore-current-inputs-after-reference.log` 记录源码、锁文件、任务工具哈希及 `CURRENT_INPUTS_RESTORED=PASS`。本轮没有新增 Rust 代码或性能优化。

### 2.6 当前验收状态

原定十二组优化的生产消费者、旧模块对应关系和迁移故障回归已记录在第一章。默认编译策略、旧编译策略对照和原始固定工作量均已得到完整成功结果，但没有一轮达到 17 分钟；这些结果不能互相替代。

| 被编译输入 | 系统与 release LTO | Cargo 用时 | 完整构建用时 | 构建终态 | 17 分钟目标 |
| --- | --- | --- | --- | --- | --- |
| 当前 `2dae544e…` / nightly-2026-09-04 | StarryOS / `fat` | 29m58s | 31m03s | PASS | 未达到 |
| 相同当前输入 | Linux / `fat` | 5m21s | 5m27s | PASS | 仅为 Linux 对照 |
| 相同当前输入 | StarryOS / 临时 `false` | 19m31s | 20m21s | PASS | 未达到 |
| 原始 `b609e4f8…` / nightly-2026-07-15 | StarryOS / 原始 `false` | 17m22s | 17m52s | PASS | 未达到 |

本轮核对没有发现新的消费者迁移遗漏，也没有依据将剩余耗时归因于一个已定位的迁移错误。用户要求只迁移原性能分支，不增加新的性能优化；因此没有改调频、增加调度策略或降低默认构建工作量。17 分钟验收和后续 PR 拆分尚未完成，未推送或创建 PR。
