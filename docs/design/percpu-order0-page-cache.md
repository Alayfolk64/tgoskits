# buddy-slab order-0 per-CPU page cache

## Problem and evidence

The OrangePi 5 Plus StarryOS self-build run20 profile contains 19,512 CPU
samples. After the rsext4 orphan-chain traversal, the largest kernel CPU group
is page-fault allocation: `CowBackend::alloc_new_frame_sized` has 467 leaf
samples, `GlobalAllocator::alloc_pages` has 333, and the generic allocator and
deallocator have 1,357 and 603 respectively. Eight compiler CPUs repeatedly
allocate and release independent 4 KiB anonymous, page-cache, and page-table
pages, but the current `ax-alloc` wrapper serializes every operation through
one outer spin lock before the buddy allocator or per-CPU slab can use its own
ownership lock.

This is a high-risk shared allocator change. Direct users include StarryOS and
ArceOS page faults, page tables, file page cache, task stacks, and kernel heap
allocations. Indirect user-visible callers include `mmap`, `brk`, `clone`,
`fork`, `execve`, user-copy faults, and ordinary file I/O. The change must not
alter their success, `ENOMEM`, isolation, zero-fill, COW, or accounting
semantics.

## Success criteria and non-goals

- An order-0 allocation normally pops a page from the current CPU without
  taking the buddy lock; an empty cache refills a bounded batch under one buddy
  lock acquisition.
- An order-0 free normally returns to the current CPU without taking the buddy
  lock; a full cache drains a bounded batch before accepting the page.
- Removing the wrapper lock must not remove the buddy allocator's cross-CPU
  exclusion or the per-CPU slab lock.
- Cached free pages are excluded from used-page accounting and remain
  available to allocation slow paths.
- Large, DMA32, and slab-growth allocations drain stranded order-0 pages and
  retry before external reclaim, so the cache cannot introduce false OOM.
- Cross-CPU frees preserve exact aggregate usage accounting.
- No public allocator API, page ownership contract, zeroing policy, or syscall
  ABI changes.

This change does not implement NUMA zones, migratetypes, dynamic watermarks,
page poisoning, compaction, or a Linux-compatible reclaim subsystem. Fixed
cache sizing is an initial bounded policy, not a claim that Linux's complete
page allocator has been reproduced.

## Prior art and alternatives

The Linux reference is commit
`980ab36ae5972c83f683b939e50c469c4947229e`. In `mm/page_alloc.c`,
`rmqueue_pcplist` and `__rmqueue_pcplist` serve common allocations from
per-CPU lists, `rmqueue_bulk` refills under one zone-lock hold, and
`free_frozen_page_commit` / `free_pcppages_bulk` batch returns to the buddy.
Linux drains PCP lists on pressure before concluding that the zone cannot
satisfy a slow-path request.

The selected design adapts that ownership split to the smaller TGOSKits
allocator:

| Alternative | Result |
| --- | --- |
| Keep one outer wrapper lock | Simple accounting, but serializes independent per-CPU slabs and every order-0 fault. |
| Remove only the wrapper lock | Improves slab concurrency, but every page fault still takes the buddy lock. |
| Cache pages inside the COW backend | Optimizes one consumer and duplicates allocator pressure/accounting policy. |
| Lock-free per-CPU stack | Avoids a local lock but complicates remote draining, ABA, CPU lifetime, and bounded accounting. |
| IRQ-safe per-CPU cache plus buddy batches | Selected: explicit ownership, bounded storage, and a pressure drain compatible with the existing runtime. |

## Ownership and synchronization

`buddy_slab_allocator::GlobalAllocator` already owns an internal buddy spin
lock. Each `PercpuSlab` already owns an IRQ-safe slab lock. The removed
`ax-alloc` wrapper lock duplicated those boundaries; it was not the owner of
allocator metadata.

`GlobalAllocator::with_runtime_state` disables local IRQs and preemption for a
single backend operation. This pins current-CPU selection and prevents
same-CPU interrupt re-entry. Cross-CPU buddy mutation remains protected by the
inner buddy lock; slab and page-cache mutation remain protected by their
per-CPU locks. No path holds a page-cache lock while acquiring the buddy lock:
refill releases buddy before publishing cached pages, and drain removes pages
from the cache before returning them to buddy.

Each CPU has at most 64 cached order-0 pages. An empty cache asks the buddy for
up to 32 independent pages under one lock acquisition, returns one to the
caller, and owns the rest as free cached pages. A full cache removes 32 pages,
releases its lock, returns that batch to buddy, and then accepts the newly freed
page. These pages remain backend-allocated so the buddy cannot hand them out a
second time.

If an uncached allocation reports `NoMemory`, the wrapper drains all CPU page
caches and retries once before invoking the existing bounded external reclaim.
This applies to large heap/slab growth, contiguous pages, and DMA32. The drain
is essential because a cached generic page may be the low-address or buddy
piece needed by the constrained request.

Usage accounting moves from one global lock to signed per-CPU atomic deltas.
Allocation charges the current CPU; a free subtracts on the freeing CPU. A
cross-CPU free may therefore make one local delta negative, but summing all
CPU deltas yields the same global value. Relaxed ordering is sufficient because
these counters do not publish ownership; allocator locks and object ownership
provide synchronization. Statistics are snapshots and may be transiently
approximate while CPUs allocate concurrently, as before.

## Compatibility and failure behavior

Order-0 cache entries carry no `UsageKind`: they are free physical pages. The
kind is charged only when a page leaves the cache and subtracted before it
enters. DMA32 allocation still uses its constrained backend. Explicit `Dma`
frees bypass the generic cache so constrained device pages remain visible to
the buddy immediately.

Allocation errors other than `NoMemory` are returned without draining or
retrying. A zero-page batch remains `NoMemory`. Batch allocation never exposes
the uninitialized tail and every returned address remains a separately owned
one-page allocation. The existing caller remains responsible for zeroing when
its semantic contract requires it; anonymous COW faults still call
`alloc_frame(true, ...)`.

The syscall guideline is applicable because allocator failure is visible to
memory-management syscalls, but no ABI rule is intentionally changed:

| Indirect syscall class | Required invariant |
| --- | --- |
| `mmap` / `brk` and fault handling | Same mapping, zero-fill, permissions, and `ENOMEM`; only false allocator contention/OOM is reduced. |
| `clone` / `fork` | Same COW sharing and break-before-write behavior. |
| `execve` | Same image construction and rollback on genuine allocation failure. |
| file I/O and user-copy faults | Same bytes and fault errors; cached pages never bypass page ownership or zeroing. |

## Validation and rollback

Validation must include:

- deterministic batch allocation/free ownership and free-page accounting;
- deterministic retry-after-cache-drain policy;
- cross-CPU signed usage aggregation and bounded cache drain tests;
- formatting, diff checks, and targeted clippy for both allocator crates;
- complete allocator crate tests and relevant Starry memory tests;
- attended 8-core board comparison of allocator samples, page-fault samples,
  total build time, and genuine OOM/reclaim diagnostics.

The implementation has no persistent format. It can be reverted as one
allocator change. Board validation is deferred while unattended; no boot,
deployment, reset, or root-filesystem write is part of local validation.
