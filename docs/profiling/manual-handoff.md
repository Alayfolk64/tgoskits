# StarryOS 编译负载 Profiling 交接

这份文档用于继续之前的本地 profiling 实验，由使用者亲自执行命令，ChatGPT 根据实际输出逐步解释和指导。仓库保存的是实验源码快照，上传不代表重新完成了构建、运行或性能验收。

## 1. 实验状态

本分支保存了 `tgoskits-orangepi5plus-selfbuild` 中的历史提交和 28 个未提交源码文件。原目录保持原样，上传副本位于 `/Users/wuxun/os/tgoskits-profiling`，分支为 `profiling/manual-20260907`。

### 1.1 已保存的源码

原始基点为 `dd466685da7c03c45fc26ee16a788c4b80dad0aa`，对应 `feat/orangepi5plus-starry-selfbuild`。当前快照包含 StarryOS 内核采样器、AArch64 中断现场入口、调度等待记录、mutex、page cache、ext4 和块读写的延迟埋点。关键代码位于 [profiler.rs](../../os/StarryOS/kernel/src/profiler.rs)、[profile.rs](../../os/arceos/modules/axsync/src/profile.rs) 和 [guest-tg-xtask-profile.sh](../../apps/starry/macos-selfbuild/guest-tg-xtask-profile.sh)。

这次上传逐文件校验了上述 28 个文件的 SHA-256，确保与原目录一致。未重写 Rust 实现，也未把历史实验分支更新为最新 `dev`。

### 1.2 需要重新准备的产物

原目录的 `target/` 已不存在，历史 `target/starry-macos-selfbuild/profiles/` 中的原始采样、内核 ELF、符号化输出和火焰图目前不可用。这次上传不包含 rootfs、工具链缓存或已编译内核；必须先检查现有本地镜像，再决定恢复方式，不能假定克隆后可以立即启动。

后续曾在 StarryOS Debian、8 vCPU、8 GiB、QEMU `-snapshot` 下测试 `arceos-helloworld` 冷编译，并发现、修复 Unix seqpacket EOF 丢唤醒问题，相关上游记录为 [PR #2269](https://github.com/rcore-os/tgoskits/pull/2269)。那一轮使用的修复分支不是本快照，不能把它的成功结果当作本分支验证结果。本机保留的相关报告在 `tgoskits/tmp/ax-net-seqpacket-pr-20260903/正确性修复和性能对比报告.md`。

## 2. 现有采样机制

本快照使用内核内建的定时器栈采样和延迟埋点。[profiler::init()](../../os/StarryOS/kernel/src/profiler.rs) 注册中断现场、定时器、同步等待和任务阻塞回调，`guest-profile` feature 控制相关功能的编译。

### 2.1 CPU 采样

`profile_irq_context()` 保存被中断现场，`profile_timer_sample()` 按 `SAMPLE_PERIOD_NS = 100_000_000` 限制采样频率，即每 CPU 约 10 Hz。`StackTrace` 最多保存 20 层内核栈，用户态以 `USER_SPACE_FRAME` 标记；这不是 rustc/LLVM 用户函数调用栈，也不提供 IPC、cache miss 等 PMU 指标。

内核构建配置通过 `BACKTRACE = "y"` 启用所需回溯支持，见 [build-aarch64-unknown-none-softfloat.toml](../../apps/starry/macos-selfbuild/build-aarch64-unknown-none-softfloat.toml)。分析必须保留同一次构建的 ELF 和原始采样，不能用另一版内核符号解释地址。

### 2.2 等待与延迟

[ProfileEvent](../../os/arceos/modules/axsync/src/profile.rs) 区分 mutex 等待、ext4 操作、page-cache 操作、块读写、off-CPU 和 ext4 全局锁等待。`profile_wait_begin()` 与 `profile_wait_end()` 记录区间，`ProfileScope` 在离开作用域时结束记录。

原始输出包含 `count`、`total_ns` 和 `max_ns`，部分事件按比例抽样。off-CPU 时间可能在多个任务之间重叠，ext4 等事件也可能包含嵌套 I/O；不能把所有延迟直接相加后当作墙钟耗时。下一次分析还应检查丢弃计数、采样器自身开销，以及关中断导致样本集中到恢复点的偏差。

### 2.3 控制与导出

[StarryProfileFile](../../os/StarryOS/kernel/src/pseudofs/proc.rs) 在 `/proc/starry_profile` 提供控制写入与快照读取。`profiler::command()` 接受 `reset`、`start` 或 `phase=prebuild`、`stop`；`profiler::snapshot()` 输出 `STARRY_PROFILE_V1` 头、`STARRY_CPU` 和 `STARRY_WAIT` 记录。

现有 guest runner 会验证启动后出现 `enabled=true phase=1`，停止后保存 `kernel-profile.raw`、`profile.meta`、`progress.log`、`run.log` 和 `SHA256SUMS`。在采用快照磁盘的新实验中，应先确定如何把这些结果传回宿主机，再关闭 QEMU，否则临时写入可能随退出丢失。

## 3. 下一轮手动实验

当前目标是理解 StarryOS 内编译 `arceos-helloworld` 时的内核开销，继续采用 macOS 原生 QEMU/HVF、8 vCPU、8 GiB、Debian 来宾和独立冷构建。先恢复可执行基线，再开启短窗口采样，之后才讨论优化。

### 3.1 先核对旧入口

[guest-tg-xtask-profile.sh](../../apps/starry/macos-selfbuild/guest-tg-xtask-profile.sh) 当前实际负载仍是 `cargo build -p tg-xtask`，窗口为 300 秒；`Compiling` 行计数表示开始编译的单元数，不是完成百分比。它尚未改为 `arceos-helloworld`，不要直接启动后把数据误认成新目标。

[qemu-aarch64-profile.toml](../../apps/starry/macos-selfbuild/qemu-aarch64-profile.toml) 当前设置 `rootfs_write_policy = "persist"`，旧 [README_CN.md](../../apps/starry/macos-selfbuild/README_CN.md) 也依赖持久化写入。后续实验要求 QEMU 使用 `-snapshot`，需要重新确定兼容的启动与采样导出流程，不能只向旧配置追加参数。旧 runner 还在来宾 `/tmp` 展开源码；重新投入使用前应按当前工作区约束确定正式源码位置。

### 3.2 逐步执行顺序

先核对宿主机架构、QEMU/HVF、工具链、源码分支、Debian rootfs 和可用磁盘。确认来宾中有可工作的 `tg-xtask` 后，再核对构建命令及其工作目录；后续目标命令为 `cargo xtask arceos build --package arceos-helloworld --arch aarch64`。命令是否能在恢复后的来宾直接执行，仍需现场验证。

每次只推进一个阶段：环境检查、基线启动、单次冷编译、采样能力检查、短窗口采集、结果导出、符号化、热点解释。对照实验固定源码、镜像、工具链和计时口径；默认不绑核、不设置 Cargo/rustc 并行度限制，并分别保存编译原始日志与 profiling 数据。

## 4. ChatGPT 协作约定

打开指定分支后，先阅读本文件，再通过其中的链接检查真实源码。给出命令时明确在 macOS 宿主机还是 StarryOS 来宾执行；用户自己操作，ChatGPT 等待输出后再判断下一步。

### 4.1 可复制的对话开场

以下文字用于新建对话，明确实验目标和教学方式。连接 GitHub 后应指定完整分支路径，避免读取仓库默认 `dev`。

> 请读取 Alayfolk64/tgoskits 的 profiling/manual-20260907 分支，先看 docs/profiling/manual-handoff.md。我想亲手在 macOS 原生 QEMU/HVF 上，对 StarryOS Debian 内编译 arceos-helloworld 做 profiling。请每次只给一个小步骤和最简单的命令，解释执行位置、参数、目的和预期输出，等我贴回结果再继续。先检查环境，不要直接开始长时间编译或采样。请区分旧 tg-xtask profiling 原型与新的 arceos-helloworld 目标，不要假定旧镜像和采样文件仍存在。QEMU 使用 snapshot，提前安排结果导出；不绑核、不限制编译线程；先用真实数据定位热点，再讨论改代码。

### 4.2 本次交付边界

本次只保存与上传现有实验源码、补充本交接文档，并执行文件一致性、差异、文档引用和现有 shell 契约检查。没有启动 QEMU、操作板卡或重新运行 Rust 构建、clippy、标准库测试；这些历史代码的完整正确性与性能仍需在接下来的手动实验中验证。
