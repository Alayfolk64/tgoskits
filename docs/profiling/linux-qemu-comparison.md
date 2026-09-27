# Linux QEMU 对照准备

对照必须保留 Starry 的 TCG multi-thread、cortex-a53、GICv3、8 核、8 GiB、
NVMe 配置和冻结 guest 工具链、源码、tg-xtask，不能使用原生宿主 runner。
新 run-linux-profile-qemu.py 只接收实验目录，要求 vmlinuz、initramfs.img、
独立可写 rootfs.img；它复用现有 qemu-aarch64-profile.toml 的虚拟硬件参数，
仅替换根盘并增加 Linux kernel/initrd/bootargs，不启动编译、不修改宿主配置。
拒绝旧 qemu.log、缺失资产、只读根盘及指向共享根盘的链接，QEMU 错误直接输出
并保留退出状态。Starry saved-kernel runner 和本次已启动的 W 不受影响。

已通过 HTTPS 从 Alpine 官方固定版本
https://dl-cdn.alpinelinux.org/alpine/v3.24/releases/aarch64/netboot-3.24.0/
获取 vmlinuz-virt 和 initramfs-virt，内核版本 6.18.35-0-virt。
记录下载后的 SHA-256（并非声称已与发布方签名清单核验）：

- vmlinuz-virt: 47970e0ee0478fe5c60824a89f162d5a353fa29466e5d3bddb0f9c506f1ed756
- initramfs-virt: d137f3f7cc813a4c027a0bb4a2826f9fadd3f048b97bc9a0a395300e8e17ef1a

官方配置中 NVMe/ext4 为模块，但 netboot initramfs 的完整 cpio 清单不含这些
模块。必须从同版本 modloop 补齐模块及依赖，再验证启动；不能换 virtio-blk
绕过缺失的 NVMe。新增零参数 prepare-linux-profile-initramfs.py，校验固定
资产后解包匹配版本 modloop，保留上游 init，加入完整模块树与 modules.dep，
重新生成 initramfs；拒绝覆盖旧产物。真实 cpio 测试 2/2 通过，连同启动和
渲染工具共 17/17 通过。

- modloop-virt: 2c9155b42b124108fa22f6099ac010e81e20c212c945955c3a5ac24a3bb7f4f3
- 生成 initramfs-nvme.img: f44791957a8016e1164c445c982e9493493149a84c5f6938a3020a83078a6231

qemu-boot-preflight 已实际启动到 Linux 6.18.35-0-virt，8 CPU，NVMe 8 I/O
queues，/dev/nvme0n1 挂载为 ext4 rw/relatime。根用户态仍为 Alpine 3.23.5。
tg-xtask SHA 为 e6823ab3b6a6d944266fc31d5e54814f463466f88949849ba139b6c575f7e6f1，
与 Starry 相同。该目录的独立根盘已安装 bpftrace 0.24.1，不再等于原始冻结盘。

初次 apk 因 guest 无网络出现 DNS transient error 和 no such package；原文
已完整展示。CONFIG_PACKET=m 而运行根盘未装该模块，DHCP 不能直接使用。
按 QEMU user 网络设置 eth0=10.0.2.15/24、网关 10.0.2.2 后 apk 成功，
未改变宿主网络或 sysctl。bpftrace --info 的 BTF/stack/tracepoint 支持已确认，
实际 10 Hz、2 秒内核栈 smoke 也成功取得 default_idle_call/do_idle 等栈。
01:17 UTC sync + poweroff 正常关机，为 Starry X 让出独占运行环境。

首次 Linux 编译窗口因宿主构建脚本静态链接失败；修正后的配置验证已取得
603 秒完整采样窗口，但整次构建未完成。使用 guest BPF，不使用 xtask perf，
不运行旧 native-host runner；内核采样准备不能当成已有性能对照。

## Guest 采样入口设计

guest-linux-profile.sh 为零参数入口；先后台启动 BPF，等待 BEGIN 标记后，
再独立调用 guest-linux-profile-workload.sh。只接受固定 AArch64 Linux、8 CPU、
NVMe 根盘和上述冻结输入哈希，不重建 tg-xtask，不使用旧宿主脚本。只清理
独立根盘中的 source/target 与 source/tmp/axbuild；旧 artifact 目录存在时
拒绝覆盖，工作负载失败时输出原始 run.log、BPF stdout/stderr 并返回失败。
成功退出 0 必须实际有 ArceOS 二进制；124 只说明 600 秒窗口到时。

guest-linux-profile.bt 使用全部 guest task 的 10 Hz CPU 栈和 1/8 阻塞栈，
保持编译子线程和文件系统 worker 可见，不沿用旧脚本有歧义的 pid/tid 跟踪。
cpu_state 单独按 CPU 和 tid==0 计数；CPU 栈最多 20 层。off-CPU 从阻塞
sched_switch 到下一次运行，包含唤醒后等待运行的时间；未恢复任务只报告
pending 数量，不伪造完整时长。它与 Starry mutex 或 ext4 持锁并非同一指标。
同时记录 /proc/stat 和 diskstats 前后快照。BPF 自身开销不同于 Starry 静态
埋点；即使硬件和工作负载一致，也不能把差值全部解释为 OS 本体差异。

参考 [bpftrace 0.24 CLI](https://bpftrace.org/docs/release_024/cli) 的
生命周期和 -f json 行格式；按
[v0.24.1 JSON 输出实现](https://github.com/bpftrace/bpftrace/blob/v0.24.1/src/output/json.cpp)
读取最终 map，检测丢事件、重复 map 和缺 END。源码旧路径请求返回 404，
随后通过 GitHub tree 找到实际 src/output/json.cpp，未把失败请求当证据。

render-linux-profile.py 只接收导出实验目录，生成 CPU、active CPU、off-CPU
折叠栈和 SVG。空 kernel stack 保留 [no-kernel-stack]，不在 AArch64 上使用
仅支持 x86_64 的 usermode helper；/proc/stat 独立记录 user/system/idle/iowait。
5 项解析/计数测试已通过，shell 语法检查通过。完整新 BPF 已在 guest 中
实际装载采样；修正后的编译窗口结果见下文。这些测试不替代实际编译成功证据。

修复盘实际预检确认两项入口前置条件：init=/bin/sh 不会自动挂载 tracefs，
未挂载时原始报错为 Tracepoint not found: sched:sched_switch；挂载后同一
BPF dry-run 退出 0。其次，bpftrace 0.24.1 的 -c '/bin/sleep 3' 短测虽然
退出 0 且 sched 栈可见，却得到空 @cpu/@cpu_state，不能用作全机 CPU 对照。
去掉 -c 独立运行，同一脚本在有效 1.287 秒内得到 8 核各 12 个 idle 样本，
合计 96，与 @cpu_state 完全一致。正式入口因此显式挂载 tracefs、等待
BEGIN 后启动工作负载，并拒绝空 CPU map；编译结束后 SIGINT 收尾 BPF。

## 离线安装前的 journal 边界修正

qemu-boot-preflight 根盘迁至 qemu-probe-preflight 后，新 BPF 预检失败：
ext4_lookup 报 deleted inode referenced: 5169，脚本打开报 No error information。
原因是前一次 Linux sync + poweroff -f 未保证 journal 清空，过早 debugfs -w
安装三个脚本，后续 Linux replay 撤销 inode 分配而保留目录项。不是 Starry
新内核造成，也不能将这次失败计为性能样本。完整原文已展示。

01:32 UTC 在该 Linux guest remount root ro 后关机，确认无 QEMU。只读 fsck
退出 4，明确仅列三个新增脚本的 dangling entry。原盘设为只读封存：
target/profiling/arceos-helloworld/linux/qemu-probe-preflight/rootfs.img，完整
SHA e431857f73d25641eda45358f2423d959f6b6b2dbd4cb96948b7ed2ae756100b。
qemu-probe-repaired 独立副本的全盘 SHA 与之相同后，执行 e2fsck -fy
-E fixes_only；只清除三个已失效的新增目录项，退出 1 表示修复完成。
修复后 fsck -fn 退出 0，重装仓库中的三个脚本后再次 fsck -fn 退出 0。
原盘及全部正式脚本仍在，清除项可恢复。

guest 入口已在最终关机前加入 mount -o remount,ro /，启动调试 Skill 同步
要求：离线写入前必须无待重放 journal，必要时先在备份副本完成恢复。修复盘
再次启动验证、只读卸载与离线 fsck 均完成后，才冻结为 Linux 测量起点。

## 首次编译窗口与 host-config 修正

修复盘重启、全机采样 smoke 和只读卸载后 fsck 通过；更新入口再次 fsck 通过。
冻结 qemu-probe-repaired/rootfs.img 与 kernel-window-600 独立工作副本的
完整 SHA 均为 844cd703d662b6b732299335919db2384bc05f5d549d25d6cdcc3c5ac232c5b7。
2026-09-10 01:46:03 UTC 编译开始；61/122/183/243 秒启动单元分别为
52/61/79/80。329 秒失败，tg-xtask rc=1，底层 Cargo/build.rs rc=101：

```text
Unable to find libclang: "the `libclang` shared library at /usr/lib/libclang.so.21.1.2 could not be opened: Dynamic loading not supported"
```

原始完整构建输出已直接展示并保存。guest file/readelf 确认失败的
ax-posix-api-75b0ddfc0eafdb40/build-script-build 是静态 ELF，没有 INTERP。
原 [host].rustflags 没有启用 nightly host-config/target-applies-to-host gate，
不会用于 tg-xtask --target 构建中的 host artifact；现有 guest-selfbuild.sh
的原生 Cargo 路径有这两个 -Z，tg-xtask profile 路径漏了它们。
参考 https://doc.rust-lang.org/cargo/reference/unstable.html#host-config 。

修正在 Starry/Linux 两个 guest runner 同时启用 CARGO_UNSTABLE_HOST_CONFIG
和 CARGO_UNSTABLE_TARGET_APPLIES_TO_HOST；保留原 -crt-static host flags、
目标链接方式、源码归档和复用 tg-xtask。旧 Starry 窗口也可能在后续相同阶段
失败，因此它们只证明已测阶段的热点，不证明最终构建可完成。修正后必须
重新测双方，不能将仅 Linux 修正后的结果当成严格环境匹配的对照。

失败盘已只读卸载、正常关机、fsck -fn 退出 0并封存；导出 11 个文件时只有
恢复 guest 属主的 EPERM（原文全量展示）。BPF 完整结束，解析验证通过：
26424 CPU 样本，每核 3303；16255 active（61.5160%），proc/stat busy
62.0617%。80.10% active 样本没有 kernel stack，明确保留 unknown，不当作
确定的用户态 PC。此为失败阶段诊断，不是 600 秒有效窗口或完整构建测速。

## Host 配置修正的 600 秒验证

`linux/host-config-preflight` 从首次失败结束盘的独立副本开始，复制前后完整
SHA 均为 d68dac3d518c0fdca1d6fe23165af612bfb0c1ddcb0b6fd8e089556b3c4fd370。
fsck 退出 0、journal start 为 0 后，仅更新 guest-linux-profile.sh；guest
核对该脚本 SHA 为 41e3dfa9a3a7ca894c06dcab2f5a7fb8eda09b96ae679e353ad2ea7bd0bbd085。
保留旧 artifacts，运行前清理工作负载 target，不继承失败编译产物。

2026-09-10 02:04:04 UTC 开始，603 秒到时 rc=124，177 个启动单元、171 个
不同 crate，build_completed=false。每核 6040 个 CPU 样本，总 48320；
active 33883、idle 14437，活跃占比 70.1221%；proc/stat busy 70.7039%。
BPF stderr 为空，BEGIN/END 和 CPU map 一致性验证通过，导出 11 项 SHA 全部
通过，guest 只读卸载后关机，离线 fsck 退出 0。结束盘已设为只读，SHA：
7eb48c99d593112a96140154e498ab75056f050074df0f7e122954a6b28d624e。

ax-posix-api 的 host build-script 现为动态 PIE，解释器
/lib/ld-musl-aarch64.so.1，BuildID 6fd1d28a3a31cd99bd34e50ca53fd319bde50cc4；
实际输出 ctypes_gen.rs（38624 字节）。原 libclang 失败点已通过。
build-script stderr 有非致命的 rustfmt 缺失提示，完整原文已展示；绑定文件
仍已生成。未修改冻结源归档或复用 tg-xtask 二进制。

活跃样本中 82.33% 没有内核栈，保持 unknown；已符号化叶子中 tlb_flush
为 1637 个样本（活跃样本的 4.83%）。不可将空栈解释成已确定的用户态 PC。
这轮用于配置修正验证：起始盘是失败结束盘，测量期间有轻量本机浏览器及
脚本检查；未运行宿主 Rust 编译。不能将它与旧 host gate 未启用的 Starry
窗口计算严格 OS 加速比。接着用同样 host gate 重新测 Starry 基线和候选。

## 结束盘压缩保存

为保留独立 A/B 起点而控制磁盘占用，`kernel-window-600` 与
`host-config-preflight` 的结束 raw 盘已各自压缩为同目录 `rootfs.img.zst`。
两份均完整解压 17179869184 字节，并用全盘 SHA 与原盘逐一核对，分别为
上述 d68dac3d… 和 7eb48c99…；只有匹配后才移除 raw 与验证副本。
原始日志、导出 artifacts、ELF 证据均保留。可用
`zstd -d --sparse <run-directory>/rootfs.img.zst` 恢复原盘，再核对上述完整 SHA。
此操作只发生在 QEMU 已退出的结束盘上，没有删除唯一数据或修改冻结起点。

## ext4 重构后的独立 600 秒对照

2026-09-10 21:50 启动 `linux/ext4-background-comparison-600`，22:00 正常关机，
QEMU 退出 0。独立复制 `qemu-probe-repaired/rootfs.img` 后全盘 SHA 匹配
`844cd703d662b6b732299335919db2384bc05f5d549d25d6cdcc3c5ac232c5b7`；
确认 clean journal，再仅替换副本 `/opt/guest-linux-profile.sh` 以启用已验证的
两项 host gate，操作前后只读 fsck 均退出 0，未更改原始基盘。
启动前副本 SHA：`9771fba4713c7e37b7a51c7731524c6d45a3a2cb588dda6d9f50ceacc14196eb`。
kernel、initramfs、入口脚本、tg-xtask 和 workload 的身份记录在该目录 `host.meta`。

实际窗口 604 秒，rc=124，177 个启动单元、171 个不同 crate，完整构建未完成。
每核 6044 个样本，总 48352；34727 active、13625 idle，活跃占比 71.8212%。
`/proc/stat` busy 为 72.4116%（独立累计计数口径），与采样结果相近。
已符号化的最大活跃叶子是 `tlb_flush`，1630 个样本，占全部活跃的 4.6938%；
28642 个活跃样本（82.4776%）没有 kernel stack，仍明确为 unknown，不当作
已解析出的用户态栈。off-CPU 按 1/8 采样估计，含任务唤醒后的调度等待，
不能与 Starry ext4 锁等待直接相减或当作墙钟耗时。

11 项导出内容 SHA 全通过，bpftrace stderr 为空，BEGIN/END、CPU 栈/每核
状态一致性验证通过；关机后只读 fsck 退出 0。debugfs 导出文件与目录的属主
恢复出现 13 次 EPERM（原文已输出），内容校验没有失败。测量期间未执行宿主
编译、测试或其他 QEMU，只进行轻量源码/文档检查；编译检查在 QEMU 退出后开始。

同配置重构后 Starry 为 607 秒、52 个启动单元、49 个不同 crate、38.8718%
active，见 [重构后窗口](ext4-background-window.md)。两边复用同一冻结源码、
tg-xtask 和工具链，8c8g / TCG / Cortex-A53 / NVMe、host gate 一致；Linux
基盘包含 bpftrace 等不同用户空间准备，且采样实现不同，不能报告严格的 OS
整体加速比。两边均在开始时清理本次构建输出，保留可复用的 tg-xtask。

产物目录 `target/profiling/arceos-helloworld/linux/ext4-background-comparison-600`：
[CPU 火焰图](../../target/profiling/arceos-helloworld/linux/ext4-background-comparison-600/rendered/cpu-active.svg)、
[off-CPU 火焰图](../../target/profiling/arceos-helloworld/linux/ext4-background-comparison-600/rendered/off-cpu.svg)、
[统计](../../target/profiling/arceos-helloworld/linux/ext4-background-comparison-600/rendered/summary.json)。
