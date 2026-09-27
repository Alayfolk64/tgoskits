# ext4 重构 syscall 影响与验证边界

本表是最终静态检查的影响清单，不是完整 Linux ABI 合规审查或 PR 可合入结论。
每个入口单列标准入口；“无法确认”表示完整 errno/权限/并发差分尚未运行，
不表示编译失败。x86_64 专有入口只对现有 dispatch 可用架构适用。

Linux 源码比较固定在 `980ab36ae5972c83f683b939e50c469c4947229e`。
已经逐行复核的持久化链是
[msync → vfs_fsync_range](https://github.com/torvalds/linux/blob/980ab36ae5972c83f683b939e50c469c4947229e/mm/msync.c#L32-L116)、
[ext4_sync_file](https://github.com/torvalds/linux/blob/980ab36ae5972c83f683b939e50c469c4947229e/fs/ext4/fsync.c#L132-L190)、
[ext4_setattr](https://github.com/torvalds/linux/blob/980ab36ae5972c83f683b939e50c469c4947229e/fs/ext4/inode.c#L5911-L6030)。
Starry 对应 `syscall/fs/io.rs` → `file/fs.rs` → `ax-fs-ng/file/handle` →
`file/cache/writeback.rs` → `fs/ext4/rsext4/fs/writeback.rs`。
msync 先释放地址空间锁，再进入 `backend/file/mod.rs::writeback_range`；
空脏页列表也必须进入 backing sync，不能用缓存状态代替 journal 完成回执。

| Syscall | 评审结论 | 对应标准 | 本轮依据/限制 |
| --- | --- | --- | --- |
| `open` | 无法确认完整 ABI；已核对影响边界 | [open(2)](https://man7.org/linux/man-pages/man2/open.2.html) | 创建与 O_SYNC/O_DSYNC 经现有 File 完成边界；flags/权限完整差分未运行 |
| `openat` | 无法确认完整 ABI；已核对影响边界 | [open(2)](https://man7.org/linux/man-pages/man2/open.2.html) | 创建与 O_SYNC/O_DSYNC 经现有 File 完成边界；flags/权限完整差分未运行 |
| `openat2` | 无法确认完整 ABI；已核对影响边界 | [open(2)](https://man7.org/linux/man-pages/man2/open.2.html) | 创建与 O_SYNC/O_DSYNC 经现有 File 完成边界；flags/权限完整差分未运行 |
| `creat` | 无法确认完整 ABI；已核对影响边界 | [open(2)](https://man7.org/linux/man-pages/man2/open.2.html) | 创建与 O_SYNC/O_DSYNC 经现有 File 完成边界；flags/权限完整差分未运行 |
| `read` | 无法确认完整 ABI；已核对影响边界 | [read(2)](https://man7.org/linux/man-pages/man2/read.2.html) | 读取迁移到同 inode 排他与 owned I/O；atime/返回长度已有真实回归，未执行 |
| `pread64` | 无法确认完整 ABI；已核对影响边界 | [pread(2)](https://man7.org/linux/man-pages/man2/pread.2.html) | 读取迁移到同 inode 排他与 owned I/O；atime/返回长度已有真实回归，未执行 |
| `readv` | 无法确认完整 ABI；已核对影响边界 | [readv(2)](https://man7.org/linux/man-pages/man2/readv.2.html) | 读取迁移到同 inode 排他与 owned I/O；atime/返回长度已有真实回归，未执行 |
| `preadv` | 无法确认完整 ABI；已核对影响边界 | [readv(2)](https://man7.org/linux/man-pages/man2/readv.2.html) | 读取迁移到同 inode 排他与 owned I/O；atime/返回长度已有真实回归，未执行 |
| `preadv2` | 无法确认完整 ABI；已核对影响边界 | [readv(2)](https://man7.org/linux/man-pages/man2/readv.2.html) | 读取迁移到同 inode 排他与 owned I/O；atime/返回长度已有真实回归，未执行 |
| `write` | 无法确认完整 ABI；已核对影响边界 | [write(2)](https://man7.org/linux/man-pages/man2/write.2.html) | 文件写完成后同步；错误不提前发布普通/append 游标；vector/positioned ABI 差分未运行 |
| `pwrite64` | 无法确认完整 ABI；已核对影响边界 | [pread(2)](https://man7.org/linux/man-pages/man2/pread.2.html) | 文件写完成后同步；错误不提前发布普通/append 游标；vector/positioned ABI 差分未运行 |
| `writev` | 无法确认完整 ABI；已核对影响边界 | [readv(2)](https://man7.org/linux/man-pages/man2/readv.2.html) | 文件写完成后同步；错误不提前发布普通/append 游标；vector/positioned ABI 差分未运行 |
| `pwritev` | 无法确认完整 ABI；已核对影响边界 | [readv(2)](https://man7.org/linux/man-pages/man2/readv.2.html) | 文件写完成后同步；错误不提前发布普通/append 游标；vector/positioned ABI 差分未运行 |
| `pwritev2` | 无法确认完整 ABI；已核对影响边界 | [readv(2)](https://man7.org/linux/man-pages/man2/readv.2.html) | 文件写完成后同步；错误不提前发布普通/append 游标；vector/positioned ABI 差分未运行 |
| `close` | 无法确认完整 ABI；已核对影响边界 | [close(2)](https://man7.org/linux/man-pages/man2/close.2.html) | 延迟 atime/最后 inode 引用沿原生命周期；close 不承诺 fsync，真实 ext4 回归未执行 |
| `close_range` | 无法确认完整 ABI；已核对影响边界 | [close_range(2)](https://man7.org/linux/man-pages/man2/close_range.2.html) | 延迟 atime/最后 inode 引用沿原生命周期；close 不承诺 fsync，真实 ext4 回归未执行 |
| `dup` | 无法确认完整 ABI；已核对影响边界 | [dup(2)](https://man7.org/linux/man-pages/man2/dup.2.html) | 继续共享打开文件对象与同步策略；描述符共享语义未改，未运行差分 |
| `dup2` | 无法确认完整 ABI；已核对影响边界 | [dup(2)](https://man7.org/linux/man-pages/man2/dup.2.html) | 继续共享打开文件对象与同步策略；描述符共享语义未改，未运行差分 |
| `dup3` | 无法确认完整 ABI；已核对影响边界 | [dup(2)](https://man7.org/linux/man-pages/man2/dup.2.html) | 继续共享打开文件对象与同步策略；描述符共享语义未改，未运行差分 |
| `fcntl` | 无法确认完整 ABI；已核对影响边界 | [fcntl(2)](https://man7.org/linux/man-pages/man2/fcntl.2.html) | 同步要求保存在打开的 File；既有 F_SETFL flags 行为未在本轮重新验证 |
| `mkdir` | 无法确认完整 ABI；已核对影响边界 | [mkdir(2)](https://man7.org/linux/man-pages/man2/mkdir.2.html) | 目录事务完成后按共享目录同步策略提交；崩溃恢复/fsck 未运行 |
| `mkdirat` | 无法确认完整 ABI；已核对影响边界 | [mkdir(2)](https://man7.org/linux/man-pages/man2/mkdir.2.html) | 目录事务完成后按共享目录同步策略提交；崩溃恢复/fsck 未运行 |
| `mknod` | 无法确认完整 ABI；已核对影响边界 | [mknod(2)](https://man7.org/linux/man-pages/man2/mknod.2.html) | 创建接口迁移，设备号编码已接线原输入；特权/类型组合差分未运行 |
| `mknodat` | 无法确认完整 ABI；已核对影响边界 | [mknod(2)](https://man7.org/linux/man-pages/man2/mknod.2.html) | 创建接口迁移，设备号编码已接线原输入；特权/类型组合差分未运行 |
| `link` | 无法确认完整 ABI；已核对影响边界 | [link(2)](https://man7.org/linux/man-pages/man2/link.2.html) | 硬链接共享 inode 排他/缓存；完成边界按目录策略提交，未运行 |
| `linkat` | 无法确认完整 ABI；已核对影响边界 | [link(2)](https://man7.org/linux/man-pages/man2/link.2.html) | 硬链接共享 inode 排他/缓存；完成边界按目录策略提交，未运行 |
| `unlink` | 无法确认完整 ABI；已核对影响边界 | [unlink(2)](https://man7.org/linux/man-pages/man2/unlink.2.html) | 先发布 zero-link，再由最后引用 reap；回收与错误路径已有回归，未执行 |
| `unlinkat` | 无法确认完整 ABI；已核对影响边界 | [unlink(2)](https://man7.org/linux/man-pages/man2/unlink.2.html) | 先发布 zero-link，再由最后引用 reap；回收与错误路径已有回归，未执行 |
| `rmdir` | 无法确认完整 ABI；已核对影响边界 | [unlink(2)](https://man7.org/linux/man-pages/man2/unlink.2.html) | 先发布 zero-link，再由最后引用 reap；回收与错误路径已有回归，未执行 |
| `rename` | 无法确认完整 ABI；已核对影响边界 | [rename(2)](https://man7.org/linux/man-pages/man2/rename.2.html) | 目录原子操作与旧目标在途读取保留；不声明新增全部 rename flags 支持 |
| `renameat` | 无法确认完整 ABI；已核对影响边界 | [rename(2)](https://man7.org/linux/man-pages/man2/rename.2.html) | 目录原子操作与旧目标在途读取保留；不声明新增全部 rename flags 支持 |
| `renameat2` | 无法确认完整 ABI；已核对影响边界 | [rename(2)](https://man7.org/linux/man-pages/man2/rename.2.html) | 目录原子操作与旧目标在途读取保留；不声明新增全部 rename flags 支持 |
| `symlink` | 无法确认完整 ABI；已核对影响边界 | [symlink(2)](https://man7.org/linux/man-pages/man2/symlink.2.html) | 原子发布 symlink 接口迁移，按目录策略同步；差分未运行 |
| `symlinkat` | 无法确认完整 ABI；已核对影响边界 | [symlink(2)](https://man7.org/linux/man-pages/man2/symlink.2.html) | 原子发布 symlink 接口迁移，按目录策略同步；差分未运行 |
| `readlink` | 无法确认完整 ABI；已核对影响边界 | [readlink(2)](https://man7.org/linux/man-pages/man2/readlink.2.html) | 读取迁移后的 symlink 内容；长度/errno 差分未运行 |
| `readlinkat` | 无法确认完整 ABI；已核对影响边界 | [readlink(2)](https://man7.org/linux/man-pages/man2/readlink.2.html) | 读取迁移后的 symlink 内容；长度/errno 差分未运行 |
| `chmod` | 无法确认完整 ABI；已核对影响边界 | [chmod(2)](https://man7.org/linux/man-pages/man2/chmod.2.html) | metadata handle 与共享同步策略；权限检查未重写，持久化回归未执行 |
| `fchmod` | 无法确认完整 ABI；已核对影响边界 | [chmod(2)](https://man7.org/linux/man-pages/man2/chmod.2.html) | metadata handle 与共享同步策略；权限检查未重写，持久化回归未执行 |
| `fchmodat` | 无法确认完整 ABI；已核对影响边界 | [chmod(2)](https://man7.org/linux/man-pages/man2/chmod.2.html) | metadata handle 与共享同步策略；权限检查未重写，持久化回归未执行 |
| `fchmodat2` | 无法确认完整 ABI；已核对影响边界 | [chmod(2)](https://man7.org/linux/man-pages/man2/chmod.2.html) | metadata handle 与共享同步策略；权限检查未重写，持久化回归未执行 |
| `chown` | 无法确认完整 ABI；已核对影响边界 | [chown(2)](https://man7.org/linux/man-pages/man2/chown.2.html) | owner 元数据与 reader rollback；UID/GID 原始输入已迁移，权限差分未运行 |
| `lchown` | 无法确认完整 ABI；已核对影响边界 | [chown(2)](https://man7.org/linux/man-pages/man2/chown.2.html) | owner 元数据与 reader rollback；UID/GID 原始输入已迁移，权限差分未运行 |
| `fchown` | 无法确认完整 ABI；已核对影响边界 | [chown(2)](https://man7.org/linux/man-pages/man2/chown.2.html) | owner 元数据与 reader rollback；UID/GID 原始输入已迁移，权限差分未运行 |
| `fchownat` | 无法确认完整 ABI；已核对影响边界 | [chown(2)](https://man7.org/linux/man-pages/man2/chown.2.html) | owner 元数据与 reader rollback；UID/GID 原始输入已迁移，权限差分未运行 |
| `utime` | 无法确认完整 ABI；已核对影响边界 | [utime(2)](https://man7.org/linux/man-pages/man2/utime.2.html) | 时间元数据由事务 owner 保持；重挂载回归已编译，未执行 |
| `utimes` | 无法确认完整 ABI；已核对影响边界 | [utime(2)](https://man7.org/linux/man-pages/man2/utime.2.html) | 时间元数据由事务 owner 保持；重挂载回归已编译，未执行 |
| `utimensat` | 无法确认完整 ABI；已核对影响边界 | [utimensat(2)](https://man7.org/linux/man-pages/man2/utimensat.2.html) | 时间元数据由事务 owner 保持；重挂载回归已编译，未执行 |
| `stat` | 无法确认完整 ABI；已核对影响边界 | [stat(2)](https://man7.org/linux/man-pages/man2/stat.2.html) | 统一 inode 解码与 canonical reader，硬链接/rollback/miss 回归未执行 |
| `lstat` | 无法确认完整 ABI；已核对影响边界 | [stat(2)](https://man7.org/linux/man-pages/man2/stat.2.html) | 统一 inode 解码与 canonical reader，硬链接/rollback/miss 回归未执行 |
| `fstat` | 无法确认完整 ABI；已核对影响边界 | [stat(2)](https://man7.org/linux/man-pages/man2/stat.2.html) | 统一 inode 解码与 canonical reader，硬链接/rollback/miss 回归未执行 |
| `newfstatat` | 无法确认完整 ABI；已核对影响边界 | [stat(2)](https://man7.org/linux/man-pages/man2/stat.2.html) | 统一 inode 解码与 canonical reader，硬链接/rollback/miss 回归未执行 |
| `statx` | 无法确认完整 ABI；已核对影响边界 | [statx(2)](https://man7.org/linux/man-pages/man2/statx.2.html) | 使用迁移后的 metadata 与 inode identity；完整结构体/flags 差分未运行 |
| `access` | 无法确认完整 ABI；已核对影响边界 | [access(2)](https://man7.org/linux/man-pages/man2/access.2.html) | 消费迁移后的 inode mode/owner；credential 判定未重写，差分未运行 |
| `faccessat` | 无法确认完整 ABI；已核对影响边界 | [access(2)](https://man7.org/linux/man-pages/man2/access.2.html) | 消费迁移后的 inode mode/owner；credential 判定未重写，差分未运行 |
| `faccessat2` | 无法确认完整 ABI；已核对影响边界 | [access(2)](https://man7.org/linux/man-pages/man2/access.2.html) | 消费迁移后的 inode mode/owner；credential 判定未重写，差分未运行 |
| `getdents` | 无法确认完整 ABI；已核对影响边界 | [getdents(2)](https://man7.org/linux/man-pages/man2/getdents.2.html) | 目录读取改用 core DirectoryReader/typed cursor；原 cookie/并发用例未执行 |
| `getdents64` | 无法确认完整 ABI；已核对影响边界 | [getdents(2)](https://man7.org/linux/man-pages/man2/getdents.2.html) | 目录读取改用 core DirectoryReader/typed cursor；原 cookie/并发用例未执行 |
| `getcwd` | 无法确认完整 ABI；已核对影响边界 | [getcwd(2)](https://man7.org/linux/man-pages/man2/getcwd.2.html) | 目录上下文/raw path 适配，原 process-root 与身份回归已接线，未执行 |
| `chdir` | 无法确认完整 ABI；已核对影响边界 | [chdir(2)](https://man7.org/linux/man-pages/man2/chdir.2.html) | 目录上下文/raw path 适配，原 process-root 与身份回归已接线，未执行 |
| `fchdir` | 无法确认完整 ABI；已核对影响边界 | [chdir(2)](https://man7.org/linux/man-pages/man2/chdir.2.html) | 目录上下文/raw path 适配，原 process-root 与身份回归已接线，未执行 |
| `chroot` | 无法确认完整 ABI；已核对影响边界 | [chroot(2)](https://man7.org/linux/man-pages/man2/chroot.2.html) | 目录上下文/raw path 适配，原 process-root 与身份回归已接线，未执行 |
| `lseek` | 无法确认完整 ABI；已核对影响边界 | [lseek(2)](https://man7.org/linux/man-pages/man2/lseek.2.html) | 查询权威长度与共享游标；未改变 ABI，差分未运行 |
| `truncate` | 无法确认完整 ABI；已核对影响边界 | [truncate(2)](https://man7.org/linux/man-pages/man2/truncate.2.html) | 缩短后先保留待失效帧；TLB 故障可返回 EBUSY 且新长度已可见，未证明完整 Linux errno 一致 |
| `ftruncate` | 无法确认完整 ABI；已核对影响边界 | [truncate(2)](https://man7.org/linux/man-pages/man2/truncate.2.html) | 缩短后先保留待失效帧；TLB 故障可返回 EBUSY 且新长度已可见，未证明完整 Linux errno 一致 |
| `fallocate` | 无法确认完整 ABI；已核对影响边界 | [fallocate(2)](https://man7.org/linux/man-pages/man2/fallocate.2.html) | 共用 cached resize/zero 路径；现有 punch-hole syscall 零填充策略未重写 |
| `fsync` | 无法确认完整 ABI；已核对影响边界 | [fsync(2)](https://man7.org/linux/man-pages/man2/fsync.2.html) | 先回写页再等待日志 durable，目录同步保留；行为/设备故障差分未运行 |
| `fdatasync` | 无法确认完整 ABI；已核对影响边界 | [fsync(2)](https://man7.org/linux/man-pages/man2/fsync.2.html) | 允许更强的整事务同步，大小/映射不遗漏；行为差分未运行 |
| `sync` | 无法确认完整 ABI；已核对影响边界 | [sync(2)](https://man7.org/linux/man-pages/man2/sync.2.html) | 现有入口只明确同步 root 元数据，不等于完整 Linux 多挂载 sync；不是本轮新增问题 |
| `syncfs` | 无法确认完整 ABI；已核对影响边界 | [sync(2)](https://man7.org/linux/man-pages/man2/sync.2.html) | 先写全局 cached pages 再 flush 目标 FS；其他文件系统额外写回是现有行为，差分未运行 |
| `sync_file_range` | 无法确认完整 ABI；已核对影响边界 | [sync_file_range(2)](https://man7.org/linux/man-pages/man2/sync_file_range.2.html) | 现有 no-op 入口未由本重构补全，不声明 Linux WAIT 语义兼容 |
| `fadvise64` | 无法确认完整 ABI；已核对影响边界 | [posix_fadvise(2)](https://man7.org/linux/man-pages/man2/posix_fadvise.2.html) | 只核对共享缓存消费者；既有建议类操作完整行为未确认 |
| `mount` | 无法确认完整 ABI；已核对影响边界 | [mount(2)](https://man7.org/linux/man-pages/man2/mount.2.html) | MS_SYNCHRONOUS=16/MS_DIRSYNC=128；普通 remount 改 sync、bind-remount 不改共享策略，C 回归未执行 |
| `umount2` | 无法确认完整 ABI；已核对影响边界 | [umount(2)](https://man7.org/linux/man-pages/man2/umount.2.html) | 挂载 lease、两级 admission 与错误恢复已复核；真实卸载/失败差分未运行 |
| `statfs` | 无法确认完整 ABI；已核对影响边界 | [statfs(2)](https://man7.org/linux/man-pages/man2/statfs.2.html) | ST_SYNCHRONOUS 取自共享挂载状态；结构体输出差分未运行 |
| `fstatfs` | 无法确认完整 ABI；已核对影响边界 | [statfs(2)](https://man7.org/linux/man-pages/man2/statfs.2.html) | ST_SYNCHRONOUS 取自共享挂载状态；结构体输出差分未运行 |
| `fsopen` | 无法确认完整 ABI；已核对影响边界 | [fsopen(2)](https://man7.org/linux/man-pages/man2/fsopen.2.html) | 相关挂载对象共享与生命周期接口未绕开；这些入口完整扩展语义未重新证明 |
| `fsconfig` | 无法确认完整 ABI；已核对影响边界 | [fsconfig(2)](https://man7.org/linux/man-pages/man2/fsconfig.2.html) | 相关挂载对象共享与生命周期接口未绕开；这些入口完整扩展语义未重新证明 |
| `fsmount` | 无法确认完整 ABI；已核对影响边界 | [fsmount(2)](https://man7.org/linux/man-pages/man2/fsmount.2.html) | 相关挂载对象共享与生命周期接口未绕开；这些入口完整扩展语义未重新证明 |
| `move_mount` | 无法确认完整 ABI；已核对影响边界 | [move_mount(2)](https://man7.org/linux/man-pages/man2/move_mount.2.html) | 相关挂载对象共享与生命周期接口未绕开；这些入口完整扩展语义未重新证明 |
| `mount_setattr` | 无法确认完整 ABI；已核对影响边界 | [mount_setattr(2)](https://man7.org/linux/man-pages/man2/mount_setattr.2.html) | 相关挂载对象共享与生命周期接口未绕开；这些入口完整扩展语义未重新证明 |
| `pivot_root` | 无法确认完整 ABI；已核对影响边界 | [pivot_root(2)](https://man7.org/linux/man-pages/man2/pivot_root.2.html) | 相关挂载对象共享与生命周期接口未绕开；这些入口完整扩展语义未重新证明 |
| `mmap` | 无法确认完整 ABI；已核对影响边界 | [mmap(2)](https://man7.org/linux/man-pages/man2/mmap.2.html) | 共享文件映射的缓存身份/失效所有权已复核；匿名映射和完整架构 ABI 不在本轮新增声明内 |
| `munmap` | 无法确认完整 ABI；已核对影响边界 | [mmap(2)](https://man7.org/linux/man-pages/man2/mmap.2.html) | 共享文件映射的缓存身份/失效所有权已复核；匿名映射和完整架构 ABI 不在本轮新增声明内 |
| `mprotect` | 无法确认完整 ABI；已核对影响边界 | [mprotect(2)](https://man7.org/linux/man-pages/man2/mprotect.2.html) | 共享文件映射的缓存身份/失效所有权已复核；匿名映射和完整架构 ABI 不在本轮新增声明内 |
| `mremap` | 无法确认完整 ABI；已核对影响边界 | [mremap(2)](https://man7.org/linux/man-pages/man2/mremap.2.html) | backend split/move 的原地址失败记录仍保留；实际并发映射迁移未运行 |
| `msync` | 无法确认完整 ABI；已核对影响边界 | [msync(2)](https://man7.org/linux/man-pages/man2/msync.2.html) | 已修 clean-page 早退；MS_ASYNC/INVALIDATE 的既有差异未重写，不能声明完整兼容 |
| `madvise` | 无法确认完整 ABI；已核对影响边界 | [madvise(2)](https://man7.org/linux/man-pages/man2/madvise.2.html) | MADV_REMOVE 的 inode/cache 消费路径保持；完整 flags 与 errno 差分未运行 |
| `io_submit` | 无法确认完整 ABI；已核对影响边界 | [io_submit(2)](https://man7.org/linux/man-pages/man2/io_submit.2.html) | AIO regular-file 写改走 File::write_at 完成边界，内核回归已编译，未执行 |
| `io_uring_enter` | 无法确认完整 ABI；已核对影响边界 | [io_uring_enter(2)](https://man7.org/linux/man-pages/man2/io_uring_enter.2.html) | 读写/fsync SQE 复用已有 syscall 路径；ring 协议未修改，差分未运行 |

扩展属性 syscall 的既有 stub 路由没有接到新 XattrOps；本重构不声称为这些入口新增
ext4 xattr 能力。fsync 的 Linux errseq_t 按打开文件传播历史写回错误，以及完整多挂载
sync/同步范围/异步 msync 语义不在本轮性能重构中凭空宣布已实现。新增错误与持久化
边界必须由门槛后的生产回归验证；已有差异不通过降低断言掩盖。
