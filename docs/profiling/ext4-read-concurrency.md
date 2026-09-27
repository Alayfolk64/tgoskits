# ext4 文件数据锁外读取：候选设计

## 问题与边界

P 窗口 608 秒内 ext4 持锁 492712567248 ns，其中 read_at 为
184993953472 ns（37.55%），缺页链为 159061396544 ns（32.28%，与读重叠）。
ext4 占 mutex 等待 78.08%，CPU 活跃 25.85%。私有文件缺页已经使用地址空间
锁外准备，但 backing read 仍将所有文件的磁盘读取放在同一个 ext4 mutex 内。

目标调用方是现有 CachedFile 的缺页/读缓存填充，目标行为是不同 inode 的
文件数据 I/O 不互相占用 ext4 元数据锁，保留现有缓存、错误和持久化语义。
非目标：不改变调度器、缺页 ABI、磁盘格式、缓存容量、journal/flush 屏障；
不做锁外写盘，不照搬 MOSS 的 lwext4 专用代码，不新增驱动协议。

## 已检查的实现与替代方案

当前工作树基于 affddc3fecec02b31df94573063e4840433c9ebe。
已读 ext4 适配器 fs/inode/block、CachedFile 填充、rsext4 file/io、
DataBlockCache、Jbd2Dev 和 cached_device 的相关生产路径；历史检索包括
ed3d4a1e6、c0b606fc7、5d13ce881、669e90adf、1ca87a306。
开放 PR 搜索 `ext4 read` 返回 #2070、#1603、#1602、#1577、#2200、#1601，
标题涉及驱动、perf 和 web UI，未据此声称完成所有 PR 的重复性审查。

MOSS 5a54e4413c9657bfb531cc32f6d688090465200d 的
vendor/axfs/src/fs/ext4/fs.rs::read_extents/read_mapped 在锁内映射并处理
lwext4 buffer cache，锁外使用 SharedBlockDevice；已完整阅读该文件。
只借鉴阶段拆分；TGOSKits 使用 rsext4，脏 DataBlockCache 和 journal 队列
都必须单独处理，不能假定裸设备字节总是最新。当前 NativeHandleBlockDevice
内部的 Arc<BlockDeviceHandle> 已支持 &self 读和既有完成/flush 协议。
本地 Linux 980ab36ae5972c83f683b939e50c469c4947229e 的
mm/filemap.c:2610 filemap_create_folio 使用 invalidate_lock 保护映射与读入，
fs/ext4/inode.c:6048 的缩短路径等待 I/O 并持有 invalidate_lock；只借鉴
“释放全局锁后仍须保护文件块生命周期”，不声称复现 Linux 的 folio/RW 锁协议。

保持现状继续串行。扩大缓存不能消除首次读串行，且历史四块设备缓存容量
存在一致性风险，不扩大。全局内容版本重试会受无关写入干扰，并不能单独
修复 inode 被 rename 回收后的存活语义，放弃作为首选。新的全局读写锁会
扩大同步原语维护面；全局文件 I/O mutex 仍串行所有读取，也不选择。

选择既有 inode 存活记录共享的每 inode 可睡眠 I/O 锁，加有界只读计划。
这是高风险并发/共享接口改动，必须先审核本设计再合入，本地实验不代表认可。

## 所有权与锁顺序

同一 ext4 inode 的所有 VFS 包装器从 live_refs 获得同一个 I/O 锁，引用计数
与现有延迟释放规则保持统一来源。规则为 inode I/O 锁 → ext4 mutex，绝不在
持有 ext4 mutex 时等待 inode I/O 锁。写、append、truncate、symlink 更新
同样经过 inode I/O 锁。最后引用释放时不可能还有借用该包装器的读操作。

unlink 的最后一个 regular-file link 已有 live_refs 延迟释放；rename 覆盖
目的 inode 的回收必须等待同一 I/O 锁。先在 ext4 锁内识别并保留目的 inode
锁引用，释放 ext4 后等待，再加 ext4 锁重检目的身份，不能使用未重检的路径
快照。目录和符号链接继续使用现有锁内路径。已注意到现有 rename 调用
delete_file 可能不保留被覆盖的打开文件；不得把本候选声称为完整 rename ABI
修复，若它妨碍安全交错验证，必须先增加确定性回归并处理该前置问题。

读取阶段：

1. 获得 inode I/O 锁；ext4 锁内验证普通文件、EOF、块号与范围，并取得块映射。
2. 快照覆盖范围内的 DataBlockCache 内容及尚未提交的 journal 更新，优先级与
   当前 read_run 保持一致；脏缓存写回后被逐出也不能使本次读取退回旧磁盘字节。
3. 释放 ext4 锁，在私有、有界缓冲区中执行独立块读取并覆盖缓存快照。
4. 重新获取 ext4 锁完成现有 atime 处理，再向调用方发布结果；保持 inode I/O
   锁直到本次操作结束。错误按现有边界返回，不吞掉读取/时间更新错误。

独立读能力仅在已有块运行期支持共享读取时启用；不支持的设备保留原路径。
不绕过分区范围、块大小/溢出校验、DMA 生命周期或现有 flush 屏障。
首版计划将有效读取限制在 1 MiB，加上首尾块对齐最多额外 4 KiB，沿用既有
MAX_RUN_IO_BYTES 的量级；更大直接请求保留完整的锁内原路径，不返回短读。
当前 ReadAheadState 最大 32 页，即 128 KiB，因此缓存窗口不被这项上限排除。
底层传输继续按至多 1 MiB 分段，计划只拥有缓冲区与块号，不借用 fs/dev。
PreparedFileRead 在执行设备读取后转换成完成态，再由完成态更新 atime 并
复制有效字节，避免暴露尚未完成的设备缓冲区。

## 验证与验收

R 采样期间只准备测试/设计，不执行宿主编译；结束后先运行旧实现的确定性
回归，确认真实数据读取时 ext4 锁仍持有，再实现候选并运行相同测试。
继续验证不同 inode 同时读、同 inode 截断/覆盖 rename 被阻止、硬链接包装器
共享保护、缓存脏字节/并发 flush、洞/EOF/非对齐/超限完整回退、设备错误和能力
不支持时的原路径。行为回归调用真实 rsext4/VFS，不解析源码文本。

修改 crate 必须通过 fmt、定向/全套 host-test、xtask clippy 及固定 Starry
配置构建；QEMU 使用同一冻结根盘完整 SHA-256、保存 ELF/BIN、600 秒窗口、
直接内核采样、导出五项哈希和离线只读 fsck。性能必须重测，不能把 CPU 活跃
或启动单元单轮增长当成稳定整机加速。

第一项回归 file_data_read_releases_ext4_metadata_lock 已加入既有 profile_tests：
用真实 ext4/VFS 创建并同步文件，再重新挂载，预热 inode 元数据后清空观测；
通过生产 Inode::read_at 读取文件，必须确实触发 BlockRead，且读事件的开始/
结束时全局元数据 mutex 都未持有。该测试在 R 结束后验证 RED，再实现候选。

## S 本地实现与验证

旧 read_at 路径回归退出 101，0 通过/1 失败，真实 BlockRead 的 Begin/End
观测均为 locked: true，完整输出已展示。实现后同一测试通过，断言未放宽。

PreparedFileRead 位于 rsext4/src/file/read_plan.rs，完成映射与缓存快照后
不再借用 fs/dev；CompletedFileRead 只有完成设备 I/O 才能取得。普通 extent
文件使用该路径，legacy/non-regular/超限和无共享 reader 的设备保留原路径。
ax-fs-ng/src/block/read.rs 复用现有 BlockDeviceHandle::read_blocks，保留
分区范围、对齐与溢出校验。每 inode 保护位于 ext4 live_refs 中，与硬链接
包装器共享；read/write/append/truncate 与 rename 覆盖回收遵循设计锁序。
原 inode.rs、fs.rs、block.rs 按生产能力/测试拆成同名目录模块，未改变模块 API。

已通过的验证（均使用项目 TMPDIR）：

- cargo test -p rsext4 --features host-test：232 通过、0 失败、1 项已有
  外部镜像依赖用例 ignored；其中 PreparedFileRead 9 项通过。
- cargo test -p ax-fs-ng --features host-test,ext4,vfs,profile --lib：112/112。
  新增真实不同 inode 嵌套读进展、同 inode 硬链接写/append/truncate 保护、
  rename 回收前门锁、无 reader/超限完整回退、EIO 释放锁，以及分区校验。
  嵌套交错是确定性回归，不冒充多线程压力测试；现有打开目标 rename 语义
  问题未修复，也未声称完整 Linux rename ABI 验证。
- cargo xtask clippy --package rsext4 --package ax-fs-ng：11/11。
- ax-fs-ng host-test/ext4/vfs/profile 的 lib+tests clippy 通过。
- Starry AArch64 固定功能组合的生产 clippy 通过，依赖 memchr 仍报告已有
  future-incompatibility 提示，不是本候选新增 lint。
- cargo fmt --package rsext4 --package ax-fs-ng 与 git diff --check 通过。

额外构造的重叠逻辑 extent 回归曾退出 101，原文为
`overlapping file extents were accepted`；补有序范围/边界校验后同一测试通过，
损坏映射返回 EUCLEAN。legacy 测试初次错误地假设旧实现支持传统块映射，
实际旧 resolve_inode_block 返回 EOPNOTSUPP；核对源码后将候选设为不启用，
测试验证旧路径仍返回同一 unsupported，不新增格式能力。失败原文均已展示。

固定 Starry 配置构建通过，完成 13044 个 kallsyms 与 BIN 刷新。
S 保存目录 ext4-read-concurrency-window-600，ELF SHA-256 为
2e4dc04614734e9aad165d02600ce966641eba87f3b298d739f9737d58fa98dd，BIN 为
50df83cf5042dd713ad526f3ef169baa1589ea5f15c667329fe12aa586088c6a。
重新校验完整工作根盘 SHA-256 为
d5e8c6347879117246530ba52dacba58a03379f06ddaf19866ca576e9cdd57d4。
2026-09-09 22:56 UTC 启动保存内核，tmux 为 tgoskits-profile-ext4-read-600；
S 没有完成采样，暂无性能结论。故障现场及后续修复见下文。

### S 运行异常与中断表修复

QEMU PID 491862，22:56:44 UTC 前发送 runner，约 22:57:31 开始正式采样。
日志确认 tg_xtask_reused=true、source_reused=true、8 CPU 和原归档/指纹不变。
到 23:05 UTC 仍没有第一条分钟进度，最后的 guest 日志时间为 102.529726 秒；
宿主 pidstat 连续约 107% CPU，一个 vCPU 线程 491870 持续运行，其余主要等待。
这是停滞迹象，不是正常性能结果；等待约定 600 秒及短收尾后采集原现场。
启动 IllegalInstruction 的地址组在 R 中也存在，尚未证明它导致这次停滞。

已按 arch-platform-porting 完整重读调试规范与 boot-debugging 引用，并核对
QEMU 10.2.1 与 GDB 17.1。本地单元测试使用宿主锁，不能取代真实内核调度/
唤醒路径验证。原现场进一步确认 CPU 6 反复处理 NVMe LPI 8199，7 核 idle；
八核 GICR_PENDBASER 错误地指向同一个 pending table。这是 base 已有的
8 KiB stride 被寄存器截断的缺陷，详见 [GIC 修复证据](gic-lpi-pending-tables.md)。
保留原根盘和完整 guest RAM；S 仍不是有效性能结果，不能归因为 ext4 死锁。

T 改为每核 64 KiB 对齐、独立 pending table。生产驱动回归 RED→GREEN，
AArch64 单元测试 22/22，定向 clippy 和固定 Starry 构建通过；修复后的独立
启动诊断实际读回八核独立地址。Unix socket 调试入口测试也已 4/4 通过。
S+T 正式 600 秒测量目录为 ext4-read-gic-alignment-measure-600，重新恢复
冻结根盘并等待完整校验后启动，不连接 GDB；结果如下。

### S+T：锁内读取下降，整体加速未成立

2026-09-09 23:31:52 UTC 启动，约 23:32:59 开始采样，23:43 正常退出。
611 秒、rc=124、build_completed=false，启动 40 单元 / 37 crate；实际采样
610787382352 ns，17197 条记录，所有 dropped/skipped 为 0。五项导出 SHA-256
通过；离线 e2fsck -fn 退出 0。debugfs 的 8 条 chown EPERM 原文已展示，
导出退出 0，内容校验通过。结束盘保留为 rootfs-profile-ext4-read-gic-window.img。

| 指标 | R | S+T |
| --- | ---: | ---: |
| 活跃 CPU 样本 / 总样本 | 14158 / 47077 | 12630 / 47813 |
| CPU 活跃率 | 30.0741% | 26.4154% |
| 启动单元 / crate | 39 / 36 | 40 / 37 |
| ext4 占 mutex 等待 | 88.1659% | 75.2286% |
| ext4 总持锁 ns | 489144742688 | 396684552096 |
| 文件读取持锁 ns | 174474450176 | 41925168880 |
| sync_to_disk 持锁 ns | 168477636800 | 196012872592 |
| 缺页调用链持锁 ns（与读取重叠） | 122658762368 | 33145198400 |
| 同步块读次数 / ns | 67630 / 337504879072 | 63795 / 415129370592 |
| 同步块写次数 / ns | 47937 / 105350864016 | 30908 / 114113722256 |

读取持锁下降 75.97%，但同步占持锁 49.41%，缺页链为 8.36%，不能相加。
CPU 活跃下降，窗口归一化后的启动单元速度仅相差约 1.26%，没有完整编译或
稳定整机加速证据。CPU 5 活跃 83.26%，主要为 rustc 用户态采样，并非先前
CPU 6 的中断循环。此轮没有复现该循环，不宣称所有停滞/PID 问题已解决。

cpu-active.png 与 ext4-lock-hold.png 已实际查看。渲染器 summary.json 新增
leaf_top、caller_top（直接上一层帧）和 page_fault_total_ns；最后一项是重叠
子集而非独立类别，缺失 caller 明确记为未知。真实解析入口回归 RED→GREEN，
渲染器 6 项、启动器 4 项均通过。R 使用相同渲染器重新归约，未修改原始记录。
位图写回预读累计 16976834576 ns，inode flush 读累计 47172664416 ns；
下一步沿同步写回调查，不移除 create 同步或任何 journal flush 屏障。
