# inode 元数据命中不再等待 ext4 全局锁

## 问题与边界

W 的 607 秒窗口中，ext4 全局锁占 mutex 等待 75.55%。直接 metadata
查询在 Ext4LockWait 下累计 385319828432 ns，len 为 27741726064 ns。
这些并发等待不是可直接减去的墙钟时间。当前两条查询无论 inode 是否已在
InodeCache 中，均先取得可能被磁盘同步长期占用的全局睡眠锁。

目标：缓存命中的 metadata/len 只从既有 inode owner 取得一个完整快照；
缓存 miss 继续走原锁和原错误路径。成功标准是生产 VFS 调用的锁/I/O hook
为零，同时元数据更新、截断、硬链接、unlink 后持有句柄仍能观察正确状态。
不改变权限检查、errno 翻译、数据读计划、块回收、create 同步、fsync 或日志。

## 设计与替代方案

InodeCache 内部的同一 SpinMutex 状态改由 Arc 持有，提供只读 reader 能力。
reader 只允许复制缓存内 Ext4Inode，不暴露 guard、修改、I/O 或自行装载功能；
不是第二份缓存。VFS 在 mount/replay 完成后保存 reader，元数据命中从该
状态取值；miss 释放缓存锁后取得原全局锁再加载。block_size 从完成挂载的
superblock 保存不可变值。锁顺序仍为 fs -> inode cache；reader 从不反向拿 fs。
更新、evict、clear 使用同一缓存锁，命中返回该次临界区内的完整快照。
现有 live_refs 保证 VFS wrapper 存活期间 inode number 不被释放复用。

保持现状无法消除命中排队；只对全局 fs 使用 try_lock 并不能读到内部缓存；
VFS 单独缓存 Metadata 会复制状态并扩大失效矩阵；共享整个 Ext4FileSystem
则扩大写权限和锁边界。因此选只读能力，不给数据映射使用该无全局锁快照。
内存代价为每个挂载一次 Arc 控制块和一个 reader，不按 inode 新增属性副本。
缓存 reader 绑定创建它的缓存实例；替换 owner 后旧 reader 不会跟随新实例。
VFS 只能在最终 mount/replay 后取得它，禁止在重放前提前发布。

这是共享 API/并发边界的高风险优化；本独立设计材料和实现均须在合入前审核。
当前只在实验工作树实现验证，不提交 PR 或改变用户仓库远端。

## 参考与兼容性

已读当前 InodeCache、mount cache 重建、VFS inode 生命周期、profile hook，
路径历史 ed3d4a1e6/c0b606fc7/5d13ce881。开放 PR #2015 的
6d5cc09f45a073680a270ae0b6047b24fd9eaff5 为相邻 inode 预装载，仍是
独占 cache owner，并无此只读共享能力。issue #2206 针对 multi-folio 块 I/O，
与本次只读 inode 快照不同；不以新增普通文件页缓存处理其问题。

MOSS 5a54e4413c9657bfb531cc32f6d688090465200d 的 vendor/axfs ext4
metadata/len 同样取得 fs 锁，不能声称直接复制了 MOSS 的这一项实现。
借鉴的是按真实热点缩短共享临界区的方法。
Linux 源码 980ab36ae5972c83f683b939e50c469c4947229e 的
fs/ext4/inode.c:ext4_getattr 和 fs/stat.c:generic_fillattr 从内存 inode
取普通属性，不要求全局 ext4 I/O 锁。Linux man-pages stat(2) 说明并发修改时
不同字段甚至可以来自不同时刻；本实现仍复制同一缓存锁下的整份 inode。
来源：https://man7.org/linux/man-pages/man2/stat.2.html 。

间接消费者包括 stat/fstat/lstat/fstatat/statx、权限查询、文件长度检查；
不更改参数解释或已有字段转换。正常返回后已完成的修改必须被 reader 看到。
先在能观察真实锁/I/O 的最低层回归：用户 syscall 无法确定性断言是否取得
某个内部 mutex，因此不添加仅依赖计时的用户态“提速测试”。已有元数据
持久化、权限/设备号转换和读写回归仍须通过，并运行同配置 QEMU 编译窗口。

## RED

旧实现新增两条生产 VFS profile 测试分别退出 101：cached metadata 和 len
均产生 Ext4 begin、Ext4LockHold begin/end、Ext4 end 四条事件。原文完整
展示，失败不依赖线程调度或超时。

## GREEN 与窗口 X

两条原始 RED 均已转 GREEN。新增真实持有全局 fs 锁时仍能完成 metadata/len
的回归；若查询尝试取得 fs 锁，profile hook 会在实际阻塞前确定性失败。
另覆盖 hardlink 更新与 unlink 后句柄、cache miss 加锁重载、evict/clear、
替换 cache 实例不改变旧 reader。ax-fs-ng 的 host-test,ext4,vfs,profile
组合 117/117 通过，host-test,ext4 组合 101/101 通过；rsext4 全套
249 项通过、1 项既有忽略。xtask 两 crate clippy 11/11 通过，额外实际
profile 特性组合的 tests clippy 通过；fmt 与 diff check 通过。

固定 AArch64 Starry 生产构建通过。X 保存于
target/profiling/arceos-helloworld/starry/cached-inode-metadata-window-600：

- ELF: cfb84689b982b41b2f1ef9783c783f4ac18d9613d62b019d8baff7c05d34c490
- BIN: 8710c2a5c932756c65b6c2fda6830ef779a8ac992ac5382d6095b2bf050d4605
- 启动前完整根盘 SHA: d5e8c6347879117246530ba52dacba58a03379f06ddaf19866ca576e9cdd57d4

W 结束盘已封存，X 从同一冻结盘复制并校验后启动。2026-09-10 01:18:11 UTC
启动 X；确认 Linux QEMU 已正常关机、没有并发虚拟机。窗口已经结束，
实测如下，不能据单元测试声称编译提速。

## X 完整窗口：局部等待下降，整体未改善

窗口正常结束：610 秒、rc=124、41 单元 / 38 crate、18246 条记录。
prebuild_ns=610319232336；dropped/skipped 全部为 0。五项 SHA256SUMS
通过，e2fsck -fn 退出 0，实际查看 CPU-active 与 ext4-lock-hold PNG。
导出时恢复 guest 属主的八条 EPERM 原文已展示，内容另行哈希验证通过。

| 指标 | W | X |
| --- | ---: | ---: |
| 活跃 CPU 样本 / 总样本 | 16202 / 47289 | 13976 / 47683 |
| 活跃比例 | 34.26% | 29.31% |
| 启动单元 / crate | 45 / 42 | 41 / 38 |
| 属性查询全局锁等待 ns | 413061554496 | 133642165520 |
| 总 mutex 等待 ns | 2101514140160 | 2258816559408 |
| ext4 mutex 等待占比 | 75.55% | 80.99% |
| ext4 持锁 ns | 392234691088 | 428845607376 |
| sync_to_disk 持锁 ns | 172056955456 | 162144833152 |
| read_inode 等锁 ns | 529542801232 | 759596368928 |
| 块读 ns / 次数 | 379814997664 / 67926 | 411374798656 / 65025 |
| 块写 ns / 次数 | 121251130576 / 49232 | 140335358928 / 40554 |
| flush ns / 次数 | 27423133984 / 10206 | 27730404000 / 9401 |

W 属性查询列为 metadata + len，X 为 inode_snapshot 的 miss fallback；
不能因为旧符号消失就把等待全部视作消除。命中走只读 cache，但 miss 仍需
全局锁。全局持锁次数从 179249 降到 118942，累计时间反而增加。
X read_inode 持锁 49033061808 ns、set_len 50897364736 ns、lookup
61641597936 ns。同步占持锁 37.81%，其中 create 136540362880 ns、
unlink 23564747520 ns、rename 1886938368 ns。

完整缺页链为 2571 / 13976 活跃 CPU 样本（18.40%），find_free_area
841 样本（6.02%），try_zero_page 381 样本。exit_preemption 1408 样本
仍须按已有 IRQ 恢复偏移证据解释，不能直接认定抢占退出函数真实耗时最高。

本单轮未证明整体提速，不把改动列为已验收性能收益。reader 不更新原 cache
LRU，可能改变淘汰模式，但目前没有命中率证据能将 I/O 上升归因于它；
不能用这个推断代替复测。当前最大可解释等待仍是 ext4 全局锁，持锁最大
类别仍为同步写盘。继续匹配 Linux QEMU 对照与下一轮热点验证。
