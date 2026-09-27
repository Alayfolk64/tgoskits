# 直接按父 inode 查找子项：实验设计

## 问题与目标

O 的 ext4 持锁累计 500402883344 ns，其中 lookup_locked 为
57148454416 ns（11.42%）；N 对应 93320881072 ns（19.02%）。
它不是最大的持锁来源；文件读取和 sync_to_disk 仍优先解释 CPU 利用率低。
本项只消除路径行走里的重复祖先查询，不声称已解决全局 ext4 锁。

当前 VFS 已有父目录句柄，但 Inode::lookup_locked 将父目录完整路径与子项名
拼接，再交给 get_file_inode 从根目录查找。深层路径每个 cache miss 都重复
已经完成的祖先解析，并占用全局 ext4 锁。

目标：复用 rsext4 当前单组件的 hash-tree/linear fallback 查找逻辑，从实际
父 inode 开始；成功时仍取得子 inode 内容、增加 live_refs 并创建现有 DirEntry。
非目标：不扩大缓存，不增加负目录项缓存，不改变 create/sync、权限、挂载、
符号链接处理或 journal 持久化边界，不修复与本项无关的完整路径 `..` 行为。

## 既有实现与替代方案

base 为 affddc3fecec02b31df94573063e4840433c9ebe 的当前工作树。
检查了相关历史 5d13ce881、1ca87a306、828538f64 的记录、当前 dir/lookup、
loopfile、hashtree/facade/lookup 及 VFS DirNode。已有 hash-tree helper，
但没有返回完整 inode 的父 inode 单组件入口；应提取复用，不另写目录扫描。
开放 PR 搜索 `ext4 lookup` 只返回 #2070、#1934 的块设备/虚拟化主题结果，
本轮未将此搜索冒充所有 PR 的完整审查。

MOSS 5a54e4413c9657bfb531cc32f6d688090465200d 的
vendor/axfs/src/fs/ext4/inode.rs::lookup_locked 调用 fs.lookup(self.ino, name)。
本地 Linux 980ab36ae5972c83f683b939e50c469c4947229e 的
fs/ext4/namei.c:1760 ext4_lookup 通过 ext4_lookup_entry 使用已选父 inode，
并加载找到的子 inode。本项只借鉴父 inode 边界，不声称移植 Linux 的加密、
目录索引校验、错误优先级或完整 namei 语义。

保持现状继续重复 I/O；扩大缓存只能掩盖重复遍历；另外增加路径缓存需要
rename/权限失效协议。选择提取当前单组件解析，再由现有 VFS 适配器调用。
新增跨 crate 查询入口需单独评审其参数、错误与生命周期；本地实验不代表合入。

## 确定性回归与验收

通过真实 ext4 镜像与生产 VFS 适配器创建 parent/child，sync 后仅逐出根目录
的干净数据块，并让该块的设备读取返回 I/O 错误。保留真实 parent 句柄，直接
调用其生产 backend lookup，避免 VFS 目录项缓存掩盖旧实现。
正确的子项查找不访问根目录数据块，应仍得到 child 的同一个 inode。
旧实现在 root-to-parent 重走时应触发设备错误。先验证失败，再修改生产路径。

N2 采样期间只准备回归，保存内核未改变。N2 正常结束后才运行测试和构建。

## 本地实现与验证

旧生产路径执行 `cargo test -p ax-fs-ng --features host-test,ext4,vfs,profile --lib child_lookup_does_not_reread_root_directory -- --nocapture`
退出 101，回归在 inode.rs:716 收到 Err(Io)，0 通过 / 1 失败。原始输出完整展示。
实施父 inode 查询后，同一命令 1/1 通过，断言和故障注入未放宽。
首次编译曾因遗漏 dir 模块的函数导出而报 E0425、退出 101；补齐导出后通过。

从 loopfile::get_file_inode 提取现有单组件扫描为 find_child_inode，完整路径
与新增 dir::get_child_inode 共同使用它，未另写 hash-tree/fallback 扫描。
新查询验证组件名后只读取父 inode、其目录项与目标 inode；VFS lookup_locked
改用该入口，live_refs 和 DirEntry 构造仍在原锁范围内。组件过长返回
ENAMETOOLONG，空组件/斜杠/NUL 返回 EINVAL，非目录父节点返回 ENOTDIR；
原 VFS 组件验证和 `.`/`..` 分支保持。辅助 API 不等同于完整 syscall 权限审查。

directory_operations 新增 4 项公共入口测试：与完整路径匹配/缺失项、非法
组件/非目录、rename 后同一父 inode 与点目录项、实际目录块 EIO 传播，均通过。
本轮未新增独立的索引目录镜像用例；既有完整路径与 hash-tree 测试共用同一
提取逻辑，不把普通目录用例冒充所有索引格式已覆盖。

`cargo test -p rsext4 --features host-test` 223 通过、0 失败、1 项已有外部镜像
依赖用例 ignored；`cargo test -p ax-fs-ng --features host-test,ext4,vfs,profile --lib`
103/103 通过。两个 crate 的 `cargo xtask clippy --package rsext4 --package ax-fs-ng`
11/11 通过；额外 ax-fs-ng 的 host-test/ext4/vfs/profile lib+tests clippy 通过。
已执行 cargo fmt 和 git diff --check，均通过，没有添加 allow。全部使用项目 TMPDIR。

固定 `cargo xtask starry build -c apps/starry/macos-selfbuild/build-aarch64-unknown-none-softfloat.toml`
通过，完成 13024 个 kallsyms 与 BIN 刷新。R 保存目录 parent-inode-lookup-window-600：
ELF `599b07c5cf5b759c0da03174d78bf2057c90fb33529836eac660979577f60641`，
BIN `db4d0678dbc247a0efb5038b05ef555e265a0b000d69b7ee7f8bc0fb0df86caa`。
包含 N + O + P + 父 inode 查询，尚未测量；先完成保存的 P 独立窗口。
需要真实 QEMU 600 秒进度/持锁数据、完整导出哈希和只读 fsck，才能判断收益。

P 完成后，2026-09-09 22:13 UTC 启动 R，tmux 为
tgoskits-profile-parent-inode-600，QEMU PID 429912；22:14:14 UTC 前发送
guest runner 并启动 pidstat。启动前重新核对上述 ELF/BIN 与冻结根盘完整 SHA
d5e8c6347879117246530ba52dacba58a03379f06ddaf19866ca576e9cdd57d4。
该保存内核不包含此后准备的锁外文件读取测试/设计。

## R 窗口结果

R 已正常结束：603 秒，rc=124、build_completed=false、tg_xtask_reused=true，
启动 39 个编译单元/36 个 crate，19775 条运行记录。实际采样
603021455312 ns，丢弃和跳过计数均为 0；14158/47077 个 CPU 样本活跃，
即 30.0741%。CPU 3 活跃 80.5382%，其余各核为 16.72%–32.48%。
这些是启动进度和采样比例，不是完成构建耗时或完整加速比。

mutex 累计等待 2418499645536 ns，其中 ext4 锁叶为 2132292506144 ns，
占 88.1659%。ext4 持锁累计 489144742688 ns，主要直接调用者：

| 调用者 | 累计 ns | 持锁占比 |
| --- | ---: | ---: |
| read_at | 174474450176 | 35.67% |
| sync_to_disk | 168477636800 | 34.44% |
| lookup_locked | 43968723360 | 8.99% |
| set_len | 33197496592 | 6.79% |

包含 handle_user_page_fault 的持锁链为 122658762368 ns（25.08%），
与 read_at 等直接调用者重叠，不能相加。块读累计 337504879072 ns，
块写 105350864016 ns，设备 flush 20627301136 ns；这些事件会嵌套，
也不能相加解释窗口墙钟时间。最长单次 mutex 等待 8577452576 ns，
最长 ext4 持锁 7892637392 ns，尚未将两者归因到同一个等待/持有关系。

与 P 比较，lookup 持锁从 58620215440 降至 43968723360 ns，CPU 活跃
25.85%→30.07%，启动单元 34→39；但总 ext4 持锁仍约 489 秒，读取和同步
继续占主导。只有一轮正向信号，必须重复验证，不能声称稳定整机提升。

导出五项 SHA-256 全部通过，离线 e2fsck -fn 退出 0，仅建议收窄 extent。
debugfs 导出返回 0，但输出 8 项非特权所有权设置警告，已完整展示；校验的是
导出内容而非恢复 owner。已生成各类 folded/SVG 并实际查看 CPU-active 与
ext4-lock-hold PNG。结束根盘保存为 rootfs-profile-parent-inode-result.img。
