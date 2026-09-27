# 块请求分段延迟诊断

## 问题与范围

inode 写回安全快照窗口 B 的 CPU 活跃仅 31.17%，ext4 占 mutex 等待 70.69%，
同步持锁 180.81 秒。块读/写/flush 现有埋点包含 DMA 准备、软件队列、设备、
IRQ、worker 与调用者调度，不能据它们认定磁盘硬件慢。

本轮新增诊断，不预先修改调度、缺页、IRQ、DMA 或 journal 语义。目标是用
同一 QEMU 编译窗口识别应优化哪段等待。没有性能提升声明，不提交/合入。
诊断跨 ax-sync、ax-fs-ng 与 Starry 内核，按共享 API/跨任务状态的高风险变更
保留独立设计，合入前需要对应维护者审核。

## 设计与备选

复用已有 `ProfileEvent`、固定容量聚合表和折叠栈输出。现有 begin/end token
要求同一 task 结束，不能把 token 交给完成 worker。因此新增记录已测量时间
区间的 hook，在原调用者取到结果并释放 completion 锁后采栈和聚合。

每个真实 CompletionCell 的既有 IRQ-aware 锁保护诊断时间戳，随一次性请求
创建、驱动接受、完成发布、调用者取回推进；不新增锁、轮询、DMA 引用或请求
所有权，不允许诊断失败改变 I/O 返回值。仅 `profile` 特性编译诊断状态。

| 区间 | 边界 | 限制 |
| --- | --- | --- |
| block-dispatch | completion 创建 → 驱动接受该请求后的 runtime 登记 | 包含软件排队和批提交，不是纯 runnable 时间 |
| block-completion | runtime 登记 → completion 结果发布 | 包含设备、IRQ 与 worker 调度，不是纯硬件服务时间 |
| block-resume | 结果发布 → 调用者取回 | 包含唤醒、运行队列及组内其它请求等待，不是纯调度器时间 |

三段按同一请求拆分，不与外层 block-read/write/flush 相加。拒绝或取消的请求
没有有效驱动接受边界时不伪造三段。只记录整个区间位于当前采样 phase 内的
请求；跨 stop/reset 的记录不归入新窗口。按原调用者 CPU/栈归属，不声称等同
硬件队列 IRQ CPU。保持旧 1–9 事件编号与格式，新增 10–12。

备选：直接迁移缺页/换调度器没有新因果证据；只有 syscall off-CPU 不能拆分
块 I/O；跨任务复用现有 token 违反 owner 校验；新增另一套无栈计数器重复已有
聚合边界；硬中断直接采栈增加风险且不能覆盖软件排队。本设计选择在 task
context 中延迟记录，明确中段仍包含 IRQ/worker 等待。

内部依据：当前 `completion.rs` 一次性发送/接收，`hctx/submission.rs` 驱动
接受前缀与 pending 登记，`profiler.rs` task-owned token 和固定聚合表。
参照当前 MOSS 的内核分阶段等待诊断方法，不移植其驱动或调度算法。本修改不
涉及外部 syscall ABI、磁盘格式或硬件寄存器，因此不需要新增外部协议实现。

## 验证计划

编译真实 completion/提交路径，覆盖三段边界、拒绝无假记录、重复完成防护、
接收者丢弃、组完成以及 feature-off；通过 renderer 行为测试检查三个事件的
纳秒单位和独立 SVG。运行 ax-fs-ng 全套及相关 clippy、fmt、Starry 构建，
随后从同一冻结盘启动独立 600 秒诊断窗口，保存 ELF、原始记录、全量哈希、
fsck 和 dropped/skipped。此窗口含新增观测开销，不作纯优化收益样本。
