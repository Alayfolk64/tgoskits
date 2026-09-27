# Concurrent file-cache fill transactions

## Problem and evidence

The complete `anonymous-zero-backing` compilation is 1676 s, 181 units,
8c8g AArch64 QEMU, versus the unchanged Linux 735 s control. It is not an
independently established improvement over 1694 s. Its largest kernel CPU
leaf remains required anonymous first-write clearing (2847 samples).

The largest removable serialization boundary established so far is private
file-fault cache loading. The saved ELF and complete raw stacks identify
422386610400 ns at `CachedRead::read_page` PC `0xffffffff803d88f4`
(`io_lock`) and 344292109856 ns at `0xffffffff803d8800`
(`page_cache`, before the update-state check). These are the dominant exact
stacks, not totals for all callers, and overlapping waits are not wall time.
The existing read window releases the cache index for backing I/O but keeps
the inode-wide I/O mutex. Single-page population, dirty eviction and their
mapping callbacks can still hold the cache index across storage operations.

Users are buffered reads and private executable/data-file faults, including
the fixed tg-xtask self-build. Success means independent misses progress
concurrently, overlapping misses share one fill, resident pages stay usable
during unrelated storage operations, and no stale fill survives a concurrent
write or resize. Performance acceptance additionally requires the complete
unchanged cold compile, matching output, and reduced measured waits/time.

## Prior art and alternatives

Linux source commit `980ab36ae5972c83f683b939e50c469c4947229e`:
[`filemap_update_page` and `filemap_create_folio`](https://github.com/torvalds/linux/blob/980ab36ae5972c83f683b939e50c469c4947229e/mm/filemap.c#L2553-L2660)
use folio-local loading/locking and a shared invalidation lock. They release
the mapping lock before waiting for an already-loading folio and retry after
truncation. This is source prior art, not the runtime control kernel version
(Linux 6.18.35-0-virt). The public
[`mmap(2)` contract](https://man7.org/linux/man-pages/man2/mmap.2.html)
requires EOF-tail zeroing and private-write isolation; this change must not
turn private file pages into writable aliases of cache storage.

Internal inspection covered `cache/{read,populate,update,resize,reclaim,
writeback,pages}.rs`, block-cache owned folios and `os::waiters::TaskWaiters`,
and cache history through `ed3d4a1e6`, `c0b606fc7`, `1ab948f77`.
Read-only GitHub discovery on 2026-09-11 found issue #2206 (block-cache
multi-folio direct I/O) and PR #2015 (rsext4 block/journal batching). Its file
list contains no ax-fs-ng file-cache fill implementation. These are adjacent
work, not an existing replacement for the file-cache transaction boundary.

| Alternative | Decision |
| --- | --- |
| Keep the exclusive read I/O owner | Preserves the measured serialization. |
| Replace it with an rwlock only | Does not resolve duplicate fills, eviction ownership or stale publication; ax-sync has no existing sleepable rwlock to reuse here. |
| Map cache PFNs directly into private mappings | Requires a separate mapping lease/COW/invalidation redesign; not needed to remove this wait boundary. |
| Background anonymous zero pool | Moves mandatory clearing elsewhere and needs allocator/reclaim policy; no evidence yet of a net win. |
| Owned fill, short admission/publication, invalidation and coalesced waits | Selected; reuses cache ownership, OS notifications and completed mapping retirement. |

## Ownership and transitions

This is a high-risk concurrency/ownership refactor. It needs independent
domain review before merging; no merge or external submission is part of
this local experiment.

1. A demand miss takes the existing I/O owner briefly, rechecks the cache,
   captures the current EOF and reserves a bounded missing range. The range
   stops at a resident page or another pending range. Overlapping demand
   attaches to the existing completion; an inode-local limit bounds pending
   windows and temporary memory independently of the number of callers.
2. The owner releases **both** I/O and cache-index locks. It reads backing
   bytes into private initialized storage and prepares fully initialized
   owned pages. No private candidate is observable through a PTE or cache.
3. Every content-update transaction invalidates all currently pending fills
   while holding the same I/O owner. Each fill has a one-way validity bit,
   so cancellation cannot wrap like a generation counter. Writers do not
   wait for old fills and old fills never acquire the mutation owner.
4. Publication retakes I/O exclusion and checks validity. Invalid candidates
   are dropped and the caller retries from current cache/EOF. A valid fill
   publishes only still-absent slots; an existing canonical page wins.
5. Removal from the pending registry and completion publication wake all
   registered waiters outside I/O/index exclusion. Errors reach every
   attached waiter; an abandoned owner completes with an explicit error.
   Waiters register before observing completion, using the existing
   lost-wakeup-safe `TaskWaiters` capability, without polling or a timeout.

The cache index continues to pin resident pages during a kernel copy. A
faultable userspace Writer receives a private snapshot only after all cache
locks are released. Copy bounds are checked against current EOF; a truncate
can shorten a read but cannot expose stale beyond-EOF bytes or create a
zero-progress loop.

Single-page population used by write/resize/shared mapping also separates
preparation from insertion. It retains its existing I/O/update owner for
mutation serialization, but releases the index for allocation and storage.
Eviction reserves a detached victim under I/O exclusion, releases the index
before listeners/writeback, and restores the same owner on rejection/error.
It must never replace a still-mapped canonical page or discard dirty data.

Lock order remains mutation/writeback (where applicable), I/O, then short
pending/cache/listener locks; no pending completion wait is allowed under
I/O or the cache index. Invalidation touches only pending atomic state.
Eviction callbacks retain the existing nonblocking address-space contract.
Cache page copying itself, writeback ownership, ext4 transactions, physical
frame allocation, syscall argument validation and user page-table formats
are non-goals. No new dependency, public API, disk format or boot option.

## Validation before acceptance

Add deterministic lowest-layer tests before implementing the new flow:
blocked backing read with another independent miss, coalesced overlapping
misses, write/resize while a captured old read is held, read failure and
retry, allocation failure, short reads/EOF zeroing, and eviction callback
lock/owner checks. Keep existing resize rollback, dirty-writeback, mapped
frame retention and resident/faultable destination tests.

After the **whole** refactor, complete three final static rounds before
runtime tests: ownership/caller audit; formatting plus production/test
clippy and build-only checks; final diff/image/configuration audit. Then run
the targeted tests and QEMU integration on 8c8g. Use the Starry test-suit
skill for any added system case and the same binary in Linux QEMU when
applicable. Only after these gates run the complete cold 181-unit workload,
preserving the exact rootfs source/tg-xtask and naturally ending profiling.
No test or speedup has been established for this design yet.
