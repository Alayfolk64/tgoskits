# StarryOS COW page-index insertion fast path

## Problem and evidence

The run20 StarryOS self-build profile shows page-fault materialization as the
largest CPU consumer after the ext4 orphan-chain traversal. The principal path
is `handle_page_fault_result -> prepare_fault_materialization ->
CowBackend::prepare_new_at_sized -> alloc_new_frame_sized`. In addition to
allocating and clearing the frame, every page publication scans the complete
`CowPageIndex` to count live entries before it can decide whether the existing
vector has enough storage. Long-lived address spaces retain expired weak
identity entries until a later rebuild, so this makes the common insertion
cost proportional to historical page count even when the vector has spare
capacity.

This index protects physical-frame identity across COW PTE preparation,
publication, rollback, and TLB-safe retirement. A fast path is valid only if it
preserves overlap rejection, keeps allocator destruction outside IRQ-saving
locks, and remains correct when another CPU changes the index between prepare
and apply.

## Success criteria and non-goals

- With spare vector capacity, inserting a non-overlapping frame performs only
  binary/neighbor checks and no complete live-entry scan or allocation.
- Expired entries that overlap a reused physical frame are removed before the
  new identity is published.
- A full vector still rebuilds from storage allocated before taking the index
  lock and carries expired weak entries outside the lock for destruction.
- Concurrent prepare/apply races retry from a stale-reservation result rather
  than allocating or dropping owners under the lock.
- COW sharing, zero-fill, mapping references, RSS classification, rollback,
  TLB retirement, and user-visible fault errors remain unchanged.

This change does not replace the vector with a tree, change page granularity,
alter page-fault batching, or weaken the existing identity checks.

## Prior art and alternatives

Linux reference commit `980ab36ae5972c83f683b939e50c469c4947229e` does not
have this Starry-specific physical-identity vector. Linux anonymous faults use
page-table/anon-vma ownership and file pages use indexed mappings; the relevant
principle is that a normal fault updates local indexed state without rebuilding
all historical entries. The Starry design retains its stronger explicit
identity object rather than pretending the implementations are structurally
identical.

| Alternative | Result |
| --- | --- |
| Scan and compact on every insertion | Eager cleanup, but O(n) work on the common fault path. |
| Never retain expired weak entries | Final weak destruction may deallocate below an IRQ-saving PTE/index lock. |
| Replace the vector with a new tree | Larger ownership rewrite without evidence that lookup dominates after the smaller fix. |
| Spare-capacity neighbor fast path | Selected: preserves the existing representation and moves only necessary cleanup to the slow path. |

## Prepare/apply transaction

The vector remains sorted by physical frame base and live entries never
overlap. For a candidate frame, `insert_reservation_capacity` first computes
its checked end address and finds the insertion position. If the vector has
spare capacity and neither possible overlap neighbor is an expired entry, it
returns a zero-capacity reservation immediately. A live overlap needs no
storage because apply will reject it; an unrelated expired tombstone may remain
until capacity pressure makes cleanup useful.

If capacity is full or an expired neighbor overlaps the reused frame, prepare
counts live entries and allocates replacement storage outside the index lock.
Apply rechecks current length, capacity, and liveness. A concurrent mutation
that invalidates the prepared capacity returns `StaleReservation`; the caller
drops displaced storage outside the lock and repeats prepare. Rebuild moves only
live entries into replacement storage, preserving sort order, and leaves all
expired owners in the reservation for destruction after unlock.

Insertion at the sorted tail uses `push`; interior insertion retains `insert`
and its in-place shift. Both are allocation-free because apply has proven spare
capacity. All overlap, frame-size, mapping-reference, and owner-identity checks
remain in apply and are not trusted solely to the optimistic prepare phase.

## User-visible compatibility

The Starry syscall guideline applies indirectly to page faults from `mmap`,
`brk`, `clone`, `fork`, `execve`, file I/O, and user-copy. This change does not
alter the ABI, address selection, permissions, signal/errno mapping, or shared
state. A true allocation failure remains `NoMemory`; an overlap or broken
ownership invariant remains `BadState`. The optimization only avoids redundant
index work before the same state transition.

## Validation and rollback

Deterministic tests cover spare-capacity insertion, full-vector rebuild,
expired weak-entry retirement, physical-frame reuse at the same address,
missing preimage restoration, overlap rejection, and stale reservation retry.
Formatting, diff checks, targeted clippy, and Starry memory tests must pass
before any runtime test. An attended board run must compare page-fault and
allocator CPU samples; local validation alone cannot claim a speedup.

There is no persistent state or public API change. The fast path can be
reverted independently from the allocator page cache. No unattended board
action is permitted for validation.
