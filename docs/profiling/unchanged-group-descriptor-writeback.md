# 跳过未变化的 group descriptor 写回

## 目标与边界

`inode-writeback-read-cache-window-600` 中，`sync_to_disk` 累计持锁
180805838160 ns；其中 group descriptor 的块写为 9163481088 ns，关联
flush 为 3246909152 ns。当前同步每次重写所有 GDT 块，即使描述符没有变化。
这些是累计等待区间，不等于可直接扣除的编译墙钟时间。

本候选在现有同步循环内比较编码后的描述符字节，仅变化的块进入 journal。
保留 checksum 更新、pending journal 优先读取、设备 flush 和错误传播。
未发布的 dirty device buffer 即使字节相同也必须提交，不能把脏缓存误当成
已经持久化的内容。32/64 字节编码及未编码的保留字节保持原样。

Linux `980ab36ae5972c83f683b939e50c469c4947229e` 的
[JBD2 checkpoint](https://github.com/torvalds/linux/blob/980ab36ae5972c83f683b939e50c469c4947229e/fs/jbd2/checkpoint.c#L197)
按 buffer dirty 状态决定是否排队写回。本项目的 group_descs 仍可被直接
修改，因此暂不增加一套需要所有分配/释放调用者维护的 dirty bitmap，而是
在已有序列化边界比较字节。这不是移植 Linux 的完整事务模型。

保留现状会重复写未变块；增加持久化副本会引入提交失败后的失效规则；本候选
不新增跨调用状态、锁、后台线程、公共 API、unsafe 或磁盘格式。代价是每个
描述符最多 64 字节的比较，不消除原有 GDT 读取。

## 当前执行策略

按用户最新要求暂停测试套件和新增测试，以内核构建、源码审查和固定 600 秒
QEMU kernel profiling 判断方向。未执行的回归不记为通过，性能数据出来前
不声称提速；本地实验也不代表已经满足合入验证要求。根文件系统和 tg-xtask
使用同一冻结输入；本候选与 inode 表锁外预读分别保留内核和采样产物。
