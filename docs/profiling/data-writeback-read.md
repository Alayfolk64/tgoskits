# 完整数据块写回避免重复读取

## 状态、证据和范围

真实 DataBlockCache 入口的回归已运行 RED：4 项中 3 项失败、1 项通过，退出 101。
确认完整块淘汰多读一次、已有完整缓存的局部修改写回重复读、完整覆盖写被读 EIO 阻断。
已应用局部 helper 优化，同一组 4 项全部 GREEN，尚未声称编译加速。本轮不改变缓存容量或锁模型。
K 的诊断复测内核已在本修改前独立保存，不包含该 helper 优化。

J `file-fault-window-600` 全部同步块读累计 351241293312 ns；包含
`DataBlockCache::write_block_static` 的完整折叠栈累计 28180158240 ns（8.02%）。
该函数无条件分配并读取旧块，再用完整 cached data 覆盖后写回。它在已有
ext4 全局锁内执行，使本可直接写回的完整块额外等待一次读取。

同一份采样中 `InodeCache::load_inode` 累计 75610981728 ns，inode flush
累计 26797340592 ns。这些是另一项候选的证据，不混入本次变更；不同调用链
可能包含彼此，不能把所有 inclusive 分类相加当作独立墙钟耗时。

## 复用和方案

基线 `affddc3fecec02b31df94573063e4840433c9ebe` 的当前本地工作树。
已实际读取开放 [PR #2015](https://github.com/rcore-os/tgoskits/pull/2015)，
head `6d5cc09f45a073680a270ae0b6047b24fd9eaff5` 的完整 data_block.rs diff。
其 `write_block_static` 已有完整块直接写、短数据保留 read-modify-write 的局部
方案；本候选复用此边界，不同时引入其批量淘汰、缓存容量与驱动修改。
未作 PR 合入评审、没有批准或拉取整套分支。

选择在原 helper 中对 `data.len() >= block_size` 使用现有 write_blocks，
保留只写前 block_size 字节的旧约定；较短数据仍读旧块保留尾部。
不新增接口、依赖、持久化格式、unsafe 或并发所有权。现有 DataBlockCache
文件超过 800 行，本次若仅修改这一 helper，不借性能修改重排整个既有 cache
状态机；新增验证放在独立集成测试文件，代码组织的后续整理不作为性能收益。

## 验证门槛

`tests/data_writeback.rs` 调用生产 create_new/modify/flush 路径，验证完整脏块
淘汰不读旧块、读设备报错不影响全覆盖写回、部分修改后的原有尾部不变，
显式写回失败仍保存脏字节并可以重试。先验证旧实现在新增断言失败，再应用
局部修复、验证相同测试通过。需要额外保留短数据 helper 的行为覆盖。

之后运行 fmt、项目 clippy、默认缓存配置测试 clippy、rsext4 全套和 ext4
适配层真实 fsck，再独立 600 秒 QEMU 比较；K 测量期间不运行宿主编译。

## 2026-09-10 验证结果

以下命令均设置 `TMPDIR=/home/wuxun/Projects/tgoskits/tmp`：

- `cargo test -p rsext4 --features host-test --test data_writeback -- --nocapture`：
  改生产实现前 3 failed / 1 passed，退出 101。完整报错直接输出，包含
  `full dirty eviction read the old block`、`flush reread an already complete cached block`
  及预期 Ok 却得到 `Err(Ext4Error { code: EIO, context: None })`。
- 修改后 `cargo test -p rsext4 --features host-test`：207 passed / 0 failed / 1 ignored。
  包含相同 4 项回归、短数据保留尾部、超长输入不覆盖下一块以及全部 8 项 journal commit 测试。
- `cargo xtask clippy --package rsext4`：3/3 检查通过。
- `cargo clippy -p rsext4 --features host-test --tests --no-deps -- -D warnings`：
  默认开启缓存的测试配置检查通过；项目 host-test 矩阵的 no-default-features 不能替代此项。
- `cargo test -p ax-fs-ng --features host-test,ext4,vfs --lib metadata_tests -- --nocapture`：
  5/5 通过，包含真实镜像 create/目录同步和 fsck 检查。
- `cargo fmt -p rsext4`、`git diff --check`：通过。

显式 flush 失败后保持脏块的测试不代表 LRU 淘汰失败状态机已全面修复；本修改
仅删除完整块覆盖写前的冗余读取，没有变更淘汰顺序或声称解决其它缓存错误。
独立 QEMU 窗口 L 已正常结束，目录为
`target/profiling/arceos-helloworld/starry/full-block-writeback-window-600/`。
固定配置内核构建通过，ELF SHA-256
`a9694de4e98dabacf61dcd06365f68039e283186869e0371a9d3392b38c8ce29`，BIN
`64dada295ea33280511adf57537eb0ead6ff6768d7d30208239553893545be91`。
直接对照为已通过五项哈希与 fsck 的 K2；仅增加本项完整块 helper 修改。

窗口 PASS：608 秒、rc=124、build_completed=false、tg_xtask_reused=true；
启动 33 个编译单元、31 个不同 crate，与 K2 相同。实际内核窗口 608052735360 ns，
47634 个样本中 11941 活跃（25.0682%），35693 idle。全部丢样计数为 0，
五项导出 SHA-256 通过，正常关机后的只读 fsck 返回 0；两张主要火焰图已经查看。

| 指标 | K2 | L |
| --- | ---: | ---: |
| 活跃样本比例 | 24.4281% | 25.0682% |
| 启动编译单元 | 33 | 33 |
| 写回旧块读取累计 ns | 20238505952 | 0 |
| 全部块读累计 ns | 365181206320 | 365252419344 |
| 全部块写累计 ns | 85974902112 | 84254340800 |
| mutex 等待累计 ns | 2595672828224 | 2654822949024 |

本 helper 的冗余读取确实消失，但总块读和构建进展未改善，不能宣称整体加速。
L 全量折叠栈：ext4 锁叶子 2003243173808 ns，占 mutex 等待 75.4568%；
CachedFile::read_at 锁叶子 631330854144 ns（23.7805%）；经过用户缺页入口
923041053360 ns（34.7685%），与前两项重叠，均非墙钟比例。
文件内容块读 203172739056 ns，inode 装载 50879059024 ns。

下一轮只增加持锁者/设备 flush 观测，不混入其它性能修改。PID identity panic
在 L 未复现，但仅有 track_caller 诊断，不声称已修复生命周期问题。
