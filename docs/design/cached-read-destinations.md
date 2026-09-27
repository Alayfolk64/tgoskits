# Cached reads with resident kernel destinations

## Problem and acceptance

The complete private-fault-transactions run takes 1869 s. Of 3167 memcpy
samples, 2618 (82.66498%) belong to private file faults. CachedFile::read_at
allocates a scratch PageCache for every call, copies cached bytes there under
the cache lock, then invokes an arbitrary Writer after unlocking. The second
step is essential for faultable user Writers, but redundant when the destination
is an unpublished, resident PreparedFrame represented by BorrowedCursor.

Refactor the entire cached-read traversal and destination boundary. A resident
kernel read must copy directly into the final initialized-range cursor, without
a scratch page. Arbitrary Writers must still receive an owned snapshot outside
all cache locks, with complete handling of short writes. Both paths must share
page selection, length clipping, readahead, cache-hit stability and miss loading.
The existing source snapshot, QEMU, 8c8g and tg-xtask workload stay frozen.

This is a high-risk shared interface/lock-boundary change, not an ext4 format
rewrite or a new syscall. This separately reviewable design precedes the code;
no project commit, push or merge is implied. A performance improvement remains
unproven until the next complete cold build.

## Prior art and alternatives

- Linux `980ab36ae5972c83f683b939e50c469c4947229e`, mm/filemap.c:2777–2890:
  filemap_read retains folios and copies them directly through
  copy_folio_to_iter; lib/iov_iter.c:361–388 maps a page and copies to the
  destination iterator. Reuse the separation of destination capability and
  page traversal, not Linux's folio ownership or user-fault locking assumptions.
- MOSS `5a54e4413c9657bfb531cc32f6d688090465200d`,
  kernel/src/mm/aspace/backend/cow.rs:400–530 directly retains/validates file
  folios for selected faults and copies for unsupported shapes. Direct mapping
  would require a larger cache-PTE/COW/truncate ownership protocol in Starry.
  Do not introduce it implicitly to remove this extra copy.
- Starry's current ax-io already uses core BorrowedBuf/BorrowedCursor for
  initialized-range I/O. Reuse those concrete types; no additional buffer trait,
  specialization hierarchy, dependency or unsafe user-buffer shortcut is needed.
- History ed3d4a1e6/c0b606fc7 and the current cache update/read path retain
  cache locking and typed VFS errors. The updating flag hides tentative content
  while slow mutations temporarily release the index lock. Preserve that rule.
- Open-title searches on 2026-09-11 for cache/fault found no equivalent cached
  kernel-destination change. Relevant #2000 remains 6c1b006a852a458ad577f92ea8a5978a56fb4e59
  (larger fault-in window), #1993 remains 95e2dc3a89d93c86c503e9f5821fe0e8e68c440c
  (fault-in errno). Their previously inspected implementations are complementary.
  Title searches do not establish exhaustive absence across every open PR.

Keeping scratch retains allocation and two copies. Specializing generic Write
inside the filesystem duplicates ax-io's buffer taxonomy. Replacing read_at with
IoBufMutExt::read_from indiscriminately risks losing bytes: its generic transfer
may read more than a short Writer accepts. Changing that shared helper expands
the scope unnecessarily. Choose an explicit concrete read_buf_at entry and one
private traversal shared with the original generic snapshot path.

## Ownership and flow

CachedRead owns only a borrowed CachedFile, its resolved FileNode, the frozen
request end, current offset and existing readahead window. It owns no page or
lock between calls. read_page accepts a BorrowedCursor and copies at most one
page fragment, bounded by the request end and cursor capacity. The cache lock
pins the source until append completes. On a stable hit, no io_lock is needed;
on a miss or tentative update, release the index, take io_lock, populate with
the existing method, reacquire the index and copy. No arbitrary callback runs
under either lock. Advance position only after a successful copy.

CachedFile::read_buf_at loops read_page directly into resident kernel storage;
its return is this call's byte count, not the cursor's lifetime filled count.
It may leave a filled prefix on error, as existing borrowed-buffer I/O does.
CachedFile::read_at uses the same traversal but retains one scratch page and
calls Writer::write_all only after read_page releases every cache lock. EOF and
zero capacity return before allocating. A short Writer must not drop the unread
part of a scratch snapshot. Empty/EOF, permission and error policy remain owned
by the existing layers; this is not a general Linux read errno certification.

FileBackend::read_buf_at dispatches cached destinations to the new method and
direct destinations to existing direct read_at, which retains its initialized
safe-slice requirement and short-read/EOF behavior. Private fault preparation
and locked file-run preparation use this entry. PTE ownership, COW, RSS,
truncation listeners, readahead policy and the on-disk format are unchanged.
Existing 32-bit cached page-number limits are not widened in this change.

## Validation and risks

Deterministic host cases cover warm reads with no page allocation or backing
read, uninitialized destinations and prefilled cursors, cross-page/EOF clipping,
zero capacity, stable hits during unrelated I/O, tentative updates, direct
backend short reads/errors, and arbitrary Writers which check lock freedom and
accept only short chunks. Existing cache truncate/reclaim/writeback tests and
Starry private-fault tests remain in the actual runners. A regression replacing
the new path with old read_at must violate the allocation assertion.

This is an optimization/behavior-preserving traversal refactor, not a claim that
old double-copy behavior returned incorrect data. After the whole implementation:
1. Read all ownership, cursor, callback and error paths against this design.
2. Run fmt, targeted host/config clippy and the actual kernel-only build.
3. Recheck final source/callers, test discovery and frozen build artifacts.
Only then run host/QEMU regressions and a new full cold 8c8g profiling workload.
Require complete compile-unit/ELF equality, raw SHA checks and read-only fsck.
No timeout or partial profile may substitute for the full result.
