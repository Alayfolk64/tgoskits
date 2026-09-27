# mmap 成功 hint 后跳过备用扫描

## 原因与方案

上一轮 `vma-range-window-600` 的非 idle 样本中，rustc 经 sys_mmap 进入
find_free_area 的叶子栈占 1,278 / 7,161（17.8467%）。源码使用
`hint_search.or(base_search())`；即使前者成功，后者也立即执行，结果随后丢弃。
修复的成功标准是：hint 成功只执行一次搜索；hint 失败才从 base 重试；
返回地址、搜索上界、对齐要求和 NoMemory 保持原样，然后验证相同 QEMU 窗口。

内部已检查 mmap/mremap 调用链和 mmap.rs 历史。mremap 的 find_free helper 已使用
or_else，但其 limit 是整个地址空间；mmap 必须保留用户栈 guard 上界，不能直接
复用那个 helper。外部依据为 [Rust Option::or 文档](https://doc.rust-lang.org/std/option/enum.Option.html#method.or)，
版本 1.98.1（48a229cea，2026-09-01）；文档明确 or 立即求值，or_else 按需调用。
编译验证仍使用仓库固定 nightly-2026-07-15。

保持重复扫描没有收益；修改 VMA 索引、增加 mmap hint 状态或改分配策略涉及更多
不变量，此处无需使用。选择把 hint/base 的顺序查询提取到私有 placement 模块，
sys_mmap 通过闭包调用原 AddrSpace::find_free_area，用 or_else 惰性执行备用查询。
边界仅在 syscall::mm 内可见，不新增公共 API、锁、unsafe、持久状态或 syscall 行为。
查询仍在原 aspace 锁内，底层只读搜索没有必须保留的副作用。

本轮是保持用户可见语义的内部性能修复，不声称新增 Linux ABI 兼容能力。
回归位于真实查询策略与 MemorySet 层，而不是计时 syscall：计时不能确定性证明
重复查询已消失，宿主也无需初始化 CPU 和硬件页表。测试用轻量 backend 只隔离
页表安装，实际 MemorySet 空闲地址查找和生产放置策略都参与执行。

## 验证

先原样提取 eager 行为，再运行测试：成功 hint 用例稳定记录
`[0x4000, 0x1000]` 而非 `[0x4000]`，失败；其它两项通过。
改为 or_else 后同一 3 项全部通过，覆盖成功 hint、失败回退和地址耗尽错误。
第一次测试编译因 StarryError 不实现 PartialEq 返回 E0369，改为成功值断言和
错误变体匹配后才进入上述红绿回归；没有修改生产错误类型。

`cargo fmt --package starry-kernel`、实际 aarch64 smp/guest-profile 的
`cargo clippy --no-deps` 均通过，沿用前轮已核对的 xtask 参数组合。
`cargo xtask starry build -c apps/starry/macos-selfbuild/build-aarch64-unknown-none-softfloat.toml`
通过，保持 SMP=8、NVMe、内核帧指针采样与相同 tg-xtask 输入。
完整平台 clippy 矩阵和宿主全套测试的既有本轮限制见 vma-range-scans.md，
本次不把定向检查表述为全套通过。

## 600 秒窗口结果

产物目录为 `target/profiling/arceos-helloworld/starry/lazy-mmap-window-600`。
ELF SHA-256 为 `49db8b680c06ab0474760a1b20ca199af45fedce9b164ca41d3a6fdad19c2bf9`；
BIN 为 `6e13b73e7782e0935ba9f70fc0b1bf375abbafcc1028b20f9ea8e29554e25ee2`。
保持前轮源码归档、tg-xtask 指纹、根文件系统、8 vCPU/8 GiB/NVMe 和采样频率不变。
内核采样窗口 604.514 秒，退出码 124 是预期窗口超时，编译未完成，日志中启动
11 个编译单元；所有采样丢失/跳过计数为 0。导出后 5 个文件 SHA 校验通过，
正常关机后的 `e2fsck -fn` 返回 0。

| 指标 | VMA 范围扫描候选 | 再加惰性 mmap 查询 |
| --- | ---: | ---: |
| 日志编译单元数 | 11 | 11 |
| CPU idle 样本占比 | 85.2250% | 85.0148% |
| 非 idle 样本 | 7,161 | 7,177 |
| find_free_area 叶子样本（所有任务） | 1,280 | 813 |
| mutex 累计等待 | 2,872.066 s | 2,886.239 s |
| 同步块读累计时间 | 409.075 s | 362.231 s |

已查看 `rendered/mutex-wait.svg` 的实际渲染图：缺页处理经 CowBackend::populate
进入 FileBackend::read_at，仍是最大等待路径。仅 ld-musl 与 rustc 两条完整栈
即占 mutex 等待 77.9711%；其它任务还有相同路径。CPU 等效使用约 1.20 / 8 核，
最忙 CPU 2 活跃 69.9059%，其余核只有 4.4637%–13.7885%。
find_free_area 仍占非 idle 叶子样本 11.3279%，但重复扫描减少并没有带来本窗口
可见的编译吞吐提升。不能声称低 CPU 占用问题已经解决。

等待指标按线程累计，可能相互重叠；page-cache/ext4 inclusive 包含其内部等待，
不能与 mutex 相加当作墙钟时间。下一项单独验证大文件缓存容量假设，见
[large-file-cache.md](large-file-cache.md)。

## J 窗口的指令级复核（2026-09-10）

`file-fault-window-600` 的相同 ELF 中，`find_free_area` 位于
`0xffffffff8000a3d4..0xffffffff8000a620`。原始 CPU 样本在这个地址区间
合计 950 次，恰好等于 renderer 的同名叶子统计（活跃样本的 7.4265%）。
主要采样 PC 为 `0xffffffff8000a518`（333 次）、`0xffffffff8000a50c`
（253 次）、`0xffffffff8000a598`（131 次）；对应反汇编均在后继区域迭代
和逐间隙检查循环中，而不是调用 `.last()` 的前驱定位部分。

已检查固定 nightly-2026-07-15 的 alloc BTreeMap::Range 实现：`last()`
直接调用 `next_back()`，故不能通过更换这两个等价写法声称优化。MOSS 当前
vendor/memory_set 的 find_free_area 也保留相同 first-fit 扫描，不能说它已经
提供可直接移植的增广间隙索引。此复核未修改 mmap 放置策略或搜索语义。
