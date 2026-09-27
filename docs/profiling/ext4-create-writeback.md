# ext4 create 的隐式全盘同步

## 结论：拒绝直接删除同步的候选

以下初始方案经真实镜像检查被否定，生产 create 的 sync_to_disk 已恢复。
未构建或运行此候选的 QEMU 性能窗口，不能报告收益。

源码及实际调用栈确认：insert_dir_entry 调用 DataBlockCache::flush，最终以
`is_metadata=false` 写目录块，绕过 journal；新 inode 和 bitmap 则仍在缓存。
删除适配层同步后，创建仅增加一次 4096 字节目录块写，不增加设备 flush。
所以最初的“创建零 write/flush”断言不成立；更重要的是，不仅是统计口径问题，
返回成功时目录项可能已经指向磁盘上仍被标为空闲的 inode。

改用真实一致性检查，而非放宽计数断言：在创建成功后、任何额外 VFS sync 前
执行 `e2fsck -fn`。文件和目录两项都失败，测试退出 101，e2fsck 退出 4：

```text
Entry 'created' in / (2) references inode 13 found in group 0's unused inodes area.
Fix? no

Entry 'created' in / (2) has deleted/unused inode 13.  Clear? no
```

这里是只读检查，未执行 journal recovery；该操作新增的唯一磁盘写是目录块，
不是一笔包含 inode/bitmap 的已提交 journal。证据镜像保留于
`tmp/ext4-metadata-tests/create-writeback-341811-0.img` 和 `-1.img`。
恢复原 create 同步后，同样两项检查通过。测试保留为当前适配层约束；不把它
推广成 Linux 普通 create 必须同步的 ABI 承诺。要继续批量异步 create，必须先
完善目录块、inode、bitmap 的事务边界与恢复验证，本轮不做这个跨层重写。

## 初始方案（已否定，保留判断过程）

## 证据与目标

G 的 600 秒窗口显示，关闭文件的块写降为 0 后，create 占剩余块写耗时
66.4202%（80,326,764,560 / 120,937,260,512 ns），ext4 全局锁仍占 mutex 等待
约 83.6%。目标是在真实创建路径保留目录项、inode、权限与 owner 的立即可见性，
消除每次 create 都强制提交整个文件系统造成的等待，并保留显式同步与错误传播。

## 设计、替代方案与风险

只删除现有 `DirNodeOps for Inode::create` 在完成缓存修改后的隐式 sync_to_disk。
直接复用已有 inode/data/bitmap/journal 缓存与目录 NodeOps::sync；不新增 API、
锁、unsafe、依赖或磁盘格式。`unlink`、`rename`、`set_symlink` 等路径不变。
同路径 `link` 原本没有末尾强制同步，但这里不推论它已有完整的崩溃一致性保证。
内部历史、PR #2015 与底层缓存方案检索见 ext4-metadata-writeback.md。

与上一候选一样，改变持久化时机按高风险处理；此材料与实现需在合入前单独审核。
保留现状会让编译中每个新文件/目录触发全局提交；批量或拆分 journal/文件系统锁
需要更大协议改动，故先采用这一已有缓存边界内的候选。不是把数据放在 tmpfs，
也不关闭日志、设备 flush 或 fsync。

Linux man-pages 6.18（2026-02-08）的
[fsync(2)](https://man7.org/linux/man-pages/man2/fsync.2.html) 明确区分文件同步和
目录项同步：目录项持久化需对相应目录显式 fsync。普通 create 不能被当成整个
文件系统的持久化屏障。本候选仍须保证显式目录同步、filesystem flush 和 shutdown
把已有缓存正确写回；不声明断电恢复的完整 Linux 等价性或周期 writeback 时限。

## 初始测试与实验计划（未按此方案运行）

在已有真实 mkfs ext4 / VFS fixture 中，分别测试普通文件和目录创建：
创建后无需全盘 write/flush 即可 lookup/stat 到正确 inode、类型、权限及 owner；
显式目录 sync 后重新挂载能观察同一对象，e2fsck 通过。旧实现必须先在设备 I/O
计数断言失败；修复后同测试通过。设备 flush 错误传播仍由前一候选测试覆盖。
这里不添加依赖耗时的 syscall 测试：缺少观察设备 flush 次数的用户态接口，
正常 create 多做同步也不会导致 Linux ABI 测试必然失败；最低层生产 VFS 测试
和原始 QEMU 编译提供确定性与集成证据。

通过 fmt、ax-fs-ng 全库测试和 clippy 后运行同配置 600 秒 H 窗口，只更新内核，
复用相同 tg-xtask、源码和工具链。G 结束后根镜像只规范化了有完整历史证据的
555 个空闲 inode 标志，所有有效文件内容未变；仍须记录这一镜像状态差异，
不能将单次 G/H 进度差宣称为严格完整构建加速倍数。失败保留日志与镜像，
明确回滚本单独候选，不删改任何工作负载结果来规避验收。
