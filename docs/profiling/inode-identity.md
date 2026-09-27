# 关闭文件时直接获取 inode 标识

## 问题与方案

G 窗口中 `ld → release_locks_on_close → File::inode_key → Location::metadata →
Ext4Filesystem::lock` 累计 mutex 等待 240,366,153,360 ns，占 10.4581%。
这不是 CPU 执行耗时，也不能直接等同于可节省的墙钟时间。

`Location::metadata` 从 inode 获取全部元数据后，只把 device 覆盖为
`mountpoint.device()`；ext4 的 metadata.inode 和 NodeOps::inode 都来自同一个
只读 ino 字段。直接使用已有 mountpoint.device 与 Location::inode 即可得到
相同锁标识，无需读取长度、权限、时间戳或等待 ext4 锁。

只改变 Starry File/Directory 对 ext4 的 FileLike::inode_key。其它文件系统保留
metadata 路径：检查发现 FAT file_metadata 目前填充固定 inode，而 overlay
可能在 copy-up 后从不同节点读取 metadata；不能把 ext4 的等价性推广给它们。
以已有 FilesystemOps::name 边界选择 ext4（缓存共享路径也使用这一边界），
不硬编码文件名、进程名或某个具体编译负载。保留 advisory lock 表、
关闭顺序、OFD 所有权、POSIX 锁释放和唤醒顺序；不新增缓存、API、锁或 unsafe。
ext4 普通文件及目录在元数据读取失败时仍然有 inode 标识，不能因为 stat 失败而
跳过锁清理。MountTableFile 继续委托 File；其它 fd 类型实现不变。

替代方案包括缓存 metadata、提前检查全局锁表是否为空、改 ext4 锁粒度；前者
引入重复状态，第二项需要新的并发证明，第三项改动较大。选择复用既有身份接口。
源码历史检查覆盖 file/fs.rs 最近四次提交，GitHub 开放 PR 查询关键词
`inode metadata` 未发现结果；不声称已穷尽所有同义议题。

Linux man-pages 6.18 的 [fcntl_locking(2)](https://man7.org/linux/man-pages/man2/fcntl_locking.2.html)
规定关闭该文件的任意 fd 释放本进程 POSIX 锁，OFD 锁则在最后关闭时释放。
[close(2)](https://man7.org/linux/man-pages/man2/close.2.html) 也说明 close 的锁清理。
本修改只去除标识获取的 metadata I/O，不重写上述协议或声称完整锁 ABI 审核。

## 风险与验证计划

涉及 close/fcntl 锁身份，按高风险局部修改记录，合入前仍需边界维护者审核。
保持原挂载 device 语义，不顺带修改同文件系统多次挂载时已有的身份策略。
Bind/克隆挂载当前复制源 device，普通新挂载从 DEVICE_COUNTER 获取新 device。

真实 FileLike 入口接入会返回 EIO 的测试 node，验证 File、Directory 的 key
不依赖 metadata；重复打开相同 inode 保持 key，不同 mount device 不混淆。
旧实现必须先失败，修改后通过。底层错误注入比用户态测试更直接：当前 syscall
层没有可控的“仅 metadata I/O 失败”注入接口。QEMU 原始编译覆盖真实 ext4/close。
fmt、定向库测试、clippy 和目标内核构建后进行 600 秒新窗口，保存原始采样、
对应 ELF、所有文件哈希与离线 fsck，量化 CPU 和等待热点变化。

## 已执行验证

- 三项 FileLike 回归：旧实现 0/3、退出 101（metadata EIO 导致 None）；改后 3/3。
- `cargo fmt -p starry-kernel -p ax-fs-ng` 和 `git diff --check` 通过。
- ax-fs-ng 的 host-test/ext4/vfs 库测试 96/96 通过，xtask clippy 8/8 通过。
- Starry xtask clippy 自动展开 110 项，包括无关实板功能；16 项已通过，运行
  第 17 项 rknpu 时主动 SIGINT 中止，退出 130，不能记为全矩阵通过。
  检查过 xtask clippy --help，没有 target/feature 筛选入口，故用 native Cargo
  明确匹配 aarch64/SMP8/guest-profile/NVMe/virtio-net，生产 clippy 通过。
- 补充 host 测试 clippy 退出 101：136 项诊断位于本候选未修改的已有测试代码，
  包括常量断言、无效类型转换、Copy 类型 clone 与测试模块后仍有 items。
  原文完整保存并展示于 `tmp/inode-identity-host-clippy.log`，没有新增 allow。
  这项检查未通过，不因目标生产 clippy 通过而覆盖该结果。
- `cargo xtask starry build -c apps/starry/macos-selfbuild/build-aarch64-unknown-none-softfloat.toml`
  通过，保留 SMP8、8 GiB、TCG 和冻结工作负载。

## 当前 QEMU 窗口

目录 `target/profiling/arceos-helloworld/starry/inode-identity-window-600`。
ELF SHA-256：`043b760eb870610b3574214030adab826055428acebdd51c251f9489c5b2bc91`。
BIN SHA-256：`04b4ad4cf23ade0b5ecb03eca1c2c1beb57907adb3b3aa8fba6a34d7d1b20542`。
2026-09-09 17:55:25 UTC 启动，600 秒采样由 guest runner 在预处理后开启。
本窗口不包含已否定的异步 create 候选。这个已保存 ELF 的初版曾对所有
File/Directory 直接取 inode；运行期间的兼容性源码检查要求将快路径收窄到
ext4，当前源码已收窄。因此此窗口只作为初版 ext4 工作负载诊断，不能作为
当前收窄版本的完整性能验收；新版本需重新构建、保存 ELF 并单独测量。

## 初版窗口结果与兼容边界

实际采样 604.512285776 秒，31 个启动编译单元、29 个不同 crate，rc=124，
build_completed=false；tg-xtask 和其源码复用标记均为 true。
导出文件 5 项 SHA-256 全部通过；原镜像离线 `e2fsck -fn` 退出 0，无需修复。
CPU 共 47417 样本，活跃 11653（24.5756%），idle 35764（75.4244%）。
低于前轮 27.5002% / 34 个编译单元，不能把此次身份查询优化宣称为整机提速。

FileLike::inode_key 的 mutex 栈累计为 0；全部 mutex 等待反而为
2553842349088 ns。ext4 全局锁调用点占 66.6984%，缓存 read_at 调用点占
30.4217%。已实际查看 mutex-wait.png、cpu-active.png 并核验对应 ELF 符号。
用户缺页调用链占全部 mutex 等待 41.2427%，后续优先处理此路径的串行化。

增加非 ext4 元数据身份测试后，直接 NodeOps inode 的宽泛版本确定性失败：
返回 (device,42)，正确结果为 (device,77)，exit 101。收窄到 ext4 后
真实 FileLike 入口四项回归全部通过。当前收窄版本与缓存命中并发一起保存为
`cache-hit-window-600` 的 ELF；该轮结果不归因于 identity 单独变化。
