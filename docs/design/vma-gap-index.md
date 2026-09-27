# Indexed first-fit virtual address placement

## Problem, evidence and scope

The complete cache-writeback-ownership build took 1253 seconds (181 units),
versus 1217 seconds immediately before it and 735 seconds on matched Linux.
Its largest resolved kernel CPU leaf is MemorySet::find_free_area: 1979 samples,
4.954% of active samples. Saved-ELF disassembly places 1976 of those samples in
the successor/gap scan at 0xffffffff800e9e78..0xffffffff800e9f28, rather than
predecessor lookup. This is a CPU bottleneck independently of the 72.349 seconds
of cumulative ext4 mount mutex wait; neither predicts wall-clock savings.

Users are Starry mmap/mremap callers and other existing MemorySet consumers.
Keep first-fit, hint-before-base placement, backend ownership, mapping flags,
PTE operations and caller synchronization. Replace linear examination of gaps
that cannot fit a request. No scheduler, ext4, device, guest workload or sampling
change belongs in this experiment. Throughput acceptance requires a fresh full
8c8g QEMU build with identical inputs, not only a reduced leaf in the profile.

## Prior art and alternatives

Linux source at 980ab36ae5972c83f683b939e50c469c4947229e:
mm/vma.c:2968 unmapped_area calls vma_iter_area_lowest, then checks alignment and
stack gaps; lib/maple_tree.c:4501 mas_empty_area descends the allocation tree.
[Maple Tree documentation](https://docs.kernel.org/core-api/maple_tree.html)
describes size-aware empty-range lookup. Borrow this pruning principle, not
Linux's C implementation, RCU machinery, top-down default or inclusive bounds.
No Linux code is copied into this Apache-2.0 crate.

Internal searches covered MemorySet, its Starry/ArceOS consumers, ranges-ext,
existing mmap-lazy-fallback and vma-range-scans experiments and path history.
The pinned alloc BTreeMap::Range::last already calls next_back; renaming it is
not an optimization. MOSS's previously inspected MemorySet also scans first-fit.
The open-PR keyword search returned unrelated broad-body matches, not evidence
of an existing gap implementation; it is not a full PR compatibility review.

| Alternative | Decision |
| --- | --- |
| Retain scan / only cache last hint | Does not skip too-small holes; a hint cache changes first-fit reuse or requires conservative invalidation. |
| Rebuild a gap vector after every mutation | Moves linear work into mmap/munmap, the same workload's hot path. |
| Size-keyed BTreeMap of gaps | Finding the lowest address across many eligible sizes can still scan linearly. |
| Replace the entire VMA container with a new tree | Unnecessarily changes borrowed iteration, backend ownership and every metadata mutation. |
| Private augmented gap index alongside existing areas | Chosen: preserves the owning map and public API, updates only affected ranges, prunes undersized subtrees. |

## Ownership and update contract

MemorySet remains the only owner. Its BTreeMap is authoritative for MemoryArea
objects; the private gap index is derived acceleration state, never independently
mutable by callers. No locks, atomics, unsafe, public traits or dependencies are
added. Existing &mut access and address-space locking publish both together.

GapIndex owns disjoint maximal finite free intervals below an implicit free
tail [tail_start, usize::MAX). New/clear remain const/allocation-free. An AVL
tree orders finite gaps by start and stores subtree maximum length and height.
Checked alignment and size arithmetic select the lowest fitting address inside
the caller's half-open bounds. Raw maximum length is a conservative pruning
bound; unusual alignment can require visiting more than one eligible gap.
Zero-size or non-power-of-two alignment requests return None explicitly. A
candidate beyond limit.end or alignment overflow must never escape the bounds.

- Successful map occupies the installed interval only after backend success.
- Failed replacement map keeps any successful preceding unmap reflected.
- Successful extension occupies only the appended interval.
- Unmap and metadata-only unmap reconcile their affected interval after success
  OR recoverable error. Expand that interval to whole overlapping areas first:
  existing partial failures can remove a right suffix outside the requested
  range. Reconcile from authoritative surviving areas, without cloning backends
  or scanning unrelated prefixes. Do not change those backend failure semantics.
- Metadata replacement and protection only partition existing coverage; they
  do not change free intervals. Clear resets the index after all backend unmaps
  succeed, preserving the current map-on-error contract.

Insertion/removal rebalance the private AVL tree and refresh summaries on every
changed ancestor. Release coalesces neighbors; occupation splits one free
interval. Allocation uses the crate's existing infallible alloc policy, with
one node per finite gap, not one per page; no hidden recovery-success path.
This adds allocation/storage on fragmented mappings and must be measured against
the saved scan baseline. It is not a full Maple Tree or O(1) allocation claim.

## Validation and acceptance

High-risk internal resource/index change: this document precedes implementation;
it is not maintainer approval or merge readiness. Regression tests use the real
MemorySet and a recording backend at the lowest deterministic boundary. Cover
first-fit after thousands of too-small gaps, alignment/hint/limit/overflow,
all mutation paths, failure after partial unmap, adjacency/coalescing, indexed
balance/max-gap invariants and exhaustive small-domain oracle comparisons.
Kernel placement regression covers the stack-style upper bound and fallback.

Finish the complete candidate, archive its sources, then restore original
production for RED (unchanged new tests). Three static rounds precede RED:
ownership/source audit, Clippy, fmt/diff/input identity. RED must fail without
timing thresholds. Restore the exact candidate, repeat three static rounds
including the actual profile kernel build, then run unchanged GREEN, full
MemorySet tests and relevant 8c8g kernel tests before a full measured compile.

Original sources: tmp/vma-gap-index.6UocCb/sources-before.tar, SHA-256
2e500b64fd1edc0e072a5600e5dd340317c33f62c689006d9a5fc7ec916af659.
No commit, push, PR or external review is part of this local experiment.
