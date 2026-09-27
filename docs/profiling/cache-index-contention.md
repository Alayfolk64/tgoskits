# 缓存锁竞争不等于缺页

## 已确认的路径与候选

`inode-writeback-read-cache-window-600` 中 CachedFile::read_at 的文件级
io_lock 累计等待 649567913424 ns，占全部 mutex 等待 28.6076%。此调用点
混合真实 miss、更新中和缓存索引 try_lock 失败，不能把全部等待都归因于后者。

现有快路径遇到 page_cache 短暂占用就返回 false，调用者随即阻塞在 io_lock。
如果 io_lock 的持有者正在读另一个缺失页，原本已命中的页面也会等整次 I/O。
本候选改为先取得缓存索引锁，再检查 updating 和目标页。只有确实缺失或
内容更新中才退回原 io_lock 路径。缓存索引锁在所有 fallback 之前释放；
写入、截断的更新保护和用户缓冲区锁外复制均保留，不新增锁嵌套或页所有权。

Linux `980ab36ae5972c83f683b939e50c469c4947229e` 的
[filemap_get_read_batch](https://github.com/torvalds/linux/blob/980ab36ae5972c83f683b939e50c469c4947229e/mm/filemap.c#L2464)
先查询页缓存，
[filemap_get_pages](https://github.com/torvalds/linux/blob/980ab36ae5972c83f683b939e50c469c4947229e/mm/filemap.c#L2676)
在目标页未就绪时处理 I/O 等待。这里仅借鉴“先确认目标页状态”，不声称
实现了 Linux 的 RCU/XArray/folio 无锁读。

比扩大缓存更直接，因为本问题可发生于已有页；自旋重试会消耗 CPU 且不能
保证进展；增加每页锁涉及更大结构改造。选择现有 sleepable cache lock，
代价是命中查询本身可能睡眠。缓存填充/驱逐的持锁路径仍可能做 I/O，本候选
不声称消除了所有页缓存锁内 I/O，也不改变 reclaim 的 try_lock。

## 执行与限制

按用户最新要求不增加或运行测试套件；保留源码检查、构建与 600 秒 QEMU
采样。与“跳过未变化 GDT 写回”合成下一轮低成本候选，记录两项改动，
不把组合效果分别归因于单项。无数据时不声称提速，未跑回归不记作通过，
合入前仍需并发行为验证。
