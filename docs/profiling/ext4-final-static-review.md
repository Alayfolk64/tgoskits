# ext4 重构最终三轮静态检查

## 放行规则与冻结输入

2026-09-10 20:35 开始最终检查。三轮全部通过前，不执行测试二进制、
QEMU、fsck 或 profiling。编译测试目标只验证接线和类型，不计行为通过。
若修改生产代码，重查受影响的前轮项目并更新快照，不借用旧结果放行。

- 分支：`profiling/manual-20260907`。
- HEAD：`affddc3fecec02b31df94573063e4840433c9ebe`；实际输入含已有工作树差异。
- 源码快照：`tmp/ext4-final-static.ryo14j/source.tar.gz`。
  SHA-256：`0250c119f76d512e65f8ad91a46e4111b333ac4c83445a7b63f64d5a55e8680c`。
- msync 完成边界修复后的快照：`tmp/ext4-final-static.ryo14j/source-msync-fixed.tar.gz`，
  SHA-256 `970c540bbcb99569c271dc4b96dc75e878f46da80c66385f00079de7c1919632`。
  此后只补充 metadata 持久化回归，最终快照在第三轮重新生成。
- 快照范围：Cargo.toml/lock、fs、net、os、memory、components、platforms、
  drivers、scripts/axbuild、test-suit/starryos。未修改的其他依赖由 HEAD 标识；
  正式构建仍使用仓库源码，不从快照或 tmp 编译。
- guest、根镜像、tg-xtask 和工作负载身份见 [产物清单](ext4-refactor-artifacts.md)。
- Rust 使用仓库固定 nightly。临时目录为仓库 `tmp/`；矩阵使用已核验的
  `tmp/ext4-static-firmware.pha7d1/firmware` 和现有 x86_64 musl 工具链。

## 第一轮：接口、模块、编译矩阵（通过，20:53）

20:35:57 开始，以下不是重构期间检查的重命名。

| 检查 | 结果 |
| --- | --- |
| `cargo fmt --package rsext4 --package ax-fs-ng --package axfs-ng-vfs --package starry-kernel --package axbuild --check` | 退出码 0，无输出 |
| `cargo xtask clippy --package rsext4 --package ax-fs-ng --package axfs-ng-vfs --package starry-kernel --package axbuild --package ax-net --package ax-api --package ax-posix-api --package ax-runtime --package ax-sync` | 20:45:27 退出码 0，185/185，耗时 9m30s；Starry 结果因随后修复需重查 |
| `cargo clippy --package rsext4 --lib --tests --features host-test,USE_MULTILEVEL_CACHE -- -D warnings` | 退出码 0 |
| `cargo clippy --package ax-fs-ng --lib --tests --features host-test,ext4,vfs,profile -- -D warnings` | 新增 persistence 测试后首次 E0369，改权限位精确比较后退出码 0，0.82s |
| `cargo clippy --package axbuild --tests -- -D warnings` | 退出码 0 |
| 实际 AArch64 SMP=8 `cfg(axtest)` 编译，含 msync 回归 | 初次 E0369（错误类型不支持 assert_eq），改精确 matches 后修复前/后均退出码 0；均未执行 |
| `cargo xtask clippy --package starry-kernel` | 20:52:25 退出码 0，110/110，4m46s |
| `cargo fmt --all --check`、`git diff --check` | 退出码均为 0；之后仅修改测试权限位断言并重新 cargo fmt，退出码 0 |

矩阵中 memchr 2.8.3 的 future-incompatibility 提示保留，不通过 `allow`
屏蔽检查。工具早期输出发生显示截断，不能声称拥有一份完整原始矩阵日志；
命令最终退出状态与汇总将单独记录，失败原文必须保留并直接显示。

### 第一轮发现并修复：clean page 不是 msync 完成回执

`backend/file/mod.rs::writeback_range` 原来在 `dirty_pns.is_empty()` 时直接成功。
真实 cache 在数据写完成后清 dirty，底层 journal 同步在之后，失败也不会让
已写入页重新变脏；下一次 msync 因而可能跳过失败/在途的持久化边界。
现在空列表也进入原 `writeback_pages`，由其 writeback owner 等待 backing sync。
新真实内核回归 `clean_page_range_still_waits_for_backing_durability` 注入 backing
同步失败，检查已清脏页仍收到错误、每次重试确实调用同步、最后成功保留数据。
旧实现必然跳过该回调，但遵守门槛，尚未执行失败/成功两侧对照。

此修改仅触及 Starry 私有文件映射实现和测试；未改变底层公共 API 或其他消费者。
因此重做 Starry 110 项、实际 axtest 编译和格式检查，保留未改依赖的矩阵结果。

## 第二轮：并发、错误与持久化顺序（通过，20:57）

逐项核对 commit/checkpoint 的所有权和 barrier、共享块缓存 redirty、页回写/
回收/失效、通知、关闭入口、metadata reader rollback 以及 syscall 完成边界。
记录实际文件与不变量；未运行的 Linux 差分或崩溃恢复不能写为通过。

| 审查项 | 冻结实现依据与结论 |
| --- | --- |
| commit 所有权 | `fs/rsext4/src/blockdev/journal/detached.rs:400` 全部可失败准备先于 seal；pending 保存原 mount/ticket/work，`:617` 先校验 receipt 再发布，foreign receipt 不被消费 |
| data/journal barrier | `PreparedCommit::execute` 在提交前 flush；`jbd2/jbd2.rs` 提交块 FUA 后才生成 checkpoint owner；错误不能推进 durable |
| checkpoint/revoke/reuse | `detached.rs::prepare_checkpoint` 要求 mutation admission 关闭、无 running updates；`jbd2.rs:1032` 新版本/revoke 过滤旧 home image，home flush 后才 FUA 推进 tail |
| 锁外同步 | adapter `fs/writeback.rs:190` 的 prepare/stage/publish 短持 ext4，execute 与 abort persistence 不持 ext4；commit gate 只串行提交 owner，不阻止普通 inode 操作 |
| 周期页回写 | `periodic_writeback` 先写有限缓存快照，再进 commit gate；页回写即使失败也提交成功前缀，返回原始页错误 |
| 页缓存回写 | `file/cache/writeback.rs:29` 先取 writeback owner，保护回调前释放 IO/page/listener 锁；只有完整写成功且 generation 相同且无 redirty 才清脏 |
| 缩短/回收 | `resize.rs::set_len` 持 mutation/WB，发布长度并保留退役帧后释放 IO，再阻塞失效；`reclaim.rs` 仅 try_lock AddrSpace，失败恢复 canonical frame |
| TLB 失败责任 | `backend/file/invalidation.rs` 校验帧身份和当前 VMA，PTE/RSS 只处理一次；`pending.rs` 保存原 VA；backend 析构后 listener 仍重试，pending 锁不跨 shootdown |
| msync 完成边界 | 第一轮发现的 clean-page 早退已移除；空列表仍进 backing sync，实际回归编译通过；依据固定 Linux `mm/msync.c:97`、`fs/ext4/fsync.c:170` |
| registry/lifetime | file registry 用 try_unwrap 原子退役，析构在列表锁外；CachedFile 的 Location 保留 Mountpoint/lease，最后 lease 退役不与仍持 Location 的缓存写者混淆；失败页仍登记 |
| 容量/redirty | `block/cache/address_space.rs` 索引只固定 Arc，不跨设备 I/O；snapshot 与 resident 共用 1024 页预算，预算满时单页原地写回；direct/buffered 先取得完整 range reservation |
| 通知 | `os/waiters.rs` 先登记再复核受锁保护的条件；发布状态之后才 notify，回调在通知列表锁外；IRQ 回收用 deferred notify |
| 关闭 | `fs/mod.rs:378` 先关 cached-write、join worker、写页，再关 core admission；`:387` reap/stop MMP/commit/checkpoint/clean，final-clean 尝试后不重开；此前失败重开不清 sticky error |
| metadata reader | canonical Arc 不随 rollback 替换；handle pause 在 shared map 修改前发布，reader try_lock 后检查 pause/alive，rollback 恢复内容后再开放 |
| ABI 影响 | [逐入口影响表](ext4-syscall-impact.md) 单列状态与标准；仅确认本重构调用边界，不冒充完整 syscall ABI 合规。既有 sync/MS_ASYNC 等差异没有被隐藏 |

第二轮命令：`cargo xtask clippy --package rsext4 --package ax-fs-ng --package axfs-ng-vfs`
20:54:03 退出码 0，14/14，耗时 6s；`cargo fmt --all --check` 与
`git diff --check` 退出码均为 0。第二轮没有修改生产代码。
结论是本轮静态不变量检查通过，允许进入第三轮，不是行为测试或合入批准。

明确保留的运行风险：busy mmap 页可超过缓存目标；失效故障时 truncate 已发布
新长度但返回 EBUSY；失败 backend 的空 listener 保留到缓存退役；冷 metadata
fallback 仍可能持 ext4 锁读盘。它们不以虚假的成功/释放处理，代价须在测试中确认。

## 第三轮：最终差异、真实构建和回归接线（通过，20:59）

复核冻结输入、所有直接消费者和 runner 发现路径，构建实际 Starry 内核，
确认 QEMU 为 8c8g；检查未接线的旧用例与替代关系。构建不启动 QEMU。

- `cargo xtask starry build --config apps/starry/macos-selfbuild/build-aarch64-unknown-none-softfloat.toml`
  退出码 0。实际日志：AArch64、SMP=8、NVMe、guest-profile、release；构建 13.94s，
  随后成功注入 14046 个 kallsyms 并更新 BIN。没有 QEMU 执行。
- ELF `target/aarch64-unknown-none-softfloat/release/starryos`：18191072 bytes，
  SHA-256 `f67cd9e25369446399abe77384e08810ee8166cd7e2b6c552a782c7d8e6f57a6`。
- BIN `target/aarch64-unknown-none-softfloat/release/starryos.bin`：14888960 bytes，
  SHA-256 `9e9c91e9ad8df7d8e32b1a8cb17be0cad42cc293cb353c29b33337c63fa31d1a`。
- 最终源码快照 `tmp/ext4-final-static.ryo14j/source-final.tar.gz`：
  SHA-256 `2443f8d6bfb307d4f18fabb3de90e49376f5b046f56ec10e64456810e3f125d5`。
  `tar -dzf tmp/ext4-final-static.ryo14j/source-final.tar.gz` 退出码 0，无差异。
- `cargo fmt --all --check`、`git diff --check` 退出码 0；HEAD 未变。
- `cc -fsyntax-only test-suit/starryos/qemu/system/test-remount-flags/src/main.c test-suit/starryos/qemu/system/test-remount-flags/src/writeback_flags.c`
  退出码 0，没有执行宿主 remount 测试。
- `axtest_exports.rs` → `axtest_memory.rs`/`axtest_fs.rs` 确认缓存失效、msync、
  同步打开 flags 与原七项 context/inode 用例接线；不是文件存在即视为执行。
- ktest 和 system 的 QEMU 参数均为 `-smp 8 -m 8G`，对应 build max_cpu_num=8。
  已检查 ktest 命令没有 build-only 开关，门槛前未使用其 QEMU 入口。
- 旧回归映射的磁盘恢复/fsck 和冷祖先读故障注入仍未取得运行证据；不把替代
  用例或编译成功当作这些旧断言已通过。已列为门槛后的定向回归事项。

20:59 三轮静态门槛满足，仅放行行为测试。随后任何生产逻辑修复均需重查受影响
静态项目；当前结果不代表完整 Linux ABI 兼容、崩溃安全验收或性能提升。

## 行为和性能结果

21:00:22 在三轮通过后首次运行
`cargo xtask ktest qemu --package starry-kernel --arch aarch64`，退出码 0。
实际 QEMU 为 8 核、8 GiB，根盘使用 snapshot=on。
`AXTEST_SUMMARY pass=51 fail=0 skip=0 total=51`、`AXTEST_SUITE_OK`。
第 7 项统一调用缓存/TLB/msync 的六项真实回归；第 37 项同步打开 flags、
第 39 项原七项 context/inode identity 断言均通过。
Io/TimedOut 日志来自这些用例的故障注入，不是 suite 失败。

- ktest ELF SHA-256：`214bae4d76a516daf1624f46b9f45f426382713de9d1dccbbc229ad07f2a7052`。
- ktest BIN SHA-256：`6754ce32151b9df3a2334ab1a53d4bae3d33b8d731f2be33e60ab82eab64ef23`。
- 最后一次输出保存在 `tmp/ext4-final-static.ryo14j/ktest-final-output.log`，
  不含最初构建和最前面的用例，不冒充完整串口日志。
- `cargo test --package rsext4 --lib --features host-test`：退出码 0，
  365 passed / 0 failed / 0 ignored，运行 0.13s。
- `cargo test --package ax-fs-ng --lib --features host-test,ext4,vfs,profile`：
  213 项中部分通过，随后 append 回归无限循环；主动 SIGTERM 后退出 101，
  不计 suite 通过。原始输出和调试栈保存于同一证据目录。

## 行为回归发现的 ax-io 缺陷与补充静态门槛

`IoBufSpec for &[u8]::write_to` 没有在成功后推进切片，导致真实 Direct append
反复写入同一输入。GDB 中 chunk=1、total=end=85255343，输入仍未消费；
不是 ext4 锁死。`IoBufMutSpec for &mut [u8]::read_from` 同样不推进输出。
两者均为本轮之前已有实现；没有绕过公共抽象修改文件调用方。

先在 `components/axio/tests/iobuf.rs` 增加定向测试并完成其 Clippy 编译：
`cargo test --package ax-io --test iobuf --features alloc slice_` 退出 101，
写/读推进两项确定性失败，错误/零进展保持一项通过。完整原文
`tmp/ext4-final-static.ryo14j/axio-red.log`。
随后修复只按成功返回的字节数推进两种切片；错误与零进展不推进，不新增
API、分配或 unsafe。沿用同 crate 的 slice Read/Write 的所有权与推进模型。

21:10:20 开始修复后的三轮补充静态检查，21:16:49 全部通过；期间未恢复测试。

1. 第一轮：`cargo xtask clippy --package ax-io --package ax-fs-ng --package starry-kernel`，
   21:14:46 退出 0，120/120，4m25s。
2. 第二轮：两种切片的成功路径推进恰好返回字节数；`?` 保留错误，零进展保持
   原位置；无新增别名、unsafe、分配或锁。Direct append/read 循环依赖该推进；
   vsock 是一次调用，不自行重复推进。泛型 fallback 与 VecDeque/BufReader 不变。
   两个旧 std 测试曾断言错误的不推进行为，现保留数据断言并检查耗尽位置。
   `cargo clippy --package ax-io --all-targets --features alloc -- -D warnings`
   首次退出 101（旧 io/buffered 测试的 11 个 truncate(0)、1 个无效 min）；
   等价改成 clear/len 后退出 0，0.22s。ax-fs-ng 的 host-test,ext4,vfs,profile
   全库及测试 Clippy 退出 0，2.53s；fmt --all --check、diff --check 退出 0。
3. 第三轮：原实际 profiling build 命令退出 0，release 11.96s、14049 kallsyms；
   实际 SMP=8 axtest 目标 Clippy 退出 0，5.58s。源码快照
   `tmp/ext4-final-static.ryo14j/source-axio-fixed.tar.gz` 完整比较无差异，SHA-256
   `88ecd2e84816af5d31a1bcf964ff74f657c3a69fedde87171888c3fe8342d99e`。
   新 ELF 18196928 bytes，SHA-256
   `d0267e4a0624e59d28ebb452d15ceefd7933e75024058707faaa8446f008f6cd`；
   新 BIN 14893056 bytes，SHA-256
   `02907f18366549a2014e7b3d8859ee60a81021869f35de6e0e952988de867f54`。
   HEAD 未变，diff --check 退出 0。

门槛后同一 `slice_` 命令退出 0：3 passed / 0 failed，完成确定性 red/green。
`cargo test --package ax-io --features alloc` 退出 0：180 项及 2 项文档测试通过。
适配层全套随后退出 0：213 passed / 0 failed / 0 ignored。
其中 background writeback 用例准备阶段原本以同步模式的冷页写制造 metadata
dirty，却断言没有 metadata dirty；已改为真实后台模式，预读并同步初始内容后
再进行缓存脏页写。生产代码不变，原 mount 隔离及回写断言全部保留。

### 全套 core 与新内核回归（21:29）

`cargo test --package rsext4 --features host-test --no-fail-fast` 退出 0，
585 passed / 0 failed / 0 ignored（14 组，另有 0 项 doctest）。包含 42 项
Linux 镜像、e2fsck、journal replay、MMP、1/2/4 KiB block 场景。
为收集全部失败使用 `--no-fail-fast`，没有跳过或忽略失败用例。
测试首轮暴露三处迁移契约，保留原始失败后仅修测试准备或精确断言：

- `extent_restart`：30-block 日志的额度只有 8，unlink 需要 24，故原测试在
  truncate 之前返回 `NoSpace/jbd2:handle_credits`。先在正常日志完成 unlink、
  sync/checkpoint，再安装同样的 30-block 日志；保留 orphan/reap 断言，并新增
  truncate 必须跨 commit 边界的断言。10/10 通过。
- `file_operations/read_plan`：建文件后直接重装虚构日志会丢失 pending owner，
  新防护正确返回 `Busy/jbd2:reinstall_with_pending_owner`。完成初始 sync/checkpoint，
  使用真实已挂载日志注入 overlay；保留数据缓存 > journal > disk 的逐字节断言。
  49/49 通过。
- `integration_test`：最终 clean 发布不再提前混入 checkpoint，卸载需两次
  primary superblock 写（checkpoint 保留 RECOVER，tail 清空后独立 clean）。
  原断言 1，实际 2；只将精确次数改成 2，保留 flush=4 等全部断言。
  独立 `clean_unmount` 顺序回归同时通过，integration 27/27 通过。

每次测试修订后均完成源码契约复核、rsext4 all-targets host-test Clippy
`-D warnings`、cargo fmt 与 git diff --check；生产代码自 21:10 未再变化。
最终快照 `tmp/ext4-final-static.ryo14j/source-profile-ready.tar.gz`：
`5bfb0dcf731ea86e672cf725df1a83e78ef4ad3000575bc88a0d8e52bd3494e4`，
完整 tar 比较无差异；上面 ELF/BIN 哈希保持不变。

新 `cargo xtask ktest qemu --package starry-kernel --arch aarch64` 首次因
镜像注册表请求 `tls handshake eof` 在 QEMU 前退出 1；保留失败，不修改
镜像检查或网络设置。21:26 同一命令重试退出 0：8c8g，51/51，
`AXTEST_SUITE_OK`。ELF SHA-256
`c913872abcccd795e1d3fb6e2e60978ab56885bbad90d7e8f93a1ff90da99164`；
BIN `9e7aeb35f0ec2fc42a25e1558b0889d8152c4f3017a69b883910ca58415af99c`。

原 51/365 结果属于修复前源码，不能直接作为新 ax-io 的回归结果。
21:30 在全部宿主测试退出后启动 `ext4-background-window-600` 的独占 QEMU。
21:41:58 窗口结束，QEMU 退出 0：607 秒、52 个启动的编译单元，完整构建未完成。
5 项内容哈希通过，结束盘只读 fsck 退出 0。持锁 389.11→249.76 秒，
CPU 活跃占比 32.98%→38.87%；一次窗口不能代替重复性或完整编译验收。
详见 [重构后窗口与新热点](ext4-background-window.md)。完整崩溃安全验收仍未完成。
