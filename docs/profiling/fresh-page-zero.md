# 新分配 4 KiB 用户页清零候选

## 证据与范围

S+T 的 cpu-active.folded 有 12630 个活跃样本，完整用户缺页链包含 2927
个（23.17%），其中 943 个叶子是 Starry kmod 的逐字节 memset。全部 memset
共 1038 个（8.22%）。这些是 CPU 样本，不是 ext4 持锁或 off-CPU 时间。
本候选不能直接消除最大的 ext4 同步等待，但针对缺页链的明确 CPU 开销。

MOSS HEAD 5a54e4413c9657bfb531cc32f6d688090465200d 的
docs/profiling/riscv-buildstorm-zicboz-zero-20260725/zicboz-zero-report.md
只在新分配 frame 发布前清零，保留通用 memset。其全局 memset 字宽实验曾
导致可复现系统级失败，独立字节验证通过并不能排除并发问题，因此本轮不改
全局符号、不改 kmod 导出和 kallsyms，也不复制该失败方案。

用户/调用方是 Starry alloc_frame(zeroed=true)。成功条件为：4 KiB 新页
内容全零且不越界、CPU 迁移不能改变中途使用的块尺寸、原缺页/COW/RSS
回归通过、最终 QEMU 的目标 CPU 栈下降且文件系统完整；不承诺墙钟收益。
大页、非 AArch64、现有共享页、页表页、设备内存和任意长度 memset 不在范围内。

## 设计与替代方案

在 ax-cpu 的 AArch64 memory 边界提供 try_zero_page，调用者保证一页完整、
4 KiB 对齐、普通可写 RAM 独占访问且执行期间禁止迁移。读取当前 CPU 的
DCZID_EL0，DZP 置位或块大小超过 4 KiB 时返回不支持且不写内存。
允许时按 4 << BS 的真实块尺寸执行 DC ZVA；不猜测 64 字节，不缓存某核
能力后在另一核使用，不启用 SIMD、不修改控制寄存器。

Starry 只在刚分配且大小为 4 KiB 时用 PreemptGuard 保持 CPU，调用成功
才跳过原 ptr::write_bytes；不可用或其他尺寸/架构沿用旧路径。锁外文件
准备和原锁内缺页都经过这个唯一清零点，清零后才注册 frame/PTE 引用。
guard 不关闭 IRQ，范围只含一个页面的清零，不包括分配、文件 I/O 或页表安装。
内联汇编不使用 nomem/readonly，不改变原有 PTE 发布/缓存维护协议。

保持现状保留已观测的逐字节成本；直接取消匿名页清零泄漏旧数据；改全局
memset 扩大影响且 MOSS 有失败记录；整组移植并发页表/folio 改变多个所有权
边界；仅优化完整新页是本轮可独立验证的最小范围。

本地 Linux 980ab36ae5972c83f683b939e50c469c4947229e 的
arch/arm64/lib/clear_page.S 先检查 DZP，再用 DCZID 的尺寸执行 DC ZVA，
不可用时普通存储清零。这里只借鉴能力检测与页面边界，不复制 GPL 汇编。
[ARM DDI0488H](https://documentation-service.arm.com/static/5e906b9fc8052b1608760b6b)
的 DCZID 描述确认 DZP/BS 字段；相关寄存器可访问性还须在
实际 EL1 QEMU 验证，不把 EL0 用户态模拟测试当成 EL1 证据。
原 ax-cpu / 内存 / 平台目录没有同等 clear_page 能力；GitHub 搜索发现
PR #2067 是 THP/分配器/拆页能力，不属于 CPU 清零接口，DC ZVA issue 搜索无结果。

## 风险和验证计划

新增架构共享 API/unsafe 属高风险，合入前须独立审核本设计；本任务不提交 PR。
页清零不改 syscall 参数、返回值、权限、共享关系或 EOF 行为；主要回归放在
真实 kernel axtest，验证整页字节、相邻哨兵、既有文件段边界和 COW。低层
寄存器解码测试覆盖 DZP 和所有 BS 编码，最终 ELF 必须看到 DC ZVA。
这是新增性能快路径；内容测试本来应在旧清零路径通过，不能冒充正确性 RED。
性能差异由同根盘 600 秒采样验证，测量进行时不编译或运行另一台 QEMU。

## V 验证进度

2026-09-10：AArch64 EL1 / cortex-a53 / 4 CPU 的内核 axtest 两次均为
47/47，第二次由 axtest 输出明确记录 fresh_frame_zeroing dc_zva_used=true。
AX_LOG=warn 会过滤第一版 info 诊断，因此改用测试框架输出；生产路径无日志。
两项 DCZID 解码测试由 qemu-aarch64 执行，2/2 通过，并通过同目标 tests clippy。
这两项仅证明寄存器字段解码，实际 DC ZVA 的证据来自上述系统级 EL1 测试。

cargo xtask clippy --package ax-cpu --package starry-kernel 已通过前 54 项，
包括 ax-cpu 全部 33 项和 Starry AArch64 基础/guest-profile 配置；第 55 项
sg2002-wifi 的 aic8800 构建脚本等待超过两分钟，占用 Cargo 锁。主动发送
SIGINT 结束该过宽矩阵，退出 130、无额外错误输出，不能记为全矩阵通过。
随后当前 AArch64 guest-profile/smp/NVMe/net 生产配置 clippy，以及带
cfg(axtest)、build-std=core,alloc 的真实 axtest_kernel clippy 均通过。
cargo fmt --package ax-cpu --package starry-kernel、git diff --check 通过。

固定 profiling 内核构建成功，13049 个 kallsyms。V 保存于
target/profiling/arceos-helloworld/starry/fresh-page-zero-window-600，
ELF SHA-256 为 d7f10be7113ca485774b8cd703f8cb203af95e864da5a2132944c5a1eeec8049，
BIN 为 09a5034005cde572404a664c3e0a2a02f4a23c37e7db16f27c71abd0248beed8。
同目录 axtest.elf 保留验证内核，disassembly.txt 可见 try_zero_page 的
DCZID 读取、DZP/BS 检查和 DC ZVA 循环，地址 0xffffffff8037b58c 起。

U 结束盘保存在 tmp/axbuild/rootfs/rootfs-profile-bitmap-writeback-window.img.zst。
2026-09-13 为后续全程测试腾出空间：对裸盘与该已有压缩包解压内容逐字节
核对全部 17179869184 字节一致后，仅删除重复裸盘，压缩备份保留只读。
裸盘 SHA-256 为 20b9760c3c1cc0204f874d03184612273541aa722269684ae90372d3db031669，
压缩包 SHA-256 为 eb23780c669fc662a2380e6571d9a10e7fbd81ff62e6ff1bdce5bd3457428262；
证据在 tmp/cache-writeback-ownership.m2tahG/cleanup.log，可完整恢复。
V 工作盘从冻结盘复制，全量哈希 d5e8c6347879117246530ba52dacba58a03379f06ddaf19866ca576e9cdd57d4
确认完成后启动；无其它 Cargo/QEMU 进程，未启用 GDB。00:24:28 UTC 启动
八核测量 guest。

## V 600 秒结果

窗口正常结束，elapsed=604、rc=124、build_completed=false，启动 40 个单元、
37 个 crate，18002 条聚合记录。5 项 SHA-256 全部匹配；debugfs 导出返回 0，
8 条 chown EPERM 是宿主无法恢复 guest 属主，不影响通过哈希验证的文件字节。
e2fsck -fn 返回 0，仅提示 extent tree 可以收窄，未修改根盘。
结束盘保留为 rootfs-profile-fresh-page-zero-window.img.zst（只读）。
2026-09-13 准备 VMA gap-index 全程测量时，全部 17179869184 字节与已有
压缩包解压内容一致、确认无进程使用后，仅删除重复裸盘，释放约 5.5 GiB。
裸盘 SHA-256 f166df30f186fb2e917a89599bb43e9ff755688ba2e86ab2c1cbf6915274c7d1；
备份 SHA-256 65152ae69d9f5b862b4de09b71bee36e6e5404f4cb59d5aa29c5ef04d60a39d2。
可完整恢复，校验和清理记录在 tmp/vma-gap-index.6UocCb/cleanup.log。

CPU 13226/47235 活跃（28.0004%），idle=34009，采样无 dropped/skipped。
完整用户缺页链 2580（活跃 CPU 的 19.5070%），其中新清零函数 359、
残余 memset 41。全部 try_zero_page 369、全部 memset 111，保守合计 480，
相对 U 全部 memset 1128 少 57.4468%；仅缺页清零 400 相对 1004 少 60.1594%。
这是相近固定窗口的采样成本变化，不是单位缺页延迟，也不是构建加速百分比。
新清零 helper 包装自身没有单独叶子样本。

活跃叶子为 user-space 5010（37.88%）、exit_preemption 1356（10.25%）、
find_free_area 818（6.18%）、spin_release 781（5.91%）、unmap 650、map 546、
memcpy 399、try_zero_page 369。CPU 3 活跃 87.16%，其它核 14.32–26.65%。
mutex 等待 2309504936480 ns，ext4 1736353429904 ns（75.1829%），
CachedFile::read_at<&mut &mut [u8]> 552229976064 ns（23.9112%）。
ext4 持锁 411761523312 ns，其中 sync_to_disk 199790058640（48.5208%）、
set_len 49295330064、lookup 45231486544、read_inode 41141901680、
create 31158388608、metadata 22637082912；缺页链子集 37972409808。
块读 62143 次 / 390046637616 ns，其中 inode flush_all 49907279056、
Jbd2Dev::read_blocks 29386858944；块写 31248 次 / 142760725888 ns，
设备 flush 9052 次 / 30087835296 ns。等待是跨任务累积，不能除以 604 秒
当作单任务或整机利用率。已实际查看 cpu-active 和 ext4-lock-hold 火焰图。

结论：目标清零 CPU 成本明显下降，但单元数 44→40、CPU 活跃 32.38→28.00%，
同步持锁 175.27→199.79 秒，因此不能声明编译整体加速。保留窄范围清零实现，
继续优化 ext4 同步中的已证实重复 I/O；需要后续重复测量排除窗口波动。

## CPU 叶子归因限制：重新开中断的采样聚集

V 原始 CPU 记录排除 task=idle 后，exit_preemption 的 1356 个样本中，
1341 个在 0xffffffff802c1710，12 个在 0xffffffff802c14e8，另两处共 3 个。
匹配 V disassembly.txt 确认 0xffffffff802c170c 是 msr daifclr, #2，
0xffffffff802c1710 是其后第一条 ldp。98.89% 的该函数样本集中于恢复 IRQ 后。
profiler 从 IRQ 入口保存的 UserRegisters.elr 采样，不是函数自身计时器。
因此该 10.25% 叶子宽度受 IRQ-off 延迟交付影响，不能解释为函数自身耗时，
也不能把所有这些样本从实际工作中丢掉或重新归给猜测的函数。

MOSS 同 HEAD 的 riscv-buildstorm-guard-inline-20260725/guard-inline-report.md
记录了相同的 release 采样归属问题。riscv-buildstorm-preempt-state-token-20260725/
preempt-state-token-report.md 的 token 微优化没有端到端收益且被撤销。
本轮不照搬该否定候选；find_free_area 的 818 个样本及页表遍历值得进一步
检查，真正 IRQ-off 耗时须用专门持有探针证明。ext4 的显式等待/持锁计时仍保留。
V 同步持锁进一步按调用者聚合：create 148620445632 ns、unlink 44839724496、
rename 6208683200、显式 fsync 两种调用栈合计 121205312。主项不是应用主动
频繁 fsync，而是适配层创建/删除后的全局同步。不能直接删除这些同步；
此前真实镜像回归已证明目录块绕过日志、inode/bitmap 尚未持久时会不一致，
详见 ext4-create-writeback.md。异步化需要先建立完整目录元数据事务和恢复边界。
