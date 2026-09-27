# ext4 元数据更新与显式同步边界

## 问题与范围

F（`cache-256m-window-600`）的内核 mutex 等待中，69.9461% 位于
`Ext4Filesystem::lock`，27.3822% 位于页缓存 `io_lock`。CPU 活跃比例仍仅
23.9669%。源码确认读文件的 `File::drop` 更新 atime，然后
`Inode::update_metadata` 无条件调用 `sync_to_disk`，在整个文件系统锁内刷新
data/bitmap/inode cache、superblock、group descriptors 和 journal。
等待火焰图证明争用位置；关闭文件的隐式同步占整体争用多少，仍需 A/B 测量。

对 F 的全部 `block-write-sync.folded` 求和（不是只取 top 栈）得到块写总耗时
165,899,628,416 ns。其中含 `File as Drop::drop` 的路径为 111,670,267,184 ns
（67.3119%），含 `DirNodeOps::create` 为 42,164,427,232 ns（25.4156%）。
已经实际查看对应块写 SVG 转换的 PNG。这里是块写区间分布，不是 mutex 等待
分布，也不能等同 CPU 执行时间；栈深上限 20 可能截断更上层的调用者。

本候选复用 rsext4 已有 dirty inode cache，只移除 `update_metadata` 后的
隐式全盘同步。所有字段仍在原锁内立即更新；显式 inode sync、filesystem flush、
shutdown 和命名空间操作保持原实现。不更换调度器、不改 tg-xtask、不改变磁盘格式。

## 设计与替代方案

这是持久化时机变化，按高风险处理；本地实验不等于通过合入审核。
不新增 API、锁、依赖、unsafe 或后台线程。保持旧实现会让只读访问也强制写盘；
关闭 atime 会改变可见时间戳，故不选；拆分 ext4 全局锁涉及更大的缓存/日志并发
契约，先不选。复用现有 dirty cache 与 `NodeOps::sync`，与已经延迟回写的
`write_at` / `set_len` 保持一致。

已检查当前实现、路径历史（包括 `5d13ce881` journal 错误传播）、开放 PR #2015
（head `6d5cc09f45a073680a270ae0b6047b24fd9eaff5`）及 ext4 sync issue 检索。
#2015 涉及底层批量缓存 I/O，未改本次 `ax-fs-ng` 元数据同步路径，不直接合并。

外部依据为 Linux man-pages 6.18（页面日期 2026-02-08）：
[close(2)](https://man7.org/linux/man-pages/man2/close.2.html) 不承诺关闭即落盘；
[fsync(2)](https://man7.org/linux/man-pages/man2/fsync.2.html) 是数据与关联元数据的
显式持久化边界。这里没有宣称实现完整 Linux writeback/journal 语义。

风险：未显式 sync 的元数据在断电后可能丢失；不提供 Linux 周期 writeback
时限保证。现有 `O_SYNC` / `O_DSYNC` 在打开 flags 接受，但当前 File::write 没有
对应同步分支，本候选不声称修复该已有缺口，也不依靠 close 冒充 O_SYNC。
`sys_sync` 当前只遍历根文件系统的限制也保持不变。

## 验证计划

最低层确定性测试使用 Linux mkfs 创建真实 ext4 镜像，经生产 Ext4Filesystem、
Inode 和 File::drop 路径统计底层 write/flush。旧实现在不应隐式 flush 的断言
上必须失败；修复后立即查询元数据、显式同步后重新挂载、flush 错误传播和只读
`e2fsck -fn` 均须通过。测试镜像保存在仓库 `tmp/ext4-metadata-tests`。

本次不增加 Starry syscall case 的具体理由：用户态没有观察底层设备 flush 次数的
接口，close 是否发出额外 flush 也不是 Linux 要求的失败结果。以耗时或断电概率
作 syscall 回归不能保证旧实现失败。真正回归放在具有设备计数与故障注入能力的
生产 VFS 边界；原始 QEMU 编译与根文件系统 fsck 提供集成覆盖。没有修改 syscall
ABI、权限校验、参数布局或 errno 映射，所有显式同步调用链保持不变。

测试通过后，固定 SMP8 / 8 GiB / aarch64 TCG 和复用根镜像中的 tg-xtask、源码包，
仅换内核，运行同样 600 秒窗口。比较 CPU idle/active、ext4 锁等待、块读写和编译
进度；完整构建未结束时不能把编译单元数量当速度倍数。失败或磁盘错误触发回滚
此单独候选并保留日志，不修改用户文件。

## 已完成的 RED / GREEN

2026-09-10 本地旧实现：3 项新测试中 2 项按预期失败（退出 101）。
元数据更新前后 `(块写次数, flush 次数)` 为 `(40, 15) → (52, 20)`；
读完关闭为 `(62, 25) → (74, 30)`，均额外产生 12 次块写和 5 次 flush。
先修正过测试直接比较 NodePermission 的编译错误（该类型无 PartialEq，改比 bits），
没有把编译错误充当 RED。

移除隐式同步后，新测试 3/3 通过，整个 ax-fs-ng host-test/ext4/vfs 库测试
94/94 通过（0 ignored），包含镜像重新挂载和每个 fixture 的只读 e2fsck。
`cargo fmt -p ax-fs-ng`、`cargo xtask clippy --package ax-fs-ng` 8/8，
以及补充 host-test/ext4/vfs 的 `cargo clippy --lib --tests --no-deps` 均通过。
QEMU 数据及镜像验收见下文。

G 实验目录：`target/profiling/arceos-helloworld/starry/metadata-writeback-window-600`。
构建命令仍是 `cargo xtask starry build -c apps/starry/macos-selfbuild/build-aarch64-unknown-none-softfloat.toml`。
配置只选择已有 SMP8/guest-profile 内核，不打包新的工作负载。保存内核哈希：

- ELF：`a72290912f8739713c91ea176e2af300371b1a30ca03270b49ca9d63793e7eea`
- BIN：`038e596f6df954572f4d78153036e10343cdbd95a3c9b8624eb3d7d571d5f0b2`

## G 窗口及历史镜像残留

G 实际采样 602.441100432 秒，34 个已启动编译单元、31 个不同 crate，rc=124，
构建未完成。CPU 样本 47,156，活跃 12,968（27.5002%）、idle 34,188。
mutex 等待总和 2,298,379,853,568 ns，ext4 锁等待 1,923,044,084,432 ns；
块读 81,678 次 / 292,211,108,560 ns，块写 47,566 次 / 120,937,260,512 ns。
page-cache inclusive 1,009,849,591,808 ns，所有 dropped/skipped 计数为 0。
已查看本轮 mutex 和 cpu-active 火焰图 PNG，五个导出文件的 SHA256SUMS 全部通过。
全部块写折叠栈中 File::drop 为 0 ns，create 为 80,326,764,560 ns（66.4202%）。
与 F 相比已有进度和 CPU 活跃改善，但不能据此给出完整构建加速比。

首次只读 e2fsck 退出 4：`Inode 75297 seems to contain garbage. Clear? no`。
75297 是有效的 `/opt/starryos-selfbuild-artifacts/arceos-helloworld-profile/SHA256SUMS`，
内容、链接数和 extent header 正常，没有清除该文件。

完整保存当前镜像到 `tmp/axbuild/rootfs/rootfs-profile-metadata-writeback-preserved.img`。
核对 75297 所在 inode table block 4995：剩余 15 个槽位 75298–75312 均为
历史已释放、零链接/大小/块数、空 block map，但仍带 EXTENTS_FL；这 15 个 inode
的完整 256 字节都与更早的 `rootfs-profile-scheduler-ready-preserved.img` 一致。
[e2fsprogs v1.47.2 inode.c](https://github.com/tytso/e2fsprogs/blob/v1.47.2/lib/ext2fs/inode.c)
的 check_inode_block_sanity 在超过半数槽位异常时把整个表块标为 INSANE，
故本轮新分配的正常邻居被报 garbage。不是仅凭文件可读就忽略 fsck。

进一步只读扫描全部 1,048,576 个 inode，发现 555 个同类零映射 extent 标志残留，
全部通过 inode bitmap 确认为空闲，且全部完整字节与上述历史备份一致。
审计清单保存在 `tmp/profile-free-inode-audit.json`。它们均早于本轮；之前只修复
当时已触发 e2fsck 的局部槽位，没有清理所有潜在残留。当前内核已经包含 free_inode
清理 EXTENTS_FL 的修复，不能用本次新出现的邻居告警误判该修复回退。

恢复只允许清除这 555 个已核验空闲槽位上的 EXTENTS_FL，并保留整盘备份；
不运行会清除有效邻居的自动 e2fsck 修复。恢复后须验证整盘差异只在这些标志位，
只读 fsck 返回 0，导出的采样和 SHA256SUMS 不变，才启动后续测量。

恢复已完成：两份恢复前 16 GiB 镜像 SHA-256 都是
`16e3a8ef0d65f10891d43dcc83786d119b8452b4db6739cec3ee0da01387950a`。
debugfs 仅对审计名单运行 `set_inode_field <ino> flags 0`，没有运行清除 inode
或删除文件的命令。随后 `e2fsck -fn` 返回 0，所有 5 个采样文件哈希再次通过。
对整盘执行 `cmp -l` 并逐项核对审计偏移，共 558 字节变化：555 个 flag 字节
从 0x08 到 0x00；另 3 字节属于 debugfs 自动更新的 superblock s_wtime 和
s_kbytes_written（增加 148 KiB）。没有其它差异，因此所有有效 inode 和文件
内容完全不变。原始失败、恢复前镜像及差异清单均保留，未把恢复前 fsck 改记为通过。

G 的 mutex 原始地址全量聚合：Ext4Filesystem::lock 调用点 `0xffffffff803dc128`
为 1,921,977,520,480 ns（约 83.6%）；CachedFile::read_at 的 io_lock 调用点
`0xffffffff80174804` 为 340,592,308,272 ns。均用 G 的 ELF 核对符号。
后续对 create 隐式同步的候选已被真实镜像一致性测试否定，详见
ext4-create-writeback.md；当前转向 inode-identity.md 所述的关闭路径等待。
不实施尚未验证的页缓存并发草案。
