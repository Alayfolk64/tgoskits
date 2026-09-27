# Ext4Disk 干净块读缓存候选

## 证据与范围

固定 8 CPU / 8192M 的 `sync-preread-write-sequence-window-600`：块读累计
469664245776 ns，其中 sync_to_disk 为 133017880032 ns；目录 lookup 持锁
70362268000 ns，read_inode 持锁 49720681040 ns。现有 rsext4 设备缓存只有
4 个槽，且 journal 提交后全部失效。锁外 inode 预读仍有大量失效与重新读取。
这些数据证明读等待值得处理，不证明下面的缓存已经加速。

候选受众是通过 Ext4Disk 使用 rsext4 的文件系统，不改变 guest 编译命令。
成功标准是在同一冻结盘的 600 秒窗口降低实际设备块读和 ext4 持锁，观察
CPU 活跃及已启动编译单元；窗口不能证明完整构建速度或所有故障恢复正确。

## 设计与替代方案

在 OS glue 的独占 Ext4Disk 内复用 ax-fs-ng 已依赖的 LruCache，保存最多
64 个已经成功读取的 4 KiB 块，payload 上限 256 KiB，另有有界索引开销。
满缓存时复用淘汰块的堆缓冲，不把 4 KiB 数组放到内核栈上。

- 只命中、保存单块读取；多块读继续原请求，不拆 I/O、不主动预读。
- 所有 write 仍立即下发，包括 journal、checkpoint、replay 和普通数据写。
  成功后用实际提交的字节更新已缓存重叠块；失败可能部分写入，直接清空
  全部干净缓存并传播原错误。缓存从不包含未提交的脏数据。
- 缓存只由 &mut Ext4Disk 访问，生产挂载后继续由 filesystem mutex 串行化；
  不新增锁、共享写接口或原子状态。独立 Ext4Reader 仍直接读设备，不填此缓存，
  不会引入“锁外旧读取在新写入之后填缓存”的交错。
- pending journal、inode/data cache 的优先级不变；原 4 槽脏设备缓存、事务容量、
  write/flush 次序均不改。BlockRead 埋点放在命中判断之后，继续统计实际设备读取。
- 缓存属于单个固定 region 的 disk 实例，销毁实例时释放；与已有 mounted ext4
  缓存一样，要求同一 backing region 不被外部写者绕过文件系统修改。

保持现状仍反复读 metadata；直接增大 rsext4 4 槽缓存会改变脏块驱逐与提交边界，
源码还明确记录更大容量曾出现恢复一致性问题。新缓存不存脏数据、不会因驱逐
产生任何写操作，且位于所有文件系统设备写入之下，因此不复用该脏块状态机。
后台写回和共享 reader 缓存还需要新的事务/并发协议，本候选不混入。
不新增 crate、公共 API、依赖或驱动契约；属于影响一致性与默认资源用量的
高风险候选，合入前仍需文件系统边界独立审核。

## Linux 依据与内部先例

Linux `980ab36ae5972c83f683b939e50c469c4947229e` 的
[`bh_uptodate_or_lock`](https://github.com/torvalds/linux/blob/980ab36ae5972c83f683b939e50c469c4947229e/fs/buffer.c#L3007)
在 buffer 已有效时不重新提交读请求，
[`bh_end_write`](https://github.com/torvalds/linux/blob/980ab36ae5972c83f683b939e50c469c4947229e/fs/buffer.c#L200)
按写完成结果更新有效性。这里只借鉴干净数据有效性，不移植 buffer_head 锁模型。
本地路径历史 ed3d4a1e6、当前 BlockDev/Ext4Disk、inode table cache 与已有
journal-write-batching 材料已核对；先前检查的 PR #2015 改动不提供此
独占 raw-disk 干净读缓存。本轮未重新审核该 PR，也未发起合入。

## 验证与回滚

按用户当前性能优先要求暂不运行测试套件。先源码检查和 rustfmt，在当前
QEMU 采样结束后执行定向 clippy、内核构建；只在新的实验目录和冻结盘副本
运行，不改已保存内核。保存对应源码、ELF/BIN、原始 profile、完整哈希和
正常退出后的只读 fsck。原有 host 回归不冒充本候选的运行验证。

未验证范围：注入部分 write 错误、回放交错、容量淘汰和完整编译回归。
若一致性或性能回退，保留候选及错误证据，以上一保存内核和冻结盘重新运行，
不覆盖或修复原始失败盘。完整测试与持久化审核缺失时，不宣称可直接合入。

## 首轮结果与预读查询衔接

`clean-disk-read-cache-window-600`：603599570496 ns，48 个启动单元 / 45 个
crate，完整构建未完成；QEMU 退出 0、rc=124、tg-xtask 复用，丢样均为 0。
活跃样本 14532/47081（30.8660%），父窗口为 26.3268%。5/5 哈希通过，
只读 fsck 退出 0；20 条 extent tree 可收窄提示未修改镜像。

| 指标 | 父窗口：descriptor 合批 | 加入干净读缓存 |
| --- | ---: | ---: |
| 实际采样 ns | 613278508656 | 603599570496 |
| 设备块读调用 | 62299 | 46868 |
| 设备块读累计 ns | 507559338528 | 374584943264 |
| ext4 持锁累计 ns | 422417752624 | 378131761184 |
| read_inode 持锁累计 ns | 60940062928 | 7878401376 |
| sync_to_disk 持锁累计 ns | 246588413264 | 272637300224 |
| 块写累计 ns | 116131727568 | 200447731824 |
| flush 累计 ns | 57577306352 | 87986328640 |

读调用下降 24.7693%、ext4 持锁下降 10.4839%，但同步写/flush 增加；
不同工作进展、窗口长度和单次波动不允许推算完整构建加速比。
当前 sync_to_disk 占 ext4 持锁 72.1011%，仍是最大持锁者。
最忙 CPU 2 的 4393 个样本属于 rustc，blk-hctx/2 只有 50 个，
不能把该核活跃 86.18% 误认成块设备维护线程占满核心。

首轮还暴露 88177413408 ns 的锁外 inode 预读：prepare_flush 只能看见
pending 和上层 4 槽缓存，不知道下层已有干净副本，仍从独立 reader 重读。
下一候选给 rsext4::BlockDevice 增加默认返回 None 的只读 cached_block 查询，
由 Ext4Disk 提供真实干净块；BlockDev 仅在本层无 clean 命中时查询下层。
pending 优先级不变，不公开底层设备对象、不新增驱动接口；只借用完整一个
block_size 的有效数据，禁止 I/O、脏状态或未完成写入副本。
查询默认 None 保持原设备实现；相比暴露 raw_device 或增加一套可选 trait
对象注册，只增加已有设备边界所需的单个只读能力。属于共享 API 扩展，
合入前须审核其有效性契约；本轮仍不运行测试套件。

ELF `42a2c2286d7bdbb312ebfaca6cdc5bfaed306fc19a8eb809cf939ebdcf2d43dd`，
BIN `4caeab87a9b7b3d255893e34646dc163eaaa9d9498e05dc1cbce7167c93e2434`。
只读结束盘 `tmp/axbuild/rootfs/rootfs-profile-clean-disk-read-cache-window.img`。
