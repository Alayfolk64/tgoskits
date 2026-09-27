# inode 表块读取候选

## 证据与状态

N 的 QEMU 采样期间仅准备测试；正常结束后运行 RED（3 失败、3 通过），
实施局部读取修改并完成 GREEN、全套测试及 clippy。M2 正常结束后构建候选
内核并完成 O 窗口，局部块读下降但未显示整机收益，见下方实测。
M 全量 block-read-sync 中 InodeCache::load_inode 累计
66112686864 ns；lookup_locked 调用链块读 77635349632 ns，二者可能重叠，
不能相加为墙钟比例。尚未测得邻近 inode 的块缓存命中率，66 秒不是预期收益。

当前 load_inode 每次 inode 缓存缺失都分配一个 4 KiB buffer，再调用
Jbd2Dev::read_blocks（绕过底层四块缓存），即便另一个 inode 刚读过相同表块。
已有 read_block/buffer 能复用块，同时先检查待提交 journal 更新；直接批量写
会刷新重叠缓存，自动 commit 后会失效缓存。只考虑复用此既有职责边界。

## 设计与风险

候选改动是 load_inode 调用 read_block 后在现有独占 &mut Jbd2Dev 生命周期内
解码 buffer，不增加锁，不让引用逃逸；inode cache 的 guard 仍在设备调用前
释放。原注释强调共享 buffer 会串行化，但现有 Jbd2Dev 已要求外部同步并通过
独占可变引用访问，当前 ext4 适配层仍持全局锁；不能把局部分配当作真正并发 I/O。

替代方案：保持旁路不引入新风险但重复 I/O；增加 inode 或块缓存容量属于另一
项实验，尤其块缓存已有历史 stale metadata 警告，本次不改变容量；新增缓存层
重复已有一致性职责。复用 read_block 需要验证 journal overlay、direct write
更新、失效、驱逐和错误路径，不能仅凭相邻读取次数决定采用。

不修改 inode 格式、checksum、显式 flush/commit 顺序、syscall 参数或 errno
映射；缓存完整提供数据时不再依赖 backing 成功，真正缺失仍传播 EIO。属于既有
数据读取边界的局部优化；没有新公共 API、unsafe、依赖、资源所有权或锁顺序。
当前依据为 affddc3fecec02b31df94573063e4840433c9ebe 工作树。已读该路径历史
1ab948f77/828538f64 的记录及现有自动提交失效、journal overlay 和 CRC 回归。
MOSS 缺页锁外准备与本候选互补，不可绕过 rsext4 自身的一致性要求。

## 验证门槛

tests/inode_table_reads.rs：相邻 inode 一次读、inode 驱逐仍复用表块、缓存命中
不受 backing EIO 影响、pending journal 覆盖旧表块、直接写刷新表块、真正读失败
不发布 inode。前三项应在旧生产代码上确定性失败，然后才实施候选。
随后全套 rsext4、ax-fs-ng 真实镜像/fsck、fmt/clippy 和独立 QEMU 600 秒验证。
本候选独立测量，不与 N 完整数据缓存读取混为同一变化。

read_block 缺失时会使用既有 clock 淘汰策略，可能提前触发脏底层缓存写回；
这是必须测试的实际副作用，并非保证所有设备写入时机完全不变。保留四块容量，
已跑自动提交后失效、journal replay、CRC、真实镜像/fsck 回归，没有静默容错。

## 2026-09-10 本地验证

`cargo test -p rsext4 --features host-test --test inode_table_reads -- --nocapture`
在旧实现上退出 101：相邻读取 2 次而非 1 次，inode 驱逐后 3 次而非 1 次，
已有块读取被 EIO 阻断。完整原始输出已展示；修改 load_inode 为
read_block + buffer 解码后，同一 6 项全部通过。

`cargo test -p rsext4 --features host-test` 219 通过、0 失败、1 项原有外部
Linux 镜像依赖用例忽略；`cargo test -p ax-fs-ng --features host-test,ext4,vfs,profile --lib`
102/102 通过。`cargo xtask clippy --package rsext4` 3/3、默认缓存 host-test
tests clippy、cargo fmt 与 git diff --check 全部通过。执行均设置项目 TMPDIR，
运行于 N 结束和 M2 启动之间，没有干扰活动测量。

M2 结束后通过固定配置的 cargo xtask starry build，完成 13021 个 kallsyms
符号注入与 BIN 刷新。O 目录 inode-table-cache-window-600，ELF SHA-256
`3813b0adef4871b83bb563465cc2063bc9a9ba5c5c26047217021089824ce3c5`，
BIN `0fc6c378105b74332597ba0bbf3579b1b256c5ba0adb0bf3e780f5365598bc01`。
相对 N 只增加本 inode 表读取候选；工作盘从只读 read-cache-base 重新复制，
启动前全量核对哈希为
`d5e8c6347879117246530ba52dacba58a03379f06ddaf19866ca576e9cdd57d4`，
继续复用根盘 tg-xtask 和源码。

## O 窗口：局部读取下降，整机未改善

612 秒、rc=124、build_completed=false、两个 reused=true，34 单元 / 31 crate，
17119 条记录。实际采样 611588989968 ns，所有丢样为 0；47924 个 CPU 样本中
11737 个活跃（24.4909%），最忙 CPU1 为 78.4444%，其余核 11.17%–24.66%。
5 项 SHA-256 通过，QEMU 正常关机后只读 fsck 退出 0；debugfs 属主恢复权限
提示已完整输出、退出 0，内容哈希无误。结束盘保留为
rootfs-profile-inode-table-cache-result.img，CPU-active 与持锁者 PNG 已实际查看。

| 指标 | N：完整数据缓存命中 | O：再加 inode 表块复用 |
| --- | ---: | ---: |
| 启动单元 / crate | 40 / 38 | 34 / 31 |
| CPU 活跃 | 31.3574% | 24.4909% |
| mutex 累计等待 ns | 2383843562752 | 2720679257888 |
| ext4 锁叶子等待 ns | 2044502141616 | 2083281072064 |
| ext4 实际持锁 ns | 490538969936 | 500402883344 |
| 读取直接持锁 ns | 146076797952 | 179284920608 |
| 同步直接持锁 ns | 135033093568 | 179024732624 |
| 查找直接持锁 ns | 93320881072 | 57148454416 |
| 元数据直接持锁 ns | 27848671776 | 19543275296 |
| 同步块读次数 / ns | 78934 / 330122100720 | 63250 / 342978131424 |
| 同步块写次数 / ns | 48006 / 104801220416 | 28424 / 108069284848 |
| 设备 flush 次数 / ns | 9335 / 16292704016 | 8268 / 23466419280 |
| InodeCache::get_or_load 读取链 ns | 69627674032 | 45272653984 |

O 的 load_inode 被优化内联，不能把没有该独立栈帧误报为读取归零；比较改用
两轮共同存在的 get_or_load 完整链（含其其它内部工作），不与 N 的单独
load_inode 68409710544 ns 混用。文件内容链读盘反增至 174583160160 ns，
用户缺页链持锁 151913537488 ns（30.3582%，与读取重叠）。

O 中 ext4 仍占 mutex 等待 76.5721%；持锁最大两项是读取 35.8281% 与同步
35.7761%，查找 11.4205%。局部块读/查找下降并未转成整体进度改善，不能据此
宣布成功，也未单凭一次窗口判定稳定退化。保持保存内核可回退，先复测完全
相同的 N 内核与根盘起点（data-read-cache-repeat-window-600），排查运行波动。

N2 复测已经完成：33 单元 / 31 crate、24.9790% 活跃，哈希与 fsck 通过。
同一 N 内核首次 40 单元 / 31.3574% 未稳定复现，不能再将 N/O 单轮差值直接
归因为本候选。N2 的 inode get_or_load 块读链为 51506200800 ns，O 为
45272653984 ns，但总进度仍相近；本项仍没有稳定的整机加速结论。
