# 有界目录缺失缓存与统一发布

## 证据、范围和成功标准

增长缓存窗口的最大 ext4 持锁来源是目录 lookup：88.787315 秒、54.00%。
VFS `lookup_and_cache` 只保存成功结果，NotFound 直接返回；相同不存在的名字
会重复进入全局 ext4 锁。该语义缺口可由真实 ext4 + VFS + profile hook 确定性
复现，但现有采样不能区分正负 lookup，不能声称全部 88.79 秒都可消除。

受众为同一 8c8g QEMU 内 tg-xtask/Cargo/rustc 及其工具链路径探测。成功条件：
同目录、无中间修改、缓存容量以内的重复缺失查询不再进入 ext4；创建/链接/
重命名后立即可见，不缓存其它错误，动态文件系统默认不启用；并发旧结果不得
发布到修改后的缓存。最终以同冻结基盘 600 秒内核采样确认收益，不能只看微测试。

本轮属于共享接口、缓存并发与资源保留的高风险变更；本设计需在合入前独立
审查，本地实施不代表获得合入认可。不改磁盘格式、syscall 参数、凭据检查、
目录查找算法或 I/O 锁粒度，不引入 Linux RCU-walk、全局 dentry shrinker。

## 内部复用与 Linux 对照

已读 VFS 目录缓存、全部命名修改入口、mount 插入/释放调用方、ext4 目录
适配与真实 mount 测试。当前 dir.rs 历史 c0b606fc7、1ab948f77、828538f64
没有负缓存。2026-09-11 查询相关开放 PR：#2287 head
`ab8fd71bd74121e7cc75057c37ad9c563169edf4` 面向 mutation credentials，文件清单
不修改 DirNode 缓存；本轮保持其凭据边界不变。未把开放 PR 的标题视为完整审核。

Linux 固定源码 `980ab36ae5972c83f683b939e50c469c4947229e`：

- [ext4_lookup](https://github.com/torvalds/linux/blob/980ab36ae5972c83f683b939e50c469c4947229e/fs/ext4/namei.c#L1760-L1813)
  对不存在名字保留 inode=NULL 的 dentry；casefold 有专门例外。当前 rsext4
  使用字节名称，挂载拒绝不支持的 incompat feature，不扩展到大小写折叠目录。
- [lookup_dcache / lookup_one_qstr_excl](https://github.com/torvalds/linux/blob/980ab36ae5972c83f683b939e50c469c4947229e/fs/namei.c#L1763-L1827)
  复用缓存，负项在非创建路径返回 ENOENT；创建仍由真实文件系统完成。
- [内核路径解析文档](https://docs.kernel.org/filesystems/path-lookup.html#more-than-just-a-cache)
  区分可以信任本地缓存与需要 revalidate 的文件系统。借鉴负项缓存语义，
  不照搬 Linux 的 dentry 所有权、RCU 和 inode rwsem。
- [path_resolution(7)](https://man7.org/linux/man-pages/man7/path_resolution.7.html)
  要求查找不存在项与权限错误分别处理。本轮不增删既有调用方权限检查，
  只为 lookup 结果不依赖调用方身份的 ext4 启用，不缓存 I/O、权限或无效名称错误。

## 替代方案与选择

| 方案 | 决定 |
| --- | --- |
| 不修改 | 重复不存在查询仍争抢全局锁，保留为性能对照 |
| 扩大 inode/block cache | 可能减少 I/O，但不会消除负查询的全局锁与目录扫描 |
| 给所有文件系统默认缓存缺失 | 拒绝；devfs/proc 等存在后端外部发布事件 |
| 扩展现有 DirNode 缓存，ext4 显式选择 | 采用；共享现有生命周期，不制造第二份 dentry 所有权 |
| ext4 目录 I/O 锁外执行 | 另一个方向；需目录版本、inode reap 与映射再验证协议，不是负缓存的前提 |

## 所有权、发布与失效

私有 DirectoryCache 拥有正项、至多 128 个负项和一个版本号，全部由现有
目录 cache mutex 保护；把散落的原子版本更新与缓存内容更新收拢到同一临界区。
负项只拥有名称，不拥有 inode/父目录。达到上限清空负项批次，正项不被逐出；
按需分配，每目录名称字节至多 128×255，此外有哈希表开销。非全局内存上限。

lookup 在 cache 锁内观察版本与缓存，锁外执行 backend，再在锁内仅发布版本
未变的结果。与 mutation 重叠的旧调用可返回其原始结果，但不能污染后续查询。
每次成功命名修改在同一锁内更新正项、清负项、更新版本；后端 mutation 报错
也清负项并更新版本，覆盖已修改目录但同步阶段失败的情况。缓存内不执行 I/O、
外部回调、递归释放；被替换的 DirEntry 返回锁外销毁。跨目录 rename 不同时
持有两个缓存锁，保留原有 user_data/mountpoint 迁移。

`DirNodeOps::negative_cache_generation` 默认 `Ok(None)`，只有名称匹配精确、
lookup 结果不依赖调用方身份且提供跨目录别名一致版本的后端可启用。
ext4 复用现有 `InodeMetadataReader` 的 `InodeInfo.change_attribute`，不新增
inode 版本副本或 I/O 锁。缓存 reader 使用 try_lock 且事务期间暂停可见性；
缺失、忙、私有事务或退休状态返回 None，必须回到真实 lookup。
`dir/insert.rs`、`hashtree/mutation.rs` 和 `metadata/apply.rs` 在父目录变化时
调用 `parent_dir_change`，更新此 inode 版本。目录 rename 后旧打开句柄与新
路径虽有不同 DirNode，但读取同一 canonical inode 版本，不能保留失效负项。

负项命中比较其记录版本与后端当前版本；未命中在 backend lookup 前后复核
同一可用版本才发布。后端版本读取在 cache 锁外、不做 I/O；正项命中不读取
后端版本，保持原有缓存开销。is_cacheable=false 仍完全绕过缓存。
完整 clear 也更新本地版本，避免进行中的 lookup 重新建起引用环。

重命名目录的失效漏洞在实施中静态推演发现，未运行过未覆盖该情况的候选。
最终选择复用已有 change attribute，而不是增加独立共享 inode 状态。

## 验证和回滚

先在前一轮已三次检查的生产版本加入真实 ext4 重复缺失回归；再次检查测试接线、
host Clippy、xtask 矩阵与实际内核静态构建后运行失败侧。新实现和并发/错误/变更
测试完成后重做三轮静态检查，然后运行相同回归及相关 crate 全套测试。
不在三轮完成前启动修改后测试、fsck 或 QEMU。

定向覆盖：真实 ext4 缓存命中不等全局锁、create/link/symlink/unlink、普通和跨目录
rename、exchange/whiteout 既有语义、动态后端默认绕过、非 NotFound 不缓存、
可控旧查询与 mutation/clear 交错、失败 mutation 后重查、负项容量不逐出正项。
本轮意图是 ABI 保持型性能变更，真实 VFS 内核入口和 ext4 回归直接覆盖状态边界；
不新增 syscall 或改变 errno，因此不另造只验证 ENOENT 的用户态用例。
若发现用户可见语义偏差，必须补正确 Starry case 并重新过门槛，不能当性能误差。

回滚无需根盘迁移：恢复本轮前源码/内核快照。触发条件包括缓存导致旧结果、
动态目录不可见、内存增长不可控或同窗口性能稳定退化。首轮性能结果见文末。

## 静态门槛与失败回归记录

2026-09-11 00:42:25，旧生产版本加真实缺失回归后，host Clippy、xtask 8 项
矩阵、全仓格式/diff 和实际内核构建三轮均退出 0。随后唯一回归退出 101：
第二次缺失查询进入 ext4 锁，被 hook 确定性拒绝而非挂死；完整原文保存在
`tmp/ext4-final-static.ryo14j/negative-red.log`。改动前两个 crate 相关源码保存于
同目录 `source-before-negative-cache.tar.gz`，没有覆盖工作区既有修改。

新重构三轮已全部结束，**00:54:46 才放行新实现测试**：

| 轮次 | 检查 | 结果 |
| --- | --- | --- |
| 1，00:53 | 两个 crate fmt；axfs-ng-vfs lib/tests host-test Clippy；ax-fs-ng lib/tests host-test,ext4,vfs,profile Clippy；复核负项上限、错误与测试接线 | 全部退出 0 |
| 2，00:54:06 | `cargo xtask clippy --package axfs-ng-vfs --package ax-fs-ng`；逐个核对 create/link/unlink/symlink/rename/clear、别名版本和锁外析构 | 11/11，退出 0 |
| 3，00:54:46 | 全仓 fmt check、git diff check、真实 AArch64 SMP=8 profiling 内核构建；复核 core parent_dir_change 与事务 reader pause/rollback | 全部退出 0；release 12.41 秒，14071 kallsyms |

第一、二轮完整输出为 `negative-static-host.log`、`negative-static-matrix.log`。
内核构建保留既有 core/memchr future-incompatibility 提示，未添加 allow。
门槛放行时新实现行为测试和新 QEMU 窗口均尚未运行。

放行后核心 RED 回归通过，VFS 49 项 lib + 10 项集成测试全部通过。
ax-fs-ng 首次全套 223 项中 222 通过、1 项失败，退出 101；单独复现同样失败。
新增用例错误预期 whiteout 成功，但 `fs/rsext4/src/file/rename.rs:519` 明确
返回 `unsupported`、operation `rename:whiteout`，原始错误为
`called Result::unwrap() on an Err value: OperationNotSupported`（终端保留原文）。
这是新增测试的能力假设错误，不是放宽现有回归：修正为精确断言 unsupported、
源/目标不变，然后继续普通跨目录 rename 的成功验证。不修改 core 或生产实现。
修正后仍重新执行三轮静态检查再放行重测，首次失败证据不删除。

测试假设修正后三轮于 **00:56:59** 全部完成：host 全特性 Clippy；xtask
两 crate 11/11 矩阵；全仓 fmt/diff 与实际 SMP=8 内核构建均退出 0，
生产代码未再改动。随后完整 ax-fs-ng lib **223/223**、无忽略、退出 0。
成功侧完整日志为 `negative-ext4-green.log`，VFS 为 `negative-vfs-tests.log`。
首次全套终端输出的成功项中部曾被工具截断，`negative-ext4-tests-first.log`
如实保留该截断；失败段及单项复现原文完整展示，不声称该文件是首次全套完整日志。

核心回归确认负缓存命中时持有 ext4 全局锁仍能完成，不只是后端调用次数减少。
真实 ext4 回归另外覆盖 create/link/symlink/unlink、重命名后旧打开目录句柄、
跨目录 rename、unsupported whiteout 不变性和 exchange 的 inode/user_data 迁移。
VFS 11 个新增用例覆盖有界负项、正项保留、错误、版本不可用、别名与可控并发交错。

首轮三次检查的 ELF SHA-256 为
`c9a7a461e3ff02eb70cffdef64445aeecd78df3da9aa5b9116ebb2d6776426aa`，BIN 为
`1df74444fae3215f11d9ef0f0e072a7b86bc96c407518b4373e0172505548e92`。
变更前局部源码快照 SHA-256 为
`f7ad07f88445d4b85b28a9ce52561e1114732b02cc90a0db3d4c03e3ef3a89b0`。
新运行目录 `target/profiling/arceos-helloworld/starry/negative-cache-window-600`；
旧增长缓存结束盘移动保留为 `tmp/axbuild/rootfs/rootfs-profile-growing-cache-window.img`。

测试假设修正后再次静态构建并注入符号，最终保存与目标构建目录相同的 ELF/BIN：
`e1a4d024f0b8e1389878aa2b663c2cdac191ac6cc2791be038a02eb7f7cf9424` /
`b2449148f0f1178bc2b7e7bbe90c4019dbc919c36bbc5d50f9f6b2e818e25523`。
运行与符号解析都使用这对最终文件，不混用前一次符号注入产物。
全量源码快照 `source-negative-cache.tar.gz` 的 SHA-256 为
`15a5836ee537394c8f0265aa5ed47e7f1ec3ff35f35d838b429b3eb4079d3045`，tar 比较退出 0。
新根盘完整 SHA 与冻结基盘一致，只读 fsck 退出 0，未优化或修改 inode extent tree。

首次启动仅回显 `/bin/sh /opt/star`，后续字符与清行均无响应，尚未进入脚本或
采样窗口。QEMU 与八个 vCPU 大部分处于宿主等待；宿主终端 -icanon、-ixon。
根因未确认，不能归因于本轮缓存。历史 [缺页实验](page-fault-concurrency.md)
也记录相同输入现象和分段输入恢复。01:04 对本次专属 QEMU PID 820053 发送
SIGTERM，原文 `qemu-system-aarch64: terminating on signal 15 from pid 820516 ()`，
启动器退出 0；不以此作性能成功。
保留日志/内核于 `negative-cache-console-stall`，结束盘为
`tmp/axbuild/rootfs/rootfs-profile-negative-cache-console-stall.img`。重新复制只读
基盘，保留同一内核与配置，以分段串口输入重试。未执行中的任务没有被中途截断。

## 600 秒窗口结果

2026-09-11 01:05:54 重启，分段输入后脚本正常执行；01:06:54 开始采样，
01:17 正常完成收尾并关机，QEMU 退出 0。实际采样 607968348848 ns，
elapsed=608、rc=124、build_completed=false；tg-xtask/source 两项复用为 true，
cargo_host_config=true，冻结源码、工具链与 host gate 没有改变。
期间没有并行宿主编译、测试或其它 QEMU，仅轻量读源码、文档与日志。

| 指标 | 前一轮增长缓存 | 本轮目录负缓存 |
| --- | ---: | ---: |
| 窗口秒数 | 611 | 608 |
| 启动编译单元 / 不同 crate | 55 / 52 | 62 / 59 |
| 非 idle 样本 / 总样本 | 19951 / 47382 | 22663 / 47016 |
| CPU 活跃采样占比 | 42.1067% | 48.2027% |
| lookup 持锁 ns | 88787315440 | 60279573088 |
| 全部 ext4 持锁 ns | 164411787408 | 163679086384 |
| ext4 等锁累计 ns | 396234723200 | 384528999328 |
| mutex 等待累计 ns | 1196876599136 | 928233803296 |
| 块读累计 ns | 404854460864 | 453753118320 |
| 块写累计 ns | 19515354768 | 34778788464 |
| flush 累计 ns | 111231574208 | 136148068400 |

lookup 持锁下降 32.11%，mutex 等待下降 22.45%，活跃占比增加 6.10 个百分点，
启动单元增加 7 个。与此同时总 ext4 持锁仅下降 0.45%，块 I/O 累计增加，
最长单次持锁 5.198→7.443 秒，最长 flush 7.232→17.759 秒。窗口编译进度
不同，不能据此把全部开销变化归因于缓存，不能声明稳定整体加速或完整构建耗时。
这是同起点的单轮正向信号，保留实现并继续用新热点引导下一轮。

当前持锁调用者前三：lookup 60.279573 秒（36.83%）、写入 38.201787 秒
（23.34%）、read_inode 27.347623 秒（16.71%）。最长的完整聚合调用栈是
ext4-commit 后台页写回 38.191479 秒（23.33%），其次是 rustc 缺页读取
22.880077 秒（13.98%）；它们不是最长的某一次调用。全部缺页持锁子集
24.907752 秒，与读取调用者重叠，不相加。前台 page_or_insert→write_locked
驱逐写回仍为 0 条持锁栈，上一轮增长缓存效果没有重新消失。

对完整 block-read-sync.folded 全量统计，lookup 内块读 54763364992 ns，
其中 find_named_entry_in_parent 目录块读取 34376646400 ns，InodeCache
装载 20386718592 ns，两组恰好覆盖该子集。前一轮对应为 81453341936 /
68656594256 / 12796747680 ns。这里是 Ext4Disk::read 的含缓存、同步等待的
累计时间，不是纯硬件耗时；不能把命中快慢直接推断成 NVMe 请求次数。
首次通过工具读取全量 folded 的返回内容被长度上限截断，随后改用 awk 在
文件上全量聚合；本段只使用后者，不使用被截断的 75 行小计。

五项导出 SHA 全通过；只读 e2fsck -fn 退出 0，只有 extent 收窄建议，未修改
根盘。debugfs 导出退出 0，8 条恢复属主 EPERM 原文已展示，文件内容校验通过。
49 条 IllegalInstruction 的全部地址和频数与
[Cargo 指令探测记录](cargo-instruction-probes.md) 一致：预检 7 条、工作负载
42 条，没有增加异常类型或新的地址。采样所有 dropped/skipped 均为 0。

已生成并查看 [持锁火焰图](../../target/profiling/arceos-helloworld/starry/negative-cache-window-600/rendered/ext4-lock-hold.svg)
和 [活跃 CPU 火焰图](../../target/profiling/arceos-helloworld/starry/negative-cache-window-600/rendered/cpu-active.svg)，
[完整统计](../../target/profiling/arceos-helloworld/starry/negative-cache-window-600/rendered/summary.json)
保留按 CPU、任务、事件和调用栈的全部聚合结果。PNG 渲染曾因 GObject 无法
转换 Cairo Context 退出 1；用系统 Rsvg pixbuf 接口成功查看，保留弃用提示。
这不影响原始采样或 SVG 的生成。

最近 Linux 同配置对照仍为 604 秒、177 单元 / 171 crate、71.8212% active，
见 [Linux 对照](linux-qemu-comparison.md)。两边都未完成完整编译，Linux
缺失内核栈的 active 样本仍标为 unknown，不把它们当作确定的用户态时间。
