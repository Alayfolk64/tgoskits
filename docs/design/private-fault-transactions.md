# Private fault preparation and commit

## Problem and success criteria

The completed `starry/batched-mapping-retirement` cold build takes 1908 s.
Its largest kernel CPU leaf is `try_zero_page`: 3495/66408 active samples
(5.26292%). Actual DC ZVA accounts for 3433 of those samples. Of the 3463
zeroing samples in user faults, 2615 pass through locked `populate` and 848
through unlocked private-file preparation. CPU active fraction is still
44.89029%, and fault stacks include 1074.239 s of mutex wait across tasks.
The precise data and attribution limits are in the matching profiling report.

The current unlocked plan handles only absent 4 KiB private file mappings.
Anonymous allocation/zeroing and resident COW allocation/copy still hold the
address-space mutex; shared-page copying additionally holds a frame IRQ lock.
File preparation zeroes an entire new page before reading bytes over it.
Kernel consumers are the existing user-fault entry and private mapping
backends, not a new userspace API or special compilation-only path.

Success means the real allocation/copy boundary runs without the address-space
or source-frame lock for the eligible expensive paths, every published byte
is initialized, racing mapping changes cannot receive stale pages, and source
owners outlive replacement invalidation. The complete fixed workload must
still produce identical compile units and final ELF, clean fsck and intact
profiling. Faster completion and wait/CPU changes are measured afterward;
moving work outside a lock is not itself a performance result.

This is high risk: ownership, concurrency, page-table replacement and user
memory visibility. This document is a separately reviewable design; no merge
or push is authorized or implied.

The complete writer audit also found a necessary ownership prerequisite:
`AddrSpace::write(&self)` writes physical pages without checking COW. Ptrace's
populate/write pair holds one lock, but proc-mem releases it between the two,
and AIO can write much later than submission-time preparation. Fork can create
new sharing in that interval. The kernel-copy entry must take exclusive
address-space access and COW-break each resident private destination under that
same lock before copying; a source pin must count for this writer as well.
Keep caller-owned access/ptrace policy unchanged and do not lazily allocate
missing pages merely because kernel-copy encountered an absent PTE. ELF loader
relocations can write read-only private pages and must retain that capability.
The existing retained-frame COW path can provide this transition; do not add
separate per-caller ownership fixes or a second frame synchronization model.

## Prior art and alternatives

- Linux `980ab36ae5972c83f683b939e50c469c4947229e`,
  `mm/memory.c:5287` (`do_anonymous_page`): anonymous allocation precedes
  the PTE lock, followed by PTE revalidation. Its shared-zero-page read policy
  is distinct and is not imported without evidence about fault access types.
- Linux at the same revision, `mm/memory.c:3853` (`wp_page_copy`): retain
  the old page, allocate/copy, recheck the PTE, clear/flush, install the new
  page, and only then remove the old mapping reference.
- Linux at that revision, `mm/memory.c:7020` (`__access_remote_vm`): obtain
  the writable page through `get_user_page_vma_remote` with FOLL_WRITE before
  copying through the kernel mapping. Starry retains its caller-owned access
  policy, but cannot skip COW ownership merely because the copy uses a physical
  alias. `arch/arm64/include/asm/tlbflush.h:561` places dsb(ishst) before
  broadcast invalidation, including the single-page flush used for permission
  changes. These are ownership/ordering references, not a full ABI comparison.
- MOSS `5a54e4413c9657bfb531cc32f6d688090465200d`,
  `kernel/src/mm/aspace/backend/cow.rs:304`: `CowFaultPlan` retains a
  mapping identity and owned source references across unlocked preparation.
  Its PFN-indexed reference table and directly mapped file folios are separate
  ownership models; do not copy them into this change implicitly.
- Existing Starry `fault.rs`/`cow/missing.rs` already implement a three-phase
  private-file fault. Extend that boundary rather than add another resolver.
  Preserve clone rollback from history `829b379d9` and anonymous mprotect COW
  isolation from `dd759372b`.
- Existing `ax-io` implements Write/IoBufMut and direct reading for
  `core::io::BorrowedCursor`. Reuse this initialized-byte capability; do not
  expose an uninitialized `[u8]` or introduce a second generic I/O interface.
- Open PR #2000, `6c1b006a852a458ad577f92ea8a5978a56fb4e59`, expands
  file read faults to a 32-page forward window, but keeps the locked populate
  path. Its actual diff is complementary, not this ownership protocol.
- Open PR #1993, `95e2dc3a89d93c86c503e9f5821fe0e8e68c440c`, changes
  fault-in OOM errno propagation; preserve the current error categories here.
- Open PR #2166, `aa083b64b03ef13f7cddb33b6a7aefea37e7f5b0`, changes
  readahead capacity and RK3588 board/NPU behavior. It does not provide the
  private-fault transaction and is outside this QEMU-only change.

Keeping the old path retains serialization. A background zeroing worker
moves CPU work into idle time but adds inventory/reclaim policy and does not
remove initialization. Wider anonymous prefaulting changes lazy RSS behavior.
A lockless/RwLock page-table conversion or direct file-folio mapping changes
additional lifetime/coherency boundaries. Choose the existing mutex-protected
commit boundary plus owned preparation; leave these alternatives separate.

## Transaction and ownership

Only eligible private COW user faults enter this transaction: absent 4 KiB
anonymous/file pages and writes to resident read-only 4 KiB COW pages. Other
backends, explicitly shared mappings, huge pages and kernel-side populate
retain their current path. An already suitable mapping completes immediately.
An exclusive resident COW page may upgrade permissions under the short locked
path without allocating/copying. No anonymous read-ahead or shared zero page
is introduced.

1. Under the address-space mutex, validate the VMA and requested access.
   Capture the backend identity Arc, range, start, page size and permissions.
   For resident shared COW, retain an additional counted source reference
   before dropping the mutex. An Arc to refcount metadata alone is not a
   physical-page owner. Release both frame-table and frame locks before any
   expensive preparation.
2. Outside the mutex, allocate and initialize an unpublished destination.
   Anonymous pages are fully zeroed. COW copies read from the retained source.
   File reads use a MaybeUninit-backed BorrowedBuf; its filled length
   determines which bytes remain to zero, including prefix, partial reads and
   EOF tail. A Direct FileNode's safe `[u8]` reader may itself require zeroed
   input through `ensure_init`; do not bypass that contract to claim savings.
   No fallible path may publish or leak a partially initialized frame.
3. Reacquire the original address-space mutex and validate both VMA identity
   and the original PTE physical address/permissions. The source pin prevents
   a reused-PFN ABA. If another fault already installed suitable permissions,
   discard the redundant destination. If the mapping changed, discard/retry
   against the new state; retain bounded retries and the established fallback.
   Current mapping state, not the obsolete plan, decides error classification.
4. Complete mapping/RSS changes under that mutex. A fresh page transfers its
   owned reference to the PTE only on successful installation. A replacement
   invalidates the old translation before publishing the new descriptor and
   keeps both the original mapping reference and temporary source pin until
   invalidation/commit permits their respective release. COW File-to-Anon
   accounting uses the live backend's established rules.

RAII owns prepared frames and source pins, including stale-plan, allocation,
read, mapping and unwinding paths. Normal production callbacks do not panic.
Do not change global refcount indexing, RSS definitions, syscall flags/errno,
file-cache coherence or truncation policy in this refactor.

### Leaf replacement

The current generic `remap_recursive` writes the replacement descriptor over
the old leaf and `remap_page` flushes afterward. For changed physical backing,
adopt clear -> synchronous invalidation -> make -> synchronous publication/flush
ordering, without freeing the still-attached table. Construct/validate the
replacement before clear so no recoverable failure strands an empty leaf.
Use `flush_batch` for both completions, including its leading store barrier;
the old single-address AArch64 flush lacks that leading publication barrier.
Keep PTE bits architecture-owned and use the established TableMeta capability;
the AArch64 implementation supplies inner-shareable completion. Other
architectures retain their existing invalidation-scope limits, not a new
claim of remote-shootdown support. Same-physical permission changes keep the
protect operation without breaking the leaf. Its completion also uses
`flush_batch`: fork must publish revoked write permission before another fault
can rely on a read-only, counted source. This prerequisite is covered by a
separate deterministic capability/descriptor regression.

The pinned core BorrowedCursor shares its cumulative `written()` count across
reborrows. The existing ax-io specialization incorrectly returned that total
as each `read_from` result. Before importing the capability into fault I/O,
the deterministic repeated-read/EOF regression proved `(Ok(3), Ok(3))` instead
of `(Ok(1), Ok(0))`. Return the before/after delta at this existing I/O boundary,
so a Direct FileBackend terminates after short reads followed by EOF. No new
I/O interface or per-caller workaround is introduced.

## Validation and gates

Add deterministic regressions before the changes they constrain. An axtest-only,
task-owned allocation observation can assert the actual address-space lock is
available at the real allocation boundary, then inject mapping replacement,
unmap, permission revocation or competing faults. It must be consumed before
callbacks run, never hold its own lock across them, ignore unrelated tasks,
and have no production feature/overhead. Verify that the old locked path
necessarily fails the lock assertion rather than merely hoping a race occurs.

At the generic page-table boundary, observe the real leaf when invalidation
completes: changed-PFN replacement must expose an unused leaf during the first
completion and the new descriptor after make. Do not use a text search or
constructor-call count as a substitute for the actual descriptor lifecycle.

Cover at least: absent anonymous zero bytes/RSS, cloned COW isolation and
reference counts, source lifetime after concurrent unmap, same-address remap,
permission loss, duplicate winner, failed file I/O, short reads/EOF and
unaligned zero prefix/tail, no leaked temporary owners and invalidation order.
Existing clone rollback, mprotect and batched-retirement cases remain required.
Kernel-copy cases cover forked and temporarily pinned sources, forced writes
without granting user write permission, cross-page physical offsets, allocation
failure, absent PTEs and the existing partial-copy behavior. These lower-layer
axtests can inject a fault exactly at the allocation boundary and observe live
PTEs/reference ownership; a userspace test cannot deterministically force that
internal interleaving or inspect those locks. They do not certify all proc-mem,
ptrace or AIO ABI/permission/error-priority combinations. No syscall number,
argument layout, access-policy check or dispatch mapping is changed here.

Before any runtime execution, record three complete static rounds appropriate
to the regression/whole-refactor state: ownership/error audit; fmt/clippy and
actual AArch64 SMP=8 static build; final source/call-site/test-discovery and
saved-artifact audit. A failed static command does not admit the next runtime
command. After the full refactor passes all three rounds, run lowest-layer
tests, the existing 8c8g Starry ktest entry, then a new full cold-profile run.
No partial performance result or compile-count cutoff is accepted.

Rollback is a source change and rebuild; no persistent format migration exists.
Keep the accepted 1908-second kernel, source archive and ended disk frozen.
