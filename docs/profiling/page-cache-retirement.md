# 后台回写的页缓存退役边界

2026-09-10，实现收尾设计；未运行测试，最终三轮静态门槛未开始。

修复前 LRU 忽略 listener 的 false，populate 把已移出的页留给成功返回后的回调，
出错则提前释放；truncate 同样忽略失效失败。回调只处理当前地址空间，不能证明
其他进程没有 PTE。内核 `on_evict` 还缺少跨 CPU TLB 完成边界。
这些是本轮页缓存与后台回写的所有权阻塞项，不扩大成整个地址空间/页表架构重写。

参考固定 Linux commit 的
[`truncate_cleanup_folio`](https://github.com/torvalds/linux/blob/980ab36ae5972c83f683b939e50c469c4947229e/mm/truncate.c#L147-L185)、
[`truncate_pagecache` / `truncate_setsize`](https://github.com/torvalds/linux/blob/980ab36ae5972c83f683b939e50c469c4947229e/mm/truncate.c#L768-L825)
和 [TLB API](https://docs.kernel.org/core-api/cachetlb.html)。不移植 Linux rmap，
但必须保留“映射失效先于 frame 释放”和“truncate 与普通写串行化”。

## 选定边界

- LRU 索引不再隐式释放容量外的页。每次 miss 有界扫描候选，只能移除全部
  listener 明确成功的页；繁忙映射仍在 canonical cache，不能返回为 detached victim。
  512 页至 256 MiB 是普通可回收页的保留目标，不是可以强制释放在用映射的硬上限。
  无法失效的页临时计入驻留内存并可超过目标，仍受物理页分配器限制；后续全局
  reclaim 继续尝试失效。这是保守的资源代价，必须在最终 8c8g 测试观察峰值。
- 普通 write/append/truncate 共用 mutation owner；页错误不取得此 owner。
  truncate 在发布新长度、取走 EOF 外页面、完成映射失效期间保留 mutation owner，
  防止普通写并发重新扩展；阻塞失效不得持 cached-I/O/page/listener 锁。
- EOF 外页交给独立退役列表，清除 dirty 并永不写回。失效失败返回错误但保留
  frame；后续 sync/resize/write 重试失效，不能把旧页装回新长度的缓存索引。
- listener 分离非阻塞淘汰与可阻塞截断失效，注册、注销和 snapshot 在短列表锁内，
  回调在列表锁外。内核失效校验真实物理页，避免旧批次撤销后来建立的新页。
- 内核只有页表操作和跨 CPU TLB 完成均成功才能返回 true。NotMapped/已只读
  不能掩盖前次 shootdown 失败，重试仍执行同步失效。
- 失效失败地址由 listener 独立持有，键为原文件页号并保存原虚拟地址。即使
  backend 被拆分、移除或销毁，也必须重试这些地址，不能以 Weak 升级失败冒充
  完成。只有失败路径需要保存地址；成功路径没有地址 Vec 分配。失败 backend
  销毁后不注销其 listener，重试成功后它成为空回调，随缓存整体销毁而释放；
  这是错误路径的明确小型元数据保留代价，不是新的强 backend/cache 引用环。

锁顺序：普通写 mutation → cached-I/O → page index；truncate mutation → writeback，
之后分别进入地址空间失效阶段与 cached-I/O 阶段，不同时持后两者。后台 writeback
只持 writeback owner 进入阻塞失效，不持 mutation owner，普通页错误不等待 writeback。

## 验证

先写确定性生产路径用例，再实现，只做编译期检查。门槛放行后验证：繁忙 LRU
保留同一 frame/dirty 内容、候选失败后仍能加载新页、截断失败保留退役 frame 且
不写回 EOF 外数据、重试释放、实际 mmap 的 PTE/TLB/RSS 顺序和失败重试。
实际内核回归还要求：注入 shootdown 失败后移除并销毁映射，再由 truncate
 完成原地址失效；pending owner 必须归零。所有新用例已编译，未执行。
任何新增读写锁顺序、资源上限变化或缺失用例均不能仅以 Clippy 通过认定安全。
