# Inode 写回复用已有表块缓存

## 问题与边界

`cached-inode-metadata-window-600` 的 ext4 持锁时间中，`sync` 为
162144833152 ns，占 37.81%。其中 inode-cache flush 调用链的块读累计
40151958080 ns。该区间是采样放大的累计值，不是可以直接扣除的编译墙钟时间。
缺页会等待同一个 ext4 锁，因此缩短写回持锁也可能让缺页继续推进；具体收益
必须由下一轮窗口验证，不能把这一项当成全部低 CPU 原因。

当前 `load_inode` 使用 `Jbd2Dev::read_block` 的现有四块缓存，但单 inode
及批量写回使用 `read_blocks` 绕过缓存。即使表块刚读过，也再次提交设备读。

这是既有 rsext4 写回边界内的局部优化，不新增公共 API、锁、缓存容量、磁盘格式
或 syscall 行为。保留 create 同步、metadata journal 标记、提交及 flush
屏障。目标是缓存命中时不再读取设备，且保留邻接 inode、待提交日志的最新
内容及失败后的 dirty 状态。非目标是延迟 create 同步或移植 MOSS folio 所有权。

## 方案与不变量

第一版候选未部署 QEMU。补充源码检查发现，`read_block` 在缓存未命中时
可能驱逐不相关的 dirty 块并直接写盘；此前通过的测试只覆盖 clean 命中及
pending 覆盖，不能证明满 dirty 缓存下的日志提交时序。新增确定性回归实际
失败，确认一次提前写盘，因此第一版被否决，不计作性能样本。

- 保持现状：语义不变，但保留上述重复读。
- 复用已有 `read_block` 后复制 `buffer()`：第一版候选，因上述驱逐风险否决；
  单纯保持目标表块 clean 不足以保证不相关脏块不提前写出。
- 只借用 pending 或现有 clean 缓存，miss 保留原直接读：第二版采用；不插入、
  不驱逐、不改变 active/clock，写入仍使用原有 `write_blocks(..., true)`。
- 改用 `buffer_mut`/`write_block`：不采用；它会提前把块缓存标成 dirty，
  增加绕过日志提交而被驱逐写出的风险。
- 扩大缓存或新增第二份缓存：不采用；没有必要引入容量和一致性变化。

源码依据为当前 `fs/rsext4/src/blockdev/journal.rs` 与
`blockdev/cached_device.rs`：日志读取覆盖待提交块；直接写成功后刷新已有缓存；
提交后使缓存失效。本修改不改变这些所有权和一致性边界。MOSS 的批量文件缺页
是另一个层次的 prior art，不能替代本轮 ext4 写回证据。

## 验证计划与状态

先让真实 `InodeCache` 与 `Jbd2Dev` 执行读取和修改，然后让底层设备拒绝读。
旧实现的单 inode/批量 flush 应确定性失败；修复后应不增加读次数并保留邻接
inode。补充日志覆盖、写失败 dirty 保留及重试验证。随后运行 rsext4 全套、
真实 ext4 镜像/fsck、目标 clippy 和 fmt，再构建 StarryOS 做独立 600 秒窗口。

## 实际验证

2026-09-10，真实 `inode_table_reads` 集成测试在修改生产实现前退出 101：
13 项中单 inode flush、批量 flush、失败后缓存重试这三项得到 EIO 而非 Ok，
另 10 项通过。只将两个写回入口的 `read_blocks` 换为 `read_block` 加
`buffer().to_vec()` 后，同一 13 项全部通过，缓存命中只保留最初一次设备读。
日志覆盖测试确认 `device.flush()` 不提前将待提交 metadata 写到原盘。

延迟写回场景归入 `USE_MULTILEVEL_CACHE` 模块：关闭该特性时 modify
本身直接写回，不存在“留下 dirty inode 等待 flush”的前置状态。原有九项
读缓存测试不受该 gate 影响。

`cargo test -p rsext4 --features host-test`：253 项通过、1 项原有外部镜像
用例忽略；包括真实镜像重挂载、journal replay 和 e2fsck 检查。
`cargo test -p ax-fs-ng --features host-test,ext4,vfs,profile --lib`：117/117。
`cargo xtask clippy --package rsext4`：3/3；额外默认缓存加 host-test 的 tests
clippy 通过。cargo fmt 与 diff check 通过。测试模块归组后默认缓存 13/13、
无默认特性 9/9，最后一遍 xtask clippy 3/3 与默认缓存 tests clippy 通过。

候选 StarryOS 内核已通过 `cargo xtask starry build` 构建，保留固定 profile
配置（SMP=8、BACKTRACE=y）。候选 ELF SHA：
ba55c57a972953955c3382a2bd7483f5c16969cfeceeb9c96718020a6c0ae298；
BIN SHA：9587a321abd254d038cde3002d62319bb0f9285b64acb98dae06ab4ff444ce34。
构建保留固定工具链的既有 core/memchr future-incompatibility 提示，未抑制。

先用保存的 X 内核重测 host-config-baseline-window-600，再测
inode-writeback-read-cache-window-600。两侧使用启用 Cargo host gate 和
源码内 TMPDIR 的同一 runner；不能拿旧配置 X 的进度直接归因于本项修改。
尚未跑本项 QEMU 对照，未宣称编译速度提升。

第二版在上述 13 项基础上加入满 dirty 缓存、未发布 dirty 邻块、miss 读失败
重试三项。第一版实际退出 101（14 通过、2 失败）：满缓存读取引发一次
未经 journal commit 的设备写；未发布 dirty 邻块被当成读取快照。
第二版通过 crate 内 `Jbd2Dev::read_block_snapshot` 统一两个 inode 写回入口，
底层 `cached_clean_block` 只借用已有 clean 缓存。pending 优先级、直接写后
刷新、提交后失效、写错误后 dirty 保留均未改变，不扩大其它读路径的行为。

同一回归第二版 16/16 通过，关闭默认缓存 9/9；rsext4 全套 256 通过、
1 个既有忽略；ax-fs-ng 117/117；xtask clippy 3/3 及默认缓存 host tests
clippy 均通过，fmt 与 diff check 通过。第二版内核 ELF SHA：
000802db93285f91a3befb361a32e1e5a5c942186ae475b84a2af70a8555df6d；
BIN SHA：a43653119eb2d0e7ea9916ee3e82ca680519e4aebdf898fc838eb60ada007d04。
第一版内核及原记录已移至 `starry/inode-writeback-read-block-rejected`，未覆盖。

新配置基线在 02:37 UTC 正常结束：613 秒、41 单元/38 crate、未完成编译。
CPU 14004/47857 活跃（29.2622%），完整缺页链 2733/14004（19.5159%）；
无 dropped/skipped，5 项 artifact SHA 与离线 fsck 全通过。ext4 占 mutex
等待 79.5999%；`sync_to_disk` 持锁 217960076992 ns，占 50.1737%。因此
当前的阻塞主因仍指向文件系统同步，不能把 CPU 叶热点与累计等待混作一种指标。

新冻结盘 `tmp/axbuild/rootfs/rootfs-profile-host-config-base.img` 从原
d5e8c634… 起点完整复制并校验，只更新 `/opt/starry-macos-run.sh`，journal
start 为 0、更新前后 fsck 均退出 0。冻结盘 SHA：
9968004595dec398480417e210d64f9ed509801d4325d4ab22f43951983f5ea3。
runner SHA：4f2580b3e557fed6cebb7d4d7f428f5e6542e6b78ea66e2ac32158a613295359。
旧 X 结束盘另存为 `rootfs-profile-cached-inode-metadata-window.img`，未覆盖。

## 第二版 QEMU 窗口结果

`inode-writeback-read-cache-window-600` 在 2026-09-10 02:55 UTC 正常结束：
墙钟 601 秒，采样 601.483072160 秒，rc=124，42 个编译单元 / 39 个 crate
已启动，完整构建未完成。五项 artifact SHA-256、只读 `e2fsck -fn` 均通过，
所有 dropped/skipped 为 0。实际打开 ext4 持锁 SVG 并查看截图。

| 指标 | 同配置基线 A | 安全快照候选 B |
| --- | ---: | ---: |
| 活跃样本 / 总样本 | 14004 / 47857 | 14633 / 46939 |
| CPU 活跃比例 | 29.2622% | 31.1745% |
| 已启动编译单元 / crate | 41 / 38 | 42 / 39 |
| ext4 占 mutex 等待 | 79.5999% | 70.6943% |
| ext4 总持锁 ns | 434410867648 | 386174367264 |
| sync_to_disk 持锁 ns | 217960076992 | 180805838160 |
| inode flush 预读 ns | 52749618112 | 41373738704 |
| 块读 ns | 390543893696 | 376341723888 |
| 块写 ns | 145475091136 | 126567599008 |
| 块 flush ns | 36393788000 | 30498827632 |

B 完整缺页 CPU 链为 2888/14633（19.7362%），ext4 持锁中同步仍占
46.8197%。缺页使用的 `CachedFile::read_at<&mut &mut [u8]>` 占 mutex 等待
28.6076%，与 ext4 等锁形成的下层串行化仍需要继续分析。

这是一组 A/B 的正向信号，不是稳定整机提速或完整编译加速比。窗口长度相差
约 11 秒，累计等待包括多任务及嵌套区间，不能直接相减作为墙钟收益。
下一步区分块请求排队、提交至完成发布、完成后调用者恢复，避免把同步块 I/O
延迟全部误称为设备服务时间。安全候选暂留作为诊断基础，尚未最终验收收益。

B 结束盘已封存只读为
`tmp/axbuild/rootfs/rootfs-profile-inode-writeback-read-cache-window.img`，完整 SHA：
60f7486407e1719b81efa8d021151e70b76709215068b964a0325a0d9da89270。
