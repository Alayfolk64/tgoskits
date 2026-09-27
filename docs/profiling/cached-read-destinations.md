# 缓存读取目标重构记录

设计见 `docs/design/cached-read-destinations.md`。本轮基线是完整冷编译
1869 秒（181/175、rc=0、SHA/ELF/fsck 通过），不是旧十分钟窗口。
最大内核叶子 memcpy 的 82.66498% 来自私有文件缺页读取，见前一轮报告。

## 实现与第一轮静态检查

整条读取遍历移入 `file/cache/read.rs`。CachedRead 只保留请求位置和现有
预读窗口，不跨页持有锁。`read_buf_at` 接收内核 resident BorrowedCursor，
在缓存锁保护源页时直接 append 到最终目的地。原 `read_at` 共用该遍历，
保留 scratch 快照，并在释放所有缓存锁后 `write_all` 到任意 Writer。
FileBackend 暴露同名类型化入口；CowBackend 的单页准备和文件批量装入
两个真实调用点均已迁移。Direct 后端仍使用现有安全切片初始化和短读循环。

第一轮已逐条检查：每页复制范围同时受页边界、原请求 end、cursor 容量
限制；cursor 只在成功复制时推进；返回本次增量；错误保留此前 filled
前缀；零容量/EOF 在分配前返回；updating/tentative 内容不可绕过；命中
不等待无关 I/O，未命中仍按旧锁顺序装入；用户 Writer 没有在缓存锁下
执行。未新增 unsafe、依赖、PTE 或磁盘格式变更，未改变 COW/RSS/截断
listener；arch-platform-porting 的新架构契约修改要求在本轮不适用。
这是去冗余复制的性能重构，不将旧双复制路径描述为数据正确性 bug。

8 个新 host 回归连接在现有 `cache/tests/mod.rs` 下，包括真实缓存命中
时页分配计数不变、无 backing read、MaybeUninit 目的地、预填游标、跨页、
EOF、无关 I/O 锁、失败游标、Direct 多次短读和可重入短写 Writer。原
tentative truncate 回归复用同一复制 helper，原 Starry 私有缺页案例仍由
ktest entry/export 进入实际 prepare_frame。

## 第二轮静态检查进度（2026-09-11）

已完成：fmt / diff check rc=0；ax-fs-ng host-test,ext4,vfs,profile 的
lib/tests clippy rc=0；`cargo xtask clippy --package ax-fs-ng` 8/8 rc=0。
正式 profiling 内核构建 rc=0；axtest release PIE 只构建 rc=0（13.48s）。
ktest 不提供 build-only，先检查 scripts/axbuild/src/ktest/mod.rs，再严格
复用上一轮实际命令的 target/features/SMP=8/PIE/linker/axtest cfg 参数
进行原生 Cargo 只构建，没有因此启动 QEMU。
Starry 110 项矩阵还在执行；不得把这一段当作全部检查完成。

日志统一为 `tmp/preemption-refactor.5NL1NQ/cached-read-*.log`。
保留 pinned core/memchr future-incompatibility warning，不抑制告警。

## 第三轮产物核对准备

冻结目录 `target/profiling/arceos-helloworld/starry/cached-read-destinations`。
ELF SHA `3428372bf42f670dde5bfea6bf83bd5c19437636ca197f2970390b7f433bc07a`；
BIN SHA `2e67c452cab3576e0cda76a2accff0af7ae317f04773aac9da77feef92cfb0ae`；
本轮及私有缺页相关源码归档 SHA
`a82015ba859d639a378436b60fac2783cbb4d4ec7648e468ff1239c75f9c23ee`。
归档覆盖 ax-fs-ng、Starry kernel、ax-io、page-table-generic 和两份设计，
不是完整干净 checkout；其它既有脏修改继续保留在工作树及此前归档中。
axtest 静态 ELF SHA
`386fa00735e9254eea2a4ec1b00a70cae6c1c0166b6a1d5d93bf4c6d20555b59`。

两份 ELF 均 AArch64 ET_DYN、无 PT_TLS；源码 tar compare 和 kernel.sha256
2/2 全量通过。实际反汇编 cached-read-backend.asm 确认 cached 分支直接
调用 CachedRead::new/read_page，保持同一个目的 cursor，没有入口 scratch
分配/第二次复制。仍须等完整矩阵结束，再完成最终一致性检查才准入测试。

新盘从只读冷基底独立复制，完整 SHA 匹配 9968004595dec398480417e210d64f9ed509801d4325d4ab22f43951983f5ea3。
仅给新副本写权限，再替换正式 runner；dump/cmp 通过，runner SHA c94b8669…
未变，只读 fsck rc=0。最终启动盘 hash 尚在执行。未改基底或上一轮结束盘，
未启动本轮 QEMU。最新剩余磁盘空间 68G。

## 最终三轮准入完成（19:45）

Starry 矩阵在 19:44:30 完成，110/110，4m48s，实际 session rc=0。
第二轮全部结束后再次执行第三轮最终一致性检查：fmt、diff check、源码
tar compare、两项 kernel SHA 全部通过；源码/入口/反汇编审查保持有效。
至此才准入全部 host suite 和 8c8g ktest，尚未声称运行通过。
新工作盘最终 hash session 也已退出 0。

读取新盘 hash 时一次在 run cwd 重复使用仓库相对路径，cat 报错原文为
`cat: target/profiling/arceos-helloworld/starry/cached-read-destinations/preboot-disk.sha256: No such file or directory`。
该 cat 子命令失败（1），同一 shell 随后的两项 kernel SHA 成功使 shell
退出 0；不能把 shell 0 算作盘哈希读取成功。随后在正确 cwd 单独读取清单。

## 运行回归通过与全程启动

三轮静态之后，ax-fs-ng 完整 host lib suite **231/231**、rc=0（0.66s），
8 个新 read case 全部实际运行；Starry 8c8g ktest **53/53**、rc=0，
原私有缺页初始化/锁外准备/交错案例都通过。ktest 使用独立 Alpine 盘
和 snapshot，不写正式测量工作盘。两项测试 session 已关闭。

完整编译准备盘 SHA
`74dfd30ff127c93570e70cb770253566a40615f218e2794d619524d2818f3059`；
patched QEMU 完整 SHA 仍为 a1d9a6080e1f025038a81749baa9d9552ae89321ea163abbf704206034f0a6a4。
新工作盘 0644，冷基底/1869 秒结束盘均 0444。接下来仅运行冻结新内核，
无编译/整盘 hash 干扰，不截断全程。

## 完整结果：1974 秒，尚未获得编译加速

2026-09-11 20:21 完整冷编译自然结束，QEMU/session rc=0，workload rc=0，
181 个编译单元、175 个不同包。Cargo 32m28s，axbuild 1965.53s，完整区间
1974s。比已验收的 1869s 慢 105s（5.61798%）；本候选不得记为性能收益。
既有 1869s 冻结内核继续作为已验收比较点，当前源码仅为待优化候选。

6/6 原始 SHA 通过；全部 181 个 name/version 与 private-fault-transactions、
batched-mapping-retirement、event-driven-preemption、full-build-rcsc 和 Linux
full-build-rcsc 全量 cmp 一致；最终 ELF 与 1869s/Linux 逐字节相同，SHA
2792c35275d91f57a38a847be87c4d2b333d7b88cd548bf9e06633288c7dba21。
只读 fsck rc=0（仅保留 extent 可更紧凑的建议，未修复）。结束盘移入本轮
rootfs.img、设为 0444，完整 SHA
2e78d36c631a060f151d8ac0653a718d86e0619ce5b586473d0941ee82c6de6b。
固定工作盘路径为空，QEMU、renderer、盘 hash session 均已关闭。

原始采样 22936 条，prebuild_ns=1974014891296，stop 后 enabled=false，
dropped_cpu/wait/pending/skipped_cpu 全 0。完整 SVG/folded/summary 已生成，
cpu-active、mutex-wait、ext4-lock-hold 三张 PNG 已实际查看。

| 指标 | 1869s 基线 | 1974s 候选 |
| --- | ---: | ---: |
| CPU active/total 样本 | 65818/144956 | 64229/153255 |
| CPU active 比例 | 45.40550% | 41.90989% |
| memcpy 叶样本 | 3167 | 2445 |
| map_range_recursive 叶样本 | 2912 | 2802 |
| try_zero_page 叶样本 | 2478 | 2513 |
| 用户缺页 inclusive 样本 | 12049 | 11095 |
| rsext4 inclusive 样本 | 1650 | 1727 |
| mutex 等待总 ns | 2386284988528 | 3517657751648 |
| ext4 锁等待总 ns | 1459780568256 | 2442824523504 |
| ext4 持锁总 ns | 509301703024 | 637300175888 |
| 同步块读总 ns | 675696045776 | 835784669920 |
| 同步块写总 ns | 128219632064 | 266386837184 |
| 同步 flush 总 ns | 732959658832 | 820997868752 |

等待值跨任务重叠且存在嵌套，不能相加或将其差值解释成 105s 墙钟回退。
本轮没有证明回退全部由缓存读取改动引起，也不能排除宿主 I/O 波动。
可以确认复制成本下降而等待上升；不能据此声称低 CPU 已解决。

当前最大内核 CPU 叶子为 map_range_recursive：2802/64229=4.36252%。
去掉重复递归帧后，2727/2802 样本最近调用者为 PrivateFault::install_missing，
不是 fork。下一候选应检查完整的单页页表安装/遍历边界，而非默认优化 fork。
ext4 持锁最大来源转为 write_locked 的 mutation（288011690160ns，45.1925%），
lookup 为 187823798576ns（29.4718%）；这仍是需要单独跟进的串行等待热点。
