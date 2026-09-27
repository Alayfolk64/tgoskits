# 路径查找的进程目录上下文快照候选

## 问题与范围

M 的最长 mutex 等待 19191633728 ns，使用该轮 ELF 解析为 cargo 的
sys_openat → with_fs（创建通知存在性查询）→ FsContext 锁，而非 ext4 锁。
N 的两个 sys_openat 调用点累计为 11841025888 和 11609393328 ns；它们不是
当前累计等待最大的瓶颈，不能把这项候选描述为已解决 ext4 全局锁。

当前 with_fs 在获取进程 root/cwd 后继续持锁调用路径解析、创建与文件打开。
共享 CLONE_FS 的线程因此不能越过另一线程在此期间的 I/O。目标只缩短目录
上下文锁的持有范围，保留路径解析、目录/inode 锁及错误传播。

## 参考与方案

已核验 MOSS 5a54e4413c9657bfb531cc32f6d688090465200d 的
kernel/src/file/fs.rs::with_fs：锁内 clone FsContext，再用只读快照执行回调。
本地 Linux 980ab36ae5972c83f683b939e50c469c4947229e 的
include/linux/fs_struct.h::get_fs_root/get_fs_pwd 获取 path 引用即释放锁；
fs/namei.c::set_root/path_init 使用该引用继续路径行走。只借鉴引用与锁范围，
不声称移植 Linux 的 RCU、rename sequence 或完整 namei 语义。

当前 affddc3fecec02b31df94573063e4840433c9ebe 工作树的 FsContext 已可 Clone，
root/cwd 为 Location，mount namespace 为 Arc；不需要新缓存或新锁类型。
读取了相关五项历史记录及开放 PR 搜索结果。PR #2287，head
f75df532a033660f3dfbd8959205f1fb2aa5aa35，修改 dirfd 权限边界和路径权限检查；
已读其 fs.rs diff，仍保留锁内回调，与本候选不是同一能力，后续合并须保留其
权限边界，不能以当前 with_current_dir 覆盖其 with_dirfd。

保持现状仍串行 I/O；换为读写锁依然让 cwd 更新等待整段路径行走；另建全局
路径缓存会引入失效与权限问题。选择已有 Clone 快照。with_fs 的回调改为
&FsContext，明确不允许经此 helper 更新进程目录；现有调用均只读取上下文，
实际文件/目录变更仍通过其既有节点 API。chdir/chroot/fchdir 的更新代码保留。

## 风险与验证门槛

属于共享锁范围调整，合入前需要文件系统/并发边界设计审核，本地实验不代表
可以合入。快照同时保留 root、cwd 和 mount namespace 引用，路径行走期间的
chdir 不改变已经选择的起点；后续操作再获取新快照。并发挂载、rename、inode
变更依赖现有节点协议，不假设 clone 会冻结整个文件系统。

直接 with_fs 调用点已核对：openat/openat2、mkdirat/mknodat、linkat、unlinkat、
symlinkat/readlinkat、renameat2、fchdir、move_mount、resolve_at 与通知辅助查询。
resolve_at 又被 stat、访问检查、属性与 inotify 路径使用。不能据此宣称这些
syscall 的全部 ABI 已验证；本候选不调整 flags、权限、errno 或参数校验。

先将当前流程提取到接受显式 FsMutex<FsContext> 的 file 域内部 helper，
行为保持不变；with_fs 继续调用它。最低层确定性回归使用真实 FsContext 和
同一生产 helper：回调中 try_lock 必须成功、已选 root/cwd 不随共享上下文
变更、回调错误原样传播。旧实现应先失败，之后才替换为快照。
这一锁持有交错无法由普通用户态 syscall 稳定控制，定向内核 host-test 为
主要回归，并追加 Starry 内核 clippy/QEMU 真实编译验证；不放宽测试条件。

O 窗口采集中只准备回归与行为保持提取，不执行宿主编译，不修改测量内核。
候选性能结果尚无；CPU、锁等待、采样丢失、编译进度与根盘 fsck 都需单独验证。

## 2026-09-10 本地验证

O 完全退出后运行
`cargo test -p starry-kernel --lib --features ax-task/host-test file::fs_tests -- --nocapture`。
旧实现退出 101，5 通过 / 2 失败：宿主 lockdep 在两个回调的 try_lock 处报
recursive acquisition（fs_tests.rs:29、:47），确定性暴露仍持锁。
改为锁内 clone、锁外只读回调后，同一命令 7/7 通过；未放宽断言。

`cargo fmt -p starry-kernel` 和匹配 aarch64-unknown-none-softfloat、NVMe、
virtio-net、guest-profile、smp 的生产 clippy（--no-deps -- -D warnings）通过。
已读 xtask clippy 的选择/展开流程，它只支持 package/since/all，不支持筛选
单个配置，会展开包括无关板卡固件的完整矩阵；本轮使用已匹配配置的原生检查，
不声称完整矩阵通过。

额外宿主 `cargo clippy -p starry-kernel --lib --tests --features ax-task/host-test --no-deps -- -D warnings`
退出 101，报告 136 项既有测试 lint（恒定断言、重复转换、测试模块布局等），
没有以 allow 消音或修改无关测试。首次输出截断后重跑保存
tmp/fs-context-host-clippy.log，1200 行已分三段完整展示，第二次仍退出 101。
这项全测试 clippy 失败必须保留，不能被生产配置通过掩盖。

固定 cargo xtask starry build 通过，完成 13023 个 kallsyms 与 BIN 刷新。
P 保存目录 fs-context-snapshot-window-600，ELF
`f1b773cb7c7919a796cda746d585407b382246fcd3bab4a147f0f9f782591165`，
BIN `c2c81e8b2cf719a3a0738be0f7317a161c686ab487a6e6e064f555bae06a54f4`。
该内核包含 N + O + 路径快照，尚未启动测量；先复测保存的 N 排查波动。

N2 已完成后，工作盘从同一只读冻结根盘重新复制，启动前完整 SHA-256 再次
为 d5e8c6347879117246530ba52dacba58a03379f06ddaf19866ca576e9cdd57d4。
P 于 2026-09-09 22:00 UTC 启动，tmux 为 tgoskits-profile-fs-context-600，
QEMU PID 427197，已发送 guest runner 命令并启动 pidstat。使用上述保存的
P ELF/BIN，不包含此时工作树已实现的父 inode 查询 R；当前采样尚未结束。

## P 窗口结果

P 已正常结束：608 秒，内核计时 607853837536 ns，34 个启动编译单元、
32 个 distinct crates，17207 条原始记录；rc=124、build_completed=false。
五项 SHA-256 全部通过，离线 e2fsck -fn 退出 0；仅提示 extent tree 可压缩，
只读检查没有执行修改。debugfs 导出退出 0，但无法恢复 guest root 属主的
8 条权限警告已完整展示，不影响通过校验的文件内容。

CPU 为 12303 / 47592 个活跃样本，即 25.850983%；最忙 CPU 1 为 81.815051%。
所有 dropped/skipped 计数为 0。CPU-active 和 ext4-lock-hold 火焰图已渲染并查看。

| 指标 | P 实测 |
| --- | ---: |
| mutex 等待累计 | 2615871192352 ns |
| ext4 mutex 等待叶 | 2042459628544 ns（78.079518%） |
| ext4 持锁累计 | 492712567248 ns |
| read_at 持锁 | 184993953472 ns（37.546019%） |
| sync_to_disk 持锁 | 169184750224 ns（34.337413%） |
| lookup_locked 持锁 | 58620215440 ns（11.897447%） |
| 缺页链路持锁 | 159061396544 ns（32.282797%，与 read 等重叠） |
| 同步块读 | 64130 次 / 339216151024 ns |
| 同步块写 | 30129 次 / 103590974784 ns |
| 设备 flush | 8328 次 / 22981964784 ns |

mutex 等待叶中不再出现 with_fs/resolve_at，目录上下文锁范围目标已由真实
运行验证，但与 O 的 34 单元 / 24.49% 活跃相比没有明确整体进度收益。
N2 为 33 单元 / 24.98%，这些窗口仍显示波动，不能声称稳定加速。

最长 mutex 等待 12744396000 ns，匹配 P ELF 后为 ld 的
writev → CachedFile::write_at_locked → page_or_insert → Inode::read_at → ext4.lock。
最长 ext4 持锁 11099636096 ns，来自 cargo 的 openat → create → sync_to_disk。
两项最大值不是配对事件，不能据此声称某个具体 holder 阻塞了该 waiter。

结束根盘保存为 tmp/axbuild/rootfs/rootfs-profile-fs-context-result.img，
原始数据与图位于 target/profiling/arceos-helloworld/starry/fs-context-snapshot-window-600。
