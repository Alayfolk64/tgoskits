# 分阶段缩短 ext4 同步持锁

## 证据与目标

最新有效 `inode-writeback-read-cache-window-600`：ext4 占 mutex 累计等待
70.6943%，同步占 ext4 持锁 46.8197%（180805838160 ns）。同步内部 inode
表写回预读为 41373738704 ns，是最大的同步块读来源。任务累计时间不是
可直接扣除的编译墙钟时间。

最终目标是同步调用等待自己的持久化边界，而不是占用整个文件系统状态锁
等待设备。当前第一阶段只拆 inode 表预读；journal commit、实际写盘和
设备 flush 仍保留原有同步与锁边界，不声称已实现完整异步写回。

## Linux 依据与替代方案

Linux `980ab36ae5972c83f683b939e50c469c4947229e`：

- [ext4 fsync](https://github.com/torvalds/linux/blob/980ab36ae5972c83f683b939e50c469c4947229e/fs/ext4/fsync.c#L142)
  等待文件数据和目标 metadata transaction，保留 barrier 与写回错误。
- [JBD2 等待](https://github.com/torvalds/linux/blob/980ab36ae5972c83f683b939e50c469c4947229e/fs/jbd2/journal.c#L650)
  在等待目标提交完成前释放 journal 状态锁。
- [提交事务切换](https://github.com/torvalds/linux/blob/980ab36ae5972c83f683b939e50c469c4947229e/fs/jbd2/commit.c#L547)
  区分 running 与 committing transaction，解锁后提交数据。

这些机制不能由一个 `drop(lock)` 替代。当前 rsext4 的 fs、可变设备缓存、
pending journal 和游标共享同一外部锁；目录块还有直接写盘路径。直接删除
create 同步已被真实 fsck 回归否定，见 ext4-create-writeback.md。

保持原状没有并发收益；仅创建后台线程仍持原锁没有解决等待；一次性拆开
整个 journal、分配器和写缓存涉及事务隔离、块重用及故障恢复，不能与本轮
预读优化混成不可单独验证的修改。先复用现有独立 reader 和 inode 快照边界。

## 第一阶段设计与安全条件

这是共享 API / 并发高风险候选；本材料用于合入前独立审核，不代表已批准。

1. 全局锁内按原顺序 flush data 和 bitmap，取得 dirty inode 快照，复用
   pending / clean table block；不驱逐设备缓存，不提前清除 inode dirty。
2. 最多准备 64 个表块（256 KiB 表块缓冲），仅有冷读时离锁，由既有共享
   reader 读取到私有缓冲。表块是 filesystem metadata，位置不因普通文件
   truncate 改变；这与需要 inode 生命周期锁的数据块读不同。
3. 回锁后检查期间是否有其他 ext4 状态锁持有者。没有则合并并写回原快照；
   有则丢弃预读结果，重新按原同步顺序完成写回，不循环重试或无限占用内存。
   采用保守的锁序号，允许无关只读操作使预读失效；之后由真实窗口评估代价。
   序号饱和后禁用预读，不能因回绕接受旧快照。
4. 继续执行原 superblock / group descriptors / journal commit / flush。
   数据读失败立即传播，inode dirty 不提前清除；写失败保留现有重试状态。
   无独立 reader、无冷块、超出界限时继续原完整同步路径。

不新增后台线程、驱动协议、锁顺序、磁盘格式、缓存容量或 syscall ABI。
fsync/fdatasync、filesystem flush 及 create/unlink/rename 的同步成功边界
不变。本轮回归放在真实 rsext4/VFS 层，因为用户态无法直接断言该 mutex
在设备读取时是否持有；后续 QEMU 编译、正常关机和离线 fsck 补集成证据。

## 验证门槛

先在未修改生产实现时执行稳定 RED：真实冷 inode-table 预读仍持全局锁。
GREEN 覆盖锁外查询进展、预读期间修改同一 inode/邻接 inode、重新同步、
读写失败与重试、无 reader 和缓存命中路径；检查实际持久化字节和 fsck。
通过 rsext4 / ax-fs-ng tests、clippy、fmt 和 Starry 构建，再从同一冻结盘
运行 600 秒 QEMU 内核 profiling。收益未测前不得宣称提速。

## 已执行检查与当前窗口

在用户要求暂不做测试之前，ax-fs-ng 123 项、rsext4 259 项通过；后者另有
1 项原有外部镜像测试 ignored。项目 clippy 11 项及两 crate 实际 host 特性
clippy 通过。原实现的持锁预读、锁序号饱和和本次预读之后自动提交触发缓存
写回三个交错均保留了先失败后修复的证据。

补充不变量：本次 flush 自己也可能填满 journal 并提交，随后驱逐 dirty
设备缓存。这时后续表块的预读也可能过期，必须通过原读取优先级重新取值；
不能只检查锁序号来排除外部交错。

内核已构建并保存至
`target/profiling/arceos-helloworld/starry/sync-inode-preread-window-600`。
仅替换内核，从同一 SHA-256 的冻结盘启动，tg-xtask 已确认复用。
用户最新要求以性能为主线，此后不再补跑测试套件；后续回归未执行不记为通过。

## 首轮结果与写入序号修订

首轮实际 611229082288 ns，47 个已启动编译单元 / 44 个 crate，rc=124，
完整构建未完成。活跃样本 16115/47604（33.8522%），上一轮为 31.1745%。
全部 dropped/skipped=0，5/5 导出哈希一致，离线 e2fsck -fn 退出 0。
ext4 持锁总量 415140390592 ns，其中 sync_to_disk 205343366560 ns（49.4636%）；
mutex 总等待 2029461004288 ns，其中 ext4 调用点 1583297350752 ns。

不能据 42→47 单元报告稳定加速：窗口比上一轮长约 10 秒，工作量不同，
同步持锁反而增加。全量块读折叠栈表明，预读为 53559431136 ns，随后原
InodeCache::flush_all 仍读了 40188692656 ns。原锁序号对无关读也失效，
没有有效消除重复读取。因此首轮实现不是最终接受版本。

下一候选保持同步边界，改为：

1. Ext4Disk 在所有实际设备写入尝试之前推进饱和写入序号，覆盖 checkpoint、
   自动提交、普通写和 dirty cache 驱逐。失败也使快照失效；达到 MAX 后禁用
   预读。独占的 &mut Ext4Disk 是唯一生产写者，filesystem mutex 提供先后关系；
   Arc<AtomicU64> 仅供 fs 和 disk 共享计数，因此采用 Relaxed，不用它发布数据。
2. 锁外预读后重新按 data/bitmap 顺序写回，再核对设备写入序号。期间任何设备
   写入仍走原保守路径；只读查询不再丢弃预读。
3. 接受表块缓冲时重新选取当前 dirty inode 和 generation，不写旧 inode 快照。
   当前 pending journal / clean cache 优先于预读；新出现的脏表块按原路径读取，
   已清理的 inode 不再重复写。当前 flush 触发自动 journal commit 后，余下表块
   仍重新读取，保持前一版本发现的自身写回失效保护。

不新增驱动能力或共享写接口，独立 reader 仍只读。该修订涉及并发校验条件，
当前只做源码检查、格式化、内核构建和 QEMU profiling；用户明确要求暂不运行
测试。已有 fixture 仅随字段迁移调整，不把前一版本的通过结果冒充修订验证。
下一窗口还包含未变化 GDT 和缓存索引竞争两个已准备的小改动；组合结果不分别
归因于各项。未运行的 GDT/cache-index 单独内核保留在 built-unrun 目录。

## 写入序号修订窗口结果

`sync-preread-write-sequence-window-600` 使用 8 CPU / 8192M，QEMU 正常退出 0；
实际采样 609425673744 ns，runner elapsed=610，rc=124，完整构建未完成。
启动 44 个单元 / 41 个不同 crate，活跃 16077/47451（33.8813%），
全部 dropped/skipped=0。tg-xtask 和冻结源码复用，5/5 哈希一致；只读
e2fsck -fn 退出 0，25 条 extent tree 可收窄提示均未修改镜像。

ext4 持锁累计 431388288544 ns，sync_to_disk 为 197812563424 ns（45.8549%），
仍为最大持锁者。mutex 等待 2055313976752 ns，ext4 调用点占 82.8050%；
CachedFile 的 slice read_at 为 304580843344 ns，缓存索引等待为 33343807376 ns。
预读接受路径已出现，但读取耗时 61377089360 ns；原 flush_all 仍读取
31914113360 ns，接受路径自身又读取 7288779280 ns。与前一版本相比，
未证明稳定编译加速：47→44 个已启动单元，活跃比例几乎不变。
所有累计时间均包含多任务等待，不能等同于墙钟时间。

同步内部的 journal commit 块写为 39737291472 ns、flush 为 26729996224 ns。
继续采用既有有界批量写，将同阶段 descriptor 与相邻 payload 合并，详见
[journal-write-batching.md](journal-write-batching.md)。同步写盘仍持锁，
后续还需降低预读失效与 metadata 重复读取；当前结果不是完整锁拆分成功声明。

修订内核 ELF `b824db1d8abbb46675bd03fed67b6fbf4ffc2d6c34f6b4051e36f37c5501f4af`，
BIN `363d5af01756a5f7ef77df8174b24ad2f6dc751ee1933f91d77b6baa6201dd02`；
结束盘只读保存在 `tmp/axbuild/rootfs/rootfs-profile-sync-preread-write-sequence-window.img`。
