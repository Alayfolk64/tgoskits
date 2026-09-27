# 位图写回重复读取

## 问题与方案

S+T 固定 QEMU 600 秒窗口中，sync_to_disk 占 ext4 持锁 49.41%；
BitmapCache::write_bitmap_static 的 backing read 累计 16976834576 ns。
这不是墙钟收益预测，只是已观测到的同步路径开销。

当前 BitmapCache 两条加载路径都建立 BLOCK_SIZE 长度的完整快照，modify
仅给调用者 &mut [u8]，不能改变长度；缓存内部不接受外部 CachedBitmap 替换。
然而 write_bitmap_static 总是读取旧块后用完整快照覆盖，导致无用 I/O。

候选只在完整块快照时直接调用原 Jbd2Dev::write_blocks(..., true)，保留
元数据 journal 路由、提交顺序、flush 屏障和脏代数验证。非完整长度仍保留
旧 read/modify/write 路径，不扩展格式或错误语义。没有新 API、锁或依赖。
成功标准是生产缓存 flush/flush_all/evict/LRU 的重复读消失，完整字节、
pending journal 可见性及失败后的重试不变；整体收益由下一轮 QEMU 决定。

替代方案：保持现状保留多余读；提高块缓存容量仍有缓存一致性风险且不消除
根本重复；取消 create 同步违反已验证的持久化约束，因此不采用。
inode 表仅更新部分记录，不能直接套用整块覆盖条件。

依据为 affddc3fecec02b31df94573063e4840433c9ebe 工作树；已检查 bitmap
路径历史 1ab948f77/828538f64、既有数据写回优化和 journal 实现，GitHub
开放 PR 搜索 bitmap writeback 无结果。外部资料为 2026-09-10 读取的
[Linux ext4 bitmap 文档](https://docs.kernel.org/filesystems/ext4/bitmaps.html)，
用于确认块/inode 位图的职责，不借此推断本项目缓存一致性。

这是既有内部写回边界的局部性能修复，不改 syscall 参数、ABI、权限、
共享关系或持久化格式；不新增 syscall 测试，最低层真实缓存/I/O 回归更能
确定性区分重复读，随后运行现有真实 ext4 镜像/fsck 和 QEMU 编译验证。

## U 本地验证与固定窗口

新增 tests/bitmap_writeback.rs 的 6 项真实缓存/API 回归，旧实现退出 101，
5 失败、1 通过：重复读取次数超出预期，且完整快照 flush/evict/journal 写回
被 backing read EIO 阻断。完整原文已展示。增加完整长度快速路径后同一
6 项通过，没有放宽断言；保留短数据 read/modify/write 兼容路径。

使用项目 TMPDIR 的验证：rsext4 host-test 全套 238 通过、0 失败、1 项原有
外部镜像依赖 ignored；ax-fs-ng host-test/ext4/vfs/profile lib 112/112。
cargo xtask clippy --package rsext4 的 3/3，以及带默认缓存的 host-test
tests clippy、cargo fmt --package rsext4、git diff --check 均通过。
已检查 xtask std runner：只有全量/按 git ref 选择，不能选择此定向回归和
ax-fs-ng 的 ext4/profile 组合，因此定向行为验证使用相应 Cargo 命令。

固定 Starry 构建完成 13045 个 kallsyms，保存目录 bitmap-writeback-window-600。
ELF SHA-256：2fbfae66b33f6ea1362bae4cd852a90cdae1eee0d573bca5ec4d24a54fe16bab；
BIN：f012eaf99d0c9a317af79871576ec0b7aa39581b909709a8cdc9de9fbe9ab7c4。
前一轮结束盘保留；工作盘重新从冻结盘复制，启动前等待全量 SHA-256 完成。
本轮相对 S+T 仅增加位图写回优化。

## U 的 QEMU 结果

2026-09-10 窗口正常结束：elapsed=608、rc=124、build_completed=false，
44 个已启动编译单元 / 41 个 crate，复用 tg-xtask 和源码。内核采样窗口
607869825616 ns，20076 条记录，所有 dropped/skipped 为 0；5 项 SHA-256
全部通过，e2fsck -fn 退出 0。debugfs 退出 0，但恢复 guest 属主的 8 条
Operation not permitted 原文已输出；文件内容由 guest 哈希完整验证。

CPU 活跃 15354/47412=32.3842%（S+T 为 26.4154%）。完整缺页链 3320 个
样本，占活跃 CPU 21.623%；其中 memset 1004 个，占缺页 30.241%。
全部 memset 1128 个、find_free_area 1070 个，分别占活跃 CPU 7.35%、6.97%。
cpu-active.png 和 ext4-lock-hold.png 已实际查看，SVG 与 folded 同目录保存。

mutex 等待 2030672805264 ns；ext4 锁 1675428326288 ns，占 82.5061%。
总 ext4 持锁 406319522768 ns；同步 175272872400 ns（43.1367%），
lookup 55235150048 ns，read_inode 50661301888 ns。缺页链持锁
46211398112 ns 是其中的重叠子集，不是完整缺页 CPU 开销。

位图写回的旧块读调用已消失（S+T 为 16976834576 ns）；块读仍有
64903 次 / 421953600496 ns，其中 inode flush 44314606752 ns，Jbd2
read_blocks 41484551136 ns。块写 36732 次 / 119666971808 ns；flush
10009 次 / 30677960528 ns。同步持锁较 S+T 下降约 10.58%，但总持锁
略增、ext4 等待没有下降，不能把单窗口更多编译进展认定为稳定整体加速。

下一轮只增加新分配页面清零候选，保持 U 作为直接前态；仍从相同冻结根盘
启动，用新采样验证缺页 CPU 热点，不将同步等待与缺页 CPU 比例混用。
