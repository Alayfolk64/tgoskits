# Operation-scoped owned unmapping

## Problem and evidence

The complete 1087-second VMA-gap-index build has 1286 active CPU leaf samples
in flush_batch (3.363%). Saved-ELF PCs put 1202 immediately after per-address
TLBI, 81 after full TLBI. 1141/1286 are COW retirement. By outer operation,
665 are munmap, 475 address-space clear, 79 faults, 54 mprotect and 13 mremap.
The larger spin_release leaf is an IRQ-enable attribution boundary, not an
exclusive instruction cost. Input records, artifacts and full call chains are
frozen in target/profiling/arceos-helloworld/starry/vma-gap-index.

Each COW backend currently creates and finishes a 64-entry retirement queue
for one VMA fragment, even inside a larger MemorySet unmap/clear operation.
The target users are those existing Starry operations. Retain the same workload,
page-table walk, remote flush capability, bounded memory and backend-error order.
Success requires real cross-range flush reduction with unchanged ownership,
followed by lower whole-build time on the matched fresh 8c8g QEMU image.

This is a high-risk reusable lifetime/API refactor. This document precedes code;
it is not maintainer design approval or merge readiness. No architecture boot,
new hardware support, filesystem implementation or syscall ABI is added.

## Prior art and alternatives

Linux at 980ab36ae5972c83f683b939e50c469c4947229e:
include/asm-generic/tlb.h requires unhook -> invalidate -> free. exit_mmap in
mm/mmap.c retains one fullmm gather across unmap_vmas and free_pgtables before
closing VMA backends. Ordinary tlb_end_vma normally flushes; fullmm or the
MERGE_VMAS architecture configuration skips it. Linux therefore does not always
batch across VMAs. Its page/range/mm distinctions and SMP postconditions are
documented at https://docs.kernel.org/core-api/cachetlb.html.

Internal work already introduced owned, bounded per-range retirement, failed
COW-clone rollback and cache-backed frame pins. Reuse those boundaries and
existing regression fixtures. The open-PR keyword query for mmu/gather/unmap
returned no results; this is not a full open-PR compatibility review.

| Alternative | Decision |
| --- | --- |
| Change only the 32-address full-flush threshold | Does not address operation ownership; trades global TLB eviction against message count without evidence. |
| Add a global/thread-local current gather | Reject hidden ownership and reentrancy around nested operations. |
| Move all PTE removal before all metadata changes | Reject changing partial-failure behavior or backend-drop ordering. |
| Replace every backend's PageTable associated type | Unnecessarily changes mapping, protection, faults and fork boundaries. |
| Explicit scoped page-table session plus MemorySet unmap callback | Chosen: borrow the real owning table, preserve metadata state-machine order, batch COW owners across existing callbacks. |

## Boundaries and invariants

1. A reusable UnmapSession exclusively borrows one owning PageTable. Repeated
   checked unmap requests share its bounded retirement queue. Existing
   unmap_owned/unmap_page remain synchronous compatibility APIs.
2. A successful range need not flush immediately. Capacity, explicit finish,
   scope exit or an error drains pending descriptors before releasing owners.
   No allocation is needed to remove mappings. The root stays owned by the
   caller, and queued table pages are never active recursion frames.
3. The session offers a named flushed-table callback for operations that cannot
   join the queue. It drains first; any replacement table's allocator is adopted
   only while the queue is empty. No unflushed table reference escapes.
4. MemorySet adds explicit unmap/clear callback entry points at its existing
   backend boundary. Ordinary APIs delegate to them. Preserve complete-area,
   split and shrink order, error handling, and the existing gap mutation guard.
   Remove duplicated private shrink/I/O orchestration, not ownership checks.
5. Starry's operation owner uses a typed retired COW page containing physical
   reference, size, address, accounting and any cache-mapping registration.
   Keep that registration alive until invalidation completes; retaining only a
   physical frame while dropping its invalidation listener early is insufficient.
6. Other backends retain their existing synchronous implementations. Before
   entering one, flush earlier COW owners. Shared/DMA/file lifetime anchors thus
   cannot be dropped ahead of their existing completion boundary. This is not
   a claim that those backends have been converted to deferred retirement.
7. Wire the same COW session into whole unmap, clear and private discard ranges.
   Standalone COW unmap uses it too. Keep RSS/VM/memfd accounting and caller
   locks; every early error must finish the successfully detached prefix before
   returning or preserving the original whole-area panic contract.

Existing partial backend-error semantics and panic contracts are not repaired
incidentally. Do not remove remote invalidation, reuse physical memory before
completion, implement ASIDs, increase batch capacity or change sampling here.
Only the existing TableMeta::flush_batch capability is used; its architecture
implementation and guarantees are unchanged.

## Validation

Complete the connected candidate before runtime, then perform three static
rounds: ownership/call-chain audit; fmt plus relevant strict Clippy and actual
profile build; final source/artifact and error-path review. No QEMU or test
execution is admitted until all three pass.

Lowest-layer tests use the real page-table engine and recording PTE/allocator
capabilities: several disjoint ranges form one final batch, capacity drains,
malformed/preparation errors drain prior work, explicit table handoff drains,
scope exit retains owners until completion, and non-present/huge/table pages
remain correct. This adds a batching capability, not a new fix for the original
clear/flush/free ordering bug already covered by existing RED/GREEN evidence.
An eager-per-range-flush mutation must fail the unchanged grouping regression;
record it as a counterfactual mutation, not byte-identical original production.

MemorySet callback tests cover actual fragment order, successful metadata and
partial-error behavior. Existing gap tests remain unchanged. Kernel tests must
exercise multiple COW VMAs, fork owners, RSS and cache invalidation ownership,
as well as the existing full 8c8g suite. Then run the whole 181-unit build with
reused rootfs tg-xtask, frozen source/compiler and direct kernel profiler;
verify all artifacts against Linux and compare full CPU/wait and wall results.
No successful build or smaller flame alone proves throughput improvement.

Inputs are archived in tmp/operation-unmap.4U8Sgf/sources-before.tar. Temporary
artifacts are not build sources. Existing unrelated dirty changes stay intact.
