# Batched mapping retirement

## Problem, evidence and scope

The completed 8c8g kernel profile `starry/event-driven-preemption` takes 2089 s.
`unmap_range_recursive` is the largest kernel CPU leaf: 3651/74094 active samples
(4.93%). Both COW and shared-file teardown currently query/unmap each base page;
the recursive walk restarts from the root and scans for empty tables repeatedly.
This is a CPU-sampling observation, not exclusive wall-time attribution.

Consumers are Starry's existing munmap, replacement mmap, address-space clear
and shrink paths. Success requires the same ownership/RSS semantics, no stale
translation after retirement, bounded temporary storage, fewer walks/barrier
round trips, and a successful full cold compilation with the frozen workload.
Performance is unmeasured until the next full run. Ext4 redesign, COW fault
preparation, frame-reference indexing and mmap placement are separate candidates.

This is high risk: shared page-table API, memory ownership and TLB ordering.
The design is independently reviewable; no merge or push is implied.

## Prior art and alternatives

- Linux `980ab36ae5972c83f683b939e50c469c4947229e`,
  `include/asm-generic/tlb.h:31`, `mm/mmu_gather.c:421`: unlink mappings,
  invalidate translations, then retire physical pages and table pages.
- Linux at the same revision, `arch/arm64/include/asm/tlbflush.h:395`:
  batch TLBI operations and their completion barrier. Publication precedes
  invalidation; a local-only invalidation is not a remote retirement proof.
- MOSS `5a54e4413c9657bfb531cc32f6d688090465200d`,
  `kernel/src/mm/aspace/backend/mod.rs:177`: retain page owners in a bounded
  gather and finish the page-table cursor before releasing them.
- TGOSKits history `4737af90a` established the opaque-PTE execution boundary.
  Current `map_region` batches mapping invalidation but supplies no owned
  unmapping callback. Existing `walk` is read-only and cannot itself guarantee
  teardown ownership or safe intermediate-table retirement.
- Open PR #2009, head `d026ff43e1375cc23009d38916462e9c5e81eb24`, already
  addresses intermediate-table invalidation, but not range batching. Its current
  diff only invalidates when `flush` is true; this candidate cannot inherit the
  stated external-reclaim alternative because the actual old implementation
  immediately frees those frames even with `flush: false`. Preserve attribution
  of this overlapping correctness work and validate the real lifecycle here.

Keeping per-page unmap retains root walks and repeated empty checks. Dropping
flushes or deferring frees without an owner is unsafe. Copying MOSS wholesale
would import a different page-table API and folio model. Instead extend the
existing generic walker with a bounded owned range operation and let ax-cpu
own batched architecture invalidation. No new crate or dependency is needed.

## Ownership and boundaries

`unmap_owned` accepts an aligned virtual range, a fallible preparation callback
and an infallible retirement callback. Preparation sees the intact leaf and
returns its owner record; it must not release the mapping's physical memory.
The engine clears the leaf only after preparation succeeds, retains the record,
and invokes retirement only after synchronous invalidation. Preparation errors
leave that leaf intact and drain earlier successful removals before returning
the original error. Error types remain caller-owned via `From<PagingError>`.

The generic implementation owns a depth-first range walk, skips entire missing
subtrees, and checks each visited table for emptiness once. It queues detached
table frames separately. Both queues are fixed-capacity; capacity pressure
completes an invalidation batch before releasing anything. The root is never
retired here. Non-present occupied leaves remain mappings for ownership purposes.
Partially intersected huge leaves are rejected, never widened into neighbors;
Starry's backends already require their own page-size-aligned ranges.

The scope guard drains on ordinary errors and unwind. Callbacks must not re-enter
the same table or panic; safe borrowing prevents ordinary table re-entry. The
existing address-space mutex remains held, and no new lock order or lockless
software page-table reader is introduced. COW preparation resolves its existing
frame-reference owner before clearing; retirement removes RSS and drops that
reference. Shared-file mappings retain their existing cache/listener ownership;
retirement updates RSS, not the cache's independent physical-page ownership.

`TableMeta::flush_batch` defaults to existing per-address invalidation. AArch64
overrides it with inner-shareable TLBI and a shared publication/completion barrier;
large batches may use inner-shareable all-entry invalidation. The existing
`flush_tlb(None)` is local-only and is deliberately not used for SMP retirement.
PTE formats, permissions, ASIDs and instructions never move into generic memory.
Other architectures keep their existing invalidation scope; their SMP behavior
is not newly certified by AArch64 tests.

The legacy single-page/range API remains available. Its intermediate-table
unlink must also precede a completed invalidation before allocator reuse, even
when leaf invalidation was requested as deferred. Existing explicit deferred
mapping callers must not receive prematurely recycled table pages.

## Validation gates

First add a deterministic production-walker regression recording PTE clear,
TLB completion and actual allocator free. After three static checks, prove it
fails on the old intermediate-table retirement order. Then implement the complete
generic gather, architecture batch operation and Starry consumers; perform three
final static rounds before GREEN or QEMU. Cover dense and sparse ranges, table
boundaries, non-present leaves, huge leaves and rejected partial huge ranges,
preparation errors, capacity drains, RSS and reference-count ownership.

Run fmt and targeted clippy for all touched crates, then lowest-layer tests and
8c8g QEMU memory regression. Freeze final kernel identities, use a new copy of
the existing cold rootfs, and profile the entire identical tg-xtask compilation.
Compare the exact compile-unit list, ELF, sampling integrity, fsck and full
duration. Keep this candidate separate from the accepted 2089-second run.
