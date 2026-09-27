# 单块读取复用尚未提交的日志快照

## 问题与最小边界

U 窗口块读 421953600496 ns，其中 Jbd2Dev::read_blocks 直接调用者
41484551136 ns，inode flush_all 调用者 44314606752 ns。两者并不
说明全部命中 pending queue，不能把这两个时间当作候选的收益预测。
源码确认 Jbd2Dev::read_blocks 总是先读 backing，再覆盖 pending bytes；
即使请求只有一块且完整内容都在当前日志队列中，仍依赖一次旧块读取。

既有 read_block 已直接复用 pending queue；pending_block 也由锁外文件
准备使用。这一候选只把同一语义用于恰好一块、恰好 BLOCK_SIZE 字节、
底层块大小一致的 bulk read。命中则复制完整快照，不提交、不更新 journal
状态、不扩大缓存、不改写磁盘。短缓冲、多块、过长缓冲、关闭 journal、
未初始化和未命中均保持原路径。单块限制覆盖 inode 读/改/写的真实调用，
避免新增多块分段读取/回滚或额外分配。

备选为保持现状、批量全命中检查、或拆分缺块 I/O；后两者扩大边界，必须
在先确认单块收益后再决定。队列访问仍通过已有 &mut Jbd2Dev 独占边界，
无新锁、公开 API、持久格式、控制参数或依赖。不能删除 create 同步和
flush 屏障，也不能让 inode 部分记录覆盖同块的其它记录。

已读 journal facade、cached_device、inode_table 调用点、既有 overlay
测试以及 5d13ce881/669e90adf 历史。当前是本项目内部缓存的行为/性能修复，
没有新增 Linux ABI 或 journal 格式；无需引入新的 syscall 语义比较。

## 回归与状态

pending_block_reads.rs 直接调用真实 Jbd2Dev，覆盖完整命中、替换后的
最新内容、miss/EIO、多块覆盖、短/长缓冲、关闭 journal 与零块请求。
先运行旧生产实现，要求 no-read/EIO 两项确定性失败，再实施快路径。
V 清零性能窗口期间只编写测试；窗口结束后运行旧生产实现，退出 101、
6 通过 / 2 失败：完整 pending 块遇到 backing EIO 返回错误，替换后的
pending 块仍产生一次 backing read。原始错误完整输出后实现 11 行快路径，
同一组 8/8 GREEN。cargo fmt --package rsext4 和 git diff --check 通过。
V 仍是纯清零候选；W 的测量需要另一次冻结根盘恢复，不能复用 V 的结束盘。

V 再次显示同步占 ext4 持锁 48.52%，inode flush_all 调用者块读
49907279056 ns，Jbd2Dev::read_blocks 调用者块读 29386858944 ns。
W 的目标是命中时的重复读，不能预先把两项时间全计为可消除成本。

## W 验证与启动

rsext4 全套测试退出 0（原有需要外部镜像的一项 ignored 保留），
ax-fs-ng host-test/ext4 101/101、加 profile 后 112/112；
cargo xtask clippy --package rsext4 的 base、USE_MULTILEVEL_CACHE、
host-test/tests 三项全部通过。固定 Starry profiling 配置构建通过，
13048 个 kallsyms，已有 core/memchr future-incompatibility 警告不属本变更。

窗口目录 target/profiling/arceos-helloworld/starry/pending-block-read-window-600。
ELF SHA-256 a2bddc8edec9dbefe1daee4e42ee777003edd43db7b8ff8e7cd7908a15c2d8fd；
BIN 682efd8d3433fef2be9e389728893e9eba7f060db3c812d078381986d5e72fa6。
V 结束盘保留后重新复制冻结根盘，完整 SHA-256
d5e8c6347879117246530ba52dacba58a03379f06ddaf19866ca576e9cdd57d4
完成后才于 00:40:50 UTC 启动 W；8 核 / 8 GiB / cortex-a53 / NVMe / TCG，
无 GDB，无其它 QEMU/构建/下载运行。

## W 完整窗口结果

正常结束 607 秒、rc=124、build_completed=false，启动 45 单元 / 42 crate，
21377 条记录。prebuild_ns=607032526192，所有 dropped/skipped 为 0。
五项 SHA256SUMS 全部通过，e2fsck -fn 退出 0。debugfs 退出 0，但恢复
八个文件/目录的属主时报 Operation not permitted；原文完整输出，导出内容
另行哈希校验通过。实际打开 cpu-active 和 ext4-lock-hold PNG 核对。

| 指标 | V（清零） | W（加 pending 读取） |
| --- | ---: | ---: |
| 活跃 CPU 样本 / 总样本 | 13226 / 47235 | 16202 / 47289 |
| 活跃比例 | 28.00% | 34.26% |
| 启动编译单元 / crate | 40 / 37 | 45 / 42 |
| ext4 持锁累计 ns | 411761523312 | 392234691088 |
| sync_to_disk 持锁 ns | 199790058640 | 172056955456 |
| inode flush_all 调用链块读 ns | 49907279056 | 41737759408 |
| 总块读累计 ns / 次数 | 390046637616 / 62143 | 379814997664 / 67926 |
| 总块写累计 ns / 次数 | 142760725888 / 31248 | 121251130576 / 49232 |
| flush 累计 ns / 次数 | 30087835296 / 9052 | 27423133984 / 10206 |

本轮互斥锁累计等待 2101514140160 ns，ext4 为 1587750567968 ns
（75.55%），CachedFile::read_at<&mut &mut [u8]> 为 501000591536 ns
（23.84%）。等待时间可跨任务重叠，不是窗口墙钟时间。
同步仍占 ext4 持锁 43.87%；其中 create 146102389184 ns、unlink
20745304512 ns、rename 5088132592 ns，其余 121129168 ns。
创建同步成本几乎没变；总同步下降主要落在删除链，不能将差额全部归因于
候选命中，因为窗口内工作量、调度和任务阶段不同。

缺页调用链 CPU 2915 / 16202（17.99%）；全局清零相关叶为 DC ZVA
437 + memset 113，缺页清零子集 432 + 33。find_free_area 仍有
1040 个叶样本（6.42%）。exit_preemption 的 1436 个叶样本需考虑
已证实的 IRQ 恢复采样偏移，不能直接解释为该函数真实独占 CPU 成本。
W 内联后部分 inode flush_all 块读叶的直接调用者变成 Jbd2Dev::read_blocks；
应按完整调用链比较，不能把旧直接调用者从排名消失当成 I/O 已消除。

单轮进度与活跃率正向，保留窄边界候选继续验证；尚无重复测量或完整构建
加速比。当前低 CPU 的最大证据仍是 ext4 全局串行等待，不是清零本身。
