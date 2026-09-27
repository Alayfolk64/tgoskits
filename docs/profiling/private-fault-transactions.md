# 私有缺页事务重构记录

设计见 `docs/design/private-fault-transactions.md`。接受的上一轮完整冷编译为
1908 秒（181 个 compile units，rc=0）；本轮完整结果为 1869 秒，详见末节。

## 回归先行：三轮静态检查（2026-09-11，运行前完成）

此阶段只增加测试及 `cfg(axtest)` 分配观测点，尚未修改生产缺页或 remap
行为。既有私有文件缺页测试无语义变动地迁入 `fault_tests/file.rs`。

1. 所有权与失败检查：匿名及 COW 回归使用真实 `resolve_page_fault`，
   分别在真实 `alloc_frame` 前检查地址空间锁。旧路径必然返回
   `(false, false)`，且先完成两条路径再断言，不嵌套持锁变更。
   测试初始化可变切片在 clone 前结束，子映射持续拥有源页。
   观测点按 TaskId 匹配、回调前取出、在自身 IRQ 锁外执行和释放捕获，
   RAII 按 Arc 身份移除未触发观测；非 axtest 编译不包含观测状态。
   remap 回归在实际 TableMeta 完成边界读取仍存活的真实叶子描述符，
   覆盖 4 KiB 和 2 MiB，观测在 PageTable 析构前移除。
2. 编译静态检查：`cargo fmt -p starry-kernel -p page-table-generic --check`
   rc=0；`cargo xtask clippy --package page-table-generic` 2/2、rc=0；
   `cargo clippy -p page-table-generic --test retirement -- -D warnings`
   rc=0；AArch64 SMP=8 的完整 axtest feature 组合 clippy rc=0。
   原始日志在 `tmp/preemption-refactor.5NL1NQ/private-fault-regression-*.log`。
3. 最终调用与产物检查：axtest entry → export → aspace test → 生产
   fault/allocator 的链完整，原有文件缺页案例仍被导出；`git diff --check`
   rc=0。检查 ktest 的参数和实际上一轮日志后，以相同 target/features/
   SMP/PIE/linker/axtest flags 做原生 Cargo 只构建（ktest 无 build-only），
   rc=0，ELF 为 AArch64 ET_DYN。静态 ELF SHA-256 为
   `df6f4667d5edf62f7cfbef5df16ecb6b09a9d5a0714c665860542737d04a3319`。

以上只准入修复前确定性回归，不等于完整重构的最终三轮静态准入。
编译器报告 pinned core/memchr 的 future-incompatibility 警告，原文保留，
不是本次 clippy 失败。只读检索发生三处旧路径错误（rg rc=2）：
`scripts/xtask`、`scripts/axbuild/src/ktest.rs` 和 `retirement-ktest.log`；
原文已直接输出，实际使用 `scripts/axbuild/src/ktest/mod.rs` 与
`retirement-starry-ktest.log`，没有修改源码来掩盖检索错误。

## 修复前回归结果

两个回归均在上述三轮之后运行。generic remap 回归 rc=101，观察到
`replacement became visible before invalidating the absent old leaf: [a00001]`。
Starry 8c8g ktest rc=1，实际新用例断言 `(false, false)` 不等于
`(true, true)`，证明匿名分配和 resident COW 分配均持有地址空间锁。
日志为 `private-fault-remap-red.log` / `private-fault-starry-red.log`，完整原文
已显示；后者由 panic regex 结束，没有声称其余 53 项全部执行。

ax-io 新回归也在单独三轮检查之后执行：先对照 pinned core 的
`written` / `reborrow` 契约审查真实调用；然后 fmt、xtask clippy 2/2、
fixture clippy 均 rc=0；最后重新检查完整 fixture、现有 diff 和 whitespace。
该回归 rc=101，实际 `(Ok(3), Ok(3))`，期望 `(Ok(1), Ok(0))`。
原文为 `private-fault-iobuf-red.log`；随后才修改生产 specialization。

## 完整实现后的源码审查：内核写入前置条件

已实现 private fault 的 snapshot/prepare/commit、计数 source pin、
PreparedFrame RAII、文件初始化范围、remap 两次同步失效，拆出 clone 与
frame 所有权模块。此时尚未运行修改后的 GREEN 或 profiling。

随后对全部直接写物理页的调用点审查发现 `AddrSpace::write(&self)` 仍然
绕过 COW：ptrace 在同一锁内准备/写入；proc-mem 在两者之间释放锁；AIO
可延后复制。不能以 PTE 只读和 pin 计数单独证明源页不被写入。因此补入
真实 fork→内核 write→子页不变的回归，并将原子 kernel-copy COW 准备纳入
设计。这是阻止本轮最终测试准入的源码问题，不是测试已通过。
检索时 `mm/user_memory.rs` 不存在（rg rc=2，原文直接显示）；实际文件
`mm/access.rs` / `mm/io.rs` 和调用点已找到，没有掩盖该检索错误。

### 内核写入回归的独立准入

2026-09-11 18:20 前完成三轮：① 逐个核对 loader、ptrace、proc-mem、AIO
物理写调用及新 fork/write 回归；新断言先于交错注入，单任务串行执行，
不依赖概率或并发数据竞争。② 新 fixture 的实际 AArch64 SMP=8 axtest
clippy rc=0，同参数 release PIE 只构建 rc=0（14.53 秒），fmt check rc=0。
③ 再查观测回调消费/释放、真实入口连接、whole diff check rc=0；最终
静态 ELF 无 PT_TLS，SHA 为
`df4a44731c54461aa697fa5570c0bebdd6b4691d0b8514d1ec177b2079093b44`。
日志为 `private-fault-kernel-write-regression-{clippy,build}.log`。
这只准入尚未修改的 `AddrSpace::write` 失败回归；不是完整重构的最终测试
准入。正在执行的 feature 矩阵也只代表当前中间源码，不算未来修复后矩阵。

内核写入回归随后实际失败（`private-fault-kernel-write-red.log`，rc=1）：
子页读到 `[165]`，预期 `[90]`。完整 QEMU 原文已输出。之后才将 resident
copy 拆到 `aspace/copy.rs`，令 write 取得 `&mut AddrSpace`，在同一锁内
逐页调用既有 COW 所有权转换再拷贝；proc-mem、AIO 只需可变 guard。

新增只读强制写/跨页/NoMemory/缺失页/临时 pin 回归时，第一次 clippy
rc=101（`private-fault-kernel-copy-clippy.log`）：四处 assert_eq 试图比较
未实现 PartialEq 的 StarryError，完整原文已输出。仅将断言改为精确
variant 匹配，复查 rc=0（`private-fault-kernel-copy-clippy-recheck.log`）。

### 写保护发布边界回归准入（18:31）

完整审查还发现 fork 的 `protect_page` 仍使用没有前置 PTE 发布屏障的
AArch64 单地址 flush；解锁读取 COW 源页依赖写保护真正生效，不能留下。
修复前先补 generic 回归。三轮静态：① 读 protect/clone/flush 调用链，
对照 Linux `980ab36ae5972c83f683b939e50c469c4947229e` 的 arm64
`__do_flush_tlb_range`（tlbflush.h:561–611）确认前置 dsb；② fixture
clippy 与 package xtask clippy 2/2 均 rc=0；③ 重读真实 descriptor
观测的指针生命周期/线程隔离及 batch 回调，fmt/diff check rc=0。
fixture SHA 为 `23756e4803645761b8100f56feba8ec94191a21f5b120f29d9af78f4cdf4f756`。
此门槛只允许独立 protect 失败回归，不准入整轮 GREEN/QEMU/profiling。

该回归随后 rc=101，原文为
`permission downgrade lacks descriptor-publication completion: [Flush]`，
完整 `private-fault-protect-red.log` 已输出。之后才将 protect_page 改为
已有 batch 完成接口；不改变 PTE flags 或复制策略。

## 最终整轮静态检查

第一轮（2026-09-11，源码）：已按设计重新审查 snapshot→prepare→revalidate→
commit、SourceFrame/PreparedFrame 的全部释放分支、clone rollback、
文件 short-read/EOF 初始化、两次 remap 完成与 fork 写保护发布。
内核物理写入的实际入口为 loader、ptrace、proc-mem 与 AIO；直接写方法
集中在 copy.rs，授权仍由既有调用者处理，缺失页面不隐式分配。
新测试覆盖原始 RED 以及冲突、撤销、重复提交、NoMemory、只读强制写、
跨页偏移、部分拷贝和精确 owner-count 恢复。此阶段无新增依赖、无
持久化格式变更、不改 syscall ABI 参数，也不声明已完成所有 syscall
差分验证。全量测试尚未运行，性能收益尚未验证。

第二轮（18:38 前全部退出 0）：Starry 全 feature/config 矩阵 110/110；
page-table-generic/ax-io 矩阵 4/4；两个 host fixture clippy、实际
AArch64 SMP=8 axtest clippy 全部 rc=0。正式 profiling 构建及匹配 ktest
参数的 release PIE 只构建均 rc=0。日志为 `private-fault-*-complete-*.log`。
没有用中间矩阵替代这些结果，没有删除 pinned core/memchr 警告。

第三轮（18:39，完整源码与产物）：重新核对 axtest 入口/export/真实
allocator 与 kernel-copy 调用点；fmt、diff check、冻结源码 tar 全量
compare 均 rc=0。反汇编确认 protect 的 PTE store 后调用 batch，remap
先 str xzr、batch 完成、再 str 新 PTE；实际 batch 为 dsb ishst →
vaae1is/vmalle1is → dsb ish → isb。两个最终 ELF 均 ET_DYN、无 PT_TLS。
源码 archive SHA `371f98f9e0580cbb6057ba4d93398d08da1598c8f7d7fd5a4856c6d3fd72c31f`；
profiling ELF `b1a9578994113614a2a21c366312d5bce7a1a6351b460583ce1c0975a8c3bd6c`；
BIN `fa0518cf47c2953f740857ea4544da8167ef7e74da8f6b50a9e8e0da1e982de9`；
axtest ELF `5ee7f48654fee7da27d909327e19dd469a516872cbabb377c8f0bc547a6fa97c`。
kernel.sha256 的两项均在实际 run 目录校验通过。这三轮完成后才准入
整轮 GREEN / ktest；完整编译 profiling 仍需回归通过及冷盘准备完成。

工具操作错误同样保留：一次 sed 误读不存在的 meta.rs（实际 lib.rs）、
一次归档 cp 在 run 目录使用仓库相对路径（未复制）、一次 sha256sum
在仓库目录找 run 的校验清单（未校验）；均已直接展示原文，修正明确路径
后执行成功，不算成功检查。新冷盘继承基底只读模式，首次 debugfs 虽
退出 0 但报 Permission denied/Filesystem not open，cmp 随后退出 2；
只给新工作副本 chmod u+w 后重做 patch/cmp 成功。第一次未成功 patch
后的并发 hash 不作为启动前证据，最终以 verified.sha256 为准。基底及
前一轮只读结束盘未改写。

## GREEN 初次执行与测试契约纠正

18:40 前 Starry ktest rc=0，`AXTEST_SUMMARY pass=53 fail=0 skip=0 total=53`；
新增 private fault 用例为第 8 项，其 PASS 行与既有故障注入 warning 交织，
原始日志保留，没有靠 grep 漏行推断跳过。ax-io 全部测试 rc=0。
泛型完整 crate 首次 rc=101，新增 `same_frame_remap_does_not_break_the_leaf`
的 query.unwrap 失败为 NotMapped；完整包含 warnings 的日志已输出。
已沿 query→translate_recursive_with_level 确认现有 API 只翻译 present
映射；该 fixture 自己将 flags 置零，正确行为就是 NotMapped。真实叶子
观测仍必须严格为单个 `[0x800000]`，不会放宽未 break 的核心断言。

仅修正此测试，不改任何生产源文件。重新完成三轮 fixture 静态检查：
① 对照实际查询与 occupied-leaf 语义并审查断言；② fmt、fixture clippy、
page-table-generic xtask clippy 2/2 全部 rc=0；③ 再读 fixture 与生命周期，
diff check、完整 validated-sources tar compare、原 kernel 两项 SHA 均通过。
因此保留已通过的 Starry 110 项矩阵、实际 kernel 构建和 53 项运行证据，
不重新运行未变更的内核。随后重跑泛型完整 crate；此处还未声称通过。

重跑实际 rc=0：泛型全 crate **79/79**（retirement **10/10**），ax-io
全套含 doctests **183/183**；Starry **53/53**。最终源码快照改名为
`private-fault-validated-sources.tar.gz`，SHA
`989ab30ff3c1359855b0a50794e54537e6ff31e4c02b5e3eb3ca3ecb33d5427b`；
先前快照保留，生产内核仍为原来 b1a957…/fa0518… 两项。
冷盘全量 SHA 与冻结基底 996800… 完全相同后，仅更新正式全程 runner；
runner dump/cmp 与启动前只读 fsck 均通过，最终盘 SHA
`580a364316710131063a306d6427d43a6ae2220e77133c92cce946903c9b6674`。
归档 clippy 日志还有一次文件名 starrry 拼写错误（cp rc=1，完整原文
已显示），修正为已存在的 starry 路径后复制成功，不影响内核或测试结果。

## 首次串口启动未进入采样

18:42 首次 boot 到 shell 后，完整输入只回显 `/bin/sh /opt/starry-macos-ru`，
清行无响应，没有 BEGIN、没有编译或 profile。QEMU/vCPU 大多休眠。
18:44 发送 Ctrl-C 被宿主 tty 转为 SIGINT，QEMU 原文为
`qemu-system-aarch64: terminating on signal 2`，Python 启动器完整
KeyboardInterrupt traceback 已输出，退出 1。随后读取已退出进程的
/proc fd 路径也按实际返回不存在，未伪装成进程仍运行。

同类未提交输入现象在 page-fault-concurrency.md、negative-directory-cache.md
已有记录；根因仍未确认，不能算本次 COW 故障或性能结果。本次目录整体
移动为 `private-fault-console-stall`，结束盘同目录 rootfs.img 只读保留，
只读 fsck rc=0。原 580a… SHA 仅属于这次未进入采样的启动盘。
重新复制原冷基底，同一生产内核/最终测试源码，分段输入。没有截断正在
执行的编译，也没有因为启动失败修改内核或放宽成功判据。

## 完整冷编译验收与下一热点（2026-09-11）

第二次冷启动分段核对串口回显后执行成功，完整编译自然结束，无 timeout
或数量截断。目录 `target/profiling/arceos-helloworld/starry/private-fault-transactions`。
冻结 patched QEMU、8c8g、Cortex-A53、GICv3、MTTCG、NVMe 和相同工作负载。
`STARRY_PROFILE_DEBUG=1` 仅创建 Unix gdb.sock，未连接调试器、未暂停 vCPU。
采样中没有其它本任务构建、QEMU 或整盘 hash。

| 验收项 | 结果 |
| --- | --- |
| 完整测量 / workload rc | 1869 秒 / 0 |
| Cargo / axbuild | 30m43s / 1862.02 秒 |
| 编译单元 | 181 个 / 175 个不同包名 |
| 原始采样记录 / 丢失 | 23505 / CPU、wait、pending、skipped 均 0 |
| 原始产物 / 内核 SHA 清单 | 6/6 与 2/2 全部通过 |
| 全部 name/version 清单 | 与前三轮 Starry 和冻结 Linux 全量 cmp 相同 |
| 最终 ELF | 与冻结 Linux、上一轮 Starry 逐字节相同 |
| 结束盘只读 fsck | `e2fsck -fn` rc=0；仅 extent 优化建议，未修复盘 |
| 图 | 全量 SVG、CPU-active / mutex-wait / ext4-lock-hold PNG 已生成并实际查看 |

启动盘 SHA `18643d9c60627c3288be82e29c98b00f663c13b672e7e2504afea5595fcd4d53`；
结束盘只读 rootfs.img SHA
`5520a4ef86905350ba7998f4bdf7f861414c4e047b4aa855e31d71880ad4a31c`；
最终 ELF SHA `2792c35275d91f57a38a847be87c4d2b333d7b88cd548bf9e06633288c7dba21`。
实际使用的内核为 b1a957…/fa0518…，最终源码归档为 989ab3…，见前文。

1869 秒相对 1908 秒减少 39 秒（2.04403%），相对 2298 秒减少 18.66841%；
仍是 Linux 735 秒的 2.54286 倍。这是单轮观察，不是统计稳健的加速证明。
CPU active 为 65818/144956（45.40550%），上一轮 44.89029%，低占用未解决。

| CPU 口径（active 为分母） | 样本 | 比例 |
| --- | ---: | ---: |
| memcpy，最大内核叶子 | 3167 | 4.81175% |
| map_range_recursive（不含 unmap） | 2912 | 4.42432% |
| try_zero_page | 2478 | 3.76493% |
| find_free_area | 2270 | 3.44890% |
| 用户缺页完整调用链 | 12049 | 18.30654% |
| rsext4 完整调用链 | 1650 | 2.50691% |

全量 folded 栈进一步确认：memcpy 有 2618/3167（82.66498%）经过
handle_user_page_fault → CowBackend::prepare_frame → FileBackend::read_at。
直接父帧中 try_copy_cached_page 占 1815；BorrowedCursor 版 read_at 占 836。
前一轮相同私有文件缺页复制子集为 1691。源码 cache/mod.rs 的 read_at
每次分配一个 scratch PageCache，先从缓存复制到 scratch，解锁后再通过
任意 Write 复制到目标。对真正的用户 Write，解锁是必要的；对本轮
PreparedFrame 的内核 BorrowedCursor，额外页与第二次复制不是必要条件。
样本增加不等于复制字节增加，不能从采样推导精确内存带宽或指令成本。
清零样本中经 populate 的子集从 2647 降为 23，其余从 848 变为 2455；
这说明调用边界迁移，不代表初始化工作全部消失。

mutex 累计 2386.28499 秒，其中 Ext4Guard::acquire 1458.98149 秒
（61.14029%）；缓存页复制 379.757 秒，文件 read_at 310.734 秒。
ext4 锁等待 1459.78057 秒，持锁 509.30170 秒，其中 lookup 45.25712%、
write 28.45437%。这些跨任务累计值可重叠，不能相加为墙钟；ext4 仍是重要
等待来源，但 CPU 最大复制热点首先对应文件缓存/私有缺页读取边界。

下一轮重构该读取边界，复用初始化范围类型区分内核目标与任意用户 Writer，
不为省复制而持缓存锁调用用户 Writer，不直接映射无完整生命周期协议的
缓存物理页。整轮实现之后仍需重新三轮静态检查才允许测试。

预览首次 `rsvg-convert` rc=127 原文为
`/bin/bash: line 2: rsvg-convert: command not found`；复用仓库内
tmp/profile-svg-env/bin/cairosvg 后三图均 rc=0，不安装全局工具。
一次 rg rc=2 的原文为
`rg: fs/ax-fs-ng/src/file/handle/read.rs: No such file or directory (os error 2)`；
实际入口已确认是 file/cache/mod.rs，没有更改代码来掩盖检索失败。
