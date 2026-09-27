# 私有文件缺页：锁外准备与锁内重检提交

## 状态与问题

本地三阶段实现与内核回归已通过，独立 600 秒窗口已完成，未显示整机收益。
高风险并发/页表改动，合入前需要
内存管理边界维护者独立审核设计。本次不提交、不推送，也不修改实板配置。

`inode-identity-window-600` 的实际采样为 604.512285776 秒，活跃 CPU 为
24.5756%，31 个编译单元。mutex 等待中用户缺页调用链占 41.2427%；最大单栈
`rustc → handle_user_page_fault → AddrSpace::handle_page_fault → CowBackend::populate
→ FileBackend::read_at` 占 20.3831%。ext4 全局锁和 CachedFile io_lock 分别占
全部 mutex 调用点等待的 66.6984% 和 30.4217%。这些是累计等待，不能相加成为
墙钟时间，也不能声称所有 ext4 等待来自缺页。

改动前 task/user.rs 在整个 populate 期间持有进程 AddrSpace 的可睡眠互斥锁。
一个线程文件缺页等待磁盘时，同地址空间的其它缺页、映射操作也必须等待。
目标是让文件读取不持有地址空间锁，同时保证被撤销或替换的映射绝不收到旧页。

## 参考与方案选择

实际核验的 MOSS 仓库：`/home/wuxun/Projects/moss-smp-scheduler-optimization`，
HEAD `5a54e4413c9657bfb531cc32f6d688090465200d`。
`kernel/src/mm/aspace/mod.rs::handle_page_fault_concurrent_inner` 实现锁内准备描述、
锁外 execute、锁内重检 commit；CowFaultPlan 使用 Arc 身份与拥有的页引用处理
映射替换及页面生存期。之前文档引用的 dc54e032/45543403 对象当前不可解析，
不使用这些旧标识作为本轮源码依据。

本轮读取当前相关路径最近 5 次提交（包括 COW clone 回滚 #2096），查询 GitHub
开放 issue/PR 的 `page fault` 关键词并读取候选 diff。PR #2000 为文件缺页
扩大到最多 32 页的预读窗口，不释放地址空间锁，与本方案互补但不同时移植。
PR #1993 改用户内存复制的 OOM 错误转换，本阶段保留当前错误分类，不包含它。
其余搜索结果多为 perf、平台和驱动能力，不与本次私有页准备协议等价。

另核验本地 Linux commit `980ab36ae5972c83f683b939e50c469c4947229e`：
`mm/filemap.c::filemap_fault` 在 I/O 前可能释放 mmap_lock，返回 VM_FAULT_RETRY
要求上层重新找 VMA；还通过 folio/invalidate 锁和长度重检处理 truncate。
本阶段不声称移植这些 Linux 文件缓存生存期协议或修复 Starry 原有 EOF 信号差异。

备选：继续调整调度器不能解除已确认的 I/O 串行化；仅扩大页缓存降低缺失率，
但不能让同进程线程越过正在读盘的缺页；仅放开缓存命中读取处理的是下层锁，
不能代替地址空间边界。完整引入 MOSS 读写地址空间锁、并发页表和 folio 引用
同时改变多个所有权层，当前先不采用。选择保持原可睡眠互斥锁，只拆私有
文件缺失页的准备阶段。缓存命中并发单独测量，防止混合归因。

## 边界与不变量

1. 仅用户缺页入口的 4 KiB 私有 Cow 文件映射、PTE 确实 NotMapped 时适用。
   PTE 是硬件页表条目；VMA 是进程的虚拟地址映射区间。现有 COW 写故障、
   共享文件、匿名映射和大页仍走原实现，不新增用户 ABI 或外部依赖。
2. 锁内提取拥有文件引用的计划，记录映射身份、区间、权限、地址和文件偏移。
   不把 MemoryArea、AddrSpace 或页表的借用带到锁外。
3. 锁外分配并零初始化独占物理页，按原 EOF/未对齐文件起点规则读取；失败释放
   临时页。文件引用保证关闭 fd、解除映射后读取对象仍有效。
4. 锁内重检映射身份、区间、权限及 PTE。Arc 身份区分同地址 unmap/remap，
   不能只比较 VA/文件 inode。另一线程已提交满足权限的 PTE 时丢弃重复临时页。
   不满足或映射变化时重试当前映射；重试次数有界，之后走既有锁内路径。
5. 安装 PTE 与 RSS 计数保持同一地址空间锁；提交失败不泄漏临时页，不重复收费。
   保留现有架构 update_mmu_cache 调用，不顺便改变跨 CPU TLB 协议。
6. 失败分类仍在锁内观察映射存在性，保持原 SIGSEGV 的 MAPERR/ACCERR 行为。
   进程退出/exec 的 aspace Arc 在整个调用期间有效；读取旧对象的完成不能重建
   已被 clear 的映射。只读 fd close 不改变已建立 mmap 的文件引用寿命。

## 验证

- 使用真实 AddrSpace、CowBackend 和页表的内核 axtest，不用源码文本或只测常量。
  自定义 FileNode 只提供可控读取边界；在 read_at 内检查实际 aspace.try_lock。
  旧实现必定返回锁不可用，先实际运行 RED，再实施三阶段路径并验证 GREEN。
- 在同一读取边界注入 unmap、相同地址重新映射、权限收紧、另一线程已提交等
  确定性交错，检查最终 PTE 内容、权限、RSS 和清理；错误读取不得留下映射。
- 此确定性锁/I/O 交错不能由普通用户态 syscall 测试控制，最低层内核 axtest
  为主要回归；真实用户态 tg-xtask/rustc QEMU 工作负载验证完整入口。不能据此
  宣称全部 mmap/mprotect/munmap ABI 已经过差分验证。
- 格式化、相关 clippy、内核 axtest 后运行单独 600 秒窗口；冻结工具链、8 核
  8 GiB QEMU、持久根镜像和工作负载。测量期间不并行跑宿主编译。
- 导出全部采样文件并检查 SHA-256；QEMU 退出后只读 e2fsck。比较 CPU 活跃、
  缺页锁等待、I/O、编译进度，不把不同事件的 inclusive 时间相加。

回滚是恢复原用户缺页调用路径；无磁盘格式变更或持久状态迁移。

## 本地实现与实际回归

`fault.rs` 负责三阶段编排，`backend/cow/missing.rs` 拥有临时页及映射快照；
Cow 原单文件按所有权、页分配和测试拆成目录模块。所有页表修改仍使用原 mutex，
没有移植 MOSS 的并发页表或 folio 直映。单页读取复用 `prepare_frame`，直接填充
独占物理页，较旧 `alloc_file_run` 的单页分支少一次中间 Vec 和复制；性能归因
需包含这一分配/复制变化，不能全归于解锁。

真实回归命令：

```sh
TMPDIR=/home/wuxun/Projects/tgoskits/tmp cargo xtask ktest qemu -p starry-kernel --arch aarch64
```

RED：原持锁读取实现退出 1，首个新用例的原始断言为
`file fault kept the address-space lock across backing read_at`。
GREEN：三阶段实现使用同一命令，4 vCPU/512 MiB QEMU 中 46 pass / 0 fail / 0 skip。
新增一个内核用例内部依次覆盖解锁读取、读取中 unmap、同址重映射、撤销权限、
重复缺页保留已提交物理页、I/O 失败不留映射六个场景。故意注入读取失败时
`Failed to prepare private file page for VA:0x40000000: filesystem I/O failed`
是预期警告，不是测试失败。完整原始输出已在执行时展示。

GREEN 保存于 `target/profiling/arceos-helloworld/starry/page-fault-regression-green/`：
ELF SHA-256 `0e3db96577036c3634a9bb5eafbe4d5c03991dc0556094ecd8612b298ae741dd`，
BIN SHA-256 `7f3d726e5671b304bf8f95dfd4711be5d56b9e86b0fe305e766448bc4d565d00`。
另通过 `cargo fmt -p starry-kernel`、匹配 QEMU feature/target 的生产 clippy，
以及 host-test 的 `mm::` 20 项、`file::fs_tests` 4 项；未声称完整 host suite 通过。

测量目录 `target/profiling/arceos-helloworld/starry/file-fault-window-600/`：
ELF SHA-256 `11fdf44b46cd390d6c8360615ca8035b937ef9be5fb285d4a754b431748a0687`，
BIN SHA-256 `241fd99f867cbcd5ec7c13e6d69af82dcde3d4d01d8d9695fa34c66a809c1afc`。
对照是上一轮 cache-hit 内核；两者都已使用 ext4 限定的 inode 身份快路径。

首个启动在串口输入过程中停在部分命令，未开启采样，保留于
`file-fault-console-stall/`；发送 Ctrl-C 被宿主 tty 转成 SIGINT 后 QEMU 退出。
离线只读 fsck 退出 0。相同内核重新启动、分段输入后工作负载正常进入采样。
该失败的串口根因尚未确认，不能认定为缺页死锁或计入性能数据。
重启时临时启用了仅本机的 GDB 端口，未连接调试器、未暂停 vCPU；启动脚本的
临时调试选项已撤回。采样结束后补充首次写缺页、读后 COW 写缺页以及未对齐
文件段前后清零三项内部场景，再次实际执行同一 QEMU ktest：46 pass / 0 fail /
0 skip；单个新用例现覆盖九种内部场景，不影响已保存的测量内核。

补充边界测试的内核另存 `page-fault-boundaries-green/`，ELF SHA-256
`19d4c2f4a0ec2666ff41f8a779125eb1978e0b0f126bee5e017ba431c5f3923e`，
BIN `2e945dcf8af73f2ca02fb0c0fe306d4738b052be9694f18abc3e07905643a00a`。
后续显式 `cfg(axtest)` clippy 发现并修复测试 hook 复杂类型与原有 FD 测试辅助
函数两处冗余 usize 转换，匹配 target/features 的 clippy 已通过；不改变生产行为。

## J 窗口结果：局部等待下降，整机未改善

`file-fault-window-600` 内核采样 612.492828240 秒，guest 墙钟 613 秒，
超时退出 124，启动 34 个编译单元 / 32 个 crate，完整构建未完成。
五项采样文件 SHA-256 全部通过，QEMU 退出后 `e2fsck -fn` 退出 0。
已生成并实际查看 CPU 活跃图及 mutex 等待图；所有采样 dropped/skipped 为 0。

| 指标 | I：缓存命中并发 | J：再加私有文件三阶段缺页 |
| --- | ---: | ---: |
| 内核采样秒数 | 605.941400512 | 612.492828240 |
| CPU 活跃样本 / 总样本 | 13551 / 47379 | 12792 / 47949 |
| CPU 活跃比例 | 28.6013% | 26.6783% |
| 启动编译单元 | 36 | 34 |
| mutex 累计等待 ns | 2415972710960 | 2533101181264 |
| 用户缺页栈 mutex 等待 ns | 674306010048 | 585413615728 |
| ext4 锁占全部 mutex 等待 | 81.6897% | 86.0439% |

J 的 ext4 锁调用点 `0xffffffff803a6248` 已用本轮 ELF 解析为
`Ext4Filesystem::lock`，累计 2179578405520 ns；缓存慢路径调用点
`0xffffffff800c73cc` 为 `CachedFile::read_at`，279675533248 ns（11.0408%）。
用户缺页调用链占全部 mutex 等待 23.1106%，相较 I 的 27.9103% 下降，
但两者都是包括下层锁的 inclusive 等待，不能等同于地址空间锁本身耗时。

J 块写累计 112305798960 ns：create 51398258816 ns（45.7663%），
unlink 38853580864 ns（34.5962%），set_len 13500075056 ns（12.0208%）。
CPU 活跃图中用户态叶子占 36.9293%，其余包括 preempt、地址区间查找、
清零与页表解除映射；不能把占全采样 73.3217% 的 idle 忽略后声称 CPU 已充分使用。

结论：只将私有文件缺页 I/O 移出地址空间锁尚未解除底层 ext4 串行化；
单轮结果不支持整机提速。当前代码保留作为下一轮对照基础，下一候选单独改变
JBD2 同阶段连续块提交，不混入更多缺页、调度或缓存容量变化。
