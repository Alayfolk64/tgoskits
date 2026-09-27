# QEMU 全程编译 profiling

## 最新候选全程结果（2026-09-11 23:53）

并发文件缓存填充重构完成三轮静态、242 项 host 测试及两项 8c8g QEMU
系统用例之后，完整冷编译 **1684 秒，rc=0，181/175**。相对 1676 秒慢
0.48%，**不接受为性能加速**。六项 SHA、全部编译单元及最终 ELF 与
Linux 对照匹配；结束盘只读 fsck 为 0，采样 dropped/skipped 全为 0。
CPU 活跃采样占比降为 41.89111%，低占用仍未解决。

新图最大已解析内核 CPU 叶是 memcpy 2380/54750（4.34703%），其中
1832 样本属于私有文件缺页准备；清零 2354（4.29954%）紧随其后。
缓存索引等待依然存在，一部分原 I/O 串行等待转移到 ext4 inode I/O 锁。
不能只因 read_buf_at 名称下的等待减少，就声称锁竞争被消除。完整图和
精确 PC 证据见 [concurrent-cache-fill.md](concurrent-cache-fill.md)。

## 上一候选全程结果（2026-09-11 22:39）

匿名零页与首次写入所有权重构完成，三轮静态、54项内核回归、Starry/Linux
同二进制系统用例全部通过。8c8g完整冷编译 **1676秒，rc=0，181/175**；
比1694秒少18秒（1.06257%），但清零热点没有下降，**不能确认稳定加速**。
完整名单、最终ELF、六项原始SHA和只读fsck均通过；无采样丢弃。CPU活跃
43.73981%，低占用未解决。最大内核CPU叶仍为try_zero_page 2847/56874
（5.00580%），其次memcpy2563（4.50645%）。清零主要来自匿名写缺页，
memcpy主要来自私有文件缺页；缺页调用链合计8743（15.37258%）。这轮保留
为已验证候选，不把1%的单轮差异当作独立验收收益。完整证据见
[anonymous-zero-backing.md](anonymous-zero-backing.md)。

## 上一轮优化结果（2026-09-11 21:30）

新页发布与旧映射失效分离的整轮重构完成全程验证：**1694 秒，rc=0，181 个
编译单元 / 175 个不同 crate**，比此前验收的 1869 秒缩短 9.36330%，比
上一候选 1974 秒缩短 14.18440%。Linux 固定对照仍为 735 秒，耗时比 2.30476。
全部名单、最终程序与 Linux 一致，原始六项 SHA 和只读 fsck 均通过。
这是当时最快的完整实测配置，不代表单轮排除了宿主 I/O 波动。

页表递归映射叶样本从 2802 降至 59；最大内核叶热点变成新页清零
`try_zero_page`，2691/56992 活跃样本，4.72172%。其中 2649 个精确 PC
落在实际 DC ZVA 清零循环。CPU 活跃比例 43.35938%，**低占用尚未解决**；
ext4 查找仍是最大 ext4 持锁来源，不能把 CPU 叶函数排名冒充等待排名。
完整火焰图、三轮静态检查、90 项页表测试、53 项内核测试、哈希及限制见
[absent-page-publication.md](absent-page-publication.md)。

## 上一轮优化结果（2026-09-11 17:35）

批量页表撤销/失效后回收重构的完整冷构建成功：**1908 秒，rc=0，181 个编译单元**。
相对上一轮 2089 秒缩短 181 秒（单轮 8.66%），相对最初 2298 秒缩短 16.97%。
Linux 冻结对照仍为 735 秒。全部编译单元和最终程序与三份参考产物逐字节一致，
采样 6/6 SHA 通过、dropped/skipped 全为 0，结束盘只读 fsck 返回 0。
新的完整图最大内核 CPU 叶热点是 `try_zero_page`：3495/66408 活跃样本，
5.26%；缺页调用链 18.71%，ext4 调用链 2.65%。CPU 活跃比例仍为 44.89%，
不能声称低占用已经解决。下一轮转入缺页准备与提交边界的重构，尚无下一轮收益。
详细产物、指令级热点、等待统计及限制见
[batched-mapping-retirement.md](batched-mapping-retirement.md)。

## 上一轮优化结果（2026-09-11 16:18）

事件驱动抢占重构后的完整冷构建自然成功：**2089 秒，rc=0，181 个编译单元**。
相对下面保留的 2298 秒基线，单轮缩短 209 秒（9.09%）。tg-xtask、源码和
工具链复用；全部编译单元及最终程序与两边基线 cmp 一致。原始采样 6/6 校验
通过、无 dropped/skipped；关机后只读 fsck 退出 0，目录计数修复通过全程验收。
CPU 活跃采样比例 45.75%，当前最大内核叶热点为页表解除映射 4.93%；缺页
调用链包含 17.38%，ext4 调用链包含 2.51%。锁等待与 CPU 采样不得混为一谈。
完整证据和限制见 [event-driven-preemption.md](event-driven-preemption.md)。
下文原始基线及其 fsck 失败记录保留，不被新结果改写。

## 最终结果（2026-09-11 04:13）

两边均已自然完成完整编译，使用同一个修复版 QEMU、8c8g、相同冻结源码、
Rust 和根文件系统复用 tg-xtask，均为冷构建；没有按 600 秒或编译条数中止。

| 本轮 `full-build-rcsc` | StarryOS | Linux |
| --- | ---: | ---: |
| 全程测量耗时 | 2298 秒（38m18s） | 735 秒（12m15s） |
| 编译返回码 / 启动单元数 | 0 / 181 | 0 / 181 |
| 活跃 CPU 样本占比 | 43.34004% | 70.80841% |
| 原始产物 SHA256SUMS | 6/6 通过 | 12/12 通过 |
| 关机后只读 fsck | **退出 4，两个目录块计数错误** | 退出 0 |

最终 AArch64 ELF 经 `cmp` 确认逐字节一致。Starry 采样表 dropped/skipped
均为 0；Linux BPF stderr 为空，渲染器通过事件错误、空表和 8 CPU 计数核对。
单轮带 profiling 的耗时比为 **3.12653**，不是重复实验得到的稳定 OS 性能比；
两种内核采样实现及开销不同，Linux 未解析 kernel stack 的样本不能冒充用户栈。
编译与全程采样已完成，但 Starry 文件系统验收没有通过，详见下文。

全部证据位于 `target/profiling/arceos-helloworld/{starry,linux}/full-build-rcsc/`。
各目录包含完整 `qemu.log`、`artifacts/`、最终程序、`rendered/` 与 `fsck.log`，
结束盘保留为只读 `rootfs.img`。旧故障轮和旧模拟器 Linux 结果均独立保留。

## 本轮范围（2026-09-11）

按用户最新要求取消 600 秒编译窗口，运行到 `tg-xtask` 自然退出。Linux 和
StarryOS 分别使用独立冷构建盘，串行运行，均为 8 vCPU、8192 MiB、
Cortex-A53、TCG multi、NVMe。不使用 xtask perf，不运行实板。
编译条数仅作进度记录，不当成固定总数或完成百分比。

复用现有内核采样、BPF 和保存内核启动器，不新增内核探针或改变编译参数。
成功必须同时满足退出码 0、生成 `arceos-helloworld`、保存二进制校验值及
完整原始采样。全程结束后仍须检查采样表丢弃计数；没有编译超时不等于没有
采样容量限制。失败保留原始输出，不将 `124` 标成通过。

备选的固定时长窗口不能覆盖链接和编译尾部；简单延长超时仍可能截断，因此
选择直接等待命令结束。BPF 的 60 秒附着检查只限定启动，不限定编译。
`ostool 0.27.1` 的 `QemuConfig` 明确以 `timeout = 0` 禁用宿主超时。
本轮只改变实验入口和离线渲染，不改变 syscall、磁盘格式或内核锁协议。

## 冻结输入

- 工作负载源码归档：`7de46545454e3f5562d78a1a065bf2ed1a736cf807f4cbf05b30d98ab2725147`。
- 根文件系统复用 tg-xtask：`e6823ab3b6a6d944266fc31d5e54814f463466f88949849ba139b6c575f7e6f1`。
- Starry ELF：`e1a4d024f0b8e1389878aa2b663c2cdac191ac6cc2791be038a02eb7f7cf9424`。
- Starry BIN：`b2449148f0f1178bc2b7e7bbe90c4019dbc919c36bbc5d50f9f6b2e818e25523`。
- 内核源码冻结包：`tmp/ext4-final-static.ryo14j/source-negative-cache.tar.gz`。
  `tar -df` 确认当前内核输入未改变，本轮 apps 修改不在该内核输入包内。
- Linux 基础盘：`linux/qemu-probe-repaired/rootfs.img`；Starry 基础盘：
  `tmp/axbuild/rootfs/rootfs-profile-host-config-base.img`。基础盘只读不修改。

## 检查与已知失败

启动前完成三轮静态检查：脚本语法及退出状态路径；ShellCheck 和配置/事件
对应关系；fmt、diff、内核输入与产物身份。相关 clippy 11/11 通过。

首次渲染器回归 5/6 通过，失败原文为
`AssertionError: 'block-dispatch' not found`。测试构造诊断事件 10–12，但
渲染器名称表仅处理 1–9；补回离线事件名称，当前内核仍仅发布 1–9。
未削弱断言或增加内核采样开销。补齐后重新执行三轮静态核对，01:40:12
之前均通过；随后渲染器 6/6、完整 shell 契约通过。启动器 9/9 通过。
小样本渲染 fixture 的 `Stack count is low` 提示是 20/7 个合成样本造成，
不是实际实验的采样质量结论。

## 运行与验收

实验目录位于 `target/profiling/arceos-helloworld/{linux,starry}/full-build`。
旧 Starry 600 秒实验末态盘保留为
`tmp/axbuild/rootfs/rootfs-profile-negative-cache-window.img`。
新盘只替换正式 guest runner，不重建 tg-xtask 或改变冻结工作负载。

准备并校验独立盘后，依次运行：

```sh
python3 apps/starry/macos-selfbuild/run-linux-profile-qemu.py target/profiling/arceos-helloworld/linux/full-build
python3 apps/starry/macos-selfbuild/run-profile-qemu.py target/profiling/arceos-helloworld/starry/full-build
```

唯一位置参数为保存内核和输出日志的实验目录。两个命令必须串行，前一个
QEMU 完全退出后才能启动下一个。客体就绪后分别调用正式零参数脚本
`/bin/sh /opt/guest-linux-profile.sh` 与 `/bin/sh /opt/starry-macos-run.sh`。
启动器无宿主时限；测量期间不并行启动构建、QEMU 或大文件校验。

启动时结果尚未完成，不能从已有 600 秒窗口宣称全程最大 CPU 热点。全程结束后
已导出原始记录、校验 SHA256SUMS、检查最终程序及文件系统，再分别统计
CPU、持锁、等待与 I/O；这些指标不能相加当作墙钟时间。

Linux 于 2026-09-11 01:42:24 +08 开始全程编译（epoch 1789062144），
QEMU PID 827388，终端 session 65727。启动日志确认 8 CPU、8 GiB、
Rust 1.99.0-nightly `da80ed070`、系统默认亲和性 0–7。新盘注入前完整哈希
分别匹配 Linux `844cd703...` 与 Starry `99680045...` 基础盘；三个 guest
runner 注入后逐一导出校验，SHA 与正式源码相同，两盘只读 fsck 均退出 0。

### Linux 全程结果

全程自然结束，`rc=0`、`build_completed=true`，测量耗时 746 秒；Cargo
输出 `Finished release` 为 12m 17s，axbuild 内部构建阶段为 741.89 秒。
最终记录 181 个启动编译单元、175 个不同包名，不能以中途 177 条代替终点。
最终 AArch64 ELF 已从盘内导出，其 SHA-256 与客体保存值一致：
`2792c35275d91f57a38a847be87c4d2b333d7b88cd548bf9e06633288c7dba21`。
十二项 SHA256SUMS 全部通过，BPF stderr 为 0 字节，关机前 remount ro，
末态只读 fsck 退出 0。Rust 的 core/memchr future-incompatibility 警告仍
保留在完整 run.log 中，不是此次失败。

CPU 共 59736 样本，43071 活跃，16665 idle，活跃占 72.10225%；
`/proc/stat` 独立统计 busy 为 72.91976%。每个 CPU 均为 7467 个采样。
BPF 区间为 746789325536 ns，结束时 408 个尚未闭合的 off-CPU 区间，
它们不能计成已完成等待；没有等所有系统后台任务退出后再停采样。

已生成并查看 `rendered/cpu-active.svg`。活跃采样中 35220 个（81.77%）
没有 kernel stack，必须标成 `[no-kernel-stack]`，不能冒充可解析用户栈。
最大可解析内核叶子为 `tlb_flush`，2199 样本，占所有活跃样本 5.11%；
不能据此宣称已解析全部 CPU 热点，或外推 StarryOS 的全程热点。

运行期间宿主空闲空间从约 11 GiB 增加到 76 GiB，本轮没有执行磁盘清理。
变动来源未确认，因此宿主外部 I/O 干扰未排除，单轮耗时不应当作稳定 OS 比值。

### StarryOS 首轮启动记录（后续确认丢失唤醒）

Linux QEMU 完全退出且导出/检查完成后，于 01:55:26 启动 StarryOS，
QEMU PID 829876，终端 session 2453。客体已确认 `tg_xtask_reused=true`、
`source_reused=true`、8 CPU、亲和性 0–7，源码与 fingerprint 相同。
后续仍等完整编译，不设 600/1200 秒终止条件。

### 全程暴露的唤醒停滞（02:57 现场）

Starry 在 2104 秒已记录 178 个启动单元，随后到 3361 秒仍未增长。
没有编译错误或自然退出。宿主 `/proc/<qemu-pid>/mem` 只读检查发现：

| 线程 | 状态 | on_cpu | 队列链接 | 未消费 wake_handoff | 保存的内核栈 |
| --- | --- | --- | --- | --- | --- |
| 1821 / opt cgu.15 | Ready | false | 空 | 指向自身 | resolve_page_fault → Mutex wait → blocked_resched |
| 2120 / opt cgu.01 | Ready | false | 空 | 指向自身 | sys_futex → prepare_user_memory → Mutex wait → blocked_resched |

8 个运行队列的 head 和 length 全为 0，锁均未持有。上述两个任务字段在
读前/读后保持相同，并在分开的多次读取中重复出现。其余 rustc/coordinator
线程阻塞；采样到的长期 off-CPU 栈以 sys_futex 为主，cargo 等在 sys_ppoll。
这是已经唤醒的任务没有入队，不是 ext4 持锁执行造成的尾部耗时。
`Ready` 并非“已经在队列内”的证明；本轮还核对了真实队列和 intrusive links。
这次编译尚未完成，不能把包含无限等待的累计图称为成功编译的全程热点。

只读诊断入口为：

```sh
sudo python3 apps/starry/macos-selfbuild/inspect-live-profile-tasks.py target/profiling/arceos-helloworld/starry/full-build
```

唯一位置参数为实验目录。脚本校验固定 ELF/BIN 哈希、PID 命令及 8 GiB
RAM 映射；布局来自该 ELF 的反汇编，不是稳定 Rust ABI。脚本不 attach
debugger、不停 CPU、不写客体内存；输出仅保存实际读取字段和栈，明确不具备
全局原子快照语义。首次正式记录为
`live-tasks-1789066646392158257.json`。脚本执行前通过语法、字段/地址/只读
边界审查、无执行编译及 diff 检查三轮。

### QEMU RCsc 指令翻译缺陷

当前宿主为 x86_64，QEMU 为 `10.2.1 (Debian 1:10.2.1+ds-1ubuntu3.1)`。
Arm 的 RCsc 要求先前 STLR 与后续 LDAR 有 StoreLoad 顺序；参见
[Arm 官方说明](https://developer.arm.com/community/arm-community-blogs/b/tools-software-ides-blog/posts/armv8-sequential-consistency)。
[2026-08-21 上游补丁](https://www.mail-archive.com/qemu-devel%40nongnu.org/msg1218202.html)
指出多线程 TCG 的 STLR 翻译只在写入前放屏障，缺少写入后屏障。
已核对 v10.2.1 源码 `2d3df8abca265c9bcc9e438d691d561592060998` 的
`target/arm/tcg/translate-a64.c:trans_STLR`，与报告缺口一致。

三轮静态核对后，独立的无磁盘 8c8g Cortex-A53/MTTCG 指令检查运行：

```sh
python3 apps/starry/macos-selfbuild/tests/check-qemu-rcsc.py
```

正式汇编只执行 STLR/LDAR 对并通过 semihosting 正常退出。检查实际翻译后
退出 1，原始断言为：
`AssertionError: RCsc StoreLoad barrier missing between guest STLR and LDAR`。
TCG 顺序为 `mb rel:all → qemu_st_i64 → qemu_ld_i64 → mb acq:all`，
两个内存访问之间没有屏障。完整 TCG 和生成的宿主机器码保存在
`tmp/qemu-rcsc-a8aqljhr/translation.log`。这是确定性的实际代码生成回归，
不是依靠多线程竞态概率触发；它证明模拟器缺陷存在，但消除本次编译停滞
仍须修复后在原始全程场景验证。

独立 QEMU 源码位于 `/home/wuxun/Projects/qemu-rcsc-profile`，不替换
系统 QEMU，不修改 Starry 的 SeqCst 协议或 ext4。此指令诊断不挂载测量盘，
也不计入性能比较。若后续使用修复后的 QEMU，Linux/Starry 必须使用同一个
新 QEMU；先前 Linux 746 秒仅保留为旧模拟器结果，不能混做 A/B。

### 修复版 QEMU 与冷盘重跑

03:09 之前修复版 QEMU 严格构建完成，`-Werror` 保持启用。只在三个
STLR 翻译入口的实际 store 后添加 RCsc 屏障；正式差异为
`apps/starry/macos-selfbuild/qemu-rcsc.patch`，没有修改 Starry 内核。
同一个确定性测试再次执行：系统版 RED
`tmp/qemu-rcsc-a7s9h1lr/translation.log`，修复版 GREEN
`tmp/qemu-rcsc-jb0rlazq/translation.log`。实际 TCG 为
`qemu_st_i64 → mb seq:all → qemu_ld_i64`。检查器最初误把打印格式写成
`mb sc:all`，造成修复版误报；核对 QEMU 打印器后改为 `seq:all`，随后
重新执行上述 RED/GREEN，没有删减屏障断言。

QEMU v10.2.1 的内置旧 libfdt 在 GCC 15 / 新 glibc 下因丢弃 const 构建
失败，完整输出保存在 QEMU 构建目录 `build.log`。安装匹配的系统
`libfdt-dev`、`libslirp-dev`，使用 `--enable-fdt=system` 解决，没有
关闭编译警告。修复版二进制 SHA-256：
`a1d9a6080e1f025038a81749baa9d9552ae89321ea163abbf704206034f0a6a4`。
构建目录的 `qemu-bundle/usr/local/share/qemu` 提供固件，启动器不需
改变固件或虚拟设备参数，也没有安装或替换系统 QEMU。

03:05 的只读现场再次确认两个丢失唤醒任务，保存完整聚合表和任务栈后，
03:10 左右按明确故障终止旧 QEMU PID 829876，而非按运行时长截止。
原始退出输出为：
`qemu-system-aarch64: terminating on signal 15 from pid 848886 ()`。
启动器退出 0 不代表 workload 完成；此轮没有成功编译，不能用作性能结果。
故障盘完整保留在 `starry/full-build/rootfs-lost-wake.img`。

新的 Starry 实验目录为 `starry/full-build-rcsc`，重用完全相同的冻结
ELF/BIN。启动前从只读基础盘重新复制，按“复制、等待整盘 SHA 完成、
注入正式 runner、导出 runner 核对、只读 fsck”的顺序准备。此前准备盘
的哈希与注入曾重叠，因此不将那次哈希作为准备证据；该盘保留为
`tmp/axbuild/rootfs/rootfs-rcsc-preflight.img`，没有运行过客体。
新全程不设编译截止，结束后再在同一个修复版 QEMU 上跑 Linux 冷构建。

### Starry 修复版 QEMU：完整编译结果与验收边界

`starry/full-build-rcsc` 自然完成：测量 2298 秒，`command_rc=0`，
`build_completed=true`，`tg_xtask_reused=true`。Cargo 的 release 阶段
37m45s，axbuild 内部阶段 2288.36s；181 个编译单元、175 个不同包名。
最终 ELF 与客体保存的 SHA 一致，且与旧 Linux 全程产物逐字节同哈希：
`2792c35275d91f57a38a847be87c4d2b333d7b88cd548bf9e06633288c7dba21`。
另行导出 tg-xtask 验证 SHA 为冻结的 e6823ab3...；六项 SHA256SUMS 全部
通过。本轮验证了修复版模拟器下原始编译可以完成，不意味着所有并发问题
均已排除，也不是旧版与新版的稳定性能加速比。

采样覆盖完整命令区间，`prebuild_ns=2297526898288`，停止状态为
`enabled=false phase=0`，dropped_cpu/wait/pending 与 skipped_cpu 均为 0。
共 178447 个 CPU 样本，活跃 77339，活跃占 **43.34004%**；8 个 CPU 均有
22065–22432 个样本。采样没有容量丢弃不等于中断采样完全无偏差，10 Hz
是目标频率，实际采样数仍受计时器及临界区延迟影响。

已生成并查看 `rendered/cpu-active.svg`、`ext4-lock-hold.svg`。按活跃
CPU 样本为分母，主要叶子为用户态未展开栈 48.44%、exit_preemption 8.28%、
unmap_range_recursive 4.41%、spin_release 4.28%、try_zero_page 3.74%、
map_range_recursive 3.67%。因此不能把“最大 CPU 叶子”说成 ext4；也不能
把抢占退出这一边界叶子直接当作完整因果解释。

对完整 cpu-active.folded 按“调用栈任一帧包含指定符号”汇总（每一类内部
每个样本只计一次）：handle_user_page_fault 为 13847 样本（17.90% 活跃），
ax_fs_ng::fs::ext4 为 2649（3.43%），sys_munmap 为 3399（4.39%），
sys_mmap 为 2639（3.41%）。这些类可重叠，且不是所有内核工作的互斥分类。
exit_preemption 的 6405 叶子样本分布在很多调用方：分配器 dealloc 852、
alloc 628、alloc_pages 270、dealloc_pages 114，in_irq_context 716，
AtomicContextSnapshot::capture 705 等。因此缺页/内存管理与细粒度锁的
抢占进出开销是全程数据支持的后续方向，但尚未做新的修改与 A/B 验证。

| 等待/持锁口径 | 全程累计 | 主要来源 |
| --- | ---: | --- |
| mutex 等待（采样加权） | 3586.702074 秒 | lookup、read_inode、set_len、COW prepare_frame |
| ext4 锁等待（采样加权） | 2085.214168 秒 | lookup 532.918 秒、read_inode 530.652 秒 |
| ext4 持锁（全量记录） | 726.173029 秒 | lookup 269.370 秒（37.09%）、write 184.160 秒、inode_info 97.720 秒 |
| 缺页调用链中的 mutex 等待（重叠子集） | 992.531998 秒 | 包含文件页准备和 COW 路径 |

等待时长跨线程可重叠，不能相加或直接换算成墙钟时间。CPU 图表明内存
映射/解除映射和抢占边界也值得分析；锁图则证明 ext4 lookup 仍是重要
串行化点。下一次优化必须区分这两类证据，不能只凭一张 CPU 图排除 I/O
等待，也不能凭锁累计值宣称所有低 CPU 利用率均由 ext4 导致。

**文件系统验收未通过。** 完整编译结果与采样已经得到，但关机后只读
`e2fsck -fn` 返回 4。完整原文保存在该实验的 `fsck.log`，关键错误为：

```text
Inode 74474, i_blocks is 40, should be 56.  Fix? no
Inode 74479, i_blocks is 56, should be 72.  Fix? no
********** WARNING: Filesystem still has errors **********
```

两个 inode 分别为 source/target/release/deps 和
source/target/aarch64-unknown-linux-musl/release/deps。debugfs 显示它们
各有两个外部 extent-tree 块，i_blocks 恰只包含 5/7 个目录数据块，漏掉
两个 4 KiB 元数据块（16 个 512-byte sectors）。结束盘完整保存为实验
目录 `rootfs.img`，没有原地修复，也不作为下一轮基础盘。

源码线索：`fs/rsext4/src/hashtree/mutation.rs:append_directory_block`
先依据旧 i_blocks 计算 updated_blocks，再调用 insert_extent，最后写回
updated_blocks。`extents_tree/insert.rs` 分裂树时通过
`add_inode_sectors_for_block` 更新同一 inode，因此最后的旧值写回会覆盖
这些增量。该执行顺序与两个现场差额吻合；新增确定性回归与修复尚未运行，
本轮不改正在比较的内核，也不把 fsck 失败隐藏在编译成功后面。

Linux 同模拟器的新全程在 `linux/full-build-rcsc`，03:59:29 启动，PID
853059 / session 16123，workload begin epoch 1789070419。独立盘整盘
SHA 与冻结 Linux 基础盘相同，注入正式脚本/BPF 前后只读 fsck 均通过。

### Linux 修复版 QEMU：完整对照结果

自然结束，测量 735 秒、rc=0、181 个编译单元 / 175 个不同包名。
Cargo release 阶段 12m06s，axbuild 731.27s。BPF 区间
735842703904 ns，399 个尚未闭合 off-CPU 区间仍单独报告，不计作已完成
等待。12 项 SHA256SUMS 全部通过，最终程序哈希匹配 Starry 并经 `cmp`
逐字节核对一致。只读重挂载后正常关机，末态 fsck 退出 0；BPF stderr
0 字节。两边相同的 Rust future-incompatibility 警告仍保留在 run.log，
不是构建失败。

CPU 58856 样本、活跃 41675（70.80841%），8 个 CPU 各 7357 样本。
/proc/stat 独立统计 busy 为 71.52973%。最大可解析叶子 `tlb_flush`
1922 样本（4.61188% 活跃）；`[no-kernel-stack]` 为 34067 样本
（81.74445% 活跃），不能将它转换成已确认的用户态执行比例。已生成并
查看本轮 `rendered/cpu-active.svg`；不要把旧 `full-build` 的 SVG 当作
这次修复版 QEMU 的结果。

对现有完整原始数据重新生成图，不需要重新编译：

```sh
python3 apps/starry/macos-selfbuild/render-kernel-profile.py target/profiling/arceos-helloworld/starry/full-build-rcsc
python3 apps/starry/macos-selfbuild/render-linux-profile.py target/profiling/arceos-helloworld/linux/full-build-rcsc
```

唯一参数为保存完整实验的目录，两条命令已实际执行成功。上文启动命令
中的实验目录属于已结束的历史运行，启动器会拒绝覆盖其日志；再次测量
必须从只读基础盘准备新的独立冷盘和新的实验目录，不能写入本轮结束盘。

### 后续私有缺页重构：1869 秒完整冷编译

详见 [私有缺页事务记录](private-fault-transactions.md)。最终三轮静态检查
和定向回归后，8c8g 相同 patched QEMU 全程自然完成，181 个编译单元、
rc=0、6/6 原始 SHA、完整 name/version 清单和最终 ELF 与冻结 Linux
对照一致，只读 fsck rc=0。1869 秒比上一轮 1908 秒减少 2.04403%，仍为
Linux 735 秒的 2.54286 倍，不能把单轮小幅改善视为已解决低 CPU 占用。
新全程 active 45.40550%，最大内核叶子 memcpy 4.81175%；其 82.66498%
来自私有文件缺页读取。下一轮沿缓存页 → scratch → 内核目标的重复复制
边界重构；ext4 仍占主要锁等待，不能只凭 CPU 图断言它不重要。

### 缓存读取候选：1974 秒，未通过加速验收

[缓存读取整轮报告](cached-read-destinations.md)：完整 181/175、rc=0，
原始 SHA、所有编译单元和最终 ELF 与 Linux/Starry 一致，只读 fsck rc=0。
1974s 比 1869s 慢 5.61798%，不是新的加速基线。memcpy 叶样本下降
3167→2445，但 active 降至 41.90989%，ext4 锁等待及块 I/O 时间上升；
单次观测不能确定 105s 回退的全部原因。当前 CPU 最大内核叶子转为
map_range_recursive 4.36252%，其中 2727/2802 样本来自缺页的新页面安装。
所有采样已完整导出并出图，继续从该实际调用边界研究重构。
