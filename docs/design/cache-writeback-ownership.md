# Cached writeback ownership and stable fault publication

## Problem and scope

High-risk concurrency refactor in the preserved profiling worktree at
affddc3fecec02b31df94573063e4840433c9ebe. This is independently reviewable design,
not maintainer approval or merge readiness. The full namespace-journal-progress
run is1217s,181units175names,8-core/8-GiB QEMU,43.418%activeCPU. Linux is735s.
Full correctness/artifact evidence is in ../profiling/namespace-journal-progress.md.

Private cache fault entry mutex waits are79.078653392s. All raw sites symbolized
against its saved ELF and then disassembled: pin_read_page at804075c4 waits
49.711565872s on io_lock,with_current_read_page at80116ffc waits28.013874384s
on that same field. Their later index acquisitions at8040766c/801170d4 total
1.352327488s; readahead adds0.000885648s. Addresses have the kernel ffffffff
prefix. The77.725440256s io_lock boundary is confirmed,not inferred only from
function names. Exact selection command/output is in
tmp/cache-writeback-ownership.m2tahG/lock-sites.log.

All cached writeback/sync entries currently hold inode-wide io_lock through
backing writes and sync. Mapping pin/publication requires this owner even for
already resident stable pages. Existing buffered resident reads already avoid
it with the updating flag +cache index. Success requires real fault pin and
publication progress at a backing-write/sync boundary,correct redirty and
failure handling,and a fresh identical full QEMU measurement. No predicted
wall-time gain is treated as measured evidence.

## Prior art and alternatives

Internal boundaries reused:CachedFileShared,CacheUpdateGuard,physical page pins,
dirty generations,writeback protection callbacks,retired pages,pending fills,
existing writeback owner and mount cached-write admission. Local histories
ed3d4a1e6,c0b606fc7,1ab948f77,6d4b21529,828538f64 were inspected. Current open
cache/writeback PR search returns#2015 at6d5cc09f45a073680a270ae0b6047b24fd9eaff5,
the previously inspected rsext4 batching scope,not this cached-fault owner.

Linux source980ab36ae5972c83f683b939e50c469c4947229e:

- [filemap_fault](https://github.com/torvalds/linux/blob/980ab36ae5972c83f683b939e50c469c4947229e/mm/filemap.c#L3540-L3694)
  retains a folio,rechecks mapping identity and i_size under page ownership,
  and retries the VMA after dropping mapping ownership for I/O.
- [filemap_map_pages](https://github.com/torvalds/linux/blob/980ab36ae5972c83f683b939e50c469c4947229e/mm/filemap.c#L3884-L3989)
  uses page identity/locking and PTE locking,not an inode-wide writeback wait.
- [writeback state](https://github.com/torvalds/linux/blob/980ab36ae5972c83f683b939e50c469c4947229e/mm/page-writeback.c#L2860-L3060)
  separates dirty state from writeback completion and serializes writable PTE
  dirtying against protection. This is not permission to clear concurrent writes.
- [folio_wait_stable](https://github.com/torvalds/linux/blob/980ab36ae5972c83f683b939e50c469c4947229e/mm/page-writeback.c#L3099-L3116)
  waits only when the backing mapping requires stable writes.
- [reclaim and truncate](https://github.com/torvalds/linux/blob/980ab36ae5972c83f683b939e50c469c4947229e/mm/truncate.c#L320-L366)
  reject reclaim during writeback; truncation has a waiting phase. TGOSKits keeps
  its existing writeback owner across resize rather than importing folio locks.
- [Linux VFS locking](https://docs.kernel.org/filesystems/locking.html#address-space-operations)
  documents the separate address-space,folio and invalidation contracts.

Keep current locking:correct baseline but retains measured serialization.
Remove io_lock only from writeback:incorrect; an eviction can write newer bytes
before the old snapshot and the old write can then overwrite them on disk.
Remove it only from faults:helps stable hits but leaves miss admission behind
writeback and does not improve writeback ownership. Replace the complete cache
with Linux folios/xarray:unnecessary public/runtime expansion. Selected:one
connected private writeback-round/owned-batch protocol plus the existing stable
cache-state rule for fault readers. No new public trait,ABI or dependency.

## State and ownership

```
writeback owner (also excludes truncate)
  -> short io/index section:select pages,retain identity,mark writeback active
  -> no io/index/listener locks:protect writable mappings
  -> short io/index sections:prepare bounded owned byte batches +versions
  -> no io/index/page-byte locks:backing writes
  -> short state section:clean only identical non-redirtied generations
  -> always retire round tracking on success/error
  -> no io/index locks:requested backing sync,including an empty selection
```

The round owns page identity and cleanup,not a borrowed LRU entry. Replace the
two independent writeback booleans with one private Idle/Active/Redirtied state.
Dirtying an active page retains Redirtied even if a later snapshot sees its
current generation. Completion checks physical identity,generation and this
state. On partial I/O failure only completed batches may be clean; remaining
dirty pages and the original error survive. Protection failure and allocation
errors also release every writeback reservation. No lock-taking destructor may
run while its corresponding guard remains held.

Eviction and reclaim must skip active writeback pages before detaching them.
They may still progress on other pages; the existing soft retention target can
temporarily be exceeded,as already happens for mapped/busy victims. Resize
retains the writeback owner across protection,backing length/tail operations,
rollback and retirement,so old writes cannot regrow a truncated file. Ordinary
buffered writers may redirty after snapshot without joining the writeback owner.
Their existing mutation/admission/update protocol and pending-fill invalidation
remain unchanged. New cache misses must never load stale bytes over a current
dirty/active page.

Owned batches bound copied bytes to1MiB (256pages),preserve ascending contiguous
write grouping and partial-EOF clipping,and allocate destination storage outside
the cache index. This replaces the old all-file per-page snapshot plus a second
contiguous copy. More backend calls for very large runs are an explicit tradeoff
to check in the full profile; no workload-specific tuning or CLI parameter.

Stable fault pinning uses cache index +Acquire observation of updating,matching
the established buffered-read contract. If state is tentative,wait/retry only
after releasing the index. Final mapping publication returns stale for tentative
state,wrong identity or changed EOF and keeps the short index section through
PTE installation. It must not wait for io_lock while owning AddrSpace. Mutation
still takes index before changing contents; truncation invalidates mappings only
after detaching under index and retains physical pages until acknowledgements.
No writable mapping capability or page-lifetime promise is weakened.

## Validation and non-goals

Add deterministic tests first,complete the connected candidate before runtime.
Then preserve candidate,restore only original broad I/O ownership for a RED
test,perform three static rounds before RED,and restore candidate with fresh
three rounds before unchanged GREEN/full tests. Real backing callbacks assert
lock availability and execute pin/publication,not timing benchmarks or source
text tests. No QEMU uses the deliberately regressed variant.

Cover every writeback entry,empty sync,dirtying during protection and real I/O,
short/zero/error writes,partial batches,protection/sync failure,active-page
eviction/reclaim,cache identity,EOF/truncate rollback,and mapping retry. Existing
adapter/core/Starry cache-mapping coverage remains relevant. The private I/O
interleaving belongs at the backing capability; no user fault-injection ABI is
introduced and no full syscall compatibility review is claimed.

Three final rounds:ownership/consumer/error audit;strict combined lib/tests
Clippy,xtask feature matrix and actual profile kernel build;fmt,diff and source/
ELF/BIN identity. Only then runtime regressions and full8c8g QEMU using the same
reusable rootfs tg-xtask and source. Compare all181units,output bytes,fsck,raw
record integrity,CPU/wait and wall time. Preserve any negative outcome.

Not changed:portable ext4 transactions,journal policy,block driver,memory-set
placement,shootdowns,scheduler,public syscall arguments/flags,guest workload,
or profiling instrumentation. The remaining65.573s ext4 state-lock wait and CPU
find_free_area hotspot are separate measured limits,not hidden by this scope.

## 当前 dev 的写回迁移

迁移使用 dev 的唯一 `CacheMappingEndpoint`，不恢复旧监听器列表。文件级
`writeback_lock` 串行化显式、后台、周期、全局与容量写回，以及文件布局改变。
一轮写回拥有有限的页号/物理身份选择，并增加页的瞬时 pin，禁止回收该轮页。
保护回调、磁盘写入和 backing sync 均在 `io_lock` 与缓存索引锁外执行。
页的 Idle/Active/Redirtied 状态、脏代际与物理身份共同决定是否清脏；析构路径
无条件释放状态与 pin，包括保护、分配、短写和同步失败。

保留 dev 的每批最多 16 页限制，改为直接生成一个拥有字节的连续批次，消除
逐页 Box 快照再拼接的第二次复制。稀疏页在文件偏移不连续处结束批次，最后
一页按该轮 EOF 截断。写回持有布局写入排他权，文件缩小必须等旧轮结束。

普通缓存写入仍由 mapping-layout、更新守卫和缓存索引发布。更新守卫增加
明确的锁外阶段：完成当前页写入后发布稳定状态、释放 io，执行脏页平衡，再
重取 io 并恢复更新状态。容量清理和低水位写回通过同一批次协议执行；若已有
写回轮持有排他权，写入只请求后台处理并允许暂时超出软容量，避免重入等待。
扩容与 truncate 在布局锁之后、io 之前取得写回排他权，防止旧快照重新增长文件。
现有同线程回调中执行 truncate 的测试不符合该排他协议，需改为确定性交错
验证 truncate 等待写回、最后长度不被恢复；不能以放宽断言代替迁移。
