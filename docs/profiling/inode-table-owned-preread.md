# inode-table 锁外预读收尾设计

本项属于 ext4 后台回写整体重构，尚未运行测试。最终三轮静态检查门槛不变。

## 问题和选择

`stage_sync_metadata -> InodeCache::flush_all` 需要整块读改写以保留同块其他
inode 和未知扩展字节。当前冷块读取仍持 adapter 的 ext4 全局锁；旧锁外预读
回归不能因迁移到新事务接口而丢失。

参考固定 Linux commit
[`__ext4_get_inode_loc`](https://github.com/torvalds/linux/blob/980ab36ae5972c83f683b939e50c469c4947229e/fs/ext4/inode.c#L4849-L5028)
与 [`ext4_reserve_inode_write`](https://github.com/torvalds/linux/blob/980ab36ae5972c83f683b939e50c469c4947229e/fs/ext4/inode.c#L6396-L6480)：
Linux 先取得并保留 inode-table buffer，再取得 journal 写权限。本项目不照搬
buffer_head 引用模型；复用现有 owned I/O 与 journal 最新块解析边界。

保持现状会保留已定位的锁内读取；扩展共享块缓存为跨 crate 长期 pin 能避免副本，
但需要新的驱动能力和预算等待契约。选择有界、一次性的 inode-table 读结果，
不增加持久缓存或第二份脏数据真相源。最多读取当前 inode cache 所涉及的块，
内存代价由现有 inode cache 上限和文件系统块大小限定。

## 所有权和一致性

1. commit gate 下、ext4 锁内收集当前脏 inode 的冷 table block；已存在 journal
   最新镜像的块无需预读。准备阶段不修改 dirty，不产生设备 I/O。
2. owned endpoint 在 ext4 锁外读取；普通 inode 操作仍可进入。读失败直接返回，
   原 dirty cache 不变。Unsupported 保持明确同步适配，不掩盖 I/O/内存错误。
3. 回锁后验证同一个后台会话、同一个已签发提交代次、无在途提交/关闭状态。
   代次使用已有 checked-add 提交序列，不允许饱和或回绕。退出后台模式即销毁
   会话身份，重新启用不能接受旧结果。
4. 只合并当前仍脏且位于预读块中的 inode；不使用准备阶段的 inode 内容。
   journal 当前最新整块镜像优先于预读 home 内容，以保留并发提交到 running
   transaction 的同块其他 inode。成功入 journal 后才清 dirty。
5. 在同一个 ext4 guard 内继续原 prepare_sync。期间新脏的其他块保留原装配路径；
   不无限等待写入者停止，也不把后到更新覆盖为旧快照。空间压力按原有限前缀
   提交/checkpoint 后重新准备，不能跨代复用旧 home 内容。

后台会话令牌只表达同步模式身份，不替代 mount、journal ticket 或 inode dirty。
原始设备能力不变，不新增 OS 依赖、磁盘格式、syscall 或配置项。

## 验证安排

先接入生产路径回归源码，再完成实现和编译检查；按用户门槛暂不执行失败/通过
实验。之后覆盖：锁外实际读、另一 inode 可推进、同块新 journal 内容优先、
checkpoint/模式切换/外部挂载拒绝旧结果、序列耗尽、读错保留 dirty、同步回退。
未执行之前不能声称性能提升或迁移回归通过。
