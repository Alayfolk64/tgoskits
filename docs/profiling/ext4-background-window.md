# ext4 后台回写重构：首个 600 秒窗口

## 结论和证据边界

2026-09-10 21:41:58（Asia/Shanghai）完成首个重构后窗口。三轮静态检查
及 ax-io 修复后的补充三轮检查均在测试启动前完成；生产源码自 21:10 未变化。
QEMU 正常退出 0，guest 报告 `elapsed=607 rc=124 build_completed=false`。
窗口完成不等于整个编译完成，也不把启动的编译单元数转换为构建加速比。

| 指标 | 重构前 clean-cache-preread | 后台回写重构后 |
| --- | ---: | ---: |
| 窗口结束时间口径 | 608 s | 607 s |
| 日志中启动的编译单元 / 不同 crate | 42 / 39 | 52 / 49 |
| 活跃 CPU 样本 / 全部样本 | 15,635 / 47,412 | 18,337 / 47,173 |
| CPU 活跃占比 | 32.9769% | 38.8718% |
| ext4 累计持锁时间 | 389.113634 s | 249.764290 s |
| ext4 累计锁等待（多任务相加） | 1,643.945411 s | 943.763106 s |
| 同步块读累计时间 | 303.562250 s | 510.837467 s |
| 同步块写累计时间 | 229.895627 s | 89.722019 s |

持锁减少 35.8120%，等待减少 42.5916%，CPU 活跃占比增加 5.8949 个百分点。
这是各一次窗口的观测，不是重复性结果。两窗口推进的工作不同；读 I/O 时间
增加，重构后 flush 累计 145.982645 s，不能只展示下降的指标。
最忙 CPU 2 活跃 88.7812%，其余 CPU 为 25.0717%–41.8553%，并行不均衡仍在。

## 新的最大热点

ext4 持锁事件按实际调用者聚合：

| 调用路径 | 累计持锁 | 占全部 ext4 持锁 |
| --- | ---: | ---: |
| Inode::write_locked → with_writeback_progress | 104.265365 s | 41.7455% |
| Inode::lookup_locked | 87.756885 s | 35.1359% |
| Ext4Filesystem::read_inode | 25.678432 s | 10.2811% |
| read_dir_with_core_reader | 14.826768 s | 5.9363% |
| sync_with_commit_gate | 0.367927 s | 0.1473% |

旧同步路径 `sync_to_disk` 累计持锁 237.015096 s；重构后的目标事务等待和
owned commit 执行已移出 ext4 状态锁。但函数边界已变，不能仅用重命名前后的
单一函数值声称端到端同步耗时下降相同比例。

最大单条持锁栈为 76.899106 s（全部持锁的 30.7887%）：

```text
rustc → sys_write → FileBackend::write_at → CachedFile::write_at_locked
      → page_or_insert → Inode::write_at → write_locked → Ext4Guard::acquire
```

`page_or_insert` 会在缓存达到容量目标时同步写回被淘汰的脏页。
当前缓存目标只在打开文件时按长度计算；新建输出从 0 增长仍固定为
512 页 / 2 MiB。这个源码事实与火焰图吻合，但尚无专用计数证明该策略
解释了全部 76.90 s，不能把假设的优化收益写成实测结果。
后台 worker 的同类写入栈另占 24.407551 s；仅修正增长策略不会消除所有
持锁数据写入。目录查找仍是第二大持锁热点，后续按新采样继续处理。

## 可复核产物

目录：`target/profiling/arceos-helloworld/starry/ext4-background-window-600`。

- 原始串口：`qemu.log`；guest 原始记录：`artifacts/arceos-helloworld-profile/`。
- [持锁火焰图](../../target/profiling/arceos-helloworld/starry/ext4-background-window-600/rendered/ext4-lock-hold.svg)、[活跃 CPU 火焰图](../../target/profiling/arceos-helloworld/starry/ext4-background-window-600/rendered/cpu-active.svg)、[完整统计](../../target/profiling/arceos-helloworld/starry/ext4-background-window-600/rendered/summary.json)。
- 5 项导出内容 SHA-256 全通过；全部 dropped/skipped 计数为 0。
- QEMU 退出后结束盘 `e2fsck -fn` 退出 0，只检查、不修复。
- debugfs 导出时 8 次 `Operation not permitted` 为宿主非 root 无法恢复
  guest 所有权；保留原始警告，内容哈希完整通过，没有据此伪造所有权一致。

输入为冻结源码与既有 rootfs tg-xtask；未重新编译 guest 的 tg-xtask。
8c8g、aarch64、Cortex-A53、TCG multi、NVMe；nightly da80ed070，
两项 Cargo host 配置开关均为 true。基线与本轮各从只读 Starry 基盘的
独立副本启动，基盘 SHA-256：
`9968004595dec398480417e210d64f9ed509801d4325d4ab22f43951983f5ea3`。
ELF：`d0267e4a0624e59d28ebb452d15ceefd7933e75024058707faaa8446f008f6cd`；
BIN：`02907f18366549a2014e7b3d8859ee60a81021869f35de6e0e952988de867f54`。
源码快照：`tmp/ext4-final-static.ryo14j/source-profile-ready.tar.gz`，
SHA-256 `5bfb0dcf731ea86e672cf725df1a83e78ef4ad3000575bc88a0d8e52bd3494e4`。

Linux 新对照使用独立 `linux/ext4-background-comparison-600` 目录，
21:50 启动；硬件、工作负载、工具链和 Cargo host 配置一致，但基盘包含
bpftrace 等 Linux 特有准备，不声称两套 OS 根文件系统逐字节相同。
22:00 窗口完成并正常关机：604 秒、177 个启动单元、171 个不同 crate，
CPU active 71.8212%，完整构建未完成。11 项内容哈希、BPF 一致性检查和
结束盘只读 fsck 均通过，详见 [Linux 对照](linux-qemu-comparison.md)。
