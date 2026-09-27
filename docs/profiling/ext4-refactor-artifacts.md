# ext4 重构冻结产物清单

本清单属于 `ext4-background-writeback-refactor.md` 的工作区保护记录。
2026-09-10 重构期间仅执行文件检查、完整哈希和只读镜像导出；没有启动测试、
QEMU 或 fsck。下列旧内核不是重构后的性能结果，不作为新代码验收依据。

## 源码起点与备份

工作区 `/home/wuxun/Projects/tgoskits`，分支 `profiling/manual-20260907`，
HEAD `affddc3fecec02b31df94573063e4840433c9ebe`，开始重构前已有未提交修改。
事务迁移基线固定为 `cc8faa92227f1f96dacd4bde1c873458b646e731`。

| 文件 | 字节数 | SHA-256 |
| --- | ---: | --- |
| `tmp/ext4-refactor-backup.HQPm3n/source.tar.gz` | 1441340 | `caf52a31e4d6864be5e6474b9bd5f0ff3578b78553e70421b8b2e33f5a766ee9` |
| `tmp/ext4-refactor-backup.HQPm3n/worktree.patch` | 274550 | `25498110084738c3b6aa6dedc5c0ad6cbdea60eb0833455940e7be98995b2179` |

归档包含 fs、Starry kernel、axruntime、rdif-block、profiling 文档和 Cargo 清单，
也包含这些目录的未跟踪源码。patch 保存当时所有已跟踪差异。备份只用于恢复
和迁移对照，不从中编译正式代码，也不拿备份替代回归测试覆盖。

## 冻结工作负载

| 产物 | 字节数 | SHA-256 |
| --- | ---: | --- |
| `tmp/axbuild/rootfs/rootfs-profile-host-config-base.img` | 17179869184 | `9968004595dec398480417e210d64f9ed509801d4325d4ab22f43951983f5ea3` |
| `target/starry-macos-selfbuild/arceos-helloworld-profile-source.tar` | 94382080 | `7de46545454e3f5562d78a1a065bf2ed1a736cf807f4cbf05b30d98ab2725147` |
| 根镜像内 `/opt/tgoskits-profile/bin/tg-xtask` | 33731720 | `e6823ab3b6a6d944266fc31d5e54814f463466f88949849ba139b6c575f7e6f1` |

前三项均重新计算完整 SHA-256。tg-xtask 使用不带 `-w` 的 debugfs 从冻结镜像
只读导出到 `tmp/ext4-artifact-inspection.9l3fXJ/tg-xtask` 后计算哈希；没有执行
该二进制，也没有改写镜像。此导出是可丢弃的校验副本，后续 guest 继续复用根盘
中的原文件，不重建 tg-xtask、不替换冻结源码归档。

后续测量只在 QEMU：8 vCPU、8192 MiB、600 秒内核 profiling 窗口。
kernel 配置使用 `apps/starry/macos-selfbuild/build-aarch64-unknown-none-softfloat.toml`；
guest 的构建命令、离线工具链、jobs 与缓存清理边界维持冻结工作负载，
不得把 guest 日志的 `SMP=1`（被编译 hello 应用配置）误读成宿主 QEMU CPU 数。

## 重构前最近窗口

目录：`target/profiling/arceos-helloworld/starry/clean-cache-preread-window-600`。

| 文件 | 字节数 | SHA-256 |
| --- | ---: | --- |
| `starryos.elf` | 17462624 | `49713f36eab6f86c90f9cba5bfaa464d92b10502f29ca6e226669d3edce058db` |
| `starryos.bin` | 14385152 | `e87d1ff5a052a8d3a3772c53398481bd467142e7012081aff38a22e9847cf6a5` |
| `changed-sources.tar` | 460800 | `67a5e5291129985f42188e0944d57f7e447a261f68f1e81018c3eb83bf1fef0e` |

已重新计算三项完整哈希，与 `host.meta` 一致。`qemu.log` 最后的 guest 标记为：

```text
===STARRY-ARCEOS-HELLOWORLD-PROFILE-WINDOW-PASS elapsed=608 rc=124 build_completed=false tg_xtask_reused=true===
```

这里只确认窗口结束、构建未完成、tg-xtask 复用；不把标记中的 PASS 表述为内核
回归测试通过。21:12 已从未修改的结束盘导出并渲染；5/5 内容 SHA-256 通过。
debugfs rdump 因普通用户不能恢复 guest root 所有权报告 Operation not permitted；
内容完整校验通过不代表所有权恢复成功。21:17 门槛后 `e2fsck -fn` 退出 0，
21 条 extent tree 可收窄提示均回答 no，未修复或修改镜像。
实际 profile.meta 为 608 秒、42 编译单元、39 个不同 crate，source_reused=true。
活跃样本 15635/47412（32.9769%），CPU/wait/pending 丢样均为 0。
ext4 持锁 389113633632 ns，其中 sync_to_disk 为 237015095712 ns（60.9115%）；
ext4 锁等待累计 1643945410608 ns，跨任务累计不能当作单次墙钟时间。
原始与 SVG 位于该实验目录的 `artifacts/` 与 `rendered/`。
`clean-disk-read-cache-window-600/rendered/summary.json` 是更早已渲染的基线，
不能与最近窗口混为一组结果。21:20 将旧结束盘无损改名保留为
`tmp/axbuild/rootfs/rootfs-profile-clean-cache-preread-window.img`。
原固定测量路径重新从只读 host-config-base 复制，完整 SHA 仍为 99680045…；
只给新副本增加属主写权限，未修改冻结起点或删除旧盘。

## 静态检查的宿主依赖

Starry 全 target Clippy 的 x86_64 配置依赖 `x86_64-linux-musl-gcc`。
`lwprintf-rs 0.3.3` 对裸机目标按架构构造此名称；其旧错误显示 `gcc -print-sysroot`
容易误导，系统原有 gcc 并未缺失。先前检查在第 82/110 项因此失败。

从 [musl.cc 发布站](https://musl.cc/) 获取 `x86_64-linux-musl-cross.tgz` 和其
`SHA512SUMS`，核对发布方校验值、2755 个归档路径及 31 个链接后，使用
`tar --keep-old-files` 安装到新的用户依赖目录
`/home/wuxun/.local/toolchains/x86_64-linux-musl-cross/`，未覆盖已有工具链。
这是社区工具链发布站，不表述为 musl 官方背书。

归档 115063639 字节，SHA-512：
`52abd1a56e670952116e35d1a62e048a9b6160471d988e16fa0e1611923dd108a581d2e00874af5eb04e4968b1ba32e0eb449a1f15c3e4d5240ebe09caf5a9f3`。
`gcc --version` 返回 `11.2.1 20211120`，`-print-sysroot` 返回安装目录中的有效路径。
仅检查进程的 PATH 前置该 `bin`，没有改用户 shell 配置或 guest 工具链。
随后 Starry 110/110 Clippy 配置退出 0，结束于 2026-09-10 17:15:08 +0800。
这是重构中间版本的证据，不计入最终三轮静态检查。

全 feature 检查另会触发 `aic8800` 的构建期固件下载。17:45 第 22/110 项
因 raw.githubusercontent.com 连接被重置失败，构建脚本退出 101，xtask 退出 1。
没有修改驱动或跳过该配置：按 `drivers/net/aic8800/build.rs` 固定的 firmware commit
`c56f910044cc854d6c553bcb9a644f3bca5a4c38`，核验已有构建缓存的全部 12 个 bin
SHA-256，与 build.rs 的固定值一致后，复制到
`tmp/ext4-static-firmware.pha7d1/firmware/`。检查进程设置项目已有的
`AIC8800_FIRMWARE_DIR`，构建脚本仍逐项校验哈希；没有执行固件或改动 guest。
随后同一 Starry 检查于 17:54:45 完成 110/110，退出 0，耗时 7 分 21 秒。
正式源码仍在 drivers/，此临时目录只存可重新生成的固件下载缓存。
