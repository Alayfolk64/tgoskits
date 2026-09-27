# Regular-file write ownership

## Status and measured problem

Design and implementation in progress; no new write-path performance result or
merge-readiness claim. The preceding directory ownership refactor has completed
the matching 181-unit, 8c8g QEMU workload in 1252 s, versus 1323 s before it and
735 s on Linux. See [the full record](../profiling/directory-read-ownership.md).
This phase retains that workload, reusable guest tg-xtask, toolchain and rootfs.

The full kernel profile identifies `write_locked` as 148560955328 ns / 58.3632%
of ext4 hold events; its block-write subset is 141231344064 ns. The dominant
caller is the ext4 worker writing VFS dirty pages. CPU active sampling remains
41.2991%. These overlapping sampled durations cannot be subtracted from wall
time or used to predict an equal speedup.

The goal is to remove ordinary extent-file data I/O from mount-state exclusion,
including growth, existing mappings, partial blocks and sparse/unwritten ranges.
Only optimizing aligned overwrites would leave the compiler's growing output
files on the original path and does not satisfy this design.

## Prior art and alternatives

Linux checkout `/home/wuxun/Projects/linux` is fixed at
`980ab36ae5972c83f683b939e50c469c4947229e` (verified 2026-09-12).

- [`ext4_end_io_end`](https://github.com/torvalds/linux/blob/980ab36ae5972c83f683b939e50c469c4947229e/fs/ext4/page-io.c#L166-L211)
  converts unwritten extents after successful I/O, not after failed writeback;
  its comment explicitly relates truncation exclusion to I/O completion.
- [`journal_submit_data_buffers` and completion](https://github.com/torvalds/linux/blob/980ab36ae5972c83f683b939e50c469c4947229e/fs/jbd2/commit.c#L229-L329)
  release the journal list lock around inode submission and waiting.
- [Commit ordering](https://github.com/torvalds/linux/blob/980ab36ae5972c83f683b939e50c469c4947229e/fs/jbd2/commit.c#L764-L810)
  waits for ordered data before the following flush/commit-record steps.
  The [ext4 journal documentation](https://docs.kernel.org/filesystems/ext4/journal.html)
  distinguishes metadata journaling, data ordering and later checkpointing.

Reuse the ordering and ownership invariants, not Linux's worker or BIO runtime.
TGOSKits already has coherent `ForkBlockIo` endpoints, detached journal receipts,
per-inode content locks, open-unlinked lifetime ownership and mount admission.
The gap is the monolithic extent-write preparation/data/conversion call, not a
missing device queue API. No portable driver or architecture change is planned.

The local file-I/O history was inspected (`1ca87a306`, `828538f64`). The current
dirty implementation is the authority; those historical files predate this
ownership work. The open PR inventory and #2015 were refreshed on 2026-09-12;
its head remains `6d5cc09f45a073680a270ae0b6047b24fd9eaff5`. Its actual file-write
diff delays runs of at most 16 blocks into the private data cache, while its
journal diff adds mkfs zero-fill/cache invalidation. Neither provides a detached
ordinary-file write owner. Its remaining cache/inode/jbd2 diff still needs full
overlap review before merge; no PR review or approval is claimed here.

| Alternative | Decision |
| --- | --- |
| Keep the mount lock across synchronous data writes | Preserves the measured dominant serialization. |
| Move dirty data into another mount-wide queue/cache | Adds a second ordinary-data owner beside VFS/device caching and requires its own ordering/error protocol. |
| Merely unlock around borrowed `&mut Ext4` or JBD2 state | Rejected: mutable state and rollback ownership cannot escape exclusion this way. |
| Duplicate a new extent allocator/parser for a fast path | Rejected: allocation, checksum and unwritten conversion must retain one implementation. |
| Split existing write state into preparation, owned I/O and completion | Selected; reuse shared algorithms for serialized compatibility and the mounted detached path. |

## Ownership and transitions

The implementation must make these phases distinct:

```text
inode lifetime + content lock + admitted operation
  -> prepare mapping/allocation under mount state
  -> immutable physical write plan + coherent endpoint
  -> execute data I/O without mount state
  -> completion receipt, including errors
  -> publish conversion/size/timestamps under mount state
  -> release admission and inode protection
```

Preparation allocates holes as unwritten and splits conversion ranges before
data submission. No newly initialized extent is published merely because I/O
was scheduled. Existing allocation and journal-progress boundaries must remain
restartable. Once data I/O has executed, completion retries must retain that
receipt instead of reallocating or writing the whole payload again.

Partial initialized blocks preserve bytes outside the requested range;
unwritten/new blocks use zeros outside it. Full runs remain batched. The plan
must retain immutable source bytes and validated physical ranges for the whole
I/O lifetime. Device geometry and checked offset/count conversions belong at
preparation/execution boundaries, not in adapter callers.

The existing per-inode lock and `InodeLifetime` must protect mapping and block
reuse through completion. Truncate, range operations, independently opened
writers and final reap may not invalidate an in-flight write. Namespace changes
can alter link counts while the mount lock is released: completion must merge
into the then-current inode, never overwrite those changes with its prepared
inode snapshot. Foreign, repeated or abandoned receipts must not publish a
successful write. Shutdown must drain admitted operations before clean publish.

Ordinary data uses the shared device cache in the adapter. The detached path
must account for any overlapping private cache image before submission and may
not let an old clean image shadow new bytes. Standalone/private-cache and legacy
indirect compatibility remain explicit; capability errors are distinct from
physical I/O, allocation and journal failures. No error chooses a silent fallback.

Journal progress and completion require particular care: a reservation held
across data I/O can prevent unrelated commit progress. A completion that itself
needs log space must not wait on a commit gate which is waiting for that same
completion. The implementation must distinguish unfinished data, completed data
and metadata publication, and document its actual ordering/admission mechanism
before the final static review. A global data fence is not assumed necessary or
sufficient merely from the Linux analogy.

## Connected implementation, 2026-09-12

The ordinary extent-file adapter now calls `write_extent_inode`: one admission
guard spans preparation, data execution and all completion retries. Its caller's
existing inode content lock and `InodeLifetime` span the same interval. The core
adds an inode mapping lease with mount/owner identity; truncation, allocation,
range changes, data reads/writes and final reap reject that inode while leased.
Remount, cache-policy transition and clean unmount reject undrained owners.
Already prepared reads still require the existing caller-owned content/lifetime
exclusion; the new write ledger is not a replacement for that read contract.

`PreparedFileWrite` shares the existing unwritten allocator/split implementation.
It releases the finish reservation before returning, then builds a physical
write plan with at most 1 MiB per full request and one-block edge buffers. Full
runs borrow immutable caller bytes; initialized edges use a transferred clean
image or a lock-external home read, and unwritten edges start with zeros. Dirty
private overlap is rejected without discarding it. Held read images are discarded
before transfer and successful completion. `FileDataEndpoint` exposes only
ordinary block reads/writes, not mutable mount or journal state.

Every executed write returns a receipt, including I/O errors. Data failure never
enters conversion. Completion reloads the current inode, retaining concurrent
links, orphan pointers, ownership and xattr accounting, and reuses the existing
conversion/rollback algorithm. JournalProgress retains the receipt and lease;
metadata retries never execute data again. Other terminal results release the
lease. The adapter's completion guard discards an already-completed owner if
external progress itself fails before publication can be retried. Dropping an
unexecuted owner remains fail-closed; explicit cancel returns an error receipt.

Commit and checkpoint may proceed while file data is in flight: before successful
completion this write has published only unwritten allocations, not initialized
new data or new size. Completion's conversion enters a later/current metadata
transaction only after data I/O returns. `PreparedCommit::execute` flushes its
coherent endpoint before journal work. The commit gate never waits for file data
completion, and ordinary data retains no journal reservation across the wait.
Thus completion can acquire journal progress without a circular data fence.
This is the source-level ordering argument; the added remount/partial-failure
tests still must execute before durability is considered runtime-validated.

The original 2462-line `file/io.rs` is now private `file/io/` domains: allocation
footprints, preallocation, range dispatch/zeroing, shift replacement, extent and
legacy removal, shared transaction steps, resize/orphan recovery, and reads.
Path-based writing lives in `file/write/path.rs`. Existing entry points and
algorithm bodies are retained; helper visibility expands only inside private
`file::io`. This movement is distinct from the connected ownership change.

## Compatibility and acceptance gates

This is high risk: it changes concurrency/resource ownership and indirect VFS
write/sync behavior. The code-quality, feature-development and Starry syscall
guidelines apply. No new syscall, on-disk format, mount option, driver dependency
or benchmark shortcut is intended. Syscall numbers, buffer validation and VFS
permission checks remain in their existing layers. This document is not a full
per-syscall compatibility review; final author inspection must enumerate the
affected write/read/stat/truncate/range/sync/unlink/rename and shutdown callers.
Maintainer design approval is still required before merge, not yet obtained.

Implement the complete ownership boundary first. Internal build checks can
compile intermediate refactor stages, but **no runtime test** starts until the
connected production change has passed three final static rounds:

1. State transitions, mapping lifetime, cache coherence, lock order, journal
   durability, every cleanup/retry path, caller coverage and compatibility.
2. Strict xtask Clippy for every changed crate, combined host-test features
   where the xtask matrix cannot express them, and the actual 8-CPU kernel build.
3. Formatting/diff checks, final source snapshot and ELF/BIN consistency, and
   confirmation that the deterministic tests are compiled by their real modules.

After the gates: real-memory-device core/adapter tests cover full/partial writes,
growth, sparse/unwritten mappings, injected errors, log pressure, concurrent
unrelated operations, same-inode exclusion, link/unlink during I/O, shutdown,
coherent rereads and reuse. Existing assertions remain unless the tested protocol
is intentionally replaced and the new exact invariant is documented. A bug found
during the refactor requires a deterministic pre-fix failure and the same test
passing after repair; do not silently turn it into a post-fix-only test.

Finally run the full fixed 181-unit QEMU workload, not a time-limited subset;
verify terminal exit, every artifact/hash/unit, Linux-identical final ELF,
read-only post-exit fsck and full direct-kernel flame graphs. Compare wall time,
CPU activity, mount hold/wait and block I/O with the 1252 s baseline. A failed or
unmeasured run cannot establish performance. Keep the original kernel/disk and
pre-edit source snapshot for scoped recovery; never reset the dirty worktree.
