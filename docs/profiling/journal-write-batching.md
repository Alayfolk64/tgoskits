# JBD2 运行期日志写入批量化候选

## 状态与证据

已完成局部实现、确定性回归和真实 ext4 一致性检查；首个独立窗口在退出阶段 panic，
没有有效性能结果；带调用位置诊断的复测已正常结束，结果见文末。私有文件缺页
三阶段测量未改善整机活跃度，按最新数据继续处理底层串行写入。本材料涉及持久化错误路径，合入前
必须由文件系统边界维护者独立审核。

`cache-hit-window-600` 全部块写折叠栈合计 107449728144 ns，其中 create 调用链
75037405344 ns（69.8349%）。全部 mutex 等待 2415972710960 ns 中，ext4 锁
调用点占 81.6897%。获取锁前的调用者主要为 metadata 21.2978%、lookup 19.6147%、
read_at 16.5000%、set_len 12.9779%；这些百分比均以全部 mutex 等待为分母。
等待方不是锁的持有者，不能说 metadata 查询本身耗掉全部对应时间。

最新 `file-fault-window-600` 的 ext4 锁占 mutex 等待 86.0439%，CPU 活跃
26.6783%，启动 34 个编译单元，未完成构建。块写共 112305798960 ns，
create 占 45.7663%、unlink 占 34.5962%、set_len 占 12.0208%。下一轮以此为
直接对照，固定当前缺页实现和缓存容量；块写来源比例随编译阶段变化，不能
把单个调用者占比变化当作优化收益。

源码事实：适配层 create 保留完整 sync_to_disk；rsext4 的事务提交依次写 descriptor、
每个日志 payload、commit record、每个 home block 和 journal superblock。
payload 和 checkpoint 当前逐个调用支持多块的 `BlockDevice::write`，每次 count=1。
Ext4Disk 已把 count 转换为一个连续块区间请求，不需要新增驱动或块接口。
本候选不删除 create 同步，也不关闭 journal、设备 flush 或错误传播。

## 参考和替代方案

当前代码基于 `affddc3fecec02b31df94573063e4840433c9ebe` 的本地 profiling 工作树。
开放 PR #2015 head `6d5cc09f45a073680a270ae0b6047b24fd9eaff5` 的 JBD2 改动已读取：
它合并的是 `create_journal_entry` 的建盘清零，不是运行期 commit；还包含独立的
data/inode cache 批量 I/O 改动，不直接导入整套补丁。本候选与其 JBD2 修改互补，
但同文件变化仍需日后集成时核对。未对该 PR 作完整评审或合入判断。

本地 Linux commit `980ab36ae5972c83f683b939e50c469c4947229e` 的
`fs/jbd2/commit.c` 将日志 I/O 提交与等待完成分开，并保留数据/日志设备屏障和
commit 顺序。本候选只借鉴“同一持久化阶段内减少提交次数”，不移植 Linux
异步事务模型，也不把完成一次大 write 当成替代 flush 的保证。

保持现状最简单但逐块等待；删除同步已经被真实 fsck 回归否定；扩展事务容量
会改变更多缓存压力及自动提交边界；引入后台提交或拆 ext4 锁需要新的事务并发
协议。候选选择局部合并物理相邻块，保留提交边界、事务容量和同步完成契约。

## 计划边界

1. 只合并同一阶段的物理连续块，不能跨 journal inode 的映射间断或 ring 回绕。
2. 保留 descriptor → payload → flush → commit → flush → checkpoint → flush
   → journal cleanup → flush 的原顺序。日志转义副本与原始 home 内容分离。
   转义判断发现原有大小端错误，已先独立 RED/GREEN 修复，详见后文。
3. 批量缓冲有固定上限，分配失败必须显式返回；不得增大 kernel stack 或吞 I/O 错误。
4. 失败不得提前清空待提交队列、虚报成功或跳过后续恢复所需的 journal 状态。
5. 不同时扩大 inode/data cache，不混入调度、缺页或工作负载配置修改。

## 验证门槛

先用真实 commit 入口和可记录块写/flush 的设备构造确定性 RED：相邻 payload
仍逐块提交时必须失败。GREEN 同时检查磁盘原始字节、转义、barrier 顺序、非连续
地址拆分和失败传播。保留原有 journal recovery/CRC 测试，并用真实 ext4 镜像
重新挂载及只读 fsck 检查 create 一致性。之后才运行独立 600 秒 QEMU 窗口，
比较同步块写次数/耗时、ext4 锁等待、CPU 活跃和编译进度。

## 已实施与验证

生产提交流程移到私有 `jbd2/commit.rs`，`commit_batch.rs` 提供有界连续写缓冲。
最多分配 `JBD2_BUFFER_MAX * BLOCK_SIZE = 40960` 字节，在任何事务磁盘写入前
通过 `try_reserve_exact` 预留，失败返回 ENOMEM；不扩大事务队列或内核栈。
payload 使用可转义副本，checkpoint 使用原始队列字节；不排序、不去重，
仅合并原顺序下相邻物理块。每阶段最后显式写完缓冲，再调用原有设备 flush。

按 [Linux JBD2 格式文档](https://www.kernel.org/doc/html/latest/filesystems/ext4/journal.html)
（2026-09-10 查阅）的磁盘大端 magic/转义定义，核验了既有 replay 的还原逻辑。
新测试证实旧 commit 用 `from_le_bytes` 会将小端字节串误转义，checkpoint
失败后 replay 把它改成大端，原文 `replayed home block 11 differs`，退出 101。
先只把 descriptor/payload 两处判断改成大端，同一恢复测试与转义标志测试通过；
之后才实施批量提交。K 内核包含此独立正确性修复，性能归因须记录这一差异。

真实 commit 入口的连续块回归先 RED（退出 101）：三块 payload 与三块 home
各自执行三个单块 write；GREEN 后各变成一个 count=3 的 write，四个设备 flush
均保留。新八项用例包括连续/碎片块、10 块缓冲上限、重复 home 块的原有顺序、
大小端转义、逐个 write/flush 失败点、部分 checkpoint 后恢复和空队列。
设备故障是测试注入，不是对根镜像制造 I/O 故障。

实际命令与结果：

- `cargo test -p rsext4 --features host-test`：新增六项用例时全套 199 通过、
  1 项外部 Linux 镜像用例忽略；已有 Linux mkfs/debugfs/fsck 用例实际通过。
- 补足八项后 `cargo test -p rsext4 --features host-test --lib jbd2::commit_tests`：
  8/8 通过。补充测试曾触发 `clippy::op_ref`，删除多余引用后项目 clippy 3/3 通过。
- `cargo test -p ax-fs-ng --features host-test,ext4,vfs --lib metadata_tests -- --nocapture`：
  5/5 通过，实际创建普通文件/目录后 fsck、重新挂载、错误传播均检查。
- `cargo fmt -p rsext4`、`git diff --check` 通过；固定配置 Starry 内核构建成功。

以上执行均设置 `TMPDIR=/home/wuxun/Projects/tgoskits/tmp`，没有使用实板。
未声称完整 Linux syscall 差分、任意磁盘故障后重试状态机或所有 ring 几何兼容性
已验证；本轮不改既有 journal cursor 算法。合入前仍需持久化边界审核。

K 保存目录 `target/profiling/arceos-helloworld/starry/journal-batch-window-600/`：
ELF SHA-256 `bfc2267648947cf853b46b0a01be7eb8394029e51eec2d61e2b8d24eb95a85b8`，
BIN SHA-256 `eeaac216c270ee5903f8a864af1532a6cf4100973cd0b51f220e131e8c9df38b`。
当前根镜像另保存为 `tmp/axbuild/rootfs/rootfs-profile-before-journal-batch.img`，
只用于回滚与核对；未修改冻结源码归档或 tg-xtask。
原镜像与备份的完整 SHA-256 均为
`83f00a751b61417207341b973ae315de034d7c62c166523c989e9b9429c62d89`。
镜像备份/全量校验会读取宿主缓存；本系列未清理全机 page cache，因此只固定
guest 冷启动与冷构建，不能声称宿主磁盘缓存也是严格冷态。

## K 首轮异常（2026-09-10）

最后一次进度为 541 秒、32 个编译单元、30 个不同 crate，随后串口原文：

```text
panicked at os/StarryOS/kernel/src/task/process/topology.rs:102:14:
process topology outlived its PID identity
BACKTRACE_BEGIN kind=panic arch=aarch64 alloc=false dwarf=false
BT_ERROR requires_alloc
BACKTRACE_END
```

未出现 WINDOW-PASS，未完成 profile 导出，不能用于 J/K 性能比较。串口位置只
确定 Weak PID identity 升级失败，尚不能确定具体调用者或认定 journal 改动导致。
下一轮仅为 `Process::identity` 添加 `track_caller`，保留失败语义以定位实际调用点。

QEMU PID 375850 已退出。失败镜像只读 `e2fsck -fn` 返回 4：提示跳过 journal
recovery，多项 deleted inode zero dtime 和 inode/block bitmap 差异。未修复原图，
已改名封存为 `tmp/axbuild/rootfs/rootfs-profile-journal-batch-panic.img`。
从上述运行前备份复制出新的固定工作镜像，其只读 fsck 返回 0。非正常退出后的
脏状态不能直接归因于批量日志写入；后续有效窗口仍须通过退出后的完整 fsck。

## K2 有效窗口（2026-09-10）

目录 `target/profiling/arceos-helloworld/starry/journal-batch-caller-window-600/`。
相对 K 只增加 `Process::identity` 的 `#[track_caller]`，没有 PID 生命周期修复，
也不包含下一项完整块写回优化。ELF SHA-256
`065867da7528086ee828ad9f00fe03a8beac22b71e7b5438cd7485b97d9faa61`，BIN
`fa82b571b83c26ff9d65808f557b8258291aaa00eb289fbe5c603116e4e8147d`。

窗口 PASS：elapsed=608、rc=124、build_completed=false、tg_xtask_reused=true。
内核采样 608384423984 ns，启动 33 个单元、31 个不同 crate；47691 个 CPU
样本中活跃 11650（24.4281%）、idle 36041。全部丢样计数为 0；五项 SHA-256
通过，正常关机后 `e2fsck -fn` 返回 0。CPU-active 与 mutex-wait 的 SVG/PNG
已经生成并实际查看。PID panic 未复现，不代表已经修复。

| 指标 | J：缺页三阶段 | K2：日志合批 |
| --- | ---: | ---: |
| 实际采样秒数 | 612.493 | 608.384 |
| 活跃样本比例 | 26.6783% | 24.4281% |
| 启动编译单元 | 34 | 33 |
| 同步块写调用数 | 48616 | 25563 |
| 同步块写累计 ns | 112305798960 | 85974902112 |
| 同步块读累计 ns | 351241293312 | 365181206320 |
| mutex 等待累计 ns | 2533101181264 | 2595672828224 |

日志合批减少了本窗口的块写次数/累计延迟，但整机活跃和构建进度没有改善，
不能宣称编译加速。所有延迟都是任务累计量，包含嵌套和采样放大，不是墙钟比例。
全量 mutex 折叠栈统计：ext4 锁叶子 1943609470128 ns（74.8788%），
CachedFile::read_at 锁叶子 586850686128 ns（22.6088%）；经过用户缺页入口
883864267760 ns（34.0515%），与上述类别重叠。

最宽单一完整等待栈是 rustc 缺页读取等待 CachedFile，478119180960 ns
（18.4199%）。最长单次 mutex 等待 13099601824 ns，对应 task=ld；用本轮 ELF
符号化为 `sys_writev → File::write → CachedFile::write_at_locked → Inode::set_len
→ Ext4Filesystem::lock`。这是等待方，尚不能确定当时的持锁者。

## Descriptor 与 payload 同阶段合批（2026-09-10，后续候选）

`sync-inode-preread-window-600` 中，`sync_to_disk` 持锁累计
205343366560 ns，占 ext4 持锁累计 49.4636%；其中 `umount_commit`
块写累计 50318980288 ns。当前提交路径已合并 payload，但 descriptor
仍单独等待一次 write，二者之间本来没有 flush 屏障。

继续复用私有 `CommitWriteBatch`：descriptor 作为同阶段的第一个块，
仅与紧邻的 payload 合并。scratch 上限从 10 块增加到 11 块（45056 字节），
其中一块给 descriptor；事务队列容量不变，不增加内核栈占用。
碎片映射、ring 回绕仍结束当前 run；commit、checkpoint、cleanup 前后的
四次 flush 保持不变。日志转义仍只修改 payload 副本，失败直接传播。
这不是后台提交，也没有把 journal I/O 移出 ext4 全局锁。

Linux `980ab36ae5972c83f683b939e50c469c4947229e` 在
[`commit.c:653`](https://github.com/torvalds/linux/blob/980ab36ae5972c83f683b939e50c469c4947229e/fs/jbd2/commit.c#L653)
把 descriptor 加入 `wbuf`，并在
[`commit.c:738`](https://github.com/torvalds/linux/blob/980ab36ae5972c83f683b939e50c469c4947229e/fs/jbd2/commit.c#L738)
一并提交这一阶段的 buffer。本候选借鉴阶段划分，不声称移植其异步完成模型。
保持现状会保留独立 descriptor 请求；增大事务容量或后台提交会改变更多
一致性边界。本次只扩展现有 phase-local 合批，不新增 API、磁盘格式或驱动契约。

按用户当前要求暂不运行测试套件；既有事件断言随预期请求边界调整，不删除
barrier、失败点或磁盘字节断言。实施后执行格式化、定向 clippy 和必要内核构建，
然后在固定 8 核 / 8 GiB、600 秒 QEMU 窗口比较。此处仅记录设计，
运行结果未取得前不宣称性能提升；合入前仍需文件系统持久化边界审核。

### Descriptor 合批窗口结果

`journal-descriptor-batch-window-600`：613278508656 ns、42 个启动单元 / 39 个
crate，rc=124，完整构建未完成；QEMU 退出 0，tg-xtask 复用，全部丢样为 0。
活跃 12635/47993（26.3268%），低于父窗口 33.8813%；5/5 哈希通过，
e2fsck -fn 退出 0，13 条 extent tree 可收窄提示均未修改原盘。

块写调用从 45172 降至 25987，但累计耗时 117513923440→116131727568 ns，
块读 469664245776→507559338528 ns、flush 39674887024→57577306352 ns。
ext4 持锁 422417752624 ns，其中 sync_to_disk 246588413264 ns（58.3755%）。
mutex 等待 2337629230448 ns，其中 ext4 调用点 1688544347408 ns（72.2332%）。
不同并行编译进展也改变各请求分布，不能把全部次数差异归因于少一次 descriptor
请求，更不能将单次合批的源码改进认定为整机加速。本候选尚未证明提速，
保存为后续对照；下一窗口只叠加独立的干净块读缓存。

ELF `d539a204f2850bb53c8ecc9e524068e4dbfac7b1a21f4655757b908f6abdc3bd`，
BIN `21e831ac068664d9b4f2eb0a217273c676d1a5684e14fec1d061273b057836e2`；
只读结束盘 `tmp/axbuild/rootfs/rootfs-profile-journal-descriptor-batch-window.img`。
