# Private file mappings backed by owned cache pages

## Evidence, scope and success criteria

The complete 8c8g `concurrent-cache-fill` run takes 1684 s / 181 units,
versus 1676 s before that change and the frozen Linux 735 s control. It is
not an accepted speedup. Its largest resolved kernel CPU leaf is memcpy:
2380/54750 active samples (4.34703%). 1832 samples include private file-fault
preparation; one complete cache-to-private-frame copy stack has 1814 samples.
Mandatory anonymous first-write clearing is a separate, similarly sized
leaf and is not removed by this design. Full evidence is in
[`concurrent-cache-fill.md`](../profiling/concurrent-cache-fill.md).

Current `CowBackend::prepare_frame` allocates and copies even for initial
read/execute faults on aligned, complete file pages. The filesystem keeps
another copy of those same bytes. Users include executable/library faults
and ordinary private file mappings during the unchanged tg-xtask workload.
Success requires real read mappings of the same cache frame, no private
frame allocation/copy on those eligible read faults, private first-write
isolation, correct ownership through all mapping lifecycles, and a complete
cold compile with matching output. A mechanism test is not proof of speed.

This is high-risk shared ownership and memory-management work. Independent
domain review is required before merge; this local experiment does not
authorize merge, commit, push, or public submission. No new disk format,
dependency, boot policy, physical board or profiling backend is introduced.

## Prior art and alternatives

Local Linux source is pinned to
`980ab36ae5972c83f683b939e50c469c4947229e`, distinct from runtime control
Linux 6.18.35-0-virt. Its
[`do_read_fault`/`do_cow_fault`](https://github.com/torvalds/linux/blob/980ab36ae5972c83f683b939e50c469c4947229e/mm/memory.c#L5840-L5911)
separate read mapping from private-write copying.
[`finish_fault`](https://github.com/torvalds/linux/blob/980ab36ae5972c83f683b939e50c469c4947229e/mm/memory.c#L5603-L5675)
consumes a page reference for the PTE;
[`filemap_fault`](https://github.com/torvalds/linux/blob/980ab36ae5972c83f683b939e50c469c4947229e/mm/filemap.c#L3540-L3682)
pins/locks cache backing, rechecks mapping identity and EOF after sleeping,
and retries an obsolete page. Borrow these ownership and revalidation
contracts, not Linux-specific folio layout, rmap machinery or locks.
The public [mmap contract](https://man7.org/linux/man-pages/man2/mmap.2.html)
requires private writes to remain private and partial EOF pages to be zeroed.

Internal inspection covers cache fill/update/read/resize/writeback/retirement,
the shared FileBackend listener, counted COW frame retirement, unlocked
PrivateFault, loader PT_LOAD boundaries, fork and forced kernel copies.
Open PR [#2000](https://github.com/rcore-os/tgoskits/pull/2000), head
`6c1b006a852a458ad577f92ea8a5978a56fb4e59`, widens private fault readahead
and still copies into private frames; its complete diff was read. It is
adjacent work, not the cache-backing ownership change here. Open-issue
search found broad ext4/app work (#2209/#1507), not an existing implementation.

| Alternative | Decision |
| --- | --- |
| Keep copying on every private read fault | Retains the measured duplicated work and memory. |
| Optimize memcpy instructions only | Still copies and allocates identical file pages per mapping. |
| Wider fault-around batches | Can reduce fault overhead, but still copies; do not conflate with PR #2000. |
| Global private snapshot cache | Adds a second cache and content-version policy beside the existing owner. |
| Return a cache physical address without a lease | Unsafe across eviction/truncate and private writes. |
| Owned read-only cache backing, first-write COW | Selected, extending existing cache and COW boundaries. |

## Ownership and publication

The cache entry owns dirty/writeback state. Its physical allocation becomes
a separate reference-counted owner retained by a typed `CachedPagePin`.
Dropping an index entry cannot free a pinned frame. Only the final physical
owner calls the filesystem page provider; a cache frame must never enter
the private allocator's deallocation path. Kernel byte access is serialized
by a page-local sleepable guard shared by entry access and COW source copies.
A pin never exposes a mutable byte slice or writable mapping permission.

Pin lifetime is not mapping validity. Before publishing a read PTE, the
fault must revalidate the current VMA identity/permissions, canonical cache
entry identity and applicable file length. Cache I/O exclusion protects
that last validation through publication. An obsolete pin is released and
retried, not installed over a concurrent truncate/replacement. No index,
frame-table or address-space lock spans the initial backing read.

Extend the existing counted PTE ownership boundary to distinguish private
allocator backing from pinned cache backing. Prepared owners transfer only
after successful PTE publication; failure/stale plans retire their physical
reference. Fork increments the same physical mapping count and preserves
the backing kind. Last mapping/source-pin release removes the frame index
entry and releases its actual backing owner after completed invalidation.
Zero-image backing remains the separate uncharged, immutable case.

A cache-backed PTE is always read-only, even with one mapping reference and
even after mprotect adds WRITE. User or forced kernel first-write handling
must allocate/copy privately, complete replacement invalidation, then
release the cache mapping reference and reclassify File RSS to Anon. It
must never use the anonymous exclusive-frame permission-upgrade shortcut.
Source pins survive unlocked preparation and a racing unmap/fork/truncate.

Hardware accessibility and physical ownership are separate queries.
`query_occupied_leaf` reuses the generic walker's existing occupied-leaf
lookup, includes PROT_NONE descriptors, and returns complete-leaf geometry
without granting access or a new lifetime pin. Cache invalidation, private
fork/rollback, RSS reconciliation and page relocation use this query while
holding the address-space owner. Ordinary fault/access queries keep their
hardware-present behavior. Dropping private file pages through
MADV_DONTNEED uses the same completed COW unmap boundary as anonymous pages,
retains the VMA/file origin, and does not discard other mappings or the file.
MADV_FREE's existing private-anonymous validation remains unchanged.

This distinction matches the pinned Linux tree's
[`pte_present`](https://github.com/torvalds/linux/blob/980ab36ae5972c83f683b939e50c469c4947229e/arch/arm64/include/asm/pgtable.h#L135-L150)
and [`pte_protnone`](https://github.com/torvalds/linux/blob/980ab36ae5972c83f683b939e50c469c4947229e/arch/arm64/include/asm/pgtable.h#L550-L573)
software-present-invalid ownership, used by
[`copy_pte_range`](https://github.com/torvalds/linux/blob/980ab36ae5972c83f683b939e50c469c4947229e/mm/memory.c#L1288-L1324).
Linux's
[`madvise_dontneed_single_vma`](https://github.com/torvalds/linux/blob/980ab36ae5972c83f683b939e50c469c4947229e/mm/madvise.c#L851-L862)
zaps eligible file-private PTEs too. Do not change Starry's generic
`PageTableEntry::present` definition to mean Linux software-present: its
translation callers require hardware-present state.
Linux's [`move_ptes`](https://github.com/torvalds/linux/blob/980ab36ae5972c83f683b939e50c469c4947229e/mm/mremap.c#L265-L319)
likewise skips only empty entries and retains the physical owner through
the old translation's invalidation. Starry keeps its existing exclusive
address-space mutation and completed unmap boundary, not Linux's rmap locks.

## Mapping invalidation and boundary cases

Private cache-backed mappings must participate in existing file-page
eviction and discard notification. The mapping owner retains only a weak
address-space reference in the listener to avoid a cycle; splits preserve
immutable file coordinates within one address space. Fork and relocation
create distinct listener ownership, retaining their exact file-page origin.
Each listener computes one VA and performs one VMA lookup, rather than
scanning every VMA on each cache-page eviction. Match the current VMA identity and exact physical page
before unmapping. Unmap through the completed retirement boundary, remove
the corresponding RSS charge, and never touch a replacement private page.
Eviction uses a nonblocking address-space attempt; rejection retains the
cache frame. Truncate callbacks run after releasing cache I/O exclusion.
Any failed invalidation must retain enough ownership to retry, including
when a mapping object is subsequently removed.

Only complete, aligned 4 KiB file-backed read/execute pages are eligible.
Unaligned PT_LOAD prefixes, segment-limited suffixes, partial EOF, initial
write faults, direct/non-cache files, anonymous and non-4KiB mappings keep
their existing owned initialization path. Those are different byte/ownership
contracts, not guessed fallback for an error. Eligible cache-read errors
remain explicit errors. Loader preparation of an unpublished bare AddrSpace
must not create a cache PTE without an invalidation owner; preserve private
initialization until the live address-space owner can bind notifications.

Preserve existing ordinary read/write, shared mapping and dirty-writeback
contracts. This work does not claim to fix every pre-existing truncate case
for already-private copied pages or provide a complete syscall ABI audit.
Direct and indirect entry points to audit include mmap, munmap, mprotect,
mremap, madvise, fork/clone/clone3, execve/execveat, read/pread/readv families,
write/pwrite/writev families, truncate/ftruncate, close, msync/fsync/fdatasync,
and forced debugger/process-memory writes. Final compatibility claims must
name the actually covered entries and preserve unresolved limitations.

Lock order for publication is address-space -> cache I/O -> cache index;
physical-reference index lookup releases its lock before reference mutation.
No page-byte guard may cross a faultable Writer, storage I/O, page-table
mutation, callback, or mapping lock acquisition. Cached writes may briefly
take index -> page-byte guard; a standalone source copy takes only the byte
guard. Eviction listener snapshots release listener/index guards before
trying the address space. No new sleepable lock is acquired under an IRQ
reference-count lock.

## Validation sequence

### 当前 dev 的迁移所有者

当前缓存页的瞬时 `CachedPagePin` 只阻止索引删除，没有独立物理所有权。
迁移先将 `PageCache` 的分配移入引用计数的 `CachedPageBacking`，由该对象
唯一释放 `FsPage`，并通过页级字节锁序列化内核复制。缓存条目继续拥有脏状态、
写回状态和瞬时 pin 计数。瞬时 pin 不变为永久 pin；额外保留物理 backing
不能代替缓存身份、EOF 或映射有效性证明。

`FilePageDomain` 仍是每个 `CachedFileIdentity` 唯一的映射端点，并为共享映射
和私有缓存映射提供同一个 PageObject。其 FrameLease 保留物理 backing；COW
写入通过 backing 的字节锁复制到私有页，不能因映射计数为 1 而直接授予 WRITE。
Cow 的源内弱索引仅用于物理地址查找，发布/撤销与缓存页退休继续进入已有域。
缺页、锁内 populate、fork、内核强制写、回滚、移动和解除映射必须一并适配。
未发布的裸地址空间仍保留复制路径，避免创建没有生命周期所有者的缓存 PTE。

最终 PTE 发布需要验证瞬时 pin 对应的缓存身份、更新屏障和完整页 EOF；取消
必须释放准备阶段的域保留。缓存物理 owner 在 PageObject 与退休回执存活期间
不能释放，回收不依赖恢复旧监听器列表或第二个全局物理帧表。已有系统回归
及物理生命周期测试须移入当前入口后才能验证；完整迁移前不启动完整编译。

Add deterministic production-path coverage for physical pin lifetime,
canonical identity rejection, shared read PFNs without private allocation,
independent private writes, fork ownership/rollback, pinned source retirement,
eviction/truncate invalidation, stale fault commits, EOF/ELF tails, mprotect,
mremap and forced kernel writes. Use allocator/read counters and actual PTEs,
not Rust source-text assertions or probability-only concurrency tests.
Existing private-fault, zero backing and shared retirement regressions stay.
System coverage goes in a normal qemu/system subcase, discovered and run by
xtask; run the same relevant test binary on Linux QEMU.

No runtime test starts until the entire refactor completes three final
static rounds: ownership/caller review; strict targeted clippy and actual
consumer build; final diff, format, symbols, image and 8c8g configuration
audit. Then run low-layer regressions and QEMU integration before a fresh
complete cold compilation. Preserve the 1684/1676/1694 artifacts, frozen
rootfs tg-xtask/workload/toolchain, complete outputs and full sampling. No
performance or runtime acceptance is established for this design yet.
