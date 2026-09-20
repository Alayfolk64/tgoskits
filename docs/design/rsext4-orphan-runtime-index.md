# rsext4 运行时 orphan 索引

## 问题与证据

OrangePi 5 Plus 的 StarryOS 自编译 run20 在 189.9 秒失败前留下 19,512 个内核 CPU 样本。除用户态与 idle 外，最集中的文件系统栈是文件扩容进入
`truncate_inode -> orphan_contains -> orphan_next -> get_inode_record`：其中一条栈有 741 个样本，并记录约 2.03 秒块读取。该遍历发生在挂载级 ext4 状态锁内，还会延长页面错误、普通读取和后台写回对同一锁的等待。

当前 `orphan_contains` 从超级块的 `s_last_orphan` 开始，每次都线性读取完整 classic orphan 链，即使被查询 inode 根本不在链上。编译会同时保留大量已 unlink、尚未释放最后引用的临时文件；每次普通目标文件扩容因此重复扫描这些无关 inode。

直接用户是 rsext4 的 truncate、unlink、rename、reap 和挂载恢复路径；间接受益者是 StarryOS 上并行创建、扩容和删除文件的构建负载。

## 成功标准与非目标

- 挂载完成后，普通 orphan 成员查询只访问挂载内存状态，不读取 inode bitmap、inode table 或块设备。
- runtime 索引与 `s_last_orphan`/`i_dtime` 链在成功事务后一致；事务失败恢复两者的旧状态。
- 挂载仍完整验证链的 inode 范围、allocation、空 inode 和环；可写挂载仍在暴露普通操作前完成 orphan recovery。
- 添加、删除、非头节点删除、失败回滚、只读挂载验证和崩溃恢复语义不变。
- 不改变 ext4 磁盘格式、journal credit、公开 API 或 StarryOS syscall 行为。

本改动不实现 ext4 orphan file 特性，不把 classic 单链表改成新的持久格式，也不承诺仅凭本地测试给出板端加速比例。板端收益必须在有人可恢复电源时，以完整编译前后数据确认。

## 内部与外部 prior art

当前实现来自 `refactor(rsext4)!: align ext4 semantics with Linux 7.1`（`9e46fbfa6c`），后续 owned-I/O 重构仍保留逐次扫描。开放的 draft PR #2469 在 head `57c42301b9567c2d455299b2ead6830d28834714` 上使用相同实现，没有可复用的 runtime membership 索引；本设计与该 PR 的事务快照边界互补，不复制另一套文件系统状态。

Linux 参考版本为 `980ab36ae5972c83f683b939e50c469c4947229e`。`fs/ext4/orphan.c` 在 `ext4_orphan_add` 中把 inode 同时加入持久 orphan 表示和超级块内存链表，在 `ext4_orphan_del` 中先通过 inode 自身的 `i_orphan` 做 O(1) 快速判空，再更新持久链；`ext4_inode_orphan_tracked` 直接检查 inode runtime 状态。Linux 只在挂载恢复时遍历持久链，不为每次成员查询重新扫描磁盘。

这里借鉴“磁盘状态负责恢复、内存状态负责运行时成员查询”的所有权划分，不复制 Linux VFS inode、链表锁或 orphan file 实现。

## 方案比较

| 方案 | 结果与取舍 |
| --- | --- |
| 保持每次完整扫描 | 单一磁盘事实源，但 CPU/I/O 为 O(链长)，并在全局锁内放大竞争。 |
| 只在文件增长时跳过查询 | 状态简单，但可能漏掉前一次失败后尚未完成的 linked truncate orphan。 |
| 根据 `s_last_orphan` 和目标 `i_dtime` 快速判断 | classic 链尾的 `i_dtime` 为零，会把非头尾节点误判成非成员。 |
| 把索引放在 StarryOS/ax-fs 适配层 | 跨层复制 rsext4 持久语义，挂载恢复和事务回滚无法成为同一所有者。 |
| 在 `Ext4FileSystem` 中维护私有 `BTreeSet<InodeNumber>` | 采用。查询 O(log n)、无 I/O；代价是必须明确派生、更新和回滚规则。 |

`BTreeSet` 已由 orphan 校验使用，不增加依赖；它保持确定性顺序和 `no_std + alloc` 兼容。哈希集合虽然平均 O(1)，但需要新的哈希依赖/策略，对预期 orphan 数量没有必要。

## 所有权与状态转换

持久链仍是崩溃恢复的唯一事实源。`Ext4FileSystem` 私有 runtime 索引只是当前挂载的完整派生视图：

1. 构造和 journal replay 重载时索引为空。
2. journal replay 完成后，挂载只遍历持久链一次，同时验证并构造新索引；构造完全成功后才替换旧索引。
3. 可写挂载使用该索引完成 orphan recovery，恢复结束时持久链和索引都为空；只读挂载保留索引但不修改。
4. `add_orphan` 先完成 inode/superblock 内存修改，再把 inode 插入索引；`remove_orphan` 先用索引快速拒绝非成员，按持久链定位 predecessor，完成链修改后再从索引移除。
5. `MetadataTransactionSnapshot` 包含索引。journal handle 或上层操作失败时，超级块、inode cache 和索引一起恢复，不允许部分成功。

```text
mount/replay -> validate disk chain -> publish runtime index
                                      |
ordinary contains --------------------+-> memory-only lookup
                                      |
transaction add/remove -> disk image + runtime index
                                      |
failure ------------------------------+-> snapshot restores both
```

运行时索引不公开，不由调用方手工同步；只有 orphan 模块能改变其成员。磁盘链扫描仍保留在挂载校验、恢复和实际删除 predecessor 定位中。

## 错误、兼容与回滚

挂载构造临时集合时仍检查范围、allocated 状态、空 inode 与循环；失败不发布不完整索引。运行中索引声明成员但磁盘定位不到时返回明确损坏错误，不静默删除索引。索引插入重复或删除缺失均按现有幂等/NotFound 语义处理。

没有磁盘格式、feature bit 或用户 API 变化，旧卷和旧内核继续兼容。回滚代码不需要数据迁移；应在同步和干净卸载后切换版本。异常复位仍必须由 Linux/initramfs fsck 闭环处理，不能依赖 runtime 索引修复磁盘损坏。

## 验证计划

先加入会在旧实现失败的确定性回归：建立一个 live orphan，清除 inode cache，仅预热另一个普通 inode，然后直接查询该普通 inode 的 orphan 成员关系；设备读取计数必须保持为零。该测试把被优化的最小边界单独隔离出来，证明成员查询不再触碰持久链，而不是只比较代码结构。

其余验证覆盖：

- 现有 classic orphan 双节点恢复、非头节点删除、环检测和失败回滚测试；
- rsext4 全部库与集成测试；
- 差异检查、rustfmt 和 `cargo xtask clippy --package rsext4` 三项静态检查先于测试；
- attended 板端完整 246-unit 编译，比较 `orphan_contains` CPU/块读取样本、ext4 锁等待、总时间和失败日志。

板端验证前不声称实际加速比例；无人值守时不切换启动项、不复位板卡，也不运行可能需要人工重新上电的测试。
