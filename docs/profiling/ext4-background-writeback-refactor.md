# ext4 事务与后台回写整体重构

## 目标与执行约束

为 StarryOS 内用根文件系统已有 `tg-xtask` 编译 `arceos-helloworld`
降低 ext4 全局锁等待。只在 QEMU 比较 StarryOS 与 Linux，配置固定为
8 vCPU、8 GiB，每个 profiling 窗口 600 秒；不修改 guest 工作负载、工具链、
已有 tg-xtask 或冻结源代码包，不运行实板，也不用 xtask perf 替代内核采样。

2026-09-10 用户要求：完整重构完成后，连续通过三轮静态检查才允许启动任何测试。
实现期间可以格式化、编译和 clippy，不执行测试二进制、QEMU、fsck 或 profiling。
测试代码的编译不代表测试执行或行为通过。发现问题后先修复，再重查受影响的检查项；
三轮必须针对最终代码，不能用重构期间的零散检查凑数。

目前状态：2026-09-10 20:59 三轮最终静态检查通过；适配层回归随后发现既有
ax-io 切片传输不推进位置，完成确定性 red/green 修复和补充三轮静态检查，
21:16:49 重新放行。新代码 QEMU 内核 51 项、rsext4 全套 585 项、适配层
213 项、ax-io 180 项及 2 项文档测试通过。21:41 首个重构后 600 秒窗口完成：
CPU 活跃占比 32.98%→38.87%，ext4 累计持锁 389.11→249.76 秒，完整构建未完成。
逐轮证据见 [最终静态检查记录](ext4-final-static-review.md)，单次测量限制、
I/O 代价和下一热点见 [窗口结果](ext4-background-window.md)。

## 证据、来源和范围

已归档的 `clean-disk-read-cache-window-600` 数据中，ext4 锁持有时间约
378.13 秒，sync 路径约 272.64 秒，占 72.10%。这说明当前优先级是
同步提交时的全局锁持有，不是仅凭 Linux 或 MOSS 的历史热点继续改缺页。
这些数字是旧实现基线，不是本次重构的结果。

迁移基线固定为 TGOSKits
`cc8faa92227f1f96dacd4bde1c873458b646e731`，包括已合入的
[PR #1957](https://github.com/rcore-os/tgoskits/pull/1957) ext4 事务基础。
不合并整个上游分支。只迁移 rsext4、必要 VFS/块缓存适配及其直接接口消费者。
上游仍在跟踪独立 committing 与后台适配
（[issue #2209](https://github.com/rcore-os/tgoskits/issues/2209)），
不能把上游同步事务实现表述为已经实现后台提交。
共享缓存边界参考 [issue #2206](https://github.com/rcore-os/tgoskits/issues/2206)，
不重新引入 rsext4 私有普通文件数据缓存。

Linux 参考源码固定为本地仓库 commit
`980ab36ae5972c83f683b939e50c469c4947229e`，重点是
`fs/jbd2/commit.c` 的 running/committing 分离、`fs/jbd2/journal.c`
目标事务等待和 `fs/ext4/fsync.c`。持久化契约参考
[JBD2 文档](https://docs.kernel.org/filesystems/journalling.html)、
[ext4 journal](https://docs.kernel.org/filesystems/ext4/journal.html) 和
[fsync(2)](https://man7.org/linux/man-pages/man2/fsync.2.html)。
借鉴语义与状态机，不把 Linux 的锁、线程或内存所有权模型移植进可复用库。

非目标：调度器、整个缺页/PTE/address-space 架构重写，完全并行的冷元数据查找，
新的 ext4 磁盘格式和不相关驱动重写。page-cache/mmap 保持既有共享可见性目标，
但收尾审查发现旧淘汰忽略失败和 TLB 完成的所有权缺口，必须修正后才能放行。
本变更改变并发、所有权和持久化时序，按高风险功能处理；设计不等于运行验收。

## 方案选择

保留旧同步实现在语义上保守，但仍让所有编译线程等待全文件系统 sync。
仅将 Mutex 换成 RwLock 不能解决修改型提交期间的排他等待。
仅删除 create/unlink/rename 的 sync 会暴露旧实现目录块不在完整事务内的问题，
既往已出现 fsck 的 unused inode 错误，不采用。
从旧日志实现自行重写会重复 credits、rollback、revoke、checksum 和 recovery 工作。
因此复用固定上游事务基础，在其上增加 owned 提交批次和 OS 层后台 worker。
代价是需要跨 core、缓存、VFS、运行时完成一次完整迁移。

## 状态、锁和所有权

```text
短 ext4 锁：完整 metadata operation → seal running → PreparedCommit
                                                 ↓ 释放 ext4 锁
commit worker：ordered data → journal writes → barriers/FUA → CommitReceipt
                                                 ↓ 短 ext4 锁
                             publish completion → checkpoint queue
fsync：先 flush file pages → capture SyncTicket → 锁外等待目标 durability
```

`PreparedCommit` 拥有不可变磁盘写入内容和事务标识，不借用整个 filesystem。
`CommitReceipt` 只由真实 I/O 完成生成；发布时校验挂载/事务身份。
`SyncTicket` 固定目标事务，后来的写入不能无限延长已有 fsync。
一个逻辑元数据操作的目录块、inode、bitmap、GDT 和 superblock
必须在同一 handle 中发布。空间预留发生在 mutation 之前；容量压力下锁外等待，
不能在半完成的元数据操作中自动提交。

running 接受新修改，committing 保留旧快照直到 durable；读可见性按
running、committing、checkpoint 的新旧顺序解析，保留 revoke 与块复用保护。
提交成功只推进 durability，普通 fsync 不要求完成所有 home-location checkpoint。
日志空间回收需要 checkpoint 顺序落盘与 tail 更新完成。

每个可写、有 journal 的挂载在 ax-fs-ng 拥有一个 commit worker，
使用已有 thread/notify/join 边界。rsext4 不依赖 scheduler、OS mutex 或全局 runtime。
正常脏事务默认最迟 5 秒开始提交，显式 sync 和空间压力提前唤醒。
只读、无 journal、运行时不可用时走明确的同步适配，不降低 durability。

后台周期还必须覆盖普通文件页缓存：只提交 core 的 dirty metadata 不会处理
覆盖写已有文件时仅停留在 CachedFile 中的脏页。OS worker 先对本文件系统的
页缓存 owner 做有限快照，释放注册表锁后沿原写保护/回写路径处理，再取得
commit gate 提交日志。不能持 commit gate 回写页缓存，因为 inode 写入可能
因日志空间不足再次要求提交。某页回写失败仍尝试提交其他已完成前缀，保留
失败页和原始错误，下个周期重试。ext4 的页缓存 owner 注册表独立于外层 VFS feature，
不依赖 scheduler；无 runtime 时仍走同步提交，但关闭和最后引用退役不会漏掉脏页。
关闭流程的页缓存冻结、最后引用和回写 admission 仍须一起完成生命周期审核。

共享块缓存保持每设备 1024 folio 的容量。folio 是包含若干磁盘块的缓存页；
短 index lock 只负责查找与固定条目，独立 folio 状态负责 loading、dirty generation
和 in-flight writeback。读盘、写盘、flush 和等待不能持有 index lock。
在途快照计入预算，只有干净且未固定条目可回收。回写成功只清理所提交的版本；
并发 redirty 保持脏状态。buffered、direct/FUA 和失败失效必须共享一致性规则。

当前实现采用独立 folio I/O 锁、数据锁和 generation；索引只固定 Arc 条目。
1024 页预算同时计入 resident 和快照，满预算时不额外分配，改为持单页数据锁
原地回写。这个分支会阻塞同页的 redirty，但不持索引锁、不阻塞不同页的缓存命中。
direct 与 buffered 范围以一个 64 位掩码一次性预留所需条带，冲突时在短 IRQ 锁外
等待，不持有 64 把睡眠锁（内核 lockdep 最多跟踪 32 把锁，且要求逆序释放）。
不相交范围可能因条带碰撞而串行，这是范围预留的明确代价。miss 插入单独串行化，
不影响已有条目命中。页预算等待复用 OS 层 TaskWaiters；可复用库不引入 OS 锁。

checkpoint 使用保守的 free/reuse admission gate：在同一次成功 seal 中关闭新的
mutation，提交并排空 checkpoint 后重新开放。所有可失败准备先于 seal，避免丢弃
已封存的 I/O owner。checkpoint 准备失败保留关闭状态，下次 sync 先完成 checkpoint，
再继续 staging。期间不持 ext4 锁等待设备，读仍可以进入。这不是 Linux 完整的
逐块复用隔离；core 与 adapter 已接通，行为验收仍未运行。

锁顺序：调用者不能持 ext4 全局锁等待 worker、日志空间或缓存容量；
worker 不持通知锁获取 filesystem 锁；notify/join 在广域锁外执行。
OS 接线采用一个独立 sleepable commit gate，而不再增加提交任务队列：后台线程、
显式 sync 和空间压力共用相同的 seal/execute/publish 入口。调用者可直接成为
commit owner，持 commit gate 在 ext4 状态锁外执行设备 I/O；普通 mutation
不获取 commit gate。这样固定 ticket 和提交顺序不需要另一套排队完成状态。
page cache 先完成已有 flush/写保护流程，再捕获日志目标。

## 错误与关闭

页缓存关闭采用两级 admission：先关闭 cached-write（包括 append、truncate 和
mmap 可写页发布）并等待已有操作完成，停止并 join 周期 worker，再排空本挂载
已登记的脏文件；其后才关闭 core operation admission 并执行 reap/commit/checkpoint。
这时内部回写仍能正常进入 inode 写入口，不需要线程身份判断、TLS 绕过或锁内回调。
VFS 只暴露可选的 cached-write admission 能力和借用式 RAII guard；没有每次写入
的堆分配，计数与等待继续由 ax-fs-ng 的挂载所有者负责，rsext4 不引入 OS 依赖。
失败在 final-clean 尝试之前允许恢复 worker 和两个入口；尝试 final-clean 后保持关闭。
全局登记表不能在 prune 时临时移走所有 owner；只移除可原子取得独占所有权的
干净对象，析构在注册表锁外进行。挂载最后引用的缓存退役也必须覆盖已 unlink 的文件。

该顺序借鉴固定 Linux commit 的
[`freeze_super`](https://github.com/torvalds/linux/blob/980ab36ae5972c83f683b939e50c469c4947229e/fs/super.c#L1980-L2099)
先停止用户写/缺页写、同步脏页，再停止内部写的分层原则；本次不新增 FIFREEZE ABI。
与只检查 core closing 的方案相比，多一个挂载级计数，但避免 cached overwrite
绕过关闭入口，以及先关闭 inode 导致最后一轮回写全部 EBUSY 的问题。

不确定的 journal I/O 错误保留失败事务和原始错误，进入 sticky abort，
唤醒所有 waiter，拒绝新的修改。不能将失败快照标为 clean 或伪造成功 receipt。
OS 缓存写 admission 同时保留第一次 journal 失败，不能让仅修改缓存的覆盖写绕过
core abort；失败关闭后的 reopen 只改变关闭状态，不清除原始 journal 错误。
shutdown 先关闭修改入口，再 drain data/commit/checkpoint；最后标记文件系统 clean。
worker stop/join 必须在 filesystem 锁外。失败不能清除恢复所需的日志状态。
仅在以上完整链路接通后，取消普通 create/unlink/rename 的隐式全盘 sync；
保留同步挂载/inode 和显式目录同步语义。

## 工作区保护与迁移记录

起点分支 `profiling/manual-20260907`，HEAD
`affddc3fecec02b31df94573063e4840433c9ebe`。
已有跟踪差异与源码（含相关目录未跟踪文件）备份在
`tmp/ext4-refactor-backup.HQPm3n/`：

- `source.tar.gz`：fs、Starry kernel、axruntime、rdif-block、profiling 文档和 Cargo 清单。
  SHA-256 `caf52a31e4d6864be5e6474b9bd5f0ff3578b78553e70421b8b2e33f5a766ee9`。
- `worktree.patch`：全工作树已跟踪差异，SHA-256
  `25498110084738c3b6aa6dedc5c0ad6cbdea60eb0833455940e7be98995b2179`。

冻结根镜像、当前运行结束镜像和旧 profiling 产物原地保留，不清理或覆盖。
迁移被上游替代的旧缓存及测试时，记录替代关系；备份不是回归覆盖的替代品。
非文件系统已有改动不回退、不格式化，不提交或推送。

### 实现进度（不是验收结果）

- 已迁移固定上游 core，并增加独立 I/O endpoint、owned commit/checkpoint、固定
  ticket、回执身份校验和失败状态保留。回执按可变借用发布，误投其他挂载不会
  消耗原挂载的完成结果。只有有效回执才允许推进日志空间和 durability。
- 已重构共享块缓存为短索引锁、独立页状态、有限次回写及总页预算；复用的
  TaskWaiters 移至 OS 层，空等待集合的通知走原子快速判断，等待项分配在 IRQ 锁外。
  页预算回收用 IRQ-safe 通知；最后一个设备消费者回写失败时，由注册表保留脏缓存，
  全局 sync 成功后才释放，不让错误日志替代资源所有权。
- 已接通后台 mount worker、显式同步共用 commit gate、容量压力 continuation 和
  shutdown admission/drain。clean superblock 在日志 checkpoint 完成后锁外发布。
  abort 的磁盘标记由独立 receipt owner 写入；运行中发生的错误不能在 ext4 锁内
  抢写 journal header，且保留原始失败与 abort 标记写入失败两个结果。
- 已编写 13 个 detached journal 用例，以及共享缓存、范围预留、等待通知、关闭入口、
  clean unmount、unlink 后 truncate 和 tmpfs symlink 原子发布的用例。全部未执行；
  编译检查也不等于行为通过。旧缓存回归替代关系仍未全部核对。
- 已迁移 Starry 目录游标/raw bytes、tmpfs/overlay 原子 symlink 接口，保留原有 poll
  接口与 `getcwd` 的 process-root 语义。tmpfs/overlay 新类型 rename flags 限于 REPLACE，
  其他值明确报不支持；并未声称完成全部 Linux renameat2 语义。
- 2026-09-10 16:35，`cargo xtask clippy --package rsext4` 的 3 个配置通过；
  16:35 ax-fs-ng 的 ext4/host-test 配置通过；16:23 Starry AArch64 8 CPU 内核构建通过。
  这些记录早于后续 checkpoint/范围预留修改，不能作为最终代码验收。
  17:15 Starry 全 feature/target Clippy 的 110 个配置通过；17:27 实际
  `cfg(axtest)` 的 AArch64 内核测试目标编译检查通过，未执行测试。
- 已恢复有界的普通文件锁外读取：短锁解析 extent、锁外读盘、回到短锁更新 atime，
  整个过程保留同 inode I/O 排他和挂载 admission。完成对象绑定原挂载，拒绝误投。
  超范围请求回退原串行实现；pending 单块读优先使用事务镜像，避免无效 home I/O。
  旧 pending-block 的 8 个回归已迁移，另有 checkpoint/revoke/设备边界静态覆盖。
- mount worker 已分离共有 stop/notify/join 所有权和 MMP/commit 各自任务；
  延迟到来的最后一个 inode Drop 不得跨过 shutdown admission 修改元数据。
- 冻结镜像、tg-xtask、源包和旧候选内核的完整哈希重新核验，记录在
  [产物清单](ext4-refactor-artifacts.md)。未改变 guest 的基准输入。
- 已接通每次打开的 `O_SYNC/O_DSYNC`，包括 positioned/AIO 写入；append 的游标
  在同步成功后才发布。17:54 Starry 全配置 110/110、ax-fs-ng 8/8 Clippy 通过，
  早于后续关闭和注册表改动，仍不是最终三轮验收。实际 cfg(axtest) 和 C syscall
  用例仅编译/语法检查，未执行。
- 周期 worker 已先写本挂载脏文件页，再提交 metadata，覆盖只改缓存的 overwrite
  和已 unlink 文件。注册表不再整体临时移出；prune 原子取得干净对象独占权后
  锁外析构，重复硬链接登记去重。回收回调不再持注册表自旋锁。
- 已接入两级关闭 admission 和第一次 journal 错误的缓存写拒绝；增加相应生产
  路径用例。此前仅 VFS 条件编译的多余 mut 已修正。18:57 ax-fs-ng 的 8/8
  配置通过；19:06 Starry 的 110/110 配置通过，退出码均为 0。
- 已接入 canonical inode reader 的事务快照、回滚身份保留和只读命中快路径；
  18:57 rsext4 的 host-test + USE_MULTILEVEL_CACHE 组合 Clippy 通过。
- 已接入同步 inode、共享挂载策略、普通/bind remount 区分，以及 statfs/proc
  观察路径；后台挂载的普通 namespace 和 metadata 操作不再无条件 sync。
  无后台 worker 的路径仍同步。18:53 axfs-ng-vfs 3/3 配置通过；C remount
  回归仅通过语法检查，未执行。ext4 Inode 已按 metadata/I/O/xattr/目录拆分。
- 已增加文件 writeback 独占 owner；msync 不再跨写保护回调持 file_data 锁。
  页回写/淘汰完成全部短写，错误保留原脏页，替换页先准备成功再移出旧页。
- 已补挂载关闭快照的独占 owner，失败挂载恢复登记且不丢失并发注册。
  inode 缓存先原子发布再安装 dentry 引用；unlink 不提前移除仍打开的缓存，
  只有真正 reap 前才移除 weak key。干净页回收保留 I/O 排他至失效/恢复完成，
  防止恢复旧页覆盖并发新脏页；回调不持 listener 列表锁。
  19:12 ax-fs-ng 的 lib/tests + host-test/ext4/vfs/profile 组合 Clippy 通过，
  退出码 0。以上全是中间编译检查，所有行为测试仍未执行。
- 已接入冷 inode-table owned 预读：锁外读取，回锁合并当前 dirty records，
  journal 最新块优先，后台会话和 checked 提交代次拒绝过期结果。具体设计见
  [inode-table 预读](inode-table-owned-preread.md)。核心 5 个用例、真实 adapter
  4 个用例和提交序列耗尽用例已编译，未执行。
- 已补原 profile 锁边界、元数据 reader rollback、硬链接 metadata、普通读取
  fallback 和 Starry 原 7 项 fs context/inode identity 的接线；映射见
  [回归迁移清单](ext4-refactor-regression-map.md)，仍有明确列出的未迁移项。
- 已接入 [页缓存退役边界](page-cache-retirement.md)：LRU 不再隐式释放仍映射页，
  非阻塞失效失败保留 canonical owner；truncate 与普通写串行化，锁外阻塞失效，
  失败帧进入退役列表而不重新写回或重装旧缓存。内核撤销 PTE 后等待跨 CPU TLB，
  重试不以 NotMapped/只读 PTE 冒充完成；旧 populate 成功回调才释放帧的路径已移除。
  普通缓存保留目标不变，但无法失效的驻留页可暂时超过目标，这是待测资源代价。
  20:08 实际 AArch64 SMP=8 axtest（含新增内核回归）和 ax-fs-ng 8/8 配置
  Clippy 通过。随后模块拆分、测试 listener 清理及新增 metadata miss/硬链接
  I/O 锁回归的组合 Clippy 与实际 axtest 编译也通过。所有测试均未执行。
- 已保留失败 TLB 失效的独立责任记录，映射 backend 析构后仍由缓存 listener
  重试原虚拟地址；相关真实内核回归已编译，未执行。AArch64 内核和 system
  QEMU/build 配置已统一为 8 CPU、8 GiB，配置契约已接线。
- 20:35 主路径实现冻结，开始最终三轮静态检查。旧回归替代映射收尾、
  mmap/回收生命周期复核、逐 syscall 兼容性证据属于本次最终审查范围；
  运行证据仍待三轮通过后取得，不能把编译通过表述为行为或性能验收。

### 同步写入口补齐设计

这是后台回写前的持久化兼容工作，不是另一轮性能测试。迁移起点的 Starry 保存了
`O_SYNC/O_DSYNC`，但没有把它们交给文件写完成边界，AIO 还直接调用 backend。
这些入口现在已经接通；下文记录方案依据，不代表已运行通过。

选择在现有 `ax_fs_ng::File` 持有每次打开确定的 `WriteSync` 策略，并由 Starry
构造函数转换 Linux flags。复用现有 `FileBackend::sync`，先完成 page-cache
写保护/回写，再等待 ext4 的固定事务目标。相比只在 `FileLike::write` 加同步，
这个位置同时覆盖 positioned/vector/AIO 写入；不把 Linux flags 放进可复用文件层。
策略默认 Buffered，Data 和 All 分别请求数据所需元数据与全部元数据同步；
ext4 暂时可以做更强的整事务同步。非普通文件/块设备不新增 fsync 行为。
不新增 RWF_* 支持，不改变 memfd seals 或设备专属写语义。

Linux 对照固定在上述 commit：
[`generic_write_sync`](https://github.com/torvalds/linux/blob/980ab36ae5972c83f683b939e50c469c4947229e/include/linux/fs.h#L2657-L2682)
在数据写入后返回 sync 的错误；
[`new_sync_write`](https://github.com/torvalds/linux/blob/980ab36ae5972c83f683b939e50c469c4947229e/fs/read_write.c#L585-L600)
和 [`ksys_write`](https://github.com/torvalds/linux/blob/980ab36ae5972c83f683b939e50c469c4947229e/fs/read_write.c#L728-L745)
只在成功时发布用户可见偏移。不能把已经更新的内部 `ki_pos` 误认为失败也推进 `f_pos`。
因此 positioned 写不移动游标；普通/append 写在同步成功后才发布新的偏移，
零字节写不额外启动同步；原始写错误不被后续同步覆盖。

验收设计：真实 File/Backend 上的 buffered 与 direct 写、Data/All、append、
positioned、零字节、写失败、同步失败及只读 fd。断言实际 backing 回调次序、
数据和偏移，不通过源码字符串判断。先编写回归，但根据用户硬门槛，buggy/fixed
两侧行为执行都延后到三轮最终静态检查之后；不能宣称已经完成 red/green。

### 同步 inode 与挂载的完成边界

在删除普通 namespace 的隐式 sync 前，增加有实际消费者的 VFS
`WritebackPolicy`：inode 提供可失败查询，挂载沿用 `FilesystemMountState`
共享策略；不另建 ext4 私有挂载 flags 副本，不在 `NodeFlags` 的不可失败查询中
隐藏磁盘错误。`File` 的正常/positioned/append 完成边界合并 inode、挂载和每次
打开的同步要求。namespace 与 metadata 在原子修改成功后按对应策略同步；
创建失败不触发同步，打开已存在文件不触发目录同步。无 runtime/无 journal 的
ext4 继续使用同步完成路径，不能依赖不存在的周期 worker。

Linux 对照：
[`IS_SYNC/IS_DIRSYNC`](https://github.com/torvalds/linux/blob/980ab36ae5972c83f683b939e50c469c4947229e/include/linux/fs.h#L2137-L2141)
合并 inode 与 superblock；
[`MS_RMT_MASK`](https://github.com/torvalds/linux/blob/980ab36ae5972c83f683b939e50c469c4947229e/include/uapi/linux/mount.h#L49-L53)
允许普通 remount 修改 synchronous，不允许修改 dirsync；
[`mount(2)`](https://man7.org/linux/man-pages/man2/mount.2.html)
说明 bind/remount 的共享边界。初始挂载先设置策略再发布，普通 remount 仅更新
可变同步位，bind-remount 保留全部文件系统同步策略。statfs 与 proc mounts
必须从同一个共享策略读取，不能只更新某个 mount 的显示 flags。

替代方案：仅改 ext4 会漏过用户态缓存写；仅改 syscall 会漏过 positioned/AIO
与内核文件调用；全部操作继续同步无法解决原热点。选择现有 VFS/文件完成边界，
代价是新增小型策略查询；命中现有 canonical inode cache 时不拿 ext4 全局锁。
新增用例覆盖策略组合、bind 共享、remount、创建与已存在打开、同步错误传播。
所有行为执行（含 buggy/fixed 对照）仍延后到三轮最终静态检查之后。

### inode 元数据命中路径的事务迁移

旧优化的只读缓存能力不能直接复制到新 core：新 metadata transaction 会 clone
并在失败时恢复整个 InodeCache。若 clone 只是共享 Arc，rollback 会共享被修改状态；
若直接替换 owner，挂载保存的 reader 会永久指向旧缓存。因此同一 canonical map
由缓存 owner 和只读 reader 共享，transaction snapshot 仍深复制 map，rollback
只恢复原 map 内容、不替换共享身份。metadata handle 活跃时 reader 返回 miss，
待成功或 rollback 完成后再发布；miss/锁忙回到原串行读取。不能读到未提交的中间 inode。

共享索引使用已在 Cargo.lock 中的 `spinning_top 0.3.0`，MIT/Apache-2.0，no_std，
基于已有 lock_api。将其声明为 workspace dependency，core 直接使用；不引入
ax-sync/scheduler，不自写 UnsafeCell 自旋锁，不新增磁盘或普通文件数据缓存。
已查阅本地固定版本源码及[版本化 API](https://docs.rs/spinning_top/0.3.0/spinning_top/)。
reader 只 try_lock 并复制一个 inode；修改 callback、inode-table I/O 和 cache victim
落盘均在缓存锁外。配置仍保持原 128 inode 上限，不在这一项扩大缓存容量。
额外成本是每挂载共享控制块、一个短锁、handle 期间的读屏蔽计数；不按 stat 分配。

VFS 使用 core 的统一 inode 解码逻辑，保留 LARGEDIR/HUGE_FILE、inode disk size、
device number 与 flags 的语义。验证需覆盖 miss、修改、rollback、evict/clear、
旧 owner 释放、硬链接和 unlink 后仍打开的 inode；旧性能报告不作为新代码结果。

## 三轮静态验收门槛

| 轮次 | 检查内容 | 状态 |
| --- | --- | --- |
| 1 | 接口、所有权、依赖、feature/target、fmt、受影响 crate clippy | 通过：185/185、修复后 Starry 110/110、真实测试模块编译、fmt |
| 2 | 锁顺序、通知丢失、redirty、事务原子性、barrier、空间/错误/关闭 | 通过：源码不变量审查、14/14 Clippy、fmt/diff |
| 3 | 完整最终 diff、生产调用链、所有迁移消费者、Starry 构建、回归覆盖映射 | 通过：实际内核构建、快照比对、fmt/diff、C 语法、runner 接线；行为待测 |

三轮记录必须包含具体文件、命令、退出码、问题和修复后结果；空泛“检查通过”无效。
三轮通过前不运行任何行为测试。重构期间的编译不计入这三轮。

之后才运行最低层生产路径回归：blocked device 下另一个 inode 可操作；
目标 fsync 不提前成功；并发 redirty；目录原子性；free/reuse/revoke；日志耗尽；
I/O 注入失败；关闭 drain；崩溃恢复和 fsck。然后运行现有 Starry 文件系统用例。
最后用冻结工作负载开展 8c8g、600 秒内核 profiling，与同规格 Linux QEMU 对比。
报告 active CPU、ext4 lock wait、commit/checkpoint 耗时和编译进度；
started units 不冒充 completed crates，单窗口不宣称稳定提速倍数。
