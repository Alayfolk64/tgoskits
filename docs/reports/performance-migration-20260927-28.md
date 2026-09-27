# StarryOS 性能优化迁移与板上自编报告（2026-09-27—28）

## 1. 任务边界与当前结论

这两天的工作围绕 Orange Pi 5 Plus 上的 StarryOS 自编展开：先将旧性能分支完整迁移到当时核对过的 `dev`，再验证迁移后的内核能够正确完成自编，最后检查完整冷编译是否在 17 分钟以内。本文按北京时间记录 2026 年 9 月 27 日至 28 日的迁移、故障处理和实测；此前已经存在的自编工具、profiling 代码和旧实验结果作为来源或对照，不算作这两天新写的性能优化。

### 1.1 用户限定的范围

用户要求先完成旧分支性能优化的迁移，再运行完整编译测试；随后进一步明确，只允许迁移旧分支已有的优化，不新增另一套性能优化。具体实施中，适配当前 `dev` 所需的所有权和正确性修复继续进行，但没有通过新增调频策略、调度策略、降低默认构建工作量或放宽测试来追求目标时间。代码已经按生产调用链核对，功能回归与最终完整冷编译均已执行；17 分钟目标没有通过。

最终集成提交位于 `integration/performance-migration-20260928`，报告编写前的提交为 `6c1f071f35240990d501cc1da4fe80645ffbab51`。这个分支已推送到用户仓库 `Alayfolk64/tgoskits`；它仍是集成分支，尚未按子系统拆成多个 PR。更细的逐项迁移和原始故障经过见[性能优化变基迁移核对](../design/performance-rebase-audit-20260927.md)。

### 1.2 可以确认的结果

运行迁移后内核时，当前迁移源码以仓库默认 `release.lto="fat"` 在 StarryOS 上冷编译完整耗时 **31 分 03 秒**；相同源码临时使用 `CARGO_PROFILE_RELEASE_LTO=false` 的对照为 **20 分 21 秒**。截图对应的旧源码、旧工具链和旧任务工具在同一迁移后内核上冷编译耗时 **17 分 52 秒**。三轮都生成并校验了 ELF 与 BIN，最后一轮距离 17 分钟仍差 52 秒。**17 分 52 秒不是最新源码的成绩。**

板子在最后一轮后已正常返回 Linux，SSH 可用，Linux 启动脚本与备份一致；板上 `/opt/tgoskits` 和 `/usr/local/bin/tg-xtask` 已恢复到当前迁移源码和当前任务工具。文件系统检查修复过日志、extent tree 和 inode bitmap padding，所以这里的“返回 Linux”不等于“文件系统未发生修复”。

## 2. LTO 与测量口径

这一组数据必须把运行内核、被编译源码、Rust 工具链、构建配置和缓存状态分开。只看串口末尾的 `Finished` 或截图中的 `real` 时间，无法确认编译得到的产物可用，也无法判断两轮是不是同一个工作量。

### 2.1 LTO 的含义

LTO 是 *Link Time Optimization*，即链接时优化。普通 release 编译会先分别优化编译单元；LTO 让 LLVM 在链接阶段继续利用更大范围的代码信息，例如跨 crate 内联和消除不再使用的代码。它可能改善产物的运行性能，但会增加构建时间，尤其是最终链接阶段。[Cargo 官方 `lto` 配置文档](https://doc.rust-lang.org/cargo/reference/profiles.html#lto)区分了以下三种容易混淆的值。

| Cargo 设置 | 实际含义 | 本轮使用情况 |
| --- | --- | --- |
| `lto = "fat"` 或 `true` | 尝试在依赖图中的 crate 之间执行整体 LTO | 当前仓库 [`Cargo.toml`](../../Cargo.toml) 的默认 release 设置；当前源码 31 分 03 秒那轮使用它 |
| `lto = false` | 不做跨 crate 的 fat LTO，但仍可能在单个 crate 的多个 codegen unit 之间做本地 Thin LTO | 旧源码原始设置；当前源码的 20 分 21 秒临时对照也使用它 |
| `lto = "off"` | 明确关闭 LTO，包括本地 Thin LTO | 本轮没有测试，也没有改成仓库默认值 |

因此，`false` 不等于“关闭全部优化”，也不等于 `"off"`；普通 release 优化仍然存在。旧性能快照及其旧基线都采用 `false`，这不是旧性能分支新引入的优化。当前仓库保留 `fat`，临时对照仅通过环境变量覆盖一次构建。当前源码两轮分别为 31 分 03 秒和 20 分 21 秒；这个差值还混有动态频率、页缓存和构建环境波动，不能把每一秒都严格归因于 LTO，也不能用改变构建工作量来声称内核运行更快。

### 2.2 自编链路与冷编译判定

板载 Linux 保存源码归档、Debian 构建 rootfs、工具链和已准备好的 `tg-xtask`；StarryOS 启动后，通过 [`init-kernel-selfbuild.sh`](../../apps/starry/orangepi-5-plus-selfbuild/init-kernel-selfbuild.sh)进入 rootfs，由 [`guest-kernel-selfbuild.sh`](../../apps/starry/orangepi-5-plus-selfbuild/guest-kernel-selfbuild.sh)拒绝已有的目标源码 `target`，在离线模式下编译并核验产物。运行中的 StarryOS 内核与它此次**编译的源码归档**是两个独立版本；旧归档的 17 分 52 秒不能转写成当前归档的成绩。

```mermaid
flowchart LR
    A[板载 Linux：确认 FIT、源码与启动脚本] --> B[启动迁移后的 StarryOS 内核]
    B --> C[进入 Debian rootfs]
    C --> D[选定源码归档、nightly 与任务工具]
    D --> E[检查目标源码 target 不存在]
    E --> F[离线编译并生成 ELF、BIN]
    F --> G[校验哈希并记录完整耗时]
    G --> H[正常重启 Linux，检查 fsck 与启动脚本]
```

这里的“冷”指目标源码树的 Cargo `target` 在计时前不存在；旧目录改名保留以供追溯，已准备好的 `tg-xtask` 不在计时内重新编译，离线 Cargo 依赖和已安装工具链仍会复用。这不是“整块板完全没有任何缓存”的声明。正式脚本输出 Cargo 自己的编译时间，并用 `STARRY-BUILD-END ... elapsed=` 给出包含任务工具、链接、kallsyms 和 BIN 转换的完整构建时间；17 分钟验收采用后者。板卡配置在[`build-aarch64-unknown-none-softfloat.toml`](../../apps/starry/orangepi-5-plus-selfbuild/build-aarch64-unknown-none-softfloat.toml)，目标为 AArch64 softfloat，最多八核，启用 RK3588 板级设备和自编 watchdog。

### 2.3 三种不能互换的源码输入

当前源码曾在迁移中先后形成冻结检查点和最终归档。原始截图对应另一份旧归档，连 nightly、锁文件、构建配置和任务工具版本都不同；这些输入放在同一表中，是为了限定结论，而不是假定它们可直接 A/B。

| 输入 | 源码归档 SHA-256 | 版本与用途 |
| --- | --- | --- |
| 旧固定工作量 | `b609e4f87fe05dbb190c141c1e0d6820590d0d63b21224237e78dbaa684aa3e1` | 元数据指向 `048c1dda…`，标记 `dirty=true`；nightly-2026-07-15；用于重现截图所用编译工作量 |
| 迁移冻结检查点 | `81e736bd5564423ec7c67d579b5591280ef980289cec13835f2b384e8b03afa0` | `82e869b6…`；nightly-2026-09-04；后来确认还缺四处生产效果 |
| 当前迁移源码 | `2dae544eca2d24ccfe6f77557fdca2622d51847a5cbc37adb7a1b9dec7be18b0` | 干净提交 `28ea965658ecc7a3ca14c883fa2d9964cfcf4948`；nightly-2026-09-04；用于最终当前源码冷编译 |

最终分支在 `28ea96565` 之后增加的是核对文档，板端当前源码归档没有这些文档提交；期间没有再修改 Rust 生产代码。旧输入使用保存的旧任务工具 SHA-256 `d68bb378…`；当前输入使用当前工具 `c2130b54…`。旧输入的 Cargo.lock SHA-256 为 `e05882da…`，当前输入为 `f1dad974…`。这些差异也限制了跨输入耗时比较的解释力。

### 2.4 板端构建环境排障

前置排障中，StarryOS 直接看到的 `/opt/tgoskits` 一度只有 `apps` 和 `tmp`，而同一板上的 Debian 构建 rootfs 内 `/opt/tgoskits` 包含完整仓库。两者是不同挂载视图，不能根据 StarryOS 外层目录就认定源码丢失。旧的手工 `setsid -c chroot` 曾因控制终端忙而失败，外层 `/bin/sh` 也没有可直接使用的 `time`；正式入口改为检查 rootfs 路径、绑定 `/proc`、`/dev`、`/sys`，再直接 `chroot` 执行已准备的构建脚本。

当时在 rootfs 内重编 `tg-xtask` 曾遇到 `secret-service` 派生宏的 `E0433: cannot find zvariant in the crate root`，所以板上是否存在源码与任务工具是否可执行必须分别检查。后续成功轮使用已准备、可校验 SHA-256 的任务工具，不把重新编译任务工具的耗时混入内核自编。第一次拿新版任务工具编译旧源码失败的原因则是双方目标描述文件接口不兼容，见第 4.3 节；不能把它当作内核性能退化。

启动路径同样分两层：U-Boot 从 `/boot/boot.scr` 执行启动脚本，StarryOS 自编脚本先经 [`restore_linux_boot.sh`](../../apps/starry/orangepi-5-plus-selfbuild/restore_linux_boot.sh)恢复正常 Linux 启动选择器，再进入耗时编译。恢复脚本只改变下一次启动所用脚本，不会立刻把当前 StarryOS 会话切换成 Linux；编译结束后仍需正常重启并检查 Linux 提示符、SSH 和 fsck 日志。早期长编译曾在 watchdog lease 到期时被复位，出现重新启动到 Armbian 的日志；那一轮没有得到完整自编结果，不能与后面的成功冷编译按耗时并列。

## 3. 9 月 27 日的变基与迁移

源头是旧性能备份 `snapshot/performance-before-rebase-20260927`（`3e99d0cc…`），其实验基线为 `affddc3f…`。为保留旧改动，先建立变基前备份 `snapshot/performance-port-before-latest-dev-20260927`（`8d7ee75bc…`），再把集成分支十九个提交变基到当时核对的 `upstream/dev` 提交 `212ba669c2385b4a7b5394bedfdb6a40c9a65308`。变基时唯一需要解决的冲突是 procfs 覆盖率导出与 profiling 入口；两种条件编译路径及节点都保留，`Cargo.lock` 没有手工合并冲突。`git merge-base --is-ancestor upstream/dev HEAD` 已通过。

### 3.1 为什么不能只检查 Git 文件是否相同

旧分支的所有者和模块边界与新 `dev` 不完全一致。例如旧 COW、缓存和文件系统代码在新版被拆分到新的生命周期端点；有些旧文件在新版根本不存在。迁移工作先检查实际模块声明、生产入口和调用方，再核对旧功能落在哪个当前所有者，而不是把旧文件原封不动复制回来。最终对旧实验全部 616 个变化路径的 Git 对象核对得到：396 个与旧备份相同，171 个采用当前表示，49 个旧新增路径在旧基线和当前树都不存在；最后一类已移动或由新版所有者承接，**这些数值不能当作功能迁移百分比**。

初次冻结检查点 `82e869b6…` 曾缺少四处关键效果，不能因为源码已能编译就称“迁移完整”：文件缓存增长策略和 256 页批次、普通唤醒对上次 CPU 的偏好，以及与私有文件缓存页所有权相关的两个身份交接场景。后续的故障回归把这些缺口变成了可验证的代码和测试。主要迁移提交为 `ed1e26bfd`，后续细化提交列在第 4 节。

### 3.2 十二组生产能力

迁移核对按内存、调度、文件缓存和 ext4 的生产消费者组织为十二组。表中的“接入”仅指代码与调用链，测试和板上性能证据仍分别见后文；这里不把“实现存在”当作“性能验收通过”。

| 组 | 旧优化在当前源码中的落点 | 关键入口与保留的约束 |
| --- | --- | --- |
| P01 VMA 空闲区间索引 | 持久 AVL 节点维护 gap 聚合信息，first-fit 查询可剪枝 | [`VmaMap::find_free_area`](../../os/StarryOS/kernel/src/mm/aspace/vma.rs)；对齐、溢出和页级约束仍检查 |
| P02 抢占与任务放置 | 无待处理请求时快速退出；普通唤醒恢复 previous CPU 优先 | [`PreemptionState::finish`](../../components/cpu-local/src/preempt.rs)、[`select_fair_wake_cpu`](../../components/ax-task/src/sched/system/task_system/dispatch/wake/placement.rs)；新线程、同步唤醒和空闲核拉取保留新版机制 |
| P03 缺失页表发布 | 锁外准备未发布页表后缀，重验证后一次发布 | [`PageTableMapPlan::prepare`](../../memory/page-table-generic/src/table.rs)；失败释放未发布资源 |
| P04 私有缺页准备 | 准备、重验证、发布和取消分阶段执行 | [`prepare_page_fault`](../../os/StarryOS/kernel/src/mm/aspace/mod.rs)；非缓存读取有界，未读尾部清零 |
| P05 共享匿名零页 | 基本页读缺页共享内核拥有的只读零页，首次写复制 | [`ZeroPage`](../../os/StarryOS/kernel/src/mm/aspace/backend/zero.rs)；不误交给普通帧回收 |
| P06 文件填充与写回 | 锁外填充，有限页集合与 256 页/1 MiB 写回批次 | [`FillOwner::prepare`](../../fs/ax-fs-ng/src/file/cache/fill/mod.rs)、[`WritebackBatch`](../../fs/ax-fs-ng/src/file/cache/writeback/batch.rs)；增长写保留容量水位条件 |
| P07 私有文件缓存页 | 读映射共享物理缓存页，首次写 COW | [`CachedPageBacking`](../../fs/ax-fs-ng/src/file/page.rs)、[`FilePageDomain`](../../os/StarryOS/kernel/src/mm/aspace/backend/file.rs)；EOF、epoch、身份与物理 owner 校验 |
| P08 ext4 锁外读 | 准备设备请求、锁外执行、回锁后重验证 | [`read_admitted_inode`](../../fs/ax-fs-ng/src/fs/ext4/rsext4/fs/read.rs)；不支持的 inode 形态走明确序列化路径 |
| P09 日志提交所有权 | 拥有式提交和锁外 abort 持久化 | [`PreparedCommit`](../../fs/rsext4/src/blockdev/journal/detached.rs)、[`execute_writeback`](../../fs/ax-fs-ng/src/fs/ext4/rsext4/fs/writeback.rs)；保留原始设备错误 |
| P10 独立写与 inode 预读 | extent 写拥有 I/O 请求，inode table 预读在锁外完成 | [`write_extent_inode`](../../fs/ax-fs-ng/src/fs/ext4/rsext4/fs/write.rs)、[`sync_with_commit_gate`](../../fs/ax-fs-ng/src/fs/ext4/rsext4/fs/writeback.rs)；回锁后封存有限提交前缀 |
| P11 目录与元数据缓存 | 正缓存、有界负缓存和 generation 重验证 | [`DirNode` 缓存](../../fs/axfs-ng-vfs/src/node/dir/cache.rs)、[`lookup_entry`](../../fs/ax-fs-ng/src/fs/ext4/rsext4/inode/directory/mod.rs)；用户复制成功后才推进目录游标 |
| P12 批量解除映射 | range walker 一次遍历，批量转移待回收页表 owner | [`unmap_range_deferred`](../../memory/page-table-generic/src/unmap.rs)；仍由新版 TLB 回执完成刷新与回收 |

共享零页与私有缓存页的迁移还需要覆盖 fork、`PROT_NONE`、解除映射、退休 MM 和失败回滚。当前 `MappingSlot`、PageObject、`FrameLease` 以及 tagged TLB 回执承担实际所有权；旧的并行实现和全局 listener 表没有重新加回。GIC LPI pending table 的独立 64 KiB stride、AArch64 硬件清零、块缓存 pending read 与测试入口也已按当前所有者核对，细节见前述逐项核对文档。

## 4. 9 月 28 日的故障修复与回归

完整迁移后的第一次板上编译没有直接证明迁移正确。真实负载暴露了私有缓存页与旧目录结构的交错问题；每个已修复缺陷都保留了错误实现上的失败证据，随后在同一路径上验证修复。未定位根因的独立故障仍标记为未确认。

### 4.1 关键修复链

以下提交均位于集成分支；测试提交与生产修复分开，便于以后按子系统拆 PR。表后列出的输出是实际运行结果，不是尚未执行的计划。

| 提交 | 触发与根因 | 最终行为及对应证明 |
| --- | --- | --- |
| `34fc22b1d` | `PROT_NONE` 的 COW 叶项仍有物理 owner，fork 被误拒绝；子进程退出时 MM 退休队列尚未完成，truncate 又遇到 `EBUSY` | occupied-leaf 身份用于克隆和回滚；缓存失效端点等待可安全等待的退休完成。真实私有缓存 backing 系统用例通过 |
| `833e03af2` | 文件页索引先全表扫描再二次升级目标弱引用，两个升级之间最后一个 owner 可以释放 | `retain_page` 只升级一次，并保留强引用到校验和发布结束；确定性交错用例在旧实现报 `BadState`，修复后通过 |
| `2260f074f` | 同目录 rename 插入目标后目录索引可能重组，旧源位置失效 | 提交前按父 inode、名字和 inode 身份重新定位源；同目录 HTree 扩容回归在错误实现报损坏，在修复后通过 |
| `1600ddf0e` | 初轮移植保留了新版 16 页/64 KiB 写回批次及固定水位，漏掉旧分支增长缓存策略 | 恢复旧 256 页/1 MiB 批次及增长写的实际 retention target 门槛；连续增长与第二批失败回归由红转绿 |
| `8e918cc82` | 缺页准备取消时过早移除弱身份，但取消 continuation 还持有旧 PageObject，重试创建第二个身份导致 COW 索引 `BadState`、rustc SIGSEGV | 取消后保留不拥有物理页的弱身份，直到最后一个强 owner 消失；同一真实缓存页缺页/取消/重试用例由红转绿 |
| `281b75594` | 普通 Fair 唤醒没有完整恢复旧分支“上次 CPU 优先”顺序 | 在当前 active/亲和性协议下恢复 previous CPU、waker CPU、其他 CPU 的次序；与空闲核拉取分离的四核测试在错误实现选择 CPU1、修复后选择 CPU0 |

写回的 `ResourceBusy` 日志在成功自编中仍出现，不能把上述修复说成解决了所有写回告警。也没有根据日志猜测出新的频率调整或调度优化；用户明确不允许增加原分支之外的性能优化。

### 4.2 自动化与真实系统验证

项目任务工具是本轮构建、静态检查和测试入口。9 月 27 日的迁移后检查中，定向 Clippy 通过 101 项组合，`axbuild` 通过一项；`cargo xtask test --since upstream/dev` 的 68 个被选软件包全部通过。9 月 28 日补齐代码后，starry-kernel Clippy 通过 92/92 项，ax-task 与 ArceOS 测试套件 Clippy 通过 51/51 项；对应标准库回归分别通过 68 和 15 个所选软件包。写回策略修复后，ax-fs-ng Clippy 通过 7/7 项，所选 13 个软件包通过。修改 Rust 时执行了仓库固定 nightly 的 `cargo fmt`，没有用 `allow` 绕过警告。

八核 AArch64 QEMU 内核测试在缓存取消修复和最终唤醒修复后均输出 `AXTEST_SUMMARY pass=224 fail=0 skip=0 total=224`。直接运行 `qemu/system/test-private-cache-backing` 得到 `PRIVATE_CACHE_BACKING_PASSED` 和分组成功标记，外层返回 0。零页与空 chroot 的 getrandom 用例在板载 Linux 与精确选择的 Starry AArch64 QEMU 路径也通过。板上四条此前触发 `core` 页错误的原生命令，修复后均返回 0；这是短链路正确性回归，不能代替完整构建计时。

若要复查原始输出，本机未跟踪目录 `tmp/performance-migration-validation-20260927/` 保存了 `kernel-axtest-wake-migration.log`、`system-private-cache-cancel-wake-fixed.log`、`clippy-cache-cancel-fixed.log`、`wake-previous-migration-{red,green}-isolated.log`、`std-writeback-migration-{red,green}.log` 等记录。日志目录不属于 Git 提交；此报告上传的是方法、关键结果和可追溯路径，不是假称已把所有串口原始数据推送到仓库。

### 4.3 未当作成功结果的失败轮次

迁移前后的失败轮次保留了各自的真实终态。一次诊断运行中 `tg-xtask` 已完成，但 Bash 最后 `Segmentation fault (core dumped)`，运行器返回 139；不能把 35 分 04 秒当作整轮成功。弱引用修复前的完整冷编译在 30 分 07 秒后失败，出现 16 次缓存页预留失败和 14 份 rustc 回溯；另一次在 3 分 52 秒、118 个编译单元启动后因 COW 身份冲突和 SIGSEGV 失败。目录哈希表诊断曾在第一个 Cargo 单元前触发 `foldhash::hash_bytes_long` 页错误，匹配 ELF 已定位 PC，但根因尚未确认；不把它改写成已解决问题。

旧固定输入的第一次重跑只用了 9 秒就失败：新版任务工具向旧源码请求不存在的 `scripts/targets/bare/aarch64-unknown-none-softfloat.json`，Cargo 返回 101，**没有启动 Rust 编译**。随后复用板上先前保存、哈希与旧成功记录完全一致的任务工具 `d68bb378…`，没有修改旧源码，第二次才得到第 5 节的 17 分 52 秒完整成功结果。截图中的约 14 分 56 秒在 ELF 已生成后因 `llvm-objcopy` 缺少匹配 `libLLVM` 而退出 127，同样不能标成已完成的冷自编。当前脚本对匹配 sysroot 的 `llvm-objcopy --version` 先做预检，实际成功轮均完成了转换。

## 5. 板上性能结果与证据范围

同一块 Orange Pi 5 Plus、八核、相同目标架构和正式板级入口提供了比较基础。比较的最大限制是：旧归档与当前归档有不同源码、锁文件、nightly 和 LTO；即使同一归档在 StarryOS 与 Linux 上运行，频率策略和实际送达频率也未完全对齐。表中的时间都是各轮正式日志的完整结果，失败轮单独列在上一节。

### 5.1 主要冷编译矩阵

运行 ID 可在本机 `target/starry-orangepi5plus-selfbuild/artifacts/<运行 ID>/run.log` 找到取回的原始输出。每轮成功结果同时检查了 `source.meta`、`starryos.elf`、`starryos.bin` 和 `SHA256SUMS`，并在完成后正常回到 Linux。

| 运行 ID | 被编译源码／系统／LTO | Cargo 时间 | 完整时间 | 终态 |
| --- | --- | --- | --- | --- |
| `performance-dev-frozen-cold-20260927` | 旧 `b609…`／StarryOS／`false` | 19m07s | 19m45s | PASS；旧输入的早期对照 |
| `performance-dev-current-cold-20260927-2` | 冻结 `81e…`／StarryOS／`fat` | 34m11s | 35m19s | PASS；当时仍缺四处生产效果 |
| `performance-complete-current-cold-20260928` | 当前 `2dae…`／StarryOS／默认 `fat` | 29m58s | **31m03s** | PASS；当前源码默认配置的结果 |
| `linux-current-complete-cold-20260928` | 同一当前 `2dae…`／Linux／`fat` | 5m21s | **5m27s** | PASS；同源码与工具链的 Linux 对照 |
| `performance-current-old-lto-cold-20260928` | 同一当前 `2dae…`／StarryOS／临时 `false` | 19m31s | **20m21s** | PASS；只改变该轮 LTO 设置 |
| `performance-reference-b609-cold-20260928-2` | 原始 `b609…`／迁移后 StarryOS／原始 `false` | 17m22s | **17m52s** | PASS；旧输入，仍超过 17 分钟 |

相同当前源码的 StarryOS 默认轮与 Linux 轮完整时间比约为 5.70；这说明存在显著系统运行差距，但当前证据不能把全部差距归因于某一个锁、文件系统路径或 CPU 频率。当前源码仅改变 LTO 设置的两次 StarryOS 冷编译差 10 分 42 秒，属于编译策略与现场运行条件合成的观察值，不能宣称新增了内核性能优化。旧 `b609…` 两轮为 19 分 45 秒与 17 分 52 秒；运行内核版本与其他现场条件并非完全锁定，不能直接把 1 分 53 秒差额归因于某个迁移提交。

### 5.2 产物、启动和文件系统

最终运行的 StarryOS 内核镜像 BIN SHA-256 为 `20600f90e37cdedfab86b8a7dad9b3a5dcbeffbb4fe5053539e6306a5d9b1b96`，打包 FIT 为 `ba28f2d4c77269720a5eeed6da167d60024b567619e6e51a3184efcf459561b4`。这是**运行**上述三轮最终 StarryOS 自编的迁移后内核；下表则是每轮**被编译**出来的产物。正式 [`fetch_artifacts.sh`](../../apps/starry/orangepi-5-plus-selfbuild/fetch_artifacts.sh)在 Linux 侧取回并校验哈希，宿主机再使用与各轮 nightly 匹配的 LLVM `llvm-objcopy` 完整转换 ELF，转换后的 BIN 均与板上产物逐字节一致。

| 被编译输入与 LTO | ELF SHA-256 | BIN SHA-256 |
| --- | --- | --- |
| 当前 `2dae…`／`fat` | `c641e9f84a157d43f1052eeb346d963312093f2ea3a531e0ef8088519997bf4d` | `2dac7153124efe28eabf2ff07c1a4e2076211c862a0d730ab33056d9b523f8e3` |
| 当前 `2dae…`／`false` | `95f18f6acb63aa09db20db63c0df960f3d95b2ed2b865ae862de3ab44875175f` | `20cb208e8ca41c6edaa446a2ae2534911e096490934efcaa2072522f0e36da81` |
| 原始 `b609…`／`false` | `ba21135d9861303aad9dafeecfe5fabb8428660e98e4bbf004d51a4709d6a611` | `474dc99ecb1139f75c97464c58324b9fb49b9ce2a2128695692e0dafce955f87` |

每轮 StarryOS 测试前检查 Linux 启动备份和 StarryOS 启动脚本，测试入口先恢复 Linux 选择器，再开始耗时构建；结束后使用正常重启路径，确认 Linux 提示符和 SSH。最后一次 Linux boot ID 为 `db8f0f0b-5776-404e-95a7-856ca6ca6566`，`/boot/boot.scr` 与备份 SHA-256 均为 `d47fa003c0210128b863a04301e17ec56b7957cb3b3b2c80c1d467ee99c965e9`。该次 `/run/initramfs/fsck.log` 报告 journal recovery、多个 extent tree 优化、`Padding at end of inode bitmap is not set. Fix? yes` 和退出状态 1，然后正常启动。旧固定输入的成功轮还产生 31 条周期写回 `ResourceBusy` 日志；这些是仍需保留的文件系统诊断线索，不构成“自编失败”或“文件系统完全无问题”的证据。

### 5.3 当前板端状态

取回旧固定工作量产物后，正式 [`install_source_link.sh`](../../apps/starry/orangepi-5-plus-selfbuild/install_source_link.sh)已将板端 `/opt/tgoskits` 恢复为当前归档 `2dae544e…`，并将 `/usr/local/bin/tg-xtask` 恢复为当前工具 SHA-256 `c2130b54…`。临时旧工具仍有备份，旧源码及其产物也保留。当前源码先前 `false` 对照的 `target` 已改名保存，因此 `/opt/tgoskits/target` 不存在；将来若重跑当前源码，仍可进行新的冷编译。`tmp/performance-migration-validation-20260927/restore-current-inputs-after-reference.log` 含最终链接、锁文件和工具哈希，标记为 `CURRENT_INPUTS_RESTORED=PASS`。

## 6. 分支、PR 方案与未完成事项

代码已从旧性能快照变基到当时核对的 `dev`，并在专用工作树 `/home/wuxun/Projects/tgoskits-performance-dev-20260927` 上完成上述测试。用户要求后续按子系统拆成多个 PR，且每个 PR 内再拆细提交；当前只有一个保留完整现场的集成分支，本文作为该分支的报告提交，没有创建 PR 或宣称已经完成拆分。

### 6.1 为什么尚未拆 PR

目前分支同时包含内存与页表、StarryOS COW、文件缓存、ext4 与日志、调度以及板级自编基础设施，还携带更早的 profiling 和自编历史提交。直接把当前大分支作为单个性能 PR 会让审查范围和性能结论失真。可审查的拆分方向是：内存/页表及 COW、文件缓存及写回、ext4 日志及目录、调度与抢占、板级自编和验证工具；每组都需保留真实依赖顺序、红绿回归和功能验证。这个方向是后续方案，不是已经建立的 PR 或完成的提交重排。

### 6.2 尚未满足的目标

当前源码在默认配置下仍需 31 分 03 秒，在临时 `false` 对照下仍需 20 分 21 秒；旧固定工作量最接近目标，但完整用时 17 分 52 秒。现有代码核对尚未发现可证实的旧优化迁移遗漏，不能在用户禁止新增优化的约束下虚构一个“已解决”的 17 分钟结果。实际 CPU 送达频率、成功构建中的周期写回 `ResourceBusy` 和 fsck 修复记录仍可作为后续诊断材料，但不能凭这些现象断言某一项就是余下耗时的根因。

这份报告只增加项目文档，不修改 Rust、板端镜像、Cargo 默认 LTO 或旧实验归档。所有时间均为已保存的运行结果；尚未做的 PR 拆分、17 分钟验收以及未定位的故障都保留为未完成状态。
