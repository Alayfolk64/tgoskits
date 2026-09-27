# ext4 重构回归迁移清单

## 判定口径

2026-09-10：20:59 三轮最终静态检查通过；随后定位并修复 ax-io 的既有
切片推进错误，21:16:49 补充三轮静态检查通过。新代码 QEMU 内核 51 项、
rsext4 全套 585 项、适配层 213 项、ax-io 180 项及 2 项文档测试全部通过。
下表保留迁移时的编译状态；实际运行状态以本节和最终检查记录为准。
已编译不等于正确，也不等于旧回归已通过；逐轮状态见
[最终静态检查记录](ext4-final-static-review.md)。
旧实现与未接线旧测试仍保留在工作树和迁移前备份中，不能把文件存在当作覆盖。

QEMU 已执行并通过：`axtest_fs::fs_context_and_inode_identity_rules_hold` 原七项，
`axtest_memory::shared_cache_retirement_and_writeback_rules_hold` 中的 PTE/RSS、
TLB 重试、backend 销毁后的 pending 责任和 clean-page msync 六项断言。
适配层完整重跑已通过，包括 persistence、metadata、inode_writeback、read、
profile、writeback、open-unlinked 缓存和同步句柄组。core 的 42 项 Linux
镜像与 fsck 场景也通过；这不等于完整 syscall ABI 或任意断电交错验收。

原始断言涉及旧缓存形状时，迁移其行为约束，不保留失效的私有实现拼写。
改变契约时明确写出区别，不能以名字相近的新测试冒充完整替代。

## 元数据、读取与 profiling

旧来源：`fs/ax-fs-ng/src/fs/ext4/rsext4/fs/profile_tests/mod.rs`，当前未接入模块树。
新来源位于同级 `fs/tests/`，由 `fs/mod.rs` 的测试模块接入。
`metadata.rs` 和 `profile.rs` 还需要 `profile` feature；已通过组合 Clippy 编译。

| 旧用例 | 新生产路径用例 | 迁移状态/差异 |
| --- | --- | --- |
| `cached_metadata_query_does_not_acquire_ext4_mutex` | `metadata::cached_vfs_metadata_and_length_complete_while_ext4_is_locked` | 已接线，真实 inode metadata，持锁期间禁止再次获取 ext4 |
| `cached_length_query_does_not_acquire_ext4_mutex` | `metadata::cached_vfs_metadata_and_length_complete_while_ext4_is_locked` | 已接线，同一用例另断言 len |
| `cached_inode_query_completes_while_global_filesystem_lock_is_held` | `metadata::cached_vfs_metadata_and_length_complete_while_ext4_is_locked` | 已接线，直接在持有真实锁时调用 |
| `cached_inode_attributes_follow_hardlink_updates_and_unlink` | `metadata::hardlink_metadata_updates_remain_visible_without_global_lock_after_unlink` | 已接线，真实硬链接、mode/owner/size/nlink 和最后 unlink 后两条引用 |
| `missing_cached_inode_uses_locked_reload_before_subsequent_hit` | `metadata::metadata_miss_reloads_under_ext4_before_the_next_cache_only_hit` | 已接线并编译，真实容量压力驱逐，断言一次锁重载、随后持 ext4 锁仍能查 metadata/len |
| `file_data_read_releases_ext4_metadata_lock` | `read::data_io_releases_ext4_lock_and_allows_another_inode_to_progress` | 已接线，实际设备读边界检查 ext4 锁释放 |
| `another_inode_read_finishes_inside_a_file_read_window` | `read::data_io_releases_ext4_lock_and_allows_another_inode_to_progress` | 已接线，在设备回调中完成另一真实 inode 的读取 |
| `file_mutations_hold_the_hardlink_shared_inode_gate` | `inode_io::hardlink_write_append_truncate_and_hole_punch_share_inode_exclusion` | 已接线并编译，逐操作要求观察到 ext4 入口、别名 inode 锁始终持有、返回后释放 |
| `rename_holds_target_inode_gate_before_reclaiming_blocks` | `read::rename_replacement_preserves_the_open_victims_in_flight_data` | 已接线并编译；真实设备读期间替换目录项，旧打开 inode 仍读到原数据、nlink 归零，新名称指向替换 inode；改为最后引用后 reap，不要求 rename 提前抢内容锁 |
| `device_without_shared_reader_keeps_the_existing_read_path` | `read::unsupported_independent_endpoint_uses_the_serialized_data_path` | 已接线，注入实际 fork capability 不支持，检查数据读取持锁 |
| `oversized_read_fallback_returns_the_entire_requested_file` | `read::oversized_read_returns_the_entire_file_through_serialized_fallback` | 已接线，2 MiB + 13 字节逐字节比较，检查未写入的输出尾部 |
| `independent_read_error_releases_both_inode_and_metadata_locks` | `read::failed_data_io_preserves_output_and_releases_inode_protection` | 已接线，设备错误、未发布输出、排他释放 |
| `hold_scope_begins_after_lock_and_ends_before_unlock` | `profile::hold_profile_begins_after_locking_and_ends_before_unlocking` | 已接线，检查真实 scope 开始/结束时锁状态 |
| `explicit_sync_records_lock_owner_and_device_flush` | `profile::explicit_background_sync_profiles_writes_and_flushes_outside_ext4` | 契约迁移：后台 commit 的设备写/flush 应锁外，不能沿用旧持锁断言 |
| `shutdown_records_lock_owner_and_device_flush` | `profile::background_shutdown_profiles_final_clean_io_outside_ext4` | 契约迁移：包含最终 clean 的锁外 I/O，另查 shutdown_attempted |
| `failed_sync_ends_both_scopes_and_releases_lock` | `profile::failed_flush_closes_profile_scopes_and_propagates_the_device_error` | 已接线，实际 flush 故障与全部已开始区间闭合；不要求失败后尚未发生的写 |

新增读取边界还包括 `independent_endpoint_failures_do_not_silently_fall_back_or_publish_bytes`：
只有 Unsupported 允许回退，I/O 和内存错误必须保留，不能伪装成成功读取。

## inode-table 冷块预读

旧来源：`fs/profile_tests/writeback.rs`，当前未接线。

| 旧用例 | 当前状态 |
| --- | --- |
| `inode_table_pre_read_releases_ext4_metadata_lock` | `fs/tests/inode_writeback::sync_table_preread_releases_ext4_and_allows_another_file_read`：真实 sync 冷表块设备读观察，已编译 |
| `inode_pre_read_allows_another_file_read_to_finish` | 同一真实表块读取回调内完成另一文件读取，已编译 |
| `newer_inode_commit_is_not_overwritten_by_a_pre_read_snapshot` | core `owned/tests/inode_writeback::preread_merges_current_inode_and_newer_journal_neighbor`：同块新 journal 邻居与后来 dirty 更新一起保留；checkpoint 过期结果另外拒绝，已编译 |
| `saturated_write_sequence_cannot_accept_a_stale_pre_read` | `detached::tests::exhausted_commit_sequence_cannot_revalidate_an_old_home_read`：实际签发最后 ticket，之后 Overflow、不回绕、不重新接受旧版本，已编译 |
| `failed_table_pre_read_keeps_dirty_inode_for_retry` | core 与 adapter 分别注入设备读错误，检查 dirty/metadata 保留和实际再次读取，已编译 |
| `table_writeback_without_shared_reader_keeps_serialized_io` | `unsupported_table_endpoint_falls_back_without_changing_sync_semantics`：仅预读 fork Unsupported 走同步装配；Io/NoMemory 单独检查不回退，已编译 |

新增 core 用例另覆盖后台模式往返和外部挂载结果拒绝。以上全未执行，不能将
已编译表述为行为通过。准备完成后新脏的其他表块仍走原装配路径；没有声称
整个 filesystem 的所有冷 metadata I/O 都已离开全局锁。

## 原元数据写回与目录一致性

旧来源：`fs/ax-fs-ng/src/fs/ext4/rsext4/metadata_tests.rs`，当前未接线。

| 旧用例 | 当前覆盖与缺口 |
| --- | --- |
| `metadata_update_defers_writeback_until_explicit_sync` | `persistence::metadata_owner_and_times_remain_durable_after_explicit_sync` 已接线并编译，完整 mode/owner/atime/mtime、无隐式 flush、显式 sync 后独立磁盘快照重新挂载断言；未执行 |
| `closing_a_read_file_updates_atime_without_flushing_the_filesystem` | `persistence::read_close_changes_atime_without_forcing_a_filesystem_commit` 已接线并编译，真实 ext4 File/read/Drop，保留 atime 与 flush 断言；未执行 |
| `explicit_metadata_sync_propagates_device_flush_failure` | 新 profile 用例覆盖原始 flush 错误；新 journal 为 sticky abort，不沿用旧故障消失后直接复用同一 owner 的恢复假设，仍需恢复镜像验证 |
| `file_creation_keeps_published_directory_entry_consistent` | 新原子 transaction 与 namespace 用例覆盖内存可见性；仍需提交前/后镜像恢复和 fsck |
| `directory_creation_keeps_published_directory_entry_consistent` | 同样待补恢复/磁盘一致性证据；普通操作不再保证立刻 durable，不能要求未显式同步的新目录必然已落盘 |

core 新增 `ext4/owned/tests/metadata.rs` 已接线：真实失败 handle、嵌套失败 handle、
restart 失败分别检查 reader 私有状态屏蔽、原身份恢复、evict/reload 后的 inode 内容。
这些用例补事务 reader 合约，不冒充上述 crash/fsck 验证。

## inode 适配、分区和内核消费者

| 旧来源/用例 | 新覆盖 | 状态 |
| --- | --- | --- |
| `inode/tests.rs::child_lookup_does_not_reread_root_directory` | 当前按 parent inode 查找 | 仍缺冷祖先块读失败的生产回归 |
| `inode/tests.rs` 的四项 `rdev_*` 往返边界 | `rsext4::disknode::tests::device_number_roundtrip_preserves_migrated_adapter_boundaries` | 已接线，保留 (1,3)、(255,255)、(0,0)、(1,256)、(1,1040)、(8,511) 全部原输入 |
| `rdev_old_on_disk_decodes_correctly` | `literal_linux_device_encodings_decode_without_the_encoder` | 已接线，直接注入 259 而不是调用编码器产生输入 |
| `rdev_new_on_disk_decodes_correctly` | `literal_linux_device_encodings_decode_without_the_encoder` | 已接线，直接注入 0x100100 |
| `block/read.rs::independent_reader_translates_partition_relative_sectors` | `block/region_tests::forked_partition_reads_translate_the_original_relative_sector_once` | 已接线并编译，包含重复 fork，扇区仅转换一次 |
| `block/read.rs::independent_reader_rejects_bounds_alignment_and_overflow_before_io` | `forked_partition_rejects_end_alignment_overflow_and_zero_geometry_without_io` | 已接线并编译，保留原 EOF/alignment/overflow/zero geometry 输入和错误类型，确认底层没有 I/O |
| Starry `file/fs_tests.rs` 的七项 context/inode identity 断言 | `axtest_fs::fs_context_and_inode_identity_rules_hold` | 全部原断言已适配新 DirectoryCursor/symlink/rename 接口并接入实际 axtest；AArch64 SMP=8 编译通过，尚未执行 |

Starry 七项分别是 callback 释放目录锁、原目录快照保持、callback 错误保持状态、
文件身份不查 metadata、目录身份不查 metadata、mount/inode 身份区分、其他文件系统
继续采用 metadata 身份。统一入口逐项调用，没有条件 skip 或源码文本测试。

## 新并发所有权回归

- `mounts/tests.rs`：失败挂载保留/重试顺序、回调并发注册不丢失、关闭 owner 在回调期间
  持有但登记列表锁释放，成功和失败返回后 owner 释放。
- `file/cache/inode_index/tests.rs`：两次 miss 后晚发布者不能替换首个 owner，过期 weak
  可以替换，不同文件系统身份不合并。
- `sync_policy::open_unlinked_hardlink_reuses_dirty_cache_until_final_inode_retirement`：
  最后 unlink 后，仍打开别名必须看到原 dirty cache，而不是按旧盘内容另建缓存。
- `file/cache/tests/eviction.rs`：短写完成、零进展/错误保留、新页读取失败不先移除旧页，
  回收失效失败保持原 frame，缓存插入排他持续到恢复完成，繁忙 I/O 时回收不拆页。
- `file/cache/tests/writeback_owner.rs`：所有 writeback 入口在 PTE 回调前取得同一 owner，
  但不持 cached-I/O/page/listener 锁；失败释放 owner 且保留脏页。

- `busy_lru_mapping_keeps_its_canonical_frame_and_dirty_data`：繁忙映射不能移出 cache，
  新页仍可进入，旧 frame 和 dirty 数据保留。
- `failed_truncate_invalidation_retains_frame_and_retries_before_regrowth`：失败帧在退役
  列表，失效在 cached-I/O/page/listener 锁外，mutation owner 保留；失败时不能 regrow，
  重试成功才释放物理页。以上 cache 用例组合编译已通过，未执行。
- Starry `backend/file/tests.rs` 接入 `axtest_memory` 的统一入口，检查真实 PTE/RSS、
  TLB 失败重试、`writeback_range` 不持 file_data 回调，以及两个地址空间间的繁忙
  cache owner 保留。实际 AArch64 SMP=8 编译检查通过，未执行。
- `failed_shootdown_survives_mapping_removal_until_cache_retirement`：真实页表失效
  注入超时，再移除并销毁 backend；缓存 truncate 必须仍能完成原虚拟地址的
  shootdown，pending owner 不得丢失。实际 AArch64 编译通过，未执行。
- `clean_page_range_still_waits_for_backing_durability`：真实页缓存写完数据后
  注入 backing sync 错误；干净页的 msync 仍必须同步、报错和重试，不能提前
  成功。实际 AArch64 编译通过，未执行；第一轮据此修复空脏页列表的早退。

旧 mmap deferred eviction 路径已移除，全部 listener 成功后才允许移出 frame；
这不代替实际 SMP/TLB 和内存压力验收。页缓存暂时超过保留目标的资源代价需要
在最终 8c8g 运行中观察；逐 syscall 兼容性与完整生命周期复核仍待完成。
