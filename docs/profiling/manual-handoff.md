# StarryOS `arceos-helloworld` 编译 Profiling

本文记录 StarryOS 在 QEMU 中执行 `tg-xtask arceos build` 的可复现实验流程。目标是用 StarryOS 内核自己的采样和延迟埋点定位编译瓶颈，再用固定工作负载做内核优化前后对照。这里不使用 `xtask perf`、`qperf` 或 QEMU profiling 插件。

当前有效口径与结果见 [full-build.md](full-build.md)，续接状态以本文最后的
日期章节为准。下面的 600 秒窗口、`rc=124` 通过规则和旧运行状态仅保留作
历史证据，已被“等待全程自然结束、必须 rc=0”的要求取代，不应照旧执行。

## 1. 固定实验口径

StarryOS 侧固定使用：

- QEMU AArch64 TCG multi-thread；
- `cortex-a53`、8 vCPU、8 GiB 内存；
- 独立的 16 GiB NVMe 根文件系统镜像；
- 默认 Cargo/rustc 并行度，不绑核，不设置额外线程上限；
- 从冷构建开始采样 600 秒，计时前删除工作区 `target/` 和 `tmp/axbuild/`；
- 完整命令为 `tg-xtask arceos build --package arceos-helloworld --arch aarch64`。

历史 Linux 记录来自原生 x86_64，与 StarryOS 的 QEMU TCG AArch64 不可作为
匹配性能对照。当前要求两侧仅在 QEMU 测量，复用冻结源码与 tg-xtask，并匹配
AArch64、TCG、CPU/内存与块设备。Linux 首次 QEMU 编译在 329 秒因静态
build-script 无法动态加载 libclang 失败，不是有效性能对照。host 配置修正
后已完成 603 秒验证窗口，活跃 70.12%、启动 177 单元、未完成构建；Starry
也已完成启用相同 host gate 的窗口。Linux 起始盘及采样工具开销仍有差异，
不能据此计算严格 OS 加速比，旧原生结果不计入本轮优化收益。

## 2. 根文件系统复用边界

[prebuild.sh](../../apps/starry/macos-selfbuild/prebuild.sh) 生成并保留 `target/starry-macos-selfbuild/arceos-helloworld-profile-source.tar`。同一轮 A/B 和 Linux 对照都使用该归档，避免工作区继续修改后悄悄改变被测源码。

根文件系统中的下列内容跨运行复用：

- `/opt/tgoskits-profile/source`：冻结源码展开目录；
- `/opt/tgoskits-profile/bin/tg-xtask`：宿主机交叉编译的静态 AArch64 tg-xtask；
- `/opt/tgoskits-profile/tg-xtask-target`：仅供 tg-xtask 自身更新时使用的 Cargo target；
- 源码归档 SHA-256 和 tg-xtask 源码指纹。

tg-xtask 指纹覆盖 `Cargo.toml`、`Cargo.lock`、`rust-toolchain.toml`、`xtask/` 和 `scripts/axbuild/`。只有指纹变化才重建 tg-xtask。每次被测负载开始前只清理源码目录内的工作负载 `target/` 与 `tmp/axbuild/`，所以计时不包含 tg-xtask 自身编译，也不会把上一次 `arceos-helloworld` 产物带入下一轮。

持久镜像路径由 [qemu-aarch64-profile.toml](../../apps/starry/macos-selfbuild/qemu-aarch64-profile.toml) 固定为：

```text
tmp/axbuild/rootfs/rootfs-aarch64-arceos-helloworld-profile.img
```

写入成功或失败标记前，guest runner 都会先执行 `sync`。非正常终止后必须先对这个精确镜像执行 ext4 检查或修复，不能在文件系统仍挂载或 QEMU 仍运行时读取结果。

## 3. StarryOS 内核直接 Profiling

[profiler.rs](../../os/StarryOS/kernel/src/profiler.rs) 在启用 `guest-profile` 时注册以下内核钩子：

- 每个 vCPU 约 10 Hz 的中断现场栈采样；
- mutex 等待；
- ext4 操作和 ext4 全局锁等待；
- page-cache 操作；
- 同步块读写；
- 任务 off-CPU 区间。

[StarryProfileFile](../../os/StarryOS/kernel/src/pseudofs/proc.rs) 通过 `/proc/starry_profile` 接受 `reset`、`phase=prebuild` 和 `stop`。guest runner 在工作负载计时前启动内核采样，命令结束后停止并导出 `STARRY_PROFILE_V1`、`STARRY_CPU` 和 `STARRY_WAIT` 记录。CPU 与等待记录保留 vCPU 编号，便于验证是否真正利用 SMP。

CPU 栈最多保留 20 层；用户态执行以 `[user-space]` 标记，因此这里观测的是 StarryOS 内核开销，不是 rustc/LLVM 的用户态函数火焰图。ext4、page-cache 和 off-CPU 事件按固定比例抽样并放大，且不同等待区间可以嵌套，不能把各类 `total_ns` 相加后直接解释成墙钟时间。

每次有效窗口至少保存以下文件；只有构建完成时才额外保存 `arceos-helloworld.sha256`：

```text
kernel-profile.raw
profile.meta
progress.log
run.log
source.meta
SHA256SUMS
```

还必须保存同一次构建的 StarryOS ELF。地址只能用对应 ELF 符号化；使用另一轮 ELF 会让内核栈结论失效。

实验目录内保存 `starryos.elf`，原始采样放在
`artifacts/arceos-helloworld-profile/kernel-profile.raw`。只传实验目录即可生成
`rendered/` 下的折叠栈与 `summary.json`：

```bash
apps/starry/macos-selfbuild/render-kernel-profile.py <run-directory>
```

唯一参数是保存本轮内核和采样的实验目录。脚本会检查输入文件；如果仓库同级的
`FlameGraph/flamegraph.pl` 已安装，还会生成 SVG，否则明确报告未出图。
`cpu.svg` 保留 idle，用于观察闲置比例；`cpu-active.svg` 只显示非 idle 任务，
用于查看真正执行中的热点。`summary.json` 同时保留总样本、活跃/idle 样本、
活跃占比和非 idle 叶子热点，后者百分比以活跃样本为分母，不能与整机占比混用。

## 4. 运行入口

先执行契约检查：

```bash
sh apps/starry/macos-selfbuild/tests/tg_xtask_profile_contract.sh
```

准备或刷新当前内核、overlay 与持久根文件系统，并在 QEMU 中执行 600 秒工作负载窗口：

```bash
STARRY_MACOS_SELFBUILD_MODE=tg-xtask-profile \
  cargo xtask starry app qemu \
  --test-case macos-selfbuild \
  --arch aarch64 \
  --qemu-config apps/starry/macos-selfbuild/qemu-aarch64-profile.toml
```

成功标记必须为：

```text
===STARRY-ARCEOS-HELLOWORLD-PROFILE-WINDOW-PASS elapsed=<seconds> rc=<0|124> build_completed=<true|false> tg_xtask_reused=<true|false>===
```

`rc=124` 表示到达采样时限，`build_completed=false` 表示尚未编译完成。这个标记只说明窗口采集成功。`compile_units` 是日志中已启动编译的单元数，不能解释为已完成数量或线性加速比。`elapsed_seconds` 包含超时退出所需的少量收尾时间，内核头中的 `prebuild_ns` 记录实际采样时长；比较时必须检查窗口偏差。

历史 linux-tg-xtask-profile.sh 是原生宿主 bpftrace runner，会调整宿主 profiling
sysctl，不满足当前仅 QEMU 和简单命令要求；本轮没有执行，不作为推荐入口。
匹配的 Linux QEMU 工作流见 [linux-qemu-comparison.md](linux-qemu-comparison.md)：
独立入口及 initramfs/渲染工具共 22 项测试通过；Linux 8 核 NVMe 启动和
独立运行的 guest BPF 全核预检成功。首次编译的失败数据已保存，不能用它
或旧原生脚本替代有效对照；两侧 runner 已补 Cargo host 配置 gate，Linux
已通过原 bindgen 失败点；Starry 的 host-config-baseline 和 inode-writeback
两个窗口也已采用同一 host 配置并进入实际编译，均未完成整个构建。契约检查已同步到当前
guest QEMU runner 和拆分后的 ext4 模块路径，实际退出 0。

## 5. 优化验收原则

2026-09-10 用户后续指示：完成整体重构之后，三轮静态检查通过才允许启动测试。
重构期间只进行源码检查、格式化、Clippy 和构建；测试目标编译不算运行。
20:59 首轮门槛通过，行为测试发现 ax-io 既有切片推进问题；修复后于
21:16:49 完成补充三轮门槛，再恢复测试。当前通过 QEMU 内核 51 项、
rsext4 585 项、适配层 213 项及 ax-io 全套。逐轮记录和仍有的覆盖缺口见
[ext4-final-static-review.md](ext4-final-static-review.md)。
未测行为必须明确标记；本地性能候选不等于满足合入门槛。

每项内核优化按以下顺序验收：

1. 固定源码归档、根文件系统、QEMU 参数、工具链和工作负载；
2. 运行未优化内核的 600 秒冷构建窗口，保存内核采样、ELF、实际窗口时间和 vCPU 利用证据；
3. 只修改由 profiling 数据支持的内核路径；
4. 完成确定性回归测试、格式检查和目标 crate clippy；
5. 每轮从同一只读冻结根盘复制工作盘，核对完整哈希，再执行优化内核的
   600 秒冷构建窗口；保存上一轮结束盘，不以连续修改过的根盘作为相同起点；
6. 比较构建进展、实际窗口时间、vCPU active/idle 样本、内核 CPU 栈和各类等待；构建提前完成时同时校验输出二进制和返回值；
7. 只保留有可重复收益且不破坏调度语义的改动。

调度优化尤其要区分新任务放置、阻塞任务唤醒与显式迁移。新任务可以跨 CPU 分布；已经运行过的任务唤醒时优先保留原 CPU，以减少缓存失效和跨 CPU 上下文风险。进一步迁移或负载均衡只有在新一轮内核数据仍显示失衡时才引入。

## 6. 当前低 CPU 利用率排查记录

详细证据按独立改动保存，不能把各轮的局部指标相加当成最终收益：

- [scheduler-placement.md](scheduler-placement.md)：运行队列发布、启动 CPU 选择、
  每核采样，以及采集中暴露的 ext4 空闲 inode 修复。
- [vma-range-scans.md](vma-range-scans.md)：限制 unmap 的 VMA 扫描范围，旧实现
  确定性回归失败、修复后通过；对应 QEMU 窗口没有显示整体进度提升。
- [mmap-lazy-fallback.md](mmap-lazy-fallback.md)：成功 hint 不再执行备用查询，
  CPU 查询热点下降，但 600 秒窗口仍只启动 11 个编译单元。
- [large-file-cache.md](large-file-cache.md)：按大文件长度增加页缓存复用窗口。
  32 MiB 候选窗口 CPU 活跃从 14.99% 增至 19.06%，启动编译单元从 11 到 18，
  仍未完成构建。256 MiB 候选窗口达到 23.97% 活跃、27 个编译单元，仍未完成；
  最大 mutex 等待已转移到 ext4 全局锁（约 70%）。继续优先检查 ext4 持锁
  同步写盘路径；不把单轮进度差当作完整构建加速比。
- [ext4-metadata-writeback.md](ext4-metadata-writeback.md)：读文件关闭不再隐式
  全盘同步。真实 ext4 回归先失败后通过，显式同步/错误传播/重新挂载/fsck 验证通过；
  `metadata-writeback-window-600` 已完成：CPU 活跃 27.50%，启动 34 个编译单元，
  仍未完成构建；关闭路径块写耗时为 0，ext4 锁约占 mutex 等待 83.6%。
  离线 fsck 发现的历史空闲 inode 标志已在完整备份与逐字节核对后修复，
  有效 inode 和文件内容未改动，最终 fsck 退出 0；详见该文档。
- [ext4-create-writeback.md](ext4-create-writeback.md)：直接移除 create 同步会
  留下目录项引用未分配 inode，真实镜像测试已否定并恢复原同步，未跑 QEMU 候选。
- [inode-identity.md](inode-identity.md)：初版窗口已完成，活跃 24.58%，启动
  31 个编译单元；inode_key 等待消失，但整机未改善。兼容性回归要求快路径
  仅用于 ext4，当前四项测试通过。原窗口五项哈希与离线 fsck 全部通过。
- [cache-hit-concurrency.md](cache-hit-concurrency.md)：缓存命中不再等待其它
  页的磁盘 I/O，同时用更新 guard 保护写入/截断回滚。98 项测试和 8 项
  clippy 通过，`cache-hit-window-600` 已测得活跃 28.60%、启动 36 个单元，
  完整构建未完成。ext4 锁占 mutex 等待 81.69%，缓存慢路径占 15.42%；
  五项 SHA-256 和离线 fsck 通过。单次进度差不是完整编译加速比。
- [page-fault-concurrency.md](page-fault-concurrency.md)：按 MOSS 三阶段方式
  实现私有文件缺页锁外准备/锁内重检，真实 AddrSpace/CowBackend 的内核
  RED 已复现持锁读取，包含九种内部场景的 GREEN 46/46 通过。
  `file-fault-window-600` 已完成：活跃 26.68%、启动 34 个单元，未完成构建；
  用户缺页 mutex 栈从约 674 秒降到 585 秒，但整体不优于上一轮。
  该轮最大等待为 ext4 全局锁，占 mutex 等待 86.04%；五项哈希与 fsck 通过。
- [journal-write-batching.md](journal-write-batching.md)：在保留 create 同步与所有
  flush 屏障的前提下合并运行期连续日志块，8 项提交/恢复回归通过。首个窗口
  临近收尾发生 PID identity panic，失败镜像封存。诊断复测 K2 正常结束：
  活跃 24.43%、启动 33 个单元；块写次数约减半，但未改善整体进度。
  五项哈希与 fsck 通过，PID panic 未复现、未宣称修复。
- [data-writeback-read.md](data-writeback-read.md)：完整块写回跳过旧块读取，
  3 项确定性 RED 转 GREEN，全套 207 项通过。L 窗口活跃 25.07%、启动 33 个单元，
  写回预读从约 20 秒降为 0，但整体进度未改善，五项哈希和 fsck 通过。
- [ext4-lock-owner.md](ext4-lock-owner.md)：持锁者与设备 flush 计时设计，
  已实现事件 8/9，真实锁时序回归先失败再通过，含 profile 全套 102 项通过，
  M 窗口正常结束，活跃 29.82%、启动 39 个单元，未完成构建，哈希/fsck 通过。
  ext4 占 mutex 等待 77.69%；持锁以同步 29.77%、读 27.84%、lookup 20.54%
  为主。缺页调用链占持锁 21.16%，与读重叠；不能仅按 MOSS 的比例排序。
- [data-read-cache.md](data-read-cache.md)：完整命中缓存时避免设备重读，
  2 项确定性 RED 转 GREEN，6 项定向回归、rsext4 全套 213 项和 ax-fs-ng
  102 项通过；N 窗口活跃 31.36%、启动 40 个单元，五项哈希与 fsck 通过。
  相同根盘起点 M2 基线为活跃 23.49%、启动 34 个单元，哈希/fsck 通过。
  N2 同内核复测为 24.98%、33 单元，哈希/fsck 通过，首次正向信号未稳定复现。
- [inode-table-read-cache.md](inode-table-read-cache.md)：复用既有四块设备缓存
  读取相邻 inode，3 项确定性 RED 转 GREEN，6 项定向回归、rsext4 219 项、
  ax-fs-ng 102 项与 clippy 通过。O 窗口活跃 24.49%、启动 34 个单元，
  哈希/fsck 通过；inode 加载链块读减少但没有稳定整机收益，N2 已证实有明显波动。
- [fs-context-snapshot.md](fs-context-snapshot.md)：借鉴 MOSS 的 root/cwd 快照，
  两项确定性 RED 转 GREEN，定向文件测试 7/7、AArch64 生产 clippy 和构建通过。
  宿主全测试 clippy 因 136 项既有 lint 失败，原文完整展示；P 已完成，608 秒、
  活跃 25.85%、启动 34 单元，哈希/fsck 通过。目录上下文 mutex 等待叶消失，
  但整体进度未明显改善；ext4 仍占 mutex 等待 78.08%，读持锁 37.55%。
- [parent-inode-lookup.md](parent-inode-lookup.md)：发现 ext4 子项查找重复从根目录
  行走，真实 I/O 故障注入回归 RED 转 GREEN；新增 4 项 API 测试、rsext4 全套
  223 项、ax-fs-ng 103 项及 11 项 clippy 通过。R 已完成：603 秒、活跃
  30.07%、启动 39 单元/36 crate，哈希/fsck 通过。lookup 持锁 58.62→43.97 秒，
  单轮正向信号待重复；总持锁仍约 489 秒，读取 35.67%、同步 34.44%。
- [ext4-read-concurrency.md](ext4-read-concurrency.md)：每 inode I/O 保护与
  锁外读计划已实现，真实读事件持锁回归 RED→GREEN；9 项计划回归、rsext4
  全套 232 项、ax-fs-ng 112 项及 11 项 xtask clippy 通过，额外宿主/ARM
  clippy 与固定 Starry 构建通过。S 的 ext4-read-concurrency-window-600
  停滞，未导出性能结果，原根盘与 8 GiB guest RAM 已保留。
- [gic-lpi-pending-tables.md](gic-lpi-pending-tables.md)：S 原现场发现
  八核 GIC pending table 地址重叠、CPU 6 反复接收 NVMe LPI 8199。
  T 修复每核 64 KiB 对齐，生产驱动回归 RED→GREEN、AArch64 测试 22/22、
  clippy 和固定 Starry 构建通过；独立启动诊断已读回八核各自独立的地址。
  正式 S+T 窗口使用 ext4-read-gic-alignment-measure-600，冻结根盘恢复后
  等待完整哈希再启动、不启用 GDB。S+T 已正常结束，611 秒、40 单元 / 37 crate、
  活跃 26.42%，哈希/fsck 通过；读取持锁 174.47→41.93 秒，但 CPU 活跃低于
  R 的 30.07%，没有整体加速结论。该轮最大持锁来源 sync_to_disk 为 49.41%，
  缺页调用链为 8.36%（与读取重叠），ext4 仍占 mutex 等待 75.23%。
  S+T 相对 R 的变化不能只归因于锁外读；继续调查同步写回的重复 I/O。

- [bitmap-writeback-read.md](bitmap-writeback-read.md)：完整位图写回不再预读
  旧块，生产缓存回归 5 项 RED→6/6 GREEN，全套 238 项和 clippy 通过。
  U 窗口 608 秒、启动 44 单元 / 41 crate、活跃 32.38%，哈希/fsck 通过，
  位图写回的旧块读消失。同步持锁 196.01→175.27 秒；但总 ext4 持锁和
  ext4 等待略增，不能据单窗口认定稳定整体加速。该轮 ext4 占 mutex
  等待 82.51%，同步占持锁 43.14%。
- [fresh-page-zero.md](fresh-page-zero.md)：U 完整缺页链占活跃 CPU 21.62%，
  其中清零占 30.24%；这与 ext4 持锁中的缺页子集不是同一口径。借鉴 MOSS
  只优化尚未发布的新页，不改全局 memset；AArch64 EL1 内核测试 47/47，
  明确执行 DC ZVA，定向 clippy 通过（过宽矩阵只通过前 54 项）。
  V 已正常结束：604 秒、40 单元 / 37 crate、活跃 28.00%，哈希/fsck 通过。
  保守清零样本 1128→480（-57.45%），缺页 CPU 占比 21.62→19.51%；
  但单元数下降，不能声明整体提速。ext4 仍占 mutex 等待 75.18%，
  同步占持锁 48.52%。
- [pending-block-read.md](pending-block-read.md)：W 复用完整未提交日志块，
  避免单块 bulk read 先读旧磁盘；8 项中 2 项确定性 RED，修复后 8/8 GREEN。
  保留多块、缓冲边界、journal 关闭和 miss 的旧 I/O 语义。W 正常结束，
  607 秒、45 单元 / 42 crate、活跃 34.26%，哈希/fsck 通过。
  同步持锁 199.79→172.06 秒，但创建同步基本不变，单轮不足以宣称稳定提速；
  ext4 仍占 mutex 等待 75.55%。Linux QEMU 已通过启动/采样预检，不是已测对照。
- [cached-inode-metadata.md](cached-inode-metadata.md)：缓存命中 metadata/len
  复用既有 inode cache 的只读快照，避开全局 fs 锁，不新增属性副本。
  两条确定性 RED 转 GREEN，ax-fs-ng profile 117/117、rsext4 249 项通过，
  11 项 clippy 与实际 profile 特性组合 clippy 通过。X 正常结束，610 秒、
  41 单元 / 38 crate、活跃 29.31%，哈希/fsck 通过。属性等锁约 413→134 秒，
  但总等锁与块读时间增加，没有整体提速结论；ext4 占 mutex 等待 80.99%。

原始实验目录均位于 `target/profiling/arceos-helloworld/starry/`，保存对应
ELF/BIN、qemu.log、导出的采样与 SHA256SUMS，以及 `rendered/` 火焰图和摘要。

## 2026-09-11 续接：增长缓存完成，目录查找成为最大持锁热点

最新生产状态与 [growing-file-cache.md](growing-file-cache.md) 的源码快照一致。
2026-09-10 22:04:01 已完成新实现的三轮静态检查，之后 5/5 定向回归、
218/218 ax-fs-ng lib 测试通过；22:18 的 `growing-cache-window-600` 正常结束。
611 秒启动 55 单元 / 52 crate，非 idle 42.1067%，ext4 持锁 164.411787 秒、
等锁 396.234723 秒，五项产物哈希与只读 fsck 通过。未完成完整编译。

当前最大持锁来源 `lookup_locked` 为 88.787315 秒（54.00%）；VFS 已有正向
dentry 缓存，但没有不存在结果缓存。尚不能从原始采样区分冷正查找与重复负查找，
下一轮需保持这一证据限制并核对 Linux dcache 的失效与并发发布规则。
没有运行中的 QEMU；固定工作盘是增长缓存窗口的结束盘。前一轮结束盘保存在
`tmp/axbuild/rootfs/rootfs-profile-ext4-background-window.img`，只读冻结基盘不变。
任何新的生产重构完成后必须重新三轮静态检查，再启动修改后测试或 QEMU。

## 2026-09-11 01:17 续接：目录负缓存窗口完成

生产状态更新为 [negative-directory-cache.md](negative-directory-cache.md)：
VFS 缓存统一所有权与发布版本，ext4 基于 canonical inode change_attribute
启用有界负项缓存，覆盖旧打开目录别名、失败 mutation 与清理时并发发布。
00:56:59 完成最终三轮静态检查后，真实 ext4 RED 回归转 GREEN；VFS 59/59、
ax-fs-ng 223/223，无忽略；首次新增 whiteout 用例预期错误已修正并保留失败证据。

`negative-cache-window-600` 正常结束：608 秒、62 个启动单元 / 59 crate，
active 48.2027%；lookup 持锁 88.787315→60.279573 秒，仍占持锁 36.83%，
总持锁 163.679086 秒，mutex 等待 928.233803 秒。仅一轮正向信号，完整
编译未完成。五项哈希、只读 fsck 通过，全部 dropped/skipped 为 0，已查看图。
Linux 最近同配置为 177 / 171、71.8212% active，不报告严格 OS 加速比。

没有运行中的 QEMU。固定工作盘现为负缓存窗口结束盘；前一轮结束盘是
`tmp/axbuild/rootfs/rootfs-profile-growing-cache-window.img`。串口未进入窗口的
首次启动现场单独保留于 `negative-cache-console-stall`，未作为性能样本。
只读冻结基盘 SHA 仍为 9968004595dec398480417e210d64f9ed509801d4325d4ab22f43951983f5ea3。
当前源码快照为 `tmp/ext4-final-static.ryo14j/source-negative-cache.tar.gz`，
SHA 15a5836ee537394c8f0265aa5ed47e7f1ec3ff35f35d838b429b3eb4079d3045；
最终 ELF e1a4d024f0b8e1389878aa2b663c2cdac191ac6cc2791be038a02eb7f7cf9424，
BIN b2449148f0f1178bc2b7e7bbe90c4019dbc919c36bbc5d50f9f6b2e818e25523。

下一阶段继续拆解最大 lookup 热点：其块读取累计 54.763365 秒，其中目录块
34.376646 秒、inode 装载 20.386719 秒。正在核对既有共享块缓存、inode 生命周期
和 Linux 元数据读取边界，尚未选定或实施新生产重构，不能声称已完成锁外目录 I/O。
任何新生产修改仍必须完成三轮静态检查，再启动测试或新的 600 秒窗口。

## 2026-09-11 01:42 续接：切换全程编译

最新用户要求直接跑完 Linux 和 Starry 全程，覆盖旧的 600 秒窗口偏好。
不要在 600 秒、1200 秒或进度暂时不变时终止运行；不再纠结 133/177 条数。
详细输入、三轮静态检查及渲染回归修复见 [full-build.md](full-build.md)。
当前 Linux QEMU PID 827388，终端 session 65727，实验目录
`target/profiling/arceos-helloworld/linux/full-build`，01:42:24 已开始编译。
启动器与 guest 均无编译超时。等它自然结束、导出和校验后，立即运行已准备好的
`target/profiling/arceos-helloworld/starry/full-build`，不要并行两个 QEMU。
Starry 固定工作盘现在是注入全程入口的新冷盘；旧负缓存窗口盘保留于
`tmp/axbuild/rootfs/rootfs-profile-negative-cache-window.img`。
Starry kernel ELF/BIN 与 01:17 那轮完全相同，不要在基线中途重建更换内核。

01:55 更新：Linux full-build 已自然完成，746 秒，rc=0，181 个编译单元，
最终 ELF 导出/哈希匹配，12 项校验通过，fsck 0，CPU active 72.10225%。
原始数据和火焰图均在 Linux full-build 目录，限制与详细数据见 full-build.md。
当前已经切换到 StarryOS：QEMU PID 829876、终端 session 2453；01:55:26
启动，正式 guest runner 已输入，复用检查通过。继续等待，不重复启动 QEMU。

## 2026-09-11 03:08 续接：全程发现丢失唤醒，正在修复 QEMU

当前 Starry PID 829876 / session 2453 仍在运行，未杀、未暂停、未 attach。
进度停在 178 个编译单元，不能自然完成的直接现场是两个 LLVM 线程已
Ready/on_cpu=false，intrusive 队列链接为空，但 wake_handoff 槽仍持有自身
引用，且 8 个运行队列全空。其余编译线程等在 futex/poll。不要继续将其称为
“ext4 慢”，也不要把此轮宣布全程成功。多次只读内存检查重复该现场。
具体偏移来自 ELF 反汇编，正式入口及证据见 [full-build.md](full-build.md)。

最新完整诊断产物：
`starry/full-build/live-profile-1789067127045059046/`（相对共同 profiling 根）；
保留 24 个原始表、CPU 元数据、kernel-profile.raw、snapshot.json、rendered/。
本次 changed aggregate rows=0、全部 dropped/skipped=0；enabled=true、phase=1，
它仍是未完成客体的非全局原子快照，不能计作有效全程性能结果。
任务快照 `live-tasks-1789067127188574595.json` 记录 50 个存活任务及保存的栈。
前一次 `live-tasks-1789066646392158257.json` 同样确认两个丢失的就绪线程。
Linux 原全程 746 秒的结果已完成并完整保存，不重复旧模拟器的测量。

已定位模拟器指令翻译缺陷：系统 QEMU 10.2.1（Ubuntu 包）将 STLR/LDAR
翻译为 store 前屏障和 load 后屏障，中间没有 RCsc StoreLoad 屏障。
正式无磁盘 8c8g 指令生成回归先 RED，原始证据在
`tmp/qemu-rcsc-a8aqljhr/translation.log`；测试入口
`python3 apps/starry/macos-selfbuild/tests/check-qemu-rcsc.py`。
检查不是概率压力测试；检查实际执行的 TCG，不能降级成读源码断言。
上游 2026-08-21 补丁提供同类缺陷说明；在固定 v10.2.1 上准备修复，尚未
获得修复后全程验证，暂不能排除其他错误。

独立源码 `/home/wuxun/Projects/qemu-rcsc-profile`，HEAD
`2d3df8abca265c9bcc9e438d691d561592060998`。不安装/覆盖系统 QEMU。
仅修改两个 translate C 文件共 6 行，正式补丁保存在
`apps/starry/macos-selfbuild/qemu-rcsc.patch`，reverse --check 已确认匹配。
构建目录是该 QEMU 仓库 `tmp/build-profile`，当前运行中 build session
35583，日志 `build-system-fdt.log`；不要重复启动 ninja。
配置启用 slirp、system fdt、werror，只构建 aarch64-softmmu，禁用文档。
首次构建失败是内置旧 libfdt 丢弃 const，原文保留在 build.log；已改用系统
libfdt，不降低 -Werror。补装 libslirp-dev 4.9.1-1ubuntu1.1、libfdt-dev
1.7.2-2ubuntu1；没有删除或升级其他包。configure-system-fdt.log 确认成功。

下一步先完成修复后第三轮静态（含严格 C 编译），运行同一个指令回归确认
GREEN，必要时完善仅用于诊断的脚本。Starry 原故障进程先保持，已有采样与
任务证据可保全后按明确故障处理，不能按耗时自动 kill。采用新 QEMU 后需要
同一新模拟器的 Linux/Starry 冷盘全程；不得混用旧 Linux 746 秒作严格 A/B。
仍使用冻结 Starry ELF/BIN 和根文件系统复用 tg-xtask，不改内核掩盖 TCG 错误。
新全程需先确认旧故障 QEMU 不再使用固定工作盘并无损保留故障盘，然后准备新盘。

## 2026-09-11 03:17 续接：修复版 QEMU 已启动全程

上面的 03:08 状态已过期。QEMU build session 35583 已退出 0，严格 C
编译完成，同一确定性测试得到系统旧版 RED、修复版 GREEN；详见
[full-build.md](full-build.md)。修复版二进制 SHA
`a1d9a6080e1f025038a81749baa9d9552ae89321ea163abbf704206034f0a6a4`。
旧故障 PID 829876 已按明确丢失唤醒终止，故障盘完整保存为
`target/profiling/arceos-helloworld/starry/full-build/rootfs-lost-wake.img`。
不要把旧启动器的退出 0 解释成编译完成，也不要重新使用该故障盘。

当前运行中的唯一测量 QEMU：PID **850065**，终端 session **47467**，
03:16:54 启动；目录 `target/profiling/arceos-helloworld/starry/full-build-rcsc`。
`/proc/850065/exe` 及其 SHA 已确认是修复版。原启动器未改动，通过 PATH
选择独立 QEMU 构建；8c8g、Cortex-A53、MTTCG、NVMe 和冻结 ELF/BIN 不变。
客体 shell 已就绪并输入 `/bin/sh /opt/starry-macos-run.sh`。
没有宿主或 guest 编译时长截止。不要重复启动，不要在测量期间重构内核、
运行其他构建或大文件复制。下一步等待自然结束、导出检查，再串行准备并运行
同一个修复版 QEMU 的 Linux 全程，不混用旧 Linux 746 秒当严格对照。

固定 Starry 工作盘已重新由只读基盘复制，等待整盘 SHA 校验完成后才注入
runner；SHA 为 `9968004595dec398480417e210d64f9ed509801d4325d4ab22f43951983f5ea3`。
注入脚本从盘导出与正式源码一致，SHA
`c94b86694e6f7e8b2c99b84c7c79ca2b1626323526add691d1636a238cd6f392`，
只读 `e2fsck -fn` 退出 0。此前尚未启动过客体的准备盘另存为
`tmp/axbuild/rootfs/rootfs-rcsc-preflight.img`，不计入测量。

## 2026-09-11 03:59 续接：Starry 全程完成，Linux 新对照运行中

Starry PID 850065 / session 47467 已自然退出 0：2298 秒、181 编译单元、
175 个不同包名、build_completed=true、tg_xtask_reused=true。Cargo 37m45s，
axbuild 2288.36s。已导出全部采样与最终 ELF；6 项 SHA256SUMS 全部通过，
实际 tg-xtask SHA 与冻结值一致。最终 ELF SHA
`2792c35275d91f57a38a847be87c4d2b333d7b88cd548bf9e06633288c7dba21`，
与旧 Linux 全程相同。原始采样为 enabled=false、phase=0，prebuild_ns
2297526898288，所有 dropped/skipped 为 0。已生成并查看 CPU/持锁图。
178447 个 CPU 样本中活跃 77339（43.34004%）；最大可解析内核叶子
exit_preemption 8.2817%（活跃样本分母）。不要将等待时间与 CPU 样本混加。

**文件系统验收失败，不能称全部通过：**只读 fsck 退出 4，原文完整保存在
`starry/full-build-rcsc/fsck.log`。inode 74474 块数 40 应为 56，inode
74479 块数 56 应为 72。分别是 source/target/release/deps 与
source/target/aarch64-unknown-linux-musl/release/deps 两个目录；两者
extent tree 均有两个外部块，而 i_blocks 只包含数据块。产生该差异的代码
原因尚未确认，不要原地 fsck 修复、不要重用此盘。完整结束盘已移到
`target/profiling/arceos-helloworld/starry/full-build-rcsc/rootfs.img`；固定
Starry 工作盘路径现在不存在。用户此次要求完整 profiling，先完成对照和
证据，再处理新发现的问题，不要丢失原始结果。

当前唯一 QEMU 是 Linux PID **853059**，终端 session **16123**，目录
`target/profiling/arceos-helloworld/linux/full-build-rcsc`，03:59:29 启动。
新盘完整哈希先匹配冻结基础盘 844cd703...，注入三份正式 runner/BPF 后
逐一 cmp 相同，注入前后只读 fsck 均退出 0。同一个修复版 QEMU、8c8g、
TCG multi、Cortex-A53、NVMe，vmlinuz/initramfs 与旧 Linux 全程相同。
客体 shell 已就绪并输入 `/bin/sh /opt/guest-linux-profile.sh`，不要重启。
等自然结束后导出 12 项校验所列文件和最终 ELF，渲染并查看 Linux 图，
与本次 Starry 全程作单轮同模拟器比较。测量时不要并发构建、大文件复制
或修改活盘；不设编译截止。

## 2026-09-11 04:14 续接：两个全程 profiling 均已结束

Linux PID 853059 / session 16123 已自然退出 0；当前没有本任务运行中的
QEMU。Linux 测量 735 秒、181 编译单元 / 175 个不同包名，tg-xtask 复用，
12 项校验通过，BPF stderr 空，末态只读 fsck 退出 0。最终程序与 Starry
经 cmp 确认逐字节相同，181 项“包名 + 版本”列表排序后也 cmp 相同。
Linux 全程 CPU 58856 样本，活跃 41675（70.80841%），每 CPU 7357；
/proc/stat busy 71.52973%，BPF 区间 735842703904 ns，pending offcpu
399。已经渲染并查看本轮 Linux CPU 图。

同模拟器单轮耗时：Starry 2298 秒 / Linux 735 秒 = 3.12653；采样后端
开销不同且未重复测量，不报告稳定加速比。完整报告首部已经更新为最终结果，
见 [full-build.md](full-build.md)，所有图和数据在双方 full-build-rcsc 目录。
结束盘均改为只读，整盘哈希输出到共同 profiling 根下
`full-build-rcsc-disks.sha256`。不要把只读结束盘当冷基础盘继续写入。

Starry fsck 仍然退出 4，未修复：两个 deps 目录各漏计两个外部 extent
块，原始盘与 fsck.log 完整保留。源码定位到
`fs/rsext4/src/hashtree/mutation.rs:append_directory_block`：
insert_extent 更新了 extent 元数据块计数，随后旧 updated_blocks 覆盖
新增计数。与现场一致，但新增回归和修复尚未执行；本轮未修改 Rust 内核。
如果下一轮修复，先增加确定性回归并完成三轮静态检查后证明 RED，再修复、
重新三轮静态与 GREEN/clippy；不得用原地 fsck 修复代替代码修复证据。

全程 CPU 内核最大叶子 exit_preemption 8.28% 活跃，缺页调用链包含
17.90% 活跃；ext4 包含栈 3.43% 活跃，但 ext4 锁等待累计 2085.214 秒，
持锁累计 726.173 秒中 lookup 占 269.370 秒（37.09%）。这几类可重叠，
不能相加当墙钟时间，也不能把 ext4 等待和缺页简单当互斥原因。下一轮
性能改动前保留这一完整基线；先处理明确的目录块计数正确性问题。

## 2026-09-11 16:11 续接：抢占重构后的完整测量正在运行

本节替代上面的运行状态，不要重复启动。当前唯一测量 QEMU PID **892991**，
终端 session **99451**，目录
`target/profiling/arceos-helloworld/starry/event-driven-preemption`。
15:43:01 启动，正式零参数 guest runner 已执行，最新进度 1621 秒、147 个
编译单元。仍需等待自然结束，测量期间不要编译、另起 QEMU、校验大文件或
修改活盘。继续使用原有修复版 QEMU、8c8g、冻结 guest 源码和 tg-xtask；
`tg_xtask_reused=true`、`source_reused=true`、亲和性 0–7 已确认。

已完成事件驱动抢占重构：task 请求发生时发布 pending；cpu-local 在上下文
所有者上直接完成 token；runtime 仅最终 pending 分支执行 IRQ 排除与
scheduler baton。普通退出不再每次查询 task、CpuPin、重复发布 pending。
保留 x86 迁移继承深度、IRQ-return 特权、首次进入与恢复 tail。正式设计和
全部检查见 `docs/design/event-driven-preemption.md` 与
`docs/profiling/event-driven-preemption.md`。

ext4 新确定性回归先退出 101（48 sectors 对比应有 64），然后修复
append_directory_block 的计数顺序。修复后重新三轮静态，GREEN 通过；目录、
extent restart、clean unmount 合计 22/22。未修复或重用旧失败结束盘。
cpu-local 状态测试 24/24；ax-task host 非 IPI 组合 59/59；8c8g QEMU
最终 19/19（抢占、IRQ、全 CPU 迁移均通过）。第一次 QEMU 的全局 FIFO
断言不适用于独立多核队列，已将 fixture 改为同 CPU 完整入队，保留顺序断言，
重做三轮静态后通过。四 crate clippy 分别为 3/3、70/70、25/25、3/3。

仍有两项原有广域检查失败，不能报全部通过：cpu-local final-image test
发现 x86 Axvisor hv 无条件选择 arm-el2；ax-task host IPI test 引用了 HEAD
已不存在的 crate::tests::run_in_test_scheduler。完整原文保存在
`tmp/preemption-refactor.5NL1NQ/`，已直接输出，不在性能主线中改动这些边界。

重要更正：完整基线的 exit_preemption 6405 个样本中 6278 个（98.02%）
落在 `ffffffff802bee88`，即 DAIFClr 后第一条指令。8.28% 是采样叶子宽度，
不是该函数的独占执行时间；IRQ-off 延迟可造成聚集。不能仅凭新图该函数变窄
声称提速，必须比较完整耗时、全部活跃样本和新的 IRQ/guard 归属。

已保存测量 ELF SHA256
`38bf7ef0f66442a898d3b561e50d22c01bd9762d2540932f5126468223094ddd`，
BIN `287bd9da5adfd0adb7aaca07e54c98e78f068101dc3f668c218758c02351c5e5`。
冷盘复制完整匹配原基底，随后仅替换全程 runner，cmp 与启动前 fsck 通过。
新盘当前位于固定工作路径，启动前 SHA 为
`3d3cac0c3c5c73bcc4cf4fdc3cf02ba61d43ee16375d6080092cae062a68d283`。
`inspect-live-profile-tasks.py` 绑定旧 ELF/偏移，不能直接用于本内核。

完成后必须：确认自然退出，逐文件导出原始产物和最终程序，检查所有 SHA，
fsck 只读验收，渲染/查看完整图，与 2298 秒 Starry / 735 秒 Linux 对比。
没有本轮性能结论。已只读检查 MOSS 的 PFN 分片引用表、锁外 COW 准备与
批量 unmap/TLB 后回收，以及本仓库逐页重复遍历；尚未实现这些下一候选，
需以本轮完整图决定后续重构重点。

## 2026-09-11 全程测量终态：2089 秒，当前转入批量 unmap 重构

上节 PID 892991 已自然退出 0，session 99451 已关闭；渲染 session 82958
也退出 0。不再有 QEMU，不要重复启动该 run。181 个包名/版本以及最终程序
分别与 Starry 和 Linux 基线 cmp 一致；原始六项 SHA 全通过，末态只读 fsck
退出 0。完整 profile prebuild_ns=2089523443840，丢弃/跳过均为 0。
结束盘从固定工作路径移动到该 run 的只读 rootfs.img，SHA256
`eb594781dd35635badb88dbf35d74a20ee72a101ce6904087252d508f0e2b846`。
整盘 hash session 28007 应只需确认完成；不要再次写该盘。

单轮耗时 2298 → 2089 秒，缩短 9.09487%；Linux 735 秒保持冻结基线。
新 CPU 74094 / 161966 活跃（45.74664%）；用户态 40049，最大内核叶子
unmap_range_recursive 3651（4.92752%），zero 3210，map_range_recursive
2995，spin_release 2779，find_free_area 2437。缺页栈 12881、ext4 栈1863。
mutex-wait 2645.181 秒，其中缺页 1058.717 秒；ext4 锁等 1503.894 秒、
持锁 564.377 秒，lookup 持锁 211.393 秒。三个图已生成 PNG 并实际查看。
详细结果已写 event-driven-preemption.md，不把 IRQ 边界叶宽当独占耗时。

下一重构已定位逐页 query + 从根递归 unmap + 重复扫描空表 + 单页 TLBI。
准备按 MOSS 和 Linux mmu_gather 设计批量遍历与延后回收，尚未写该生产实现。
关键新发现：ax_cpu::asm::flush_tlb(None) AArch64 为本地 vmalle1，不能
直接拿来替换原逐页 vaae1is；新批处理必须完成跨 CPU 失效后才能释放页面
和页表页。保留三轮静态门槛与确定性生命周期回归，不允许仅删 flush。

## 2026-09-11 批量撤销重构已实现，正在最终静态验收

上节“尚未实现”已经过时。当前新增 `page-table-generic::unmap_owned`，
一次深度优先遍历、64 项有界回收批次、先跨核失效再释放页面/页表；
AArch64 的 `flush_batch` 提供 ishst/TLBI-IS/ish/isb，Starry COW 与
shared-file range unmap 已迁移。设计和逐项证据见
`docs/design/batched-mapping-retirement.md`、`docs/profiling/batched-mapping-retirement.md`。
泛型生命周期测试已合规 RED，完整生产重构后的 GREEN/QEMU 尚未运行。
第一轮 RED 曾因命令编排失误越过失败的测试 clippy 门槛，已直接报告，
不计验收；修正最小 fixture 并重做三轮检查后重复 RED 才是有效证据。

泛型 clippy 2/2、测试 clippy、ax-cpu clippy 33/33 和实际 profiling 构建
已退出 0。Starry 110 项矩阵首次在第 22 项因 Wi-Fi 固件 DNS 失败退出 1，
已改用现有、逐文件校验的离线缓存重跑，日志为
`tmp/preemption-refactor.5NL1NQ/retirement-starry-clippy-retry.log`。
目前没有 QEMU；新测量必须复制冻结冷盘，不能修改上轮只读结束盘。

## 2026-09-11 17:02：新重构通过三轮静态与回归，全程 profiling 已启动

最终静态三轮已记录于 batched-mapping-retirement.md。Starry 重跑矩阵
110/110、实际 AArch64 SMP=8 axtest clippy、泛型 2/2、ax-cpu 33/33
全部退出 0。之后泛型新回归 7/7、完整 crate 76/76、Starry 8c8g
ktest 52/52 全过（不把既有 mock warning 说成不存在）。ktest 下载器读到
历史 `/tmp/tgosimages` 配置，本次两个生成文件已迁回仓库 tmp/tgosimages；
后续 xtask 镜像入口必须显式设置 TGOS_IMAGE_DOWNLOAD_DIR 为该路径。

全程 candidate 是 `target/profiling/arceos-helloworld/starry/batched-mapping-retirement/`。
QEMU PID 927344，PTY session 65278。已到 shell 并执行
`/bin/sh /opt/starry-macos-run.sh`。只用冻结 patched QEMU、8c8g、GICv3、MTTCG。
不截断构建，不在测量期间运行其它 QEMU、编译或整盘 hash。
ELF `7a40b7261151c17d2db0f473b1704d671d63a105935461581f9336ef4a4ee01c`，
BIN `29080f2f927c03076b02ac48e35cebdd257163ef68887dc62bbd7b4d8bbf1151`；
源码 snapshot `retirement-sources.tar.gz` 保存完整三个 affected crate。
新盘复制全量匹配冷基底，仅替换正式 runner，cmp/fsck 通过；启动前整盘
SHA `e7a09de553c8628870d08c0a01b1056e03fcae6a87435cf97e02d1f42e53f935`
已完成后才开机。内核地址较上一轮有变化，必须使用本轮保存的 ELF/符号表。

完成后仍须：自然退出、导出原始六项 SHA 与最终 ELF、逐项校验、只读 fsck、
核对 181 个完整编译单元和程序与两条基线一致，渲染/实际查看新全程图，再
给出本轮增益或退化。当前尚无本重构的性能结果。

## 2026-09-11 17:35：批量撤销全程成功，转入缺页准备重构

上节 PID 927344 已自然退出 0，session 65278 已关闭。渲染 session 30299
和整盘 hash session 62220 均已退出 0。目前没有本任务 QEMU 或构建在运行。
本轮 1908 秒、rc=0、181/175；对上一轮 2089 秒单轮缩短 8.66443%，
对最初 2298 秒缩短 16.97128%。源码/tg-xtask 均复用，全部编译单元清单及
最终程序与两个 full-build-rcsc 基线和 event-driven-preemption 全量 cmp 一致。
原始 6/6 SHA 和 kernel 两项 SHA 通过，fsck 只读退出 0，未做修复。
新完整采样 prebuild_ns=1908172087152，dropped/skipped 均 0；完整 CPU、
mutex、ext4 hold 图已渲染并实际查看。

结束盘已从固定工作路径移入本 candidate 的只读 rootfs.img，完整 SHA 为
`3b60eefee6560f0e09a3698b7b91339f22a082ba2af98a546e19d93986b8d7ec`。
固定工作盘路径目前不存在；下一轮仍须复制原冻结冷盘，不得写此结束盘。
导出曾遇到 guest-root 属主无法在宿主复刻的 rdump 警告，以及一次镜像文件名
拼错；原文已展示。修正后的内容由上述全量 SHA/cmp 验收，不能仅看 debugfs
的 0 返回码。完整记录见 batched-mapping-retirement.md。

新 CPU 共 147934 样本、66408 活跃（44.89029%）。最大内核叶子为
try_zero_page 3495（5.26292%），map_range_recursive 2919，spin_release
2591，memcpy 2103，find_free_area 2014，clone_map 1412，flush_batch 1219。
原 unmap 全调用链样本按相同字符串口径为 5672 → 3090，不能只用旧函数消失
计算收益。新缺页包含 12422，ext4 包含 1760。mutex 等待 2641.041 秒（缺页
1074.239 秒），ext4 等待 1447.399 秒、持锁 543.123 秒（lookup 41.07827%）。
CPU 占用仍低，不能把这轮加速写成解决低占用。

zero-hotspot.asm 与 zero-hotspot-pcs.txt 确认 3433/3495（98.22604%）
样本落在实际 DC ZVA 指令，不是 DAIFClr 归属偏差。3463 个清零样本走用户
缺页，其中 2615 个经过持 aspace 锁的 populate，848 个走私有文件锁外准备。
待重构整个私有缺页计划/资源所有权/重检提交边界，覆盖匿名清零和 resident
COW 复制，并审查文件读取后的初始化区间，避免无谓清零。仅扩展现有私有文件
解锁曾未显收益，不能重复把“解锁”本身当验收。新设计/生产实现尚未写入，
下一次运行仍须完整重构后重新三轮静态检查。

## 2026-09-11 18:34：私有缺页整轮重构完成，最终静态检查中

上节“尚未实现”已过时。设计/证据为 `private-fault-transactions.md`。
匿名清零、resident COW 复制已纳入现有 snapshot/prepare/commit；计数
source pin 与 PreparedFrame RAII 负责锁外所有权；文件读取只补齐未初始化
前缀/尾部。clone 与 frame 模块已拆分。generic remap 为 clear→batch
失效→make→batch 完成，protect 同样使用带 PTE 发布屏障的完成边界。

完整审查发现的两个必要前置修复也已纳入：ax-io BorrowedCursor 每次
读取返回增量而非累计长度；AddrSpace::write 必须在同一独占访问内逐页
拆分 COW 再写物理页。保留调用者的 ptrace/loader 强制写授权，缺失页面
仍报错。新 kernel_copy fixture 覆盖只读、跨页、NoMemory 和部分写，
interleavings 覆盖临时 pin、fork、unmap、替换、权限撤销和重复提交。

四个根因均已有合规修复前 RED 原文：锁仍被持有、remap 先 make、cursor
累计进度、内核写污染子页；另加 protect 缺少 publication completion 的
RED。中间 Starry 矩阵 110/110 不算最终结果。新测试 clippy 一次因错误类型
不支持 PartialEq 退出 101，已仅改精确 variant 断言并复查 rc=0。

当前第一轮完整源码审查已记录，第二轮执行中：Starry 最终矩阵 session
58446，components 64420，实际 AArch64 SMP=8 axtest clippy 22019；
泛型 fixture clippy 82519，ax-io fixture clippy 已退出 0。日志统一位于
`tmp/preemption-refactor.5NL1NQ/private-fault-*-complete-*.log`。
必须等全部退出 0，补实际 axtest/profile 只构建与第三轮完整产物审查，
然后才能运行整轮 GREEN、8c8g ktest 及新全程 profiling。尚无本轮性能结果。
没有 QEMU，没有新工作盘，1908 秒的只读结束盘与冷基底均未改写。

## 2026-09-11 18:42：私有缺页重构通过门槛，全程 profiling 已启动

最终三轮静态与运行证据详见 private-fault-transactions.md。Starry
clippy 110/110、组件 4/4、实际 A64 SMP=8 axtest 与两个 fixture clippy、
正式 profile/axtest build、最终 ELF/源码 compare 均通过。之后 Starry
ktest 53/53、ax-io 183/183；泛型 fixture 一个非 present query 的错误
预期已纠正，重新三轮静态后全 crate 79/79（retirement 10/10）通过，
没有为此修改生产代码或重跑未变内核。所有失败原文和纠正均留档。

当前运行目录 `target/profiling/arceos-helloworld/starry/private-fault-transactions`。
QEMU PID **973649**，PTY session **94360**，已进入 shell 并执行
`/bin/sh /opt/starry-macos-run.sh`。只用冻结 patched QEMU、8c8g、GICv3、
MTTCG、原 NVMe 参数，无 timeout 或 compile-count 截断。
新工作盘为固定 rootfs-aarch64-arceos-helloworld-profile.img；冷复制全量
匹配基底后只替换 runner，最终 cmp/只读 fsck 通过，启动前 SHA
`580a364316710131063a306d6427d43a6ae2220e77133c92cce946903c9b6674`。
第一次复制继承只读权限，debugfs 失败后仅 chmod 新工作副本重做成功；
第一次不正确时点的 hash 不作证据，使用 preboot-disk-verified.sha256。

冻结 ELF `b1a9578994113614a2a21c366312d5bce7a1a6351b460583ce1c0975a8c3bd6c`；
BIN `fa0518cf47c2953f740857ea4544da8167ef7e74da8f6b50a9e8e0da1e982de9`；
最终全源码 archive `private-fault-validated-sources.tar.gz` 的 SHA
`989ab30ff3c1359855b0a50794e54537e6ff31e4c02b5e3eb3ca3ecb33d5427b`。
不要使用此目录内较早的 private-fault-sources.tar.gz 作为最终 fixture
版本；两者生产代码相同，差别仅 host 回归断言。旧 live-task 检查脚本
绑定另一 ELF，不能照用。其余任务编译/测试/hash session 均已关闭。

测量期间不启动其它 QEMU、编译或整盘 hash，不修改源码、不调试暂停。
完成后确认自然退出、导出全部原始产物与最终程序、6/6 SHA、编译单元
和 ELF 与 Linux/Starry 基线全量 cmp、只读 fsck、完整图渲染及实际查看。
然后对比 1908 秒最新 Starry 与 735 秒 Linux，并按新图继续优化。
当前尚无本轮性能结论。

## 2026-09-11 18:46：首次仅串口部分回显，冷启动重试中

上节 PID 973649 / session 94360 已退出 1：未出现 BEGIN，也没有启动
编译。输入只回显到 `/bin/sh /opt/starry-macos-ru`，Ctrl-U 无响应，
随后 Ctrl-C 被宿主 tty 变为 SIGINT，QEMU/Python 完整原文已显示。
相同现象历史已有，根因未确认，不归因于新缺页路径，不作为性能结果。
整目录已保留为 `private-fault-console-stall`，其 rootfs.img 只读、
只读 fsck rc=0。旧启动盘 SHA 580a… 不属于接下来的新运行。

当前重新建立 `private-fault-transactions` 目录，复用相同 b1a957…/
fa0518… ELF/BIN 与 989ab3… validated-sources。第二份冷复制/hash
session 75044 正在执行；之后必须重新 patch 正式 runner、dump/cmp、
fsck 和启动前完整 hash，再开机分段输入。所有生产源码与验证结论不变。

## 2026-09-11 18:49：冷盘重试分段输入成功

当前 QEMU PID **974802**，PTY session **12523**，仍为
`starry/private-fault-transactions` 目录。冷盘 copy/hash 75044 和最终 hash
65740 均退出 0；runner dump/cmp、只读 fsck 通过。当前实际启动盘 SHA
`18643d9c60627c3288be82e29c98b00f663c13b672e7e2504afea5595fcd4d53`。
已逐段 `/bin/sh `、`/opt/starry`、`-macos-run.sh` 核对回显后发送换行，
脚本开始执行。等待 BEGIN；不要重新发送命令。
本次 `STARRY_PROFILE_DEBUG=1` 仅创建 run 内 Unix gdb.sock，没有连接
调试器、没有暂停 vCPU。仍禁止正式测量中连接/中断、其它编译/QEMU/hash。

本次随后已出现正式 BEGIN、`enabled=true phase=1`，8 核/affinity 0–7，
源码及 tg-xtask reuse 均 true，两个源码身份哈希与基线相同。首次进度
elapsed=62、compile_units=8、distinct_crates=8。PID 974802 / session
12523 是唯一在运行的本任务进程；保持监控直到完整构建自然结束。

## 2026-09-11 19:32：1869 秒完整测量已验收，继续缓存读取边界重构

上节运行已自然结束：QEMU PID 974802 / session 12523 rc=0，完整编译
1869 秒、181/175、workload rc=0。所有本任务进程/session 已关闭，没有
正在运行的 QEMU。6/6 原始 SHA、2/2 kernel SHA、所有编译单元与最终
ELF 全量 Linux/Starry 对照、只读 fsck 全通过。完整三个 PNG 已实际查看。
结束盘移至 run/rootfs.img 并只读，SHA
5520a4ef86905350ba7998f4bdf7f861414c4e047b4aa855e31d71880ad4a31c；
固定工作盘路径已空。完整结果与失败保留见 private-fault-transactions.md。

本轮只快 2.04403%，active 45.40550%，低 CPU 仍未解决。新最大内核叶子
memcpy 3167（4.81175% active），其中私有文件缺页读取 2618（82.66498%）。
cache/mod.rs::read_at 先复制到额外 scratch PageCache，再复制到最终
BorrowedCursor。正在设计整个缓存读取入口的类型/进度/锁边界重构，去除
内核目标不需要的 scratch 页与第二次复制，同时保留任意用户 Writer 的
锁外调用。尚未改下一轮生产代码，未启动下一轮测试。必须完成设计、必要
确定性回归与完整重构，重新三轮静态检查之后才允许测试和全程新冷盘运行。
不改变当前冻结工作负载、源码/tg-xtask、QEMU 或 8c8g 配置。

## 2026-09-11 19:44：缓存读取整轮已实现，最终矩阵接近完成

设计/证据为 cached-read-destinations.md（design/profiling 各一份）。
整个 CachedRead 遍历已拆出；内核 read_buf_at 无 scratch，用户 read_at
仍锁外 write_all；FileBackend 与 Cow 单页/批量两个调用点都接入。
8 个新真实 host case 已写入并通过 clippy，但尚未运行。生产源码从开始
最终矩阵后未再改动。第一轮完整源码检查完成；第二轮 host clippy 0、
fs 矩阵 8/8、profile/axtest 只构建均 0；Starry 110 项矩阵 session
48759 尚在运行。第三轮已准备源码 tar compare、实际 ELF/调用链审查，
等矩阵退出并复查最终一致性后才能启动测试。

当前仅剩 session 48759（矩阵）、55185（最终冷盘 hash）以及可关闭的
71641（fmt check）；没有 QEMU。正式新目录 starry/cached-read-destinations
已冻结 kernel/源码，SHA 详见本轮报告。固定工作盘已从冷基底复制并全量
匹配，patch 正式 runner、dump/cmp、只读 fsck 均通过。其它构建/copy
session 已关闭。下一步完成最终三轮门槛，跑 ax-fs-ng 完整 host suite 与
Starry 8c8g ktest，通过后用冻结 patched QEMU 开始完整新冷编译，不截断。

## 2026-09-11 19:46：三轮检查与回归均通过，启动缓存读取全程测量

Starry 矩阵已实际退出 0、110/110；最后一次源码 compare、fmt/diff、
kernel 2/2 SHA 都通过。之后 fs host **231/231**、Starry **53/53**，
两个 session 已关闭。新盘 SHA 74dfd30ff127c93570e70cb770253566a40615f218e2794d619524d2818f3059；
patched QEMU SHA 不变。即将用 run-profile-qemu.py 启动
starry/cached-read-destinations，必须分段输入脚本命令并核对回显再换行。
所有本轮构建/check/hash 已结束。进入正式采样后不再编译、hash 整盘、
调试暂停或改生产源码。继续直至完整编译自然结束。

当前唯一正式 QEMU PID **994056**，PTY session **71077**，19:46:42 启动。
三个命令片段已分别核对完整回显后发送换行，runner 正在做启动校验；
不要重复发送命令。与上一轮同样 STARRY_PROFILE_DEBUG=1，仅有未连接的
Unix gdb.sock，没有调试暂停。等正式 BEGIN 后持续观察编译进度，完成后
导出原始六项/最终 ELF，全量 SHA/name-version/cmp、只读 fsck 和全程图。

本次已出现完整 BEGIN、`enabled=true phase=1`，logical_cpus=8、affinity
0–7、source_reused/tg_xtask_reused=true，两个源码身份 SHA 均与冻结基线
相同。保持唯一 session 71077 / PID 994056；不是只停在 shell 的失败启动。

19:51 最新完整进度：elapsed=187，compile_units=23，distinct_crates=22。
此前 elapsed=62/123 时为 8/13 个单元，正常推进。继续监控，不启动新的
实验或中断采样。完整编译结束之后再处理结束盘/导出/图和下一轮热点。

## 2026-09-11 20:21 后：1974 秒全程结束，分析下一轮页表安装边界

QEMU PID 994056/session 71077 已自然退出 0；renderer 52716、结束盘 hash
70400 均退出 0。目前无本任务 QEMU/编译/hash。完整 1974s、181/175、rc=0，
比 1869s 慢 5.61798%，不能验收为加速。全部原始六项 SHA、完整 name/version
跨四轮 Starry 和 Linux、最终 ELF 全量 cmp、只读 fsck 均通过。结束盘只读
保留 run/rootfs.img，SHA 2e78d36c631a060f151d8ac0653a718d86e0619ce5b586473d0941ee82c6de6b。
固定工作盘已不存在；全部 SVG/folded/summary 与三个实际查看的 PNG 就绪。

详细表见 cached-read-destinations.md。最大 CPU 内核叶子 map_range_recursive
2802/64229（4.36252%），2727 来自 PrivateFault::install_missing；copy 减少，
CPU active 降至 41.90989%，ext4 持锁写入/等待显著增大。现有缓存读取源码
是未验收候选，已验收比较点仍为 1869s；保留所有候选证据，不能冒充成功。
正在审查单页页表安装的完整遍历路径及 Linux/MOSS 对应实现，尚未修改下一轮
生产代码。重构后重新三轮静态才允许测试；仍为冻结工作负载、patched QEMU、
8c8g 全程编译，不提交、不推送、不使用 xtask perf。

## 2026-09-11 20:55 后：缺页发布整轮已实现，补齐单页撤销完成边界

新设计 docs/design/absent-page-publication.md，新报告
docs/profiling/absent-page-publication.md。1974s 全程原始 PC 全量符号化：
map_range_recursive 的 2749/2802 落在 TLBI 后 DSB，不能误认页表递归计算。
新 install.rs 离线创建子树/RAII 回滚；publish_new_mapping 区分新页发布
与旧映射撤销；Starry PrivateFault::install_missing 真实接入。第三轮源码
核查发现旧 unmap_page 相邻页存在时绕过前置屏障，先完成三轮宿主 RED
检查后运行真实表用例，确实 rc101（Invalidate 而非 Retire，原文已展开）。
RED 源码/ELF/hash 和全部失败保存在 tmp/preemption-refactor.5NL1NQ/。
随后 unmap_page 已改用 unmap_owned，同一用例尚未 GREEN。最终静态轮正在
重跑：Starry 110 矩阵 session59238、generic97147、ax-cpu46997、正式
profile build32957；host fixture clippy 已0但保留旧 mock warnings。
没有 QEMU；最终三轮未全部完成，不允许先启动运行测试。冷盘尚未新复制。
后续关闭这些 session、完成 profile/axtest 构建与最终源码/机器码校验，
再 host 全 suite / Starry 53、全程新冷盘编译。保持所有既有 dirty 改动。

## 2026-09-11 20:59：缺页发布三轮与回归已通过，即将完整测量

最终 Starry110/110 于20:58:35 退出0；generic2/2、ax-cpu33/33、正式
profile/axtest 构建均0，最终源码tar compare、fmt/diff与实际机器码核查
完成。其后 generic host90/90含旧RED同一用例GREEN，Starry8c8g53/53，
ktest session5785已退出0。所有矩阵/hash/build/回归session已关闭。
新正式目录 starry/absent-page-publication 已冻结源码与ELF/BIN（本轮
报告记录完整hash），固定工作盘新冷基底全量SHA匹配，runner替换后
dump/cmp与只读fsck0；预启动盘03bd3db8a4ef5ab31f5038d6732c76c899e9fb3c8135fbaebf3f4a049fc84a56。
即将启动run-profile-qemu.py，分段命令确认回显再回车；正式运行后不得
继续构建/改源/全盘hash/GDB暂停，保持8c8g、原patchedQEMU直到自然结束。

唯一正式 QEMU PID1022005，PTY session37958，21:00:21启动。已看到shell，
分三段输入/bin/sh /opt/starry-macos-run.sh，每段回显完整后才发送换行。
当前runner已启动，不要重复发送命令。STARRY_PROFILE_DEBUG=1只创建未连接
gdb.sock，不暂停采样。后续等待完整BEGIN，定期读PTY/日志至自然退出，
再导出六项、finalELF、全量181名单/SHA/cmp/只读fsck及火焰图分析。

## 2026-09-11 21:30 后：缺页发布全程完成，1694 秒新实测比较点

QEMU PID1022005 / session37958 已自然退出0；renderer66305、结束盘
hash68174 已退出0，所有本任务构建/测试/QEMU/hash会话均已关闭。
完整181/175、workload rc0、1694秒；比1869秒缩短9.36330%，比未验收
候选1974秒缩短14.18440%。全部六项原始SHA、完整名单对六份对照、最终
ELF对Linux及两个Starry对照均通过；只读fsck0，ELF/BIN结束后再核2/2。
所有SVG已渲染，CPU-active、mutex-wait、ext4-lock-hold PNG均已查看。
结束盘保存在starry/absent-page-publication/rootfs.img，0444，SHA
0ba106ab5304f825ae1845e15f6c8b0d6a1a24faa74063112de10a7b91f351c7；固定
工作盘路径不存在。下一轮必须重新复制冷基底，不得重用这个已经编译过的盘。

完整报告docs/profiling/absent-page-publication.md已更新。CPU活跃
56992/131441=43.35938%，idle74449；低占用没解决。map_range_recursive
叶样本59（之前2802），install_missing91/newinstall22/install_entry24；
新最大内核叶try_zero_page2691，其中2649位于0xffffffff8037f614的DC ZVA，
不是IRQ恢复误归因。2654为用户缺页准备清零、34为锁内population、3内联。
下一步读Linux/MOSS及当前分配所有权，针对初始化策略确定下一轮完整边界；
尚未实现下一轮代码。不要继续假设ext4是最大CPU叶，或声称低占用已解决。

rdump导出发生root文件chown权限警告，原文完整显示，内容6/6SHA通过，
没有提权、更改宿主权限或修复原盘。ps在QEMU退出后返回1是PID不存在，
执行会话已独立确认0。报告更新首次apply_patch上下文不匹配失败且未改文件，
原文已展示；按实际行重试成功。保留这些失败记录，不伪装首次即通过。

## 2026-09-11 22:04：匿名零页整体重构，三轮静态完成，即将运行回归

新设计 docs/design/anonymous-zero-backing.md，新报告
docs/profiling/anonymous-zero-backing.md。完整重构已覆盖缺页/锁内population/
kernel-write/fork/rollback/unmap/protect/move/RSS，7组确定性交错内核场景和
test-anonymous-zero-backing 系统用例已写完。三轮最终静态全部通过：
110严格矩阵session53234退出0、实际axtest严格clippy93866退出0，正式build
2879和axtest build62894均0。实际axtest ELF54描述符；fmt/diff/tar compare/
正式ELF/BIN两项hash0。完整hash和机器码证据见本轮报告。
尚未启动任何运行测试；无QEMU，固定正式工作盘尚不存在，新冷盘尚未复制。
当前候选formal目录starry/anonymous-zero-backing只保存ELF/BIN/源码archive。
下一步8c8g内核54项、Starry系统case、Linux同程序QEMU验证，再完整冷盘181项。
实际test runner会重写managed rootfs路径，不要因为旧默认busybox.img是目录
就先改TOML；沿storage实际解析路径核对。旧1694结束盘仍0444完整保留。

## 2026-09-11 22:10：匿名零页回归全部通过，完整采样启动

ktest session99495退出0，54/54；Starry系统case session34795退出0，选中唯一
test-anonymous-zero-backing真实运行通过。系统C二进制从保留cache导出，SHA
e945e3fbf6c18e27f0e823b9cd3db406e035cbcb26677def95aadf1902b4ffa1，Linux
同文件在8c8g上PASS/rc0/guestSHA一致。Linux85034已remount-ro并poweroff退出0，
之后只读fsck0，rootfs.img已0444。所有构建/测试/hash会话均结束。
旧busybox目录完整保存到tmp/axbuild/rootfs/rootfs-aarch64-busybox.legacy-directory；
新当前工具生成regularfile。overlay工具4条预先rm不存在目标的原始debugfs诊断
已完整展示并查代码原因，运行成功；不是隐去失败。几次rg无文件由尚未构建/
runner自动清理解释，二进制实际从cache成功导出，非重复运行。

新正式盘冷复制SHA996800...匹配冻结基底，只更新runner，回读cmp/fsck0，
启动前盘SHA42457f76d70f7ec49a393530720bc4b6f65dfcf3587462bb1d72b58ac73e17be。
正式source tar compare和ELF/BIN2/2再核通过；完整信息见本轮报告。
唯一正式QEMU PID1045638，PTY session15990，22:09:36启动；已见shell，命令
/bin/sh /opt/starry-macos-run.sh分三段回显完整后发送回车。不要再发送命令！
接下来等待完整BEGIN和181项自然退出，不能中途构建/改源码/盘hash/GDB连接。
gdb.sock仅待机未连接。继续每分钟观察，结束后导出/六SHA/全量名单/最终ELF/
只读fsck/所有火焰图并看PNG，再比较1694和Linux735。绝不能把尚未完成这一轮
称作性能收益。rootfs仍固定工作路径，当前禁止离线访问它。

## 2026-09-11 22:46：匿名零页全程1676秒，完整证据已收尾

QEMU1045638/session15990自然退出0；完整181/175，workload0，1676s。
renderer55840、结束盘hash21647退出0，所有本任务会话已关闭，无QEMU。
原始22852records，prebuild_ns1676515593328，dropped/skipped全0。
六SHA0，完整181名单对7个旧对照cmp0，finalELF对Linux/1694/1869均cmp0
（2792c352...）；最终内核ELF/BIN2/2再核0；只读fsck0。直接逐文件dump无
rdump权限错误。新ended-rootfs已移到starry/anonymous-zero-backing/rootfs.img，
0444，SHA8729f23cb4eb0e32ffefa74fb4c87d190847b8feb672714d0e663d746399f281。
固定工作盘不存在；下一轮重新冷复制996800...基底。所有SVG渲染0，三个
PNG用tmp/profile-svg-env/bin/cairosvg渲染并已view_image查看。

本轮完整报告docs/profiling/anonymous-zero-backing.md及full-build.md已更新。
18s/1.06257%小差异不构成稳定收益证明。新零页重构仍留工作树为已验证候选，
不当独立accepted优化；旧1694仍是较强此前对照。Linux735，当前比2.28027。
CPU56874/130028=43.73981%，idle73154；最大内核叶try_zero_page2847
（5.00580%，2790个exactPC80381614DCZVA，52exit、5entry），主要匿名
首次写准备2727、userfault不含preparecaller88、锁内population32。零页读
分支没有clear，fileprepare用uninitialized，不可误判为文件页重复清零。
memcpy2563，2097parentCachedRead::read_page，2037完整privatefilefault链，
2397exactPC80541f10。整条handle_user_page_fault8743=15.37258%。
mutex wait2434642484224ns，最大read_buf_at771263158752=31.67870%；
ext4hold486435335584ns，lookup222462743072=45.73326%，write129163626400。
share_mapping1318中1316exactPC80237c64是80237c60开中断后的branch，
不是refcount热点。不要针对归因边界做无效重构。

后处理最初Python摘要命令SyntaxError（漏花括号）原文已展开，修正成功。
rsvg-convert不存在、systemPython无cairosvg，找到既有profile-svg-env后3PNG0；
rg旧日志无匹配返回1明确记录。没有隐藏这些失败。当前正在根据匿名写清零与
私有文件缺页/缓存等待确定下一轮完整准备/所有权边界，尚未写下一轮代码。
仍不允许提交/推送项目；不要回退巨大既有dirty worktree或启动实板。

## 2026-09-11 23:18：缓存填充整体重构完成，新一轮全程启动

新实现/设计/证据见 `docs/design/concurrent-cache-fill.md` 与
`docs/profiling/concurrent-cache-fill.md`。1676原始完整wait精确到
read_page 803d88f4的I/O锁422386610400ns，以及803d8800的索引锁
344292109856ns（各为主导完整stack，不是全事件总量）。匿名首次写清零
仍必须保留；这一轮针对已确认的文件缺页串行等待，不再猜ext4是CPU最大叶。

实现PendingFills有界16×32页事务、TaskWaiters合并等待、writer单向作废旧读、
无I/O/索引锁准备、重新拿I/O锁校验发布；单页加载与淘汰回调/写回也移出
索引锁。OOM/失败/取消归还临时页与登记；容量等待不继承无关I/O错误。
新的optional-copy-count保持截断EOF没有零进度循环。整个实现完成后三轮
静态检查：所有权全链审查；ax-fs-ng8/8 strict矩阵与host lib/tests strict0、
真实AArch64profile build0；fmt/diff/source-tar compare0、ELF ET_DYN无TLS、
系统case discovery与8c8g核对。之后才启动任何runtime。

host242/242（含11新测试）0。QEMU test-concurrent-mmap0（12断言，tmpfs），
syscall-test-pagecache-cap0（1400磁盘文件，9341KiB增量，27.567s）。两次
overlay各4条rm不存在目标原文已显示，既有注入器行为；无regex放宽。
中途clippy2写法、旧bool断言/原子方法改名、base cfg导出遗漏均修正，
最终所有检查0。原文在工具输出，日志在tmp/cache-fill.dbJ0bd。

候选已保存starry/concurrent-cache-fill：ELF cc8b3d57a1f6b9e156c1a4a18bd7ca8c553c313b8f4fe067f05e26d6052255f8，
BIN34564fdd986e2e052e109e8855852c24e427359995dbed07a3c63b0c95239d10，
source f38ff793c8bca0ca20c8d0fc5660b79af1e1dd3544cec921f1e0740f2b64a01b。
新固定working disk由996800...基底冷复制全SHA匹配，仅注入冻结runner，
回读cmp与两次只读fsck0；prepared SHA58ae796fc12f6ff1d3f6707604b7e66c481eba9c6db73dd9a15bcc7f4c727c04。
所有测试、构建、hash会话已结束。正式QEMU PTY11103于23:18:36启动，
8c8g/CortexA53/GIC3/MTTCG/NVMe与前轮一致，gdb.sock待机未连接。
下一步只发送一次 `/bin/sh /opt/starry-macos-run.sh`（三段核对回显），等待
完整181/175自然退出；测量期间禁止构建/源文件修改/盘hash/GDB连接。
结束后用tmp/cache-fill.dbJ0bd/export.debugfs逐文件导出，六SHA、完整单位
名单、最终ELF cmp、只读fsck、SVG/实际PNG查看全部完成后再接受或否定收益。
不要复用结束磁盘做下一轮冷测。当前磁盘空间34GiB，不需再启动清理。

## 2026-09-11 23:59：并发填充全程1684秒，未建立收益

session11103/PID1066228自然退出0，181/175，workload0，elapsed1684。
较1676慢8秒（0.48%），不能accept为加速。六SHA0，完整181名单对七份
Starry以及Linux735名单cmp0，最终ELF对Linuxcmp0/SHA2792c352...。
只读fsck0，extent tree建议原文显示未修改。22629records，
prebuild_ns1684150932432，所有dropped/skipped0。所有SVG渲染0，三PNG
（cpu-active/mutex-wait/ext4-lock-hold）真实查看。renderer19887和hash84628
均0关闭，全部本任务会话关闭，无QEMU。结束盘已移入
starry/concurrent-cache-fill/rootfs.img并0444，SHA
d1e3a9800335099e0ed35f3610c424b7f99097a275c5eaeaee2f88f8ea2e2e00。
固定working disk不存在，下一轮必须从996800...基底新冷复制。

CPU54750/130696=41.89111%活跃，idle75946。最大内核CPU叶memcpy2380
（4.34703%），其次zero2354（4.29954%），find_free1855，spin1838，
flush1488。memcpy1832含prepare_missing，1894含CachedRead，1936完整
userfault；主导1814样本stack叶PC80544fd8、read_page调用8044f028。
zero主要2295prepare_missing，仍为匿名首次写清零，不能删除。
mutex总2411932033328，ext4wait1427056498240，hold486602683904。
caller排名read_inode388176746224、CachedRead362204680288、
read_buf_at340991056032。精确disas确认8044dc80仍page_cache索引锁
shared+b0，主导stack339069934784ns；803c612c为Inode.io锁，
主导privatefaultstack359096513056ns。旧file.io锁部分等待转移到下层，
不能按函数排名说消除了等待。ext4hold最大lookup203512351008ns。

本轮报告concurrent-cache-fill.md/full-build.md已更新。当前正核对Linux
do_read_fault/finish_fault/do_cow_fault与Starry私有文件页所有权，准备
对memcpy重复复制做下一轮完整重构，尚未修改下一轮代码。考虑read-only
file-cache backing + first-write COW，但必须解决pin、fork/retirement、
truncate、forced kernel writes、unaligned ELF/tail，不能暴露裸PA。
本地Linux HEAD980ab36ae5972c83f683b939e50c469c4947229e；memory.c
finish_fault5617、do_read_fault5840、do_cow_fault5872，filemap_fault3540。
注意fs/ax-fs-ng/.../inode/file.rs是旧未编译文件，实际mod.rs引用io.rs；
已确认不能拿旧文件解释正在运行的锁。未执行任何项目commit/push。
# 2026-09-12 private-file-cache-backing implementation checkpoint

Current candidate is implemented but has not completed final static gates;
no runtime tests or QEMU have started for it. Read
`docs/design/private-file-cache-backing.md` and
`docs/profiling/private-file-cache-backing.md` before continuing.
The current kernel clippy process is session 14501, logging
`tmp/private-cache.Lz1bM2/kernel-clippy.log`, 110 checks. Do not duplicate it.
Filesystem matrix retry passes 8/8; host C case builds without execution.
The new physical-pin/byte-guard API, counted cache frame backing, validated
read-fault publication, forced COW, fork binding and O(log VMA) invalidation
are wired. Tests are under aspace/fault_tests/cache and
test-suit/starryos/qemu/system/test-private-cache-backing. No test result or
performance acceptance exists for this candidate yet. The fixed formal
working rootfs path remains absent; use a fresh frozen-base copy only after
the complete refactor and three final static rounds, then regressions.

## 2026-09-12 00:58 private-cache static gates and first runtime retry

Supersedes the checkpoint above: kernel clippy 14501 ended 110/110 exit 0.
The complete refactor passed all three static rounds before runtime.
Actual cfg(axtest) strict clippy/build-only, fs 8/8 and host/profile strict
clippy, C11/host-CMake/AArch64-musl build, real profile consumer build,
fmt/diff/source/archive/ELF/BIN checks pass. Details and exact hashes are
in private-file-cache-backing.md. Saved formal kernel lives under
starry/private-file-cache-backing, ELF39a1aab8... / BIN57ba4ecd... .
Host fs tests pass 246/246. First kernel QEMU exits 1 due to new fixture
assuming the shared disk backend grants WRITE on the first missing-page
fault. Source proves its two-fault contract; fixture now read-faults then
write-faults and keeps every identity/isolation/WRITE assertion. No
production fix for that fixture. Three static checks repeated before retry;
current session 67653 is kernel-qemu-two-fault.log under tmp/private-cache.Lz1bM2.
No result yet. No other QEMU. Previous sessions are closed.

The fixed profile working disk NOW EXISTS: a fresh frozen-base copy,
fsck -fn 0 before injection, chmod u+w, with the static C case injected at
/root/test-private-cache-backing. It is a REGRESSION-ONLY disk, not a clean
formal performance baseline. Once no QEMU owns it, preserve/move it before
creating a fresh formal copy. tmp/private-cache.Lz1bM2/old-kernel-case holds
the old concurrent-cache-fill ELF/BIN ready for paired C regression; it has
not been booted. The first source tar preserves the failed test harness;
sources-two-fault.tar matches the corrected test and current source,
SHA090c2085... . Formal performance has NOT started, no speedup claimed.

## 2026-09-12 01:11 cache-fork conflict diagnostic corrected

Session 67653 exits 1 at ownership.rs's exact existing_paddr assertion.
Confirmed source cause: page-table-generic map_range_recursive reports the
requested PFN in its ordinary/huge conflict branches. Added two host
regressions to the existing retirement fixture; after three static rounds,
both deterministically fail with 0x600000 instead of existing 0x400000.
Fixed the two error payloads from the occupied PTE, not the rollback
algorithm or expectations. Added actual-error diagnostics to the cache
fork assertion. Post-fix three static rounds pass again (crate 2/2,
strict retirement and actual axtest clippy/build, profile consumer build,
fmt/diff/archive/55-descriptor/final ELF-BIN and actual TLBI checks).
Retirement host suite is GREEN 12/12. Logs are under tmp/private-cache.Lz1bM2.
Current session 20189 runs kernel-qemu-paging-error.log; no other QEMU.
No outcome yet. Generic_contracts strict clippy hit 10 old shared-mock
warnings; new tests use the clean retirement fixture, no allow suppression.

The saved candidate ELF/BIN are now 499181d4.../fb3d7e4a... . Matching
sources-paging-conflict.tar is 5701ae4c... . Prior unmeasured kernel copies
are preserved in tmp/private-cache.Lz1bM2/private-cache-before-paging-fix.*.
The regression-only fixed-path rootfs has passed fsck -fn 0 after injection
and debugfs dump/cmp 0 for /root/test-private-cache-backing. It still has
not been booted. Old saved-kernel C case directory has not been launched.
Formal full compilation has NOT started. Continue kernel regression,
xtask system case, old/current/Linux same-C comparison, then fresh frozen
base for full profiling. Do not use the injected/warmed disk as baseline.

## 2026-09-12 01:34 occupied-PTE ownership refactor final static gate

Supersedes all live-session statements above. Kernel 20189 exited 1 at
PROT_NONE truncate RSS (actual 1, expected 0). Added an inaccessible
cache/private fork regression before fixing production; after three static
rounds, kernel 1829 exited 1 at child file RSS (actual 0, expected 1).
Old saved kernel C run 40546 failed earlier at MADV_DONTNEED bytes 0x69
instead of 0x30, guest command 1, then sync/poweroff and QEMU 0. That run
does not prove later fork/mremap behavior. Original errors were displayed.

Current source uses query_occupied_leaf for invalidation, private clone,
rollback, RSS reconciliation and move; hardware translation is unchanged.
discard_range now handles private file COW pages too. Final tests include
13 kernel scenario groups, one new generic occupied-geometry regression,
and C inaccessible fork/mremap. Generic 2/2, retirement strict clippy,
actual cfg(axtest) strict clippy and strict C syntax/static build pass.
20297 remains the full 110-entry kernel clippy matrix; do not duplicate.
After it ends, rebuild actual axtest and profiling consumer, complete
the third static gate, then runtime tests. No QEMU currently running.

Linux independent frozen-base copy 22084 ended 0, chmod u+w and fsck -fn 0.
Its rootfs is tmp/private-cache.Lz1bM2/linux-case/rootfs.img, with matching
vmlinuz/initramfs.img; no Linux boot yet. Rootfs SHA verification session
76986 is running. Current C binary private-cache-occupied-aarch64 has not
been injected or run. Old working disk is REGRESSION-ONLY and warmed;
old-kernel-post-fsck.log exits 0. Preserve it before fresh formal base copy.
Saved candidate ELF499181.../BINfb3d7... are STALE relative to current
occupied-PTE correction and must be rebuilt/resaved before measurement.
See private-file-cache-backing.md for precise failure and validation scope.

## 2026-09-12 01:36 occupied-PTE static gates passed; QEMU running

20297 completed 110/110 exit 0 at 01:32:50 (7m13s). Rootfs hash session
76986 completed: Linux copied base is 996800... exactly. Actual axtest
build-only 5804 and profile consumer build 70009 both exit 0. Final third
gate fmt/diff/tar-d/55-descriptor/ELF-BIN/no-TLS/actual TLBI inspection passes.
Generic retirement now GREEN 13/13. Kernel QEMU session 24775 is running
kernel-occupied-qemu.log; do not duplicate. No performance run yet.

Current saved candidate ELF9d2980c8.../BIN1f7c6dae... and selected
sources-occupied.tar f1954240... match current source. Previous499181.../
fb3d7e4a... are preserved as private-cache-before-occupied.elf/bin under
tmp/private-cache.Lz1bM2. Current C binary private-cache-occupied-aarch64
SHA3eb5ab66... is injected as /root/test-private-cache-occupied on both
the independent Linux case disk and the regression-only Starry working
disk; dump/cmp0 and post-injection fsck0 on both. No Linux boot yet.
Next kernel result, xtask system C, same-binary old/current/Linux QEMU,
then preserve regression disk and fresh frozen-base full profiling.

## 2026-09-12 01:42 kernel ownership regression GREEN

24775 failed at generic assert_missing. Added track_caller and the actual
occupied leaf to the assertion (stronger, not loosened), then three static
rounds. Diagnostic 55012 failed at invalidation.rs:141: the new eviction
fixture assumed a fixed 512-page limit, but production grows the target
with file length. No eviction had been forced. Replaced that premise with
real page_cache_reclaim on a one-page fixture, once with aspace locked and
then unlocked, retaining PTE/RSS/pin/count/data checks. Updated stale test
helper comment only; production code unchanged. Three test-only static
rounds pass again, then 41526/kernel-real-reclaim-qemu.log GREEN55/55,
no fail/skip, outer0. Earlier failure logs preserved and original errors shown.

Live session44622 now runs cargo xtask starry test qemu --arch aarch64
-c qemu/test-private-cache-backing, log tmp/private-cache.Lz1bM2/system-case.log.
Do not start another QEMU until it exits. Current profile ELF9d2980c8.../
BIN1f7c6dae... unchanged; final source archive sources-real-reclaim.tar
SHA37834821... includes test-only corrections (tar-d0). Static host CMake
rebuilt latest C0; C3eb5ab66... unchanged and already injected/cmp/fsck0.
Old/current saved-kernel comparison directories old-occupied-case and
current-occupied-case are prepared under tmp/private-cache.Lz1bM2, no logs
yet; oldELFcc8b3d57/currentELF9d2980c8 verified. Linux-case also not booted.
Next finish system case, same-C old/current/Linux, then fresh formal disk
and full compilation. No performance run or accepted speedup yet.

## 2026-09-12 01:55 complete refactor functional gates passed

All previously named sessions are closed, except final prepared-disk hash
12666. Kernel55/55, generic13/13, actual xtask system1/1 pass. C3eb5ab66...
runs on old/current/Linux kernels: old fails private-file MADV_DONTNEED
(0x69 instead of0x30), Linux6.18.35 and current saved profileELF9d2980c8...
both pass rc0. All use8c8gQEMU, with explicitGICv3 for system/saved/Linux.
The package axtest8c8g uses machine virt's defaultGIC, not explicitGICv3.
Details and operational errors are in private-file-cache-backing.md.

Old occupied-case shutdown stalled; an early fsck was invalid while it
remained live, and an accidental second launch was rejected by disk write
locking. No offline write or lock bypass occurred. Verified oldPID1124668
was SIGTERM-stopped, then true offline fsck found pending journal recovery.
Keep tmp/private-cache.Lz1bM2/old-occupied-case/rootfs-stalled.img read-only;
never count the helper0 as a natural shutdown or fsck0 as a clean journal.
Linux root was remountedRO before poweroff0. Current saved profile kernel
session81208 passed, separate sync returned, separate poweroff exited0,
post-fsck0 no pendingjournal. Both ended regression disks are retained444
alongside logs (current-occupied-clean-case/rootfs.img and linux-case/rootfs.img).

The fixed formal working disk is a NEW basecopy, full pre-injectionSHA
9968004595dec398480417e210d64f9ed509801d4325d4ab22f43951983f5ea3.
Only the full runner c94b8669... is now injected, exported/cmp0, pre/post
fsck0; NO C executable injection. The base's historical600s runner comparison
was expected to differ; full diff shown and same replacement as priorfullruns
performed. Final prepared-diskSHA12666 is running; waitforitsactualexit.
11GiB hostspace remains. NoQEMU/buildcurrentlylive. SavedELF9d2980c8...,
BIN1f7c6dae..., sources-real-reclaim.tar37834821... verified, tar-d0.
target/release/starryos is the SYSTEM kernel aftercasebuild: doNOTuseit.

Next start run-profile-qemu.py on starry/private-file-cache-backing once
the hash ends, send /bin/sh /opt/starry-macos-run.sh exactlyonce afterprompt,
run fullcompile to naturalend, no10mincutoff. During measuredcompile no
sourceedits/builds/largehashes/GDB/offlinediskaccess; only serial/status.
Then truepost-exitfsck, allartifactchecks, fullflamegraphs and matchedLinux
comparison. No measuredresult or accepted speedup exists yet.

## 2026-09-12 01:56 full profile booted

Prepared-disk hash12666 ended0: e9efce5f557842eec6d60c0c9c821145b2c0370f9b664a36ce34374128c86137.
All preparation processes ended. Formal QEMU session2754 now runs
target/profiling/arceos-helloworld/starry/private-file-cache-backing.
Boot UTC17:56:19, SMP8,8GiB; exact inputs are saved in input.meta.
Do not launch another QEMU or touch its disk offline. The runner is about
to be sent once at the shell prompt. Continue this session and consult
qemu.log to confirm BEGIN/progress/end, rather than sending it again.
There is no deadline; wait for all181units and naturalrc0. No source edits,
builds, disk hashes or debugger connection during the measured interval.

## 2026-09-12 02:00 interactive cold full-profile launch

Supersedes2754: that launch omittedtty=true. write_stdin rejected closed
stdin BEFORE the runner command; no measurement began. ConfirmedPID1129212
was SIGTERM-stopped and session2754 ended0, notnaturalshutdown. Its log,
inputchecks and disk are sealed under tmp/private-cache.Lz1bM2/boot-closed-stdin.
Post-exitfsck0 reports pendingjournal; no repair/reuse. Originalerror shown.

Created ANOTHER freshbasecopy. BaseSHA996800...; only fullrunnerc94b8669...
injected, dump/cmp0, pre/postfsck0. Hashes95536and91221 bothended0.
Current preparedSHA7ffc82b322020772d7ba8d1ccb7009c1a69a023dc341a72e95f3b3df3e916535.
SavedELF/BIN/source-tar unchanged and verifiedagain;5.3GiBhostspaceleft.

LIVEsession80041 uses tty=true, bootUTC18:00:14/SMP8/8GiB. AllotherQEMUand
hashsessionsclosed. The formalrunpath remainsstarry/private-file-cache-backing;
itsqemu.log nowbelongsONLYtothisnewcoldboot. Runnerisabouttobesentonce;
inspectqemu.log forBEGIN/progressbeforeanyresubmission. Continue80041,
waitfullcompilewithoutdeadline. NOedits/builds/largehashes/GDB/offlinedisk
accessduringmeasurement. Afteractualexit: fsck, fullartifactexport/checks,
renderallflamegraphs, compareall181name/versionunitsandELFwithLinux/baseline.

## 2026-09-12 private-cache complete profile: 1323 seconds

This supersedes every live-session statement above. Session 80041 / QEMU
PID 1130900 naturally exits 0 after the guest's full-build pass at 1323 s,
181 units / 175 distinct names, command 0, tg-xtask/source reused. Build
starts 02:01:06 +08; end logs are around 02:23 +08. Post-processing is
recorded later that day. No QEMU, hash, build or renderer session remains.

Compared with concurrent-cache-fill1684 s, this single run saves361 s /
21.437%. It is1.8times the frozenLinux735 s, not a statistical OS ratio.
Do not claim repeatability or solved CPU utilization. All6 guest checksums
pass, all181 sortedname/versionunits match both baseline andLinux, and the
finalELF is byte-identical toLinux SHA2792c35275d91f57a38a847be87c4d2b333d7b88cd548bf9e06633288c7dba21.
Truepost-exitfsck0 has no pendingjournal; no repair. The ended image is
sealed444 at starry/private-file-cache-backing/rootfs.img, SHA214122d833996ce9553cc634f1055c27a0ad536726323e32a062b4cbb4443446.
The fixedworkingdiskpath is ABSENT. About5GiBhostspace remains; preserve
completed baselines and be deliberate about disposable regression copies.

Fullraw21616records, prebuild_ns1322921285344, all dropped/skipped0.
All11 nonemptySVGs rendered; cpu-active/mutex-wait/ext4-lock-hold alsoPNG
and visuallyinspected. CurrentCPU102942total/39182active/63760idle,
38.0622%active versus41.8911%baseline. memcpy2380→291 /4.3470%→0.7427%active.
CurrentlargestresolvedkernelCPUleaf find_free_area1816 /4.6348%; actual
PCs800e9e84(665)and800e9e78(528) are the inlined BTree range iteration,
not TLBI. The pinnedRust Range::last alreadycallsnext_back; spelling-only
changes cannot remove this scan. allocate_frame533of556samplesstopat
80160ef4afterIRQenable; do notclaim these are allocator/zeroing operations.

Remaininglargestmutexowner Ext4Guard1386202358880ns /69.3599%mutexwait.
Totalmutex1998565186336, pagecache1646263274496, ext4wait1387389660592,
ext4hold418947462256, blockread650210768480, blockwrite134682590272,
flush558037677440, offcpu49675659014784. Overlappingaggregates, NOTwalltime.
ext4waitcallers lookup35.0849%,set_len25.1346%,read_inode21.3452%.
ext4hold lookup33.6266%,write33.5679%,set_len11.8196%,inode_info10.1283%.

Follow-on source inspection: actual Inode implementation is inode/io.rs
and inode/directory/mod.rs, not the inactive oldfile.rs/dir.rs. lookup_locked
holds globalfs across corelookup, childmetadata,inc_ref,andentrycreation.
rsext4 owned/directory.rs lookup_child takes&mutself through parentload,
directoryscan andchildload. LocalLinuxHEAD980ab36ae5972c83f683b939e50c469c4947229e
confirmedagain; read fs/ext4/namei.c1750–1814,fs/namei.c1830–1956,
Documentation/filesystems/path-lookup.rst246–322. Linux slowlookup uses
per-directorysharedinode lock andper-nameinflightdcache coordination;
ext4lookup resolvesentry thenext4_iget. Furthernamespaceconcurrency work
must preserve inode lifetime,namespace invalidation,rename andfailurepaths;
do not just remove the globalguard. No newproductionchange for that boundary
has been made. Current private-cache candidate remains positive but its
combined correctness/optimization changes have not been isolated individually.

Full report is private-file-cache-backing.md. Last report patch context
failed atomically, originalerror shown; actualparagraphread and corrected
patch applied. Finalinput.meta includeschecksandoutcomes. No commit/push/PR.

## 2026-09-12 directory-read ownership: connected and host-validated

This supersedes the preceding statement that no directory-boundary production
change exists. Parent inode loads, selected directory/HTree/extents/legacy reads,
and child inode loads now execute through owned endpoints outside the mount
mutex. Short locked phases preserve authoritative cache/journal visibility and
validated publication. Child allocation is pinned before namespace guards are
released. Namespace read/write gates and typed inode lifetime owners are wired
into all active directory mutations; rename/rmdir conservatively exclude
topology. Writes/readdir/reap are not claimed parallelized.

Design: `docs/design/directory-read-ownership.md`. Evidence and every failure:
`docs/profiling/directory-read-ownership.md`. Three final static rounds completed
before runtime tests. Actual 8-CPU AArch64 guest-profile build succeeds. Saved
ELF `620eb21891e8a4a34a763b241371c2464a474b8ab23ae6e18ec331ec09b1b8b3`,
BIN `3d31314f812b16874ea194e40372300b99b602f9d55597ce13670547f3bc4be3`.
Full library tests: rsext4 394/394, ax-fs-ng 260/260, axfs-ng-vfs 49/49,
all terminal exit 0, no ignored/filtered cases. Test fixture corrections each
repeat scoped static gates. No production correction after the final build.

New run directory: `target/profiling/arceos-helloworld/starry/directory-read-ownership`.
It contains the saved ELF/BIN and selected refactor source `sources.tar`, SHA
`c123f2c2b26a3a7f659b38615d262032403215b03de1bc80dd2070d695064844`.
This is a selected-tree dirty snapshot, not all kernel inputs or a clean commit.
No new performance number yet. Reuse frozen base `99680045...`, full runner
`c94b8669...`, patched MTTCG `a1d9a608...`, 8c8g and the entire 181-unit build.
Use PTY and send the runner once after the guest prompt. Never xtask perf,
truncate the full run, or do source edits/builds/offline disk work during it.

Before preparing a new working disk, two successful non-performance regression
copies under `tmp/private-cache.Lz1bM2/` are being compressed to `rootfs.img.zst`.
Both archives passed full zstd validation and their raw images were removed
recoverably; approximately 12 GiB was available before the fresh base copy.
Keep their logs, kernels/test binary, all actual performance baselines, and the
failed old-occupied/closed-stdin disks. Check live tool state before continuing.

## 2026-09-12 directory-read full result: 1252 seconds; write owner next

This supersedes all preceding pending/live statements for directory-read.
QEMU session21453 / PID1202532 naturally exits0, all181units/175distinct,
guest0, reusedsource/tg-xtask. Boot17:43:37+08, actualbuild17:44:13,
passaround18:05. Continuation later collects the same completed session,
not a second run. Runner1252s, cargo20m35s, axbuild1245.94s,
kernel1252597972768ns. Singlematchedgain71s/5.3666% vs1323; Linux735s ratio1.7034.

Post-exitfsck0 no recoverywarning. All8artifactsexported,6hashespass,
full181name/versionmultisetcmp0 againstboth1323baselineandLinux,
finalELFcmp0withLinux SHA2792c35275d91f57a38a847be87c4d2b333d7b88cd548bf9e06633288c7dba21.
Sealedread-onlyendeddiskisrun/rootfs.img,
SHA4f1daf2a4b3ae722b772dd021d9b0df91330084a8412f88d2907b2d43e2ed7f7.
FixedworkingimagepathABSENT; about6.3GiBhostspaceleft. Bothsuccessful
non-performanceprivate-cache regressiondisksnowhavevalidated1.69GiBzstd
archives; rawcopiesremovedrecoverably. Failedandperformancebaselinespreserved.

All26830profilerecordsrendered,alllost/skipped0,11nonemptySVGs,3PNGviews
inspectedusingexistingtmp/profile-svg-env/bin/cairosvg. SystemPythonimport
failswithModuleNotFoundError; rawshown, no globalinstall. CurrentELF/BIN
hashesstill620eb218.../3d31314f..., selectedsourcetar-d0.
No live QEMU/hash/render/build/test session remains at this checkpoint.

CPU97312total,40189active=41.2991%,57123idle. LowCPUremainsunsolved.
Totalmutex1236373598912ns; ext4wait749497540064ns(-45.9779%),
ext4hold254545792800ns(-39.2416%);lookupinclusivehold9251557232ns
versus140877707632(-93.4329%). Newlookupsumusesallfoldedstackscontaining
InodeLifetime>::lookup, notonlyits7841691600nsimmediatecallerbucket.
Pagecache1278189824512,blockread709661952080,blockwrite148111705472,
flush339599478320,offcpu47400646979712ns. Overlapping sampled durations,
notwalltimeoradditivesavings. Read/writeeventtotalsincrease.

LargestmutexownerExt4Guard748491731856ns/60.5393%. Newlargestext4holder
write_locked148560955328ns/58.3632%, mostlyext4-commitbackgroundpagewriteback.
Samewrite_lockedchainblockwrite141231344064ns,blockreadonly163854624ns;
wholeext4-commitworkerhold148852100848ns. Readpreparation14.7227%,
set_len9.0313%. Nextmajorrefactorisregular-filedatawriteI/O ownershipoutside
mountstate, preservingordered-data/journal,dirtystate,partialblocks,inodepin,
failureandretry,shutdown. No production write-owner change yet.

LargestresolvedkernelCPUleafindependentlyfind_free_area1854/4.6132%active;
spin_release1823,flush_batch1267. Do not infer exact instructions from names.
Namespace:: offcpustackmatchis0butinlining/capturedepthmeansnozerowaitproof.
All49IllegalInstructionwarningPCcountsidenticaltobaselineknownOpenSSLprobes.

Authoritative report: directory-read-ownership.md; run/input.meta and
run/comparison.json record all outcomes. No commit/push/PR. Nextproduction
refactoragainneedscompleteimplementationthen3finalstaticroundsBEFOREtests.

## 2026-09-12 file-write ownership: production state separation underway

This supersedes the preceding statement that there are no production write
changes. The measured baseline remains 1252 s; **no new runtime or performance
test has run**, and no QEMU/build/test tool session is live at this checkpoint.

Read `docs/design/file-write-ownership.md` and
`docs/profiling/file-write-ownership.md` for the current phase. Before-edit
snapshot is `tmp/file-write-owner.VHVeZy/sources-before.tar`, SHA-256
`a4037b4f03117e64bff8fbb54aa097eea9086e3014e5b4d300bf175d3be18524`.
Current phase snapshot is `sources-phase.tar` in that directory, SHA-256
`520458ff99f7c0c0bc175d85770109b0b5c6208ac2a837bcff7c63222252c5c4`;
its `tar -df` comparison exits 0. The scope is recorded in the report.

`fs/rsext4/src/file/write/` now owns the real write entry, block data,
legacy rollback and unwritten prepare/data/completion. The old flat public
`file::write_inode_data` is retained. Production unwritten writes now flow
through separate `PreparedUnwrittenWrite` and `CompletedUnwrittenWrite` types;
both successful and failed I/O retain cleanup ownership until completion.
The current adapter STILL HOLDS THE MOUNT LOCK across data writes. Do not
claim the hotspot is removed or run another benchmark at this intermediate
stage. The remaining `file/io.rs` mixed domain also needs final boundary review.

Intermediate xtask Clippy passes 3/3 twice; after the final type change it ends
19:58:35 +0800. Combined host-test/multilevel strict Clippy, fmt/check and diff
checks all exit 0. The two final Clippy logs are in the snapshot directory.
These are intermediate compilation checks, NOT the final three static gates.
No test assertions were changed or executed. No warning suppression was added.

Continue by connecting owned physical-data plans and coherent endpoints for
all ordinary extent writes, including growth and partial blocks. Completion
must preserve concurrent current link/orphan metadata rather than publishing
its stale inode snapshot, and retain completed data across journal-progress
retries. Resolve reserved-handle/commit-gate cycles and private/held cache
coherence before switching the adapter. Keep inode lifetime/content lock and
mount admission across the off-lock phase; retain explicit legacy compatibility.
Then add deterministic production-path tests, finish the entire refactor,
repeat all three final static rounds, and only then run runtime/full profiling.

Linux local checkout `/home/wuxun/Projects/linux` is still at
`980ab36ae5972c83f683b939e50c469c4947229e`. Read `fs/ext4/page-io.c` 166–305
and `fs/jbd2/commit.c` 229–329, 764–810: failed data must not initialize extents,
truncate waits for overlapping I/O, journal submission/wait release list lock,
and data completion precedes the commit-record phase. Links are in the design.
Open #2015 remains `6d5cc09f45a073680a270ae0b6047b24fd9eaff5`; body/files and
actual write/journal diffs were read. They add small-run cache delay and mkfs
batching, not an ordinary-file detached owner. Full remaining semantic overlap
and maintainer design approval are still needed before any merge claim.

## 2026-09-12 file-write ownership connected; static and host runtime gates pass

This supersedes the earlier serialized intermediate-stage status. Ordinary
extent file data now runs outside mount-state exclusion through typed prepared /
completed owners, core inode mapping leases and adapter admission/content-lock
ownership. Growth and partial/unwritten ranges are connected; current metadata
is merged at completion; journal pressure retries metadata only. The remaining
2462-line io.rs was split into private file/io domains without algorithm changes.

Three final static rounds passed after the complete change: source invariants,
core 3/3 and adapter 8/8 Clippy plus combined features and actual 8-CPU AArch64
kernel build; fmt/diff, source archive match and independent ELF-to-BIN cmp.
Build begins 20:40:57 +0800, exit 0, release stage 14.28 s. Pinned core/memchr
future-incompatibility warning remains visible. No test ran before the gates.
New tests 15 core +7 adapter pass, then full 409+267+49=725 tests pass, all0.
No runtime failure, source fix or assertion relaxation needed.

Authoritative design/report: docs/design/file-write-ownership.md and
docs/profiling/file-write-ownership.md. Work/snapshot/log directory:
tmp/file-write-owner.VHVeZy; sources-connected.tar SHA
78f7e8207db340e087d6139234f28521711ea8a69699a6794a9a99c6c842c55d.
Saved ELF332ea2c0cfe8b06ea3dfd6ff3e4c961d05ae6635fdce7212819618b244c86a6b;
BIN9ae1825213daa5ebabb60b29ecaef9becbd029f5257a4497b9753d61921693a7.
New run directory target/profiling/arceos-helloworld/starry/file-write-ownership
contains saved kernels and sources.tar. No new performance result yet.

Disk: removed only two old archive-verification copies after cmp0 against their
retained raw originals; .zst files also remain. Exact targets and checks are in
the report. Freed about10GiB,17GiBfree before fresh base copy. Fresh working image
copy completed; base hash/fsck processes are starting at this checkpoint, not
QEMU. Check live tool sessions before continuing; do not touch the disk while
QEMU is live. Cold base is tmp/axbuild/rootfs/rootfs-profile-host-config-base.img,
SHA9968004595dec398480417e210d64f9ed509801d4325d4ab22f43951983f5ea3.
Next: verify fresh image, replace only /opt/starry-macos-run.sh, readonly fsck,
dump/cmp runner, then full8c8g profiling via saved-kernel PTY helper and one runner
invocation. No host builds/source edits during actual measured interval.

Preparation update at 20:52 +0800: first base hash overlapped runner injection;
discarded as identity evidence even though its result matched. Collected terminal
exit, confirmed no disk users, re-copied disposable working disk from frozen base,
waited for the second full base hash to finish before injecting. Valid base hash
matches 99680045...; both new fsck runs exit0, runner dump/cmp0, mode0755.
Prepared whole-disk SHA d77f5402bb355a9e78596296640599ee02a5e080ebc7c8e3cff692b502d78446.
All hashing has ended. Saved-source tar comparison, kernel/QEMU/runner hashes
and diff whitespace check pass. input.meta and preparation logs are in the new
file-write-ownership run directory. Now launch once through the existing PTY helper.

## 2026-09-12 file-write ownership full profiling passed, next read hotspot

PTY79418 / QEMU PID1232212 naturally exited0 after full1222s compilation;
181 units175 crates match baseline and Linux, six guest hashes pass, finalELF
byte-identical Linux2792c35275d91f57a38a847be87c4d2b333d7b88cd548bf9e06633288c7dba21.
Post-exit fsck0, ended disk now read-only at
target/profiling/arceos-helloworld/starry/file-write-ownership/rootfs.img,
SHA890da3b4e9778d5fb61664802e018847c63b4055941699b39837071f1759a683.
No QEMU/hash/render process remains. Input meta/logs/summary/comparison/11SVG/
3viewedPNG saved. Exact49 known IllegalInstruction PC counts match baseline.
rdump could not restore guest root ownership but all file contents verified;
two summary-analysis command errors and successful retry recorded in report.

Gain is30s/2.3962% single run vs1252s, not repeatability; Linux735s ratio1.6626.
CPUactive42.1426% vs41.2991%, low CPU unresolved. Ext4wait749497540064 ->
94474594592ns, hold254545792800 ->74929037136ns. Wholewrite_locked inclusive
hold148682596848 ->6060707520ns, ext4commitworker hold148852100848 ->
6243982112ns. Ordinary physical write ownership refactor worked.

New largest mutex owner Inode::read_at121319296256ns/38.0700%; fault ->
prepare_missing -> pin_read_page -> populate_page_window dominates visible
stack. Source inode/io.rs confirms it takes an exclusive shared-inode mutex
across all read phases. Mount-lock leaf is next93369088000ns/29.2992%;
largest ext4holder read_inode25780589040ns/34.4067%. CPUleaf find_free_area
1863/4.6587%active remains separate. Continue Linux/source investigation then
whole shared-reader/content-mutation ownership refactor, not another benchmark.
New production changes require three final static rounds before runtime again.

## 2026-09-12 shared inode readers connected, three static gates pass

New design/report: docs/design/inode-read-sharing.md,
docs/profiling/inode-read-sharing.md. Reused namespace gate at fs/access with
ReadAccess/WriteAccess, inode weak identity registry and InodeLifetime now keep
same gate. read_at shared, all seven content/metadata/xattr mutation entries
exclusive; unchanged core mappings/atime/current metadata/lifetime and cache
PendingFills protocols. Added contention profiling, also for namespace gates
(instrumentation expansion must be accounted for in future total comparisons).
No changes to historical unwired inode/dir.rs or fs/profile_tests.

Three static rounds now pass after all edits: scoped ownership audit, strict
combined features +8/8xtaskClippy21:31:37–43 +actual8CPUAArch64build21:32:12,
fmt/diff/sourcearchive and independentELF-BINcmp0. No runtime before these gates.
Saved tmp/inode-read-sharing.FNhQ6c: before archive57a7c52140589ce6075e4c4054d8d9b20f859a3376da7f1b4f58ea0adcecdcab;
connected b987319db270a6b47bf8014e0b34f66bbe0d417511bff95ccd8c4b216465444c;
ELF16fae63c2879a26a8559d4fe08062d97d6676c84346e304f71900e649fa59b46;
BIN5d22aaadab6e3819777e75f6aa1b42e3f4e78ccb9193a20524fbe294d5e121b6.
Full adapter host suite is now starting (collect existing session, do not rerun).
Expected six additional cases, including actualwait/wake, counters, sameinode
device-read nesting/errors and contention profile. No new QEMU/rootfs prepared.

Host runtime update: adapter full suite273/273,exit0,noignored/filtered,0.70s.
All six new cases pass without fixes after runtime. Log adapter-full.log saved.
Unchanged core409/VFS49 were not rerun this phase. Preparing fresh base for new
run target/profiling/arceos-helloworld/starry/inode-read-sharing, kernels and
sources.tar copied. Working disk path was absent; exact QEMU prefix pgrep1empty
(no live QEMU). Cold base sparse copy started; collect current session before
hash/injection. Do not reuse the ended1222s disk.

Preparation complete21:37 +0800: valid fullbaseSHA99680045... finished before
injection; pre/postfsck0, runnerdumpcmp0,mode0755. Prepared fullSHA
e0eee5d6666e6de81b55516a50e03237dcbd27d93d85f62e272c567a400cd40d.
All copy/hash processes ended. New input.meta/pre/postfsck/base-hash logs saved.
Next action launch saved-kernel helper once with PTY, wait prompt, invoke
/bin/sh /opt/starry-macos-run.sh once. No hostbuild/sourceedit/liveimageaccess
during measured interval. Full no-timeout181unit build, currentbaseline1222s.

Run started: QEMU PID1242991, PTY14817, boot13:38:05UTC, runner invoked once
13:41:47UTC after prompt. Guest confirms8CPUs/reusedtgxtask/source and begins
kernel profiling. Continue this existing session until full completion and
natural exit; do not launch another QEMU or touch the live image. input.meta
now records running state. No final time or hotspot result yet.

## 2026-09-12 shared-read full run verified, continue extent-read ownership

PTY14817/QEMU1242991 naturally exited0. Full1218s,181units175crates;
complete compile multiset equals1222s baseline and Linux735s, six hashes pass,
finalELF byte-identical Linux2792c352...; no source edits/hostbuilds in measurement.
Ended readonly rootfs moved to inode-read-sharing/rootfs.img, fullSHA
191af79811bbd10409b7df74a4e62212b91ad152bb7619226c928dc9ec09c51e, fsck0.
All25697records rendered (9593CPU+16104wait), dropped/skipped0,11SVG/3viewedPNG,
same49 instruction probes. Hash78967 and renderer both finished0. No live QEMU.
Input.meta/comparison.json/report updated. 4s/0.3273% single-run delta, not
significant speedup. CPU42.3271% vs42.1426%, lowCPUstillunresolved.

Inode read gate wait121319296256->668269264ns (-99.4492%). Namespace gate
instrumentation adds30699611008ns; not inode-reader waiting. Newlargestmutex
mount lock109409064896ns/40.1314%, cachepin63013796656ns/23.1136% next.
Largestmount holderread_inode32359739008ns/37.3340%, coldextentchild reads
23997756768ns visible in block-read graph. Read owned/read.rs and
file/read_plan.rs: preparation still walks extent nodes under mount state.
Existing owned/directory_read already provides snapshot+MetadataBlockRead+
short visibility callback+version validation outside mount; reuse that boundary
for full file mapping/data reads instead of merely replacing another lock.
Next production phase has not started; design/priorart and before archive first,
complete connected refactor, then three new static rounds before any runtime.

## 2026-09-13 extent-read ownership implemented and host-validated

Current report docs/profiling/extent-read-ownership.md; design counterpart.
Promoted old directory snapshot/visible-image/independent endpoint boundary to
owned/block_read.rs for files and directories. File read phases now cold inode,
independent extent/data read, final version/atime validation, lock-external copy.
One visibility mount critical section per contiguous data run. Retained inode
shared content gate, mount admission, allocation lifetime span all phases.
Existing extent parser, directory API aliases and lower PreparedFileRead API
remain; current errors typed, invalidated results/errors retry, explicit fallback.

Three static rounds passed before runtime: scoped ownership audit; strict core
and adapter combined lib+tests Clippy, xtask 11/11, actual fixed8CPU AArch64
kernel; fmt/diff/frozen source and independent ELF→BIN comparison. Artifacts
tmp/extent-read-ownership.iQN3IJ, ELF ece779487086e718d0e418d6705d0e4a1d4379046b3ccd2fe36d8d925262005a,
BIN 50992a390cfdcbfccf82c3ffa18ae73e98316484c93226191a6f7162ef1637fb.
First core run423/424 failed only journal fixture precondition (ordinary cache
flush did not journal). Corrected fixture uses flush_metadata and additionally
proves home remains old bytes. Renewed source/Clippy/fmt/diff/archive gates;
only test file changed, production and kernel unchanged. Final core424/424,
adapter275/275, publicfile_operations49/49. Logs/raw failure retained.
Final selected source tar 4e832d81224eadc0e1506d9566274197993b3dfd00b086a0ebf8ab101357c254.

Space: one old clean-cache-preread raw image losslessly compressed and full
zstd decode-tested; raw removed, recoverable .img.zst retained; report has both
SHA hashes. Net3.8GiB freed, remaining4.4GiB after fresh base copy. Other images,
logs and flamegraphs preserved. Empty unused verification temp directory removed.

New run target/profiling/arceos-helloworld/starry/extent-read-ownership has saved
ELF/BIN/sources. Freshworking disk fullbase hash99680045... finished before runner
injection. Pre/postreadonlyfsck0,0755 runnerdumpcmp0,source tarcompare0. Prepared
whole-image hash session5270 is currently running; no QEMU/build. Collect it
before creating input.meta/launch. Full8c8g NVMe QEMU, no timeout, invoke existing
runner once, no hostbuild/source edits/liveimage access during measured interval.

Prepared hash5270 finished0 with SHA26e60f3a0c5e33ab61b5bfce27121382f57fb86d54965cda77f50bd89446aa86;
saved prepared-hash.log/input.meta. QEMU launched once: PID1267869, PTY37189,
boot2026-09-12T16:41:57Z, promptobserved. Invoked /bin/sh /opt/starry-macos-run.sh
once at16:42:09Z. Continue this live session; no further build, source edit,
live rootfs access or additional runner invocation until natural completion.

Latest live observation: elapsed122s, compile_units30/distinct28. Guest confirmed
8CPUs,affinity0–7,tg_xtask_reused=true,source_reused=true; archive/fingerprint
match previous. 49instructionprobePC/FAR/count multiset exactly matches1218s
run. Source/meta/log state saved. PTY37189 and PID1267869 remain live; collect
this session, do not start a second run. No completion/performance claim yet.

## 2026-09-13 extent-read full run completed, next namespace pressure boundary

The preceding live state is superseded. QEMU PID1267869 / PTY37189 naturally
exited 0 at full1297s,181units175names. This is +79s/+6.486% versus1218s,
not an accepted speedup. Full details: extent-read-ownership.md. Saved ELF and
all26961rawrecords verified, loss0,11SVGs,3visually inspected. Compile name/version
multisets match previous and Linux; output ELF cmp0 against both. SHA256SUMS6/6,
endedreadonlyfsck0. Ended rootfs moved to that run/rootfs.img,0444,
SHAba66044a22c9326e4b5aa5d3d7848fcc8b8d500110fdbf80d7a909cbd49526a1.
Original working rootfs is absent; no QEMU/build currently active.

Read_inode mount hold improved32.360s ->5.495s, all mount hold86.676s ->68.889s,
but CPUactive42.327% ->38.944%. Largestmutexleaf AccessGate::read112.705s,
namespace lookup callers; sync_core commit-gate wait53.485s (mkdir19.154s).
Namespace mutations hold directory/topology rights through journal progress,
amplifying slow flushes (203.089s ->336.415s aggregate). Flush count decreased;
cause of slower individual flushes is not yet established. Hostfree4.2GiB.
Next connected phase separates atomic namespace attempts from pressure waits;
must revalidate names/parents and retain lifetime, mount admission, rollback,
sync/error boundaries. Existing inode content ownership is not to be released
across partial file writes. New production changes require three static rounds
before runtime. Goal remains active; no commit/push/PR or subagents authorized.

## 2026-09-13 namespace journal progress ready for full run

Connected namespace attempt/completion refactor now covers create/symlink/link/
unlink/rmdir/rename. Admission and inode references span journal progress;
namespace/mount guards do not. Fresh attempts recheck names and removed parents.
See namespace-journal-progress.md and ../design/namespace-journal-progress.md.
Old broad-guard regression deterministically failed after RED static gates;
candidate restored byte-for-byte, final three static rounds all passed, then
unchanged GREEN and all287 adapter tests passed. No QEMU started yet.
Artifacts tmp/namespace-progress.m0n7Zy contain logs/sources/ELF/BIN; saved run
target/profiling/arceos-helloworld/starry/namespace-journal-progress created.
No runtime source changes after gates. Before fresh image creation, verified
old cached-inode-metadata-window raw and existing zstd archive byte-for-byte
over all17179869184 bytes, SHA d0853a7a31eb603f646deddbc3b579088aeaf28237c651595de33d714f6b830a.
Only this unused duplicate raw may be removed; archive and all evidence remain.

Unused duplicate raw removed after full byte comparison; retained zstd SHA
0874fa030ce5b5d0fe7f0d54582e9d90c60cd74143081b994f830f1c8b711791.
Fresh working image verified against base99680045... before runner injection;
pre/postfsck0,8657byte0755runnerdumpcmp0,tg-xtaskhash e6823ab3... unchanged.
Prepared full diskSHA d14b88fd9364f6b14480a87c56ac9333fecf7a974a723e1c02925b1f6bd3caa0.
QEMU boot2026-09-12T17:42:58Z, PID1281594, PTY38598; prompt observed and
/bin/sh /opt/starry-macos-run.sh invoked once. Continue this live session until
natural completion; no hostbuild/source edits/live-image access or second run.

## 2026-09-13 namespace full run VERIFIED, next cache ownership diagnosis

The preceding live status is superseded. PID1281594/PTY38598 naturally exited0.
Full1217s,181compileunits175names; matching previous,best-prior,and Linux full
name/version multisets. ELF cmp0 against allthree, SHA2792c352... unchanged.
SHA256SUMS6/6,endedreadonlyfsck0,all27050records parsed10056CPU/16994wait,
zero dropped/skipped,phase1only,all8CPUs,summary/foldedtotalsmatch,11SVGs,
mutex-wait/ext4-lock-hold/cpu-active actually viewed.49instructionprobes match.
All evidence and recovered postprocessing errors in namespace-journal-progress.md
and target/profiling/arceos-helloworld/starry/namespace-journal-progress.
Ended rootfs.img0444 SHA7c0b5c8136108519cab71ad4e66c32243433c198a7ebbd5d5ed7412fe65e413b.
Original working rootfs path is absent. No QEMU or other command remains running.
No kernel source edit after final gates; latest all287 adapter tests pass.

Outcome:80s/6.168% below immediate1297s,only1s below1218s baseline; no meaningful
new end-to-end speedup beyond that earlier baseline. CPUactive43.418%,formerly
38.944%; AccessGate::readwait112.705s ->10.791s. Local namespace fix is proven
by RED/GREEN and wait reduction,not a blanket ext4 or wall-time attribution.

Current largest individual mutexleaf Ext4Guard::acquire65.573s; dedicated
mountwait66.528s,totalhold54.692s,createowner15.235s. Cache private-fault entries
pin_read_page50.637s +with_current_read_page28.441s=79.079s (36.38% of217.349s
mutexwait). mapping.rs takes cached io_lock then index for both; writeback.rs
holds io_lock across backing writes/sync. Existing buffered resident read hits
already use updating flag +index in update.rs. Next safe work: identify exact
sampled lock sites and investigate connected cached writeback/publication
ownership,not delete locks blindly or start another measurement without changes.
Read this turn:cache/mod.rs,read.rs,mapping.rs,update.rs,resize.rs,retirement.rs,
writeback.rs (fully). Still need other fill/populate/reclaim/page consumers,
fault caller/lifetime contracts,tests and Linux folio/writeback prior art before
design/implementation. Current largest non-user CPUleaf remainsfind_free_area
1991samples4.851%active,spin_release1678,flush_batch1278; not exclusively ext4.

Raw export note:rdump prints ownership EPERM even with exit0; contents verified.
Use individual debugfs dump for future exports. Preview Python is existing
tmp/profile-svg-env/bin/python,not default Python (missing cairosvg). Profile
integrity checker must count1phase+9event definitions in addition to header and
samples; initial checker assumption failed and original output was retained.
functions stores namespace_qemu/render/ended_hash are all terminal; do not poll
38598/89625/73126 again. Metrics/comparison/integrity and all evidence are saved.
Goal remains active; no commit/push/PR/subagents authorized. Next source edits
still require full applicable guideline reads after compaction and three static
rounds before runtime. No unsafe performance acceptance or merge claim.

## 2026-09-13 cache writeback ownership refactor validated, full run preparation

Previous diagnosis is superseded by a connected implementation. All measured
fault sites were checked against the saved ELF: 77.725440256 s on cached io_lock,
1.352327488 s on the page index. New finite writeback round, 1 MiB owned batches,
Idle/Active/Redirtied state, active-page eviction/reclaim protection, stable
index/updating-based pin and PTE publication. No cached I/O lock spans writeback
data writes or sync. See cache-writeback-ownership.md and its design document.

Candidate archive tmp/cache-writeback-ownership.m2tahG/sources-candidate.tar
SHA 00ad2b3372d8eaac5998928f4836522dbf2ebb79cb80e32a5e5182b887485827.
Original production page/mapping/populate/reclaim/writeback was restored for
RED (only test-only accessor added); after three static rounds the unchanged
real-NodeOps regression failed exactly at retained io_lock, exit 101 / 0.00 s.
Candidate restored byte-identically across all 58 archived files. Three final
static rounds passed: ownership audit, strict combined Clippy + xtask 8/8 +
actual profile kernel build, fmt/diff/source/ELF-to-BIN checks. Then unchanged
GREEN passed; full ax-fs-ng adapter tests 302/302 in 0.88 s; real QEMU 8c8g
kernel tests 55/55 exit 0, including private cache/PTE and shared writeback.
The adapter tool transcript was truncated in its middle; terminal totals are
complete and the saved log explicitly notes this. Kernel log is complete.
All commands and original development/RED errors are saved in that tmp folder.
No source change after final gates. No new performance result yet.

Profile ELF SHA 4876be49ff3026887fc789881ea130bb97318bdc80a15181dfa2a634541dd54b;
BIN SHA 2dc26f87fb567e2ceed1f99d71df80b6ab0c13c9f8d081a60a761e5e21d0a6d8.
Both saved under target/profiling/arceos-helloworld/starry/cache-writeback-ownership
with sources-candidate.tar. Kernel correctness test used a separate test ELF.

Freed 5.5 GiB by deleting only old duplicate bitmap-writeback-window raw after
all 17179869184 bytes matched its existing zstd archive and fuser proved unused.
Retained archive SHA eb23780c669fc662a2380e6571d9a10e7fbd81ff62e6ff1bdce5bd3457428262;
raw SHA 20b9760c3c1cc0204f874d03184612273541aa722269684ae90372d3db031669.
Python 3.14 compression.zstd is available for streaming comparison without shell
pipes or temporary decompression. See cleanup.log; backup remains 0444.

Fresh working rootfs copy now exists at the normal profile path. Copy finished;
full base-copy SHA verification was started in PTY 78197. No runner injection
or new profile QEMU has started yet. Finish the hash, then pre-fsck, inject the
8657-byte full-build runner, verify guest tg-xtask and runner, post-fsck and
prepared disk hash. Then use saved-kernel run-profile-qemu.py for full 181 units.
Only ended QEMU permits rootfs inspection/export. Do not rebuild host code or
edit runtime source during measurement. Goal remains active; no PR or delegation.

### Cache writeback ownership full profile now LIVE

Preparation completed: base-copy hash 9968004595dec398480417e210d64f9ed509801d4325d4ab22f43951983f5ea3;
pre/post readonly fsck 0; runner 8657 bytes / 0755 / dump cmp 0;
guest tg-xtask SHA e6823ab3b6a6d944266fc31d5e54814f463466f88949849ba139b6c575f7e6f1.
Prepared disk SHA a047e66c42778a293062996e1b7af9af9c694f4bccec06aa8752d7aa7a899799,
hash finished before boot. No host Cargo/other QEMU active at boot.
QEMU boot 2026-09-12T18:44:55Z, PID 1294228, active PTY session 49059.
Observed shell prompt and invoked /bin/sh /opt/starry-macos-run.sh once.
Continue this session until natural completion; do NOT invoke the runner twice,
build on host, edit runtime source, touch the live image or launch a second QEMU.
Saved log: target/profiling/arceos-helloworld/starry/cache-writeback-ownership/qemu.log.
The old sessions 78197/73939/67591/47358/42724 are terminal; do not poll them.
functions store cache_writeback_qemu contains active command/output chunks.
Post-exit: readonly fsck, individual debugfs dumps, artifact hashes, render all
kernel records against saved ELF, view flame graphs, compare all 181 compile
units and output bytes with previous full Starry and Linux. No speedup claimed yet.

Guest confirmed logical_cpus=8, affinity=0-7, tg_xtask_reused=true and
source_reused=true, exact source archive/fingerprint unchanged. Backend is
starry-kernel-fp, sample_hz=10, phase=1; initial loss counters zero.
All 49 startup instruction-probe ip/far/kind/esr/ec/iss signatures and counts
already match previous namespace full run exactly; this is not a new failure.
Runner invocation is bounded by observed UTC 18:45:10..18:45:59; an exact
invocation timestamp was not captured and is not invented in input.meta.

### Cache writeback full run TERMINAL and verified; VMA gap-index work starts

All LIVE statements above are superseded. QEMU PID 1294228 / PTY 49059 exited 0;
1253 seconds, 181 units / 175 names, build_completed=true, workload rc 0. The
run is 36 seconds / 2.958% slower than immediate 1217, not accepted as a build
speedup. All 26911 profile records parsed, phase 1, all 8 CPUs, no drops/skips;
all folded/summary totals verified. 11 SVGs rendered and 4 key graphs viewed.
49 startup probes and all compile multisets match previous Starry and Linux.
6/6 guest hashes, ended fsck 0, output byte-identical with previous and Linux.
Ended rootfs is run/rootfs.img0444, SHA
524b980721c5bd0312649625fca4bea1f5764ec5aa7828b7f310a7c35f5316c2.
Former working image is absent. No measured run or hash/render session remains.
All 58 cache candidate files unchanged after execution; no RED code remains.

Cache pin/publication mutex leaves 79.079s ->20.391s (74.21% lower); all mutex
217.349s ->157.914s. Largest individual mutexleaf ext4guard72.349s, dedicated
mountwait72.922s, createowner17.889s/31.14%hold. Blockread689.171s; data-read
plan450.827s. Do not add overlapping durations or attribute the 36s wall loss
without critical-path/repeat evidence. Full cache report and run input.meta
now record completed_verified, hashes, negative wall result and limitations.

Next connected experiment is private VMA gap indexing. Current saved ELF has
find_free_area1979 CPU samples (4.954%active),1976 inside successor/gap scan;
PCs are saved in run/free-area-pcs.log. Pinned BTreeMap last is already next_back.
Linux980ab36ae5972c83f683b939e50c469c4947229e mm/vma.c and Maple Tree source/docs
were inspected. Design docs/design/vma-gap-index.md written before implementation.
Original memory_set plus manifests/placement source archived at
tmp/vma-gap-index.6UocCb/sources-before.tar SHA
2e500b64fd1edc0e072a5600e5dd340317c33f62c689006d9a5fc7ec916af659.
No VMA candidate tests or builds have started. Complete refactor and three static
rounds must precede runtime. Last free disk 4.4GiB; need recoverable duplicate
image cleanup before another fresh full-build disk. Goal remains active.

### VMA gap-index refactor and corrected verification COMPLETE; full-run disk preparing

Implemented private AVL finite-gap index + implicit tail; complete map/grow/
unmap/metadata-only-unmap/clear coverage maintenance and partial-error guard.
set.rs moved to set/mod.rs + mutation.rs; public API unchanged except explicit
invalid request rejection and enforcement of advertised upper bound. Full
description in profiling/vma-gap-index.md. Largest CPU hotspot targeted, not an
unproven ext4 rewrite. No performance result for this candidate yet.

Original-production RED after3staticrounds: bound test Some(12288) vs None,
scan test4107comparisons vs<200, both exit101. Complete candidate restored.
Native host Starry test Clippy failed on existing cfg(axtest) export/COW imports;
unrelated errors left untouched. Removed only our extra redundant host placement
test; placement.rs is exactly original again. MemorySet regressions unchanged.
Project MemorySet Clippy, all-targets Clippy, actual AArch64 profile kernel
Clippy/build passed. Full MemorySet24tests+1doc and8c8gQEMU55/55 passed exit0.
No live kernel test; PTY9875 terminal0. No profile QEMU launched yet.

Important disclosed process error: first final artifact cmps failed but tool
orchestration accidentally ran3hostGREEN cases and recorded a false match.
Those results were invalidated and the log corrected immediately. Full BIN
comparison proved differences only in kallsyms; final ELF/BIN saved separately.
ALL3STATICROUNDSREPEATED with checked exit codes before accepted full tests or
anyQEMU. Use corrected-static.log, not invalidated first final-static.log.
Both earlier artifacts and premature test logs retained; never hide this.

Final source archive21files tmp/vma-gap-index.6UocCb/sources-final.tar
SHA69eb2246b187782ae783a9a2dcb820bf368779328445df266228761ad14b898a.
Final ELF e907be91d46a4c8cb1dfb741ac428bf7661ae65c769be5a62550a76d78d06e11;
BIN6508f6258113708b6fe6b33d2976bcf203cadf7d7fd54504bc4ba2406c5b74d6.
Both and source archive copied to target/profiling/arceos-helloworld/starry/vma-gap-index.
All source bytes stillmatch aftertests. Do not use pre-final kernels.

Freed5.5GiB: old fresh-page-zero-window raw removed ONLYafter all16GiB matched
existingzst and fuserunused. Retained0444zst SHA
65152ae69d9f5b862b4de09b71bee36e6e5404f4cb59d5aa29c5ef04d60a39d2;
rawSHAf166df30f186fb2e917a89599bb43e9ff755688ba2e86ab2c1cbf6915274c7d1.
Firstcomparisonfailedatstat-equalityincludingatime,no deletion; corrected full
bytecomparison+dev/ino/size/mtime/ctimecheck0. Bothlogsretained. Lastspace9.1GiB.

Fresh base copy to normal working profile-image path now started; see functions
store vma_gap_base_copy for active session. Finish copy, hash against frozen
9968004595dec398480417e210d64f9ed509801d4325d4ab22f43951983f5ea3, pre-fsck,
replace ONLY guest runner, dump/compare runner and tg-xtask, post-fsck/prepared
hash, then boot saved kernel once and invoke runner once after shell prompt.
No timeout, 181units,8c8gNVMe,nohostbuild/sourceedit/liveimageinspection during
measurement. All other vma_gap commands terminal; no delegation/commit/push.

### VMA full profile is now LIVE (supersedes preparing state)

Fresh base hash verified; pre/post fsck0; runner8657bytes0755dumpcmp0;
guest tg-xtask33731720bytes0755SHAe6823ab3b6a6d944266fc31d5e54814f463466f88949849ba139b6c575f7e6f1.
Prepared rootfs SHAdee720b3ef765b91c83e58cdc8f79cf05da4b7bde73477c61e6b04a0600a2d7f.
No other Cargo/QEMU at launch. Old PID251108 is tmux:server, not QEMU.
QEMU1304938 boot2026-09-12T19:44:46Z, ACTIVE PTY98551.
Observed root@starry prompt; sent /bin/sh /opt/starry-macos-run.sh ONCE,
sendtimebounded19:45:08..19:45:09UTC. Do NOT invoke it again.
functions store vma_gap_qemu has command/chunks; vma_gap_runner_sent records1call.
All base/hash/cleanup/test/build sessions terminal; only98551remains active.
Continue serial/log monitoring until natural full completion. No timeout,
hostbuild, runtime-sourceedit, secondQEMUor live rootfsinspection.
Afterterminal0: readonlyfsck, individualdebugfsdumps,manifestcheck,allrawrecords
andcompilemultisetsverify,outputbytecmpLinux/previous,renderwithsavedELF,view.
No performance result or final throughput acceptance yet. Goal stays active.

### VMA full run CLOSED and verified; next is unmap retirement investigation

This supersedes LIVE above. QEMU1304938 and PTY98551 exited naturally0.
Full build1087s,181units175names,rc0; previous1253s =>166s/13.248% less.
Previousbest1217s =>130s/10.682% less. Linux735s, ratio1.47891.
FinalELF2792c35275d91f57a38a847be87c4d2b333d7b88cd548bf9e06633288c7dba21
byte-matches cache-writeback and Linux; all5compilemultisetsmatch; 6/6guesthashes.
All25987rawrecords(9807CPU+16180wait),1phase9events,8CPUs,allloss/skips0,
folded/summarytotalsmatch.49exceptionmultisets(ip/far/kind/esr/ec/iss)match.
Readonlyfsck0,extent-narrowingsuggestionsdeclined,norepairs. Endeddisk moved to
run/rootfs.img0444 SHA491373c5aa9895d7084dbe625ad4361ffa5e00e3dc8ca1efa50c9452b86b1c69.
Fixedworkingimagepathabsent.21finalsourcefilesexact,savedELF/BIN/archiveunchanged.
Renderer0,11SVGs;4completePNGsactuallyviewed. Spaceabout4.0GiB.
Allsessionsnowterminal includinghash22831,render89759,preview70643. Do NOTpoll.

CPUactive41.037=>45.396%,39951=>38245samples. Oldfirstfitinclusive1990;
newgapsearch14leaf;allnewgapindex/query/mutation/allocrelatedstacks246.
Newmutex161.305s,mountmutex82.944s,dedicatedmountwait83.614s,hold64.336s.
createhold17.114s(26.602%).read654.572s,write140.366s,flush203.720s.
CurrentCPUspin_release1758allpostDAIFClr;share_mapping1003/1007postDAIFClr.
flush_batch1286(3.363%active),1202aftervaae1is,81aftervmalle1is;
1141/1286(88.725%)viaCOWunmapretirement. can_access_range968(2.531%)prefixscan.
CurrentactualELFPCaudit+asm+callers saved ashot-pcs/asm/callers.log.
Fullresults docs/profiling/vma-gap-index.md andrun/input.meta completed_verified.

Next: investigate operation-scoped unmap retirement versus pinned Linux
mmu_gather. No next-candidate source edit/design/test yet. Do not change existing
remote invalidation contract or free owners/tables before completion. Preserve
VMA candidate as measured local improvement, not a statistical/merge claim.
functions stores vma_gap_integrity,metrics,extra_metrics,hot_pcs,hot_callers,
hot_asm,postrun_source_identity,output_identity,export,ended_fsck/hash,render,
preview allretaincommands/fullresults. Goal remains active; no subagents/push.

### Operation-scoped unmap candidate admitted; full profiling LIVE

Supersedes the prior investigation-only state. Connected refactor implemented:
owning PageTable multi-range UnmapSession, MemorySet callback unmap/clear,
Starry operation-scoped COW ownership/RSS/cache-listener retention, bounded
capacity and flush-before-other-backend handoff. Architecture flush contract,
threshold, capacity, guest workload and profiler unchanged. See design and
profiling/operation-scoped-unmap.md. No throughput acceptance yet.

Counterfactual eager-per-range flush RED exit101 after its own3staticrounds.
Candidate restored byte-identically; all3finalstaticrounds repeated and passed.
MemorySet30tests+1doc and page-table-generic copy-from101tests passed0.
Kernel first attempt failed registry TLS EOF BEFORE QEMU; unchanged retry
passed56/56,exit0. Terminal12685 now CLOSED. Do not poll/relaunch kernel test.
Its last tool chunk c3f5d7 had141tokens middle truncation; complete retry log
was not separately captured. First network failure fully preserved.

Artifacts tmp/operation-unmap.4U8Sgf/: final-static.log, component-tests.log,
mutation-static.log, mutation-red.log, kernel-registry-failure.log.
Finalsourcearchive sources-admitted.tar84filesSHA
8444798c0c35bd81ae3eb7534bb8592f9499b06a90283846ade92cafcf7a87f0.
FinalELF ded4dc497c21c661c6553068a4b2bbe791c4b3acb825c8bc4ac05fa00e8a9a5a;
BIN e2916eaf07dbe3cc8508c5cd36e2ca082b8c79104128814e029582800e09deaa.
All84sourcefiles andglobalfinalELF/BIN matchedaftertests.
Saved run target/profiling/arceos-helloworld/starry/operation-scoped-unmap/
contains starryos.elf,starryos.bin,sources.tar,input.meta.

Old cache-writeback ended rootfs compressed and all16GiB compared; onlyverified
raw duplicate removed;0444zst kept,seeoperationreport. VMAendedrootfs untouched.
Fresh frozenbasecopyfullSHA9968004595dec398480417e210d64f9ed509801d4325d4ab22f43951983f5ea3
finishedmatchingBEFORErunnerreplacement. Pre/postreadonlyfsck0;runner8657bytes
0755dumpcmp0;tg-xtaskSHAe6823ab3b6a6d944266fc31d5e54814f463466f88949849ba139b6c575f7e6f1.
Prepared rootfsSHA7db74bb4d1cdccf023f06bcbb3fbaec40e64d461347854bcb5be24887acdfad4.
Finalpreparedhash finished before QEMU. Last hostfree1.6GiB,monitor space.

ACTIVE QEMU1327426, PTY27621,boot2026-09-12T20:54:25Z.
Observed shellprompt; runner sent ONCE20:54:42..20:54:43UTC. Do NOT send again.
functions store operation_unmap_qemu command/chunks and _runner_sent evidence.
Allotheroperation sessions terminal. No timeout,hostbuild,runtime-sourceedit,
secondQEMUor live rootfsinspection. Monitor until full181units naturallyfinish.
Then readonlyfsck,individualdebugfs exports,6manifesthashes,allrawrecords and
compilemultisetsverify,outputELFbytecmpLinux/previous,render11SVGs andview.
Baseline1087s;Linux735s. Goal remains active;no delegation/commit/push.
