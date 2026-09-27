# Cargo 初始化时的非法指令探测

2026-09-10 的 `starry/host-config-baseline-window-600` 日志共 49 条
`IllegalInstruction`，分成 7 组，每组地址偏移顺序相同。首组后打印 Cargo
版本，其余组后编译继续；最终窗口 613 秒到时 rc=124，不是 SIGILL 终止。

关机后从该轮结束盘导出 `/opt/rust-nightly/bin/cargo`，未读取运行中的根盘。
文件是 AArch64 PIE，SHA：
2142dd1e0de20b631612d8a60c4ea3bfb90ab6e93d05ff5bb13ae2743276c25b。
二进制字符串包含 OpenSSL 3.6.3；`/opt/cargo-nightly-sysroot` 显式使用
`/lib/ld-musl-aarch64.so.1` 启动这个 Cargo。

对实际 ELF 反汇编，7 种指令按日志顺序对应如下。表中基址是根据全部地址
差值推断的加载偏移，不是本轮通过 GDB 或 `/proc/pid/maps` 捕获的运行时基址。

| 探测函数 | ELF 地址 | 后六组日志 PC（+0x1000） | 首组日志 PC（+0xc1000） |
| --- | --- | --- | --- |
| `_armv8_sm3_probe` | 0xffbca0 | 0xffcca0 | 0x10bfca0 |
| `_armv8_sm4_probe` | 0xffbc70 | 0xffcc70 | 0x10bfc70 |
| `_armv8_sha512_probe` | 0xffbc78 | 0xffcc78 | 0x10bfc78 |
| `_armv8_eor3_probe` | 0xffbc80 | 0xffcc80 | 0x10bfc80 |
| `_armv8_sve_probe` | 0xffbc88 | 0xffcc88 | 0x10bfc88 |
| `_armv8_sve2_probe` | 0xffbc90 | 0xffcc90 | 0x10bfc90 |
| `_armv8_rng_probe` | 0xffbd84 | 0xffcd84 | 0x10bfd84 |

[OpenSSL 3.6.3 armcap.c](https://raw.githubusercontent.com/openssl/openssl/openssl-3.6.3/crypto/armcap.c)
在没有选用 auxv 查询的配置下，注册 SIGILL handler，通过 `sigsetjmp` 和
`siglongjmp` 检测这些指令；探测失败不设置对应能力位，然后继续执行。
源码顺序、实际 ELF 指令、全部日志偏移及后续继续运行相互吻合，支持这些
警告来自正常 CPU 能力探测。不能仅凭异常日志断言编译崩溃，也不能据此修改
内核信号语义或强制声明 CPU 支持这些扩展。未屏蔽警告、未设置能力覆盖变量。
