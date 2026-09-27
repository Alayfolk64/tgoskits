# Shared inode read ownership

## Problem and success criteria

The verified full1222s/181-unit run in
[file-write-ownership](../profiling/file-write-ownership.md) removes the dominant
mount-held write I/O. Its largest remaining mutex leaf is Inode::read_at:
121319296256ns,38.0700% of mutex wait, predominantly page-fault cache fills.
All wrappers of the same inode currently hold one exclusive sleepable mutex
across mapping preparation, physical reads and completion, even for independent
read-only ranges. CPU active remains42.1426%; this is measured blocking evidence,
not a claim that page-fault instruction execution is the largest CPU cost.

Callers are regular file reads and existing concurrent page-cache fills. Success:
two same-inode readers can reach real device I/O before either completes, while
write/append/truncate/range changes cannot enter until readers drain. Hardlinks
share the same owner, failed waits/I/O release ownership, current metadata and
unlinked inode lifetime remain correct. Full benchmark must match frozen Linux
inputs/ELF again; no speedup is claimed before that run.

Non-goals: new syscall/ABI, on-disk formats, driver changes, unlocked inode
mutation, range-parallel writes, new cache policy, scheduler changes, bypassing
flush barriers, or a wholesale Linux folio implementation.

## Prior art and alternatives

Local Linux commit980ab36ae5972c83f683b939e50c469c4947229e:
[ext4 buffered/direct read dispatch](https://github.com/torvalds/linux/blob/980ab36ae5972c83f683b939e50c469c4947229e/fs/ext4/file.c#L67-L149)
uses shared inode exclusion for direct reads and generic_file_read_iter for
buffered reads, not one exclusive inode lock across every buffered read.
[filemap_update_page / filemap_create_folio](https://github.com/torvalds/linux/blob/980ab36ae5972c83f683b939e50c469c4947229e/mm/filemap.c#L2552-L2660)
use shared mapping invalidation protection with individual folio locking while
bringing data in; this prevents truncate/hole-punch from freeing mapped blocks.
[Kernel locking rules](https://docs.kernel.org/filesystems/locking.html#address-space-operations)
and [read(2), man-pages6.19](https://man7.org/linux/man-pages/man2/read.2.html)
were checked2026-09-12. Borrow the ownership separation, not exact Linux locking,
error priorities or ABI coverage claims.

Internal: existing PendingFills already coalesces overlapping page windows,
permits disjoint fill owners, and invalidates before content updates. Existing
PreparedInodeRead already owns data/mapping snapshots and a coherent endpoint;
its atime completion reloads current metadata under mount exclusion. Existing
namespace DirectoryGate already implements sleepable shared readers, writer
preference, checked counts, cancellation and wake-outside-IRQ-state.
Reuse this state machine at a private ext4 access boundary rather than duplicate
it or introduce a generic public RwLock/crate. SpinRwLock is inappropriate for
sleepable I/O. A new global RwLock would serialize unrelated writers and change
the mount model. Removing all protection risks stale mapping/block reuse.
Range locking has a larger overlap/upgrade protocol without evidence it is
needed for the current read/read bottleneck.

History inspected: ed3d4a1e6,c0b606fc7,5d13ce881,669e90adf,828538f64.
Open ext4 search refreshed; #2015 remains6d5cc09f45a073680a270ae0b6047b24fd9eaff5
(prior inspected small-write cache batching/mkfs), not shared read ownership.
This is scoped research, not an exhaustive PR review or merge approval.
Old unwired inode/dir.rs, inode/file.rs and fs/profile_tests files are historical
worktree assets: do not modify them as if they were current module consumers.

## Connected design

Move the existing gate to private fs/access, with distinct ReadAccess and
WriteAccess owners; namespace keeps its existing topology/directory protocol.
The inode registry now holds weak gate identities. InodeLifetime retains that
identity for every live wrapper, so concurrent hardlinks cannot get separate
gates, and stale weak keys do not pin dead inodes.

Lock order: registry lookup may nest under mount state but never acquires the
gate there; data operations acquire shared/exclusive inode access, then mount
admission/state. Gate state is only briefly IRQ-locked. Waiter registration and
predicate recheck use existing TaskWaiters, with no mount/IRQ lock during sleep.
New readers stop once any writer queues; cancelled writers restore admission.
No recursive read or read-to-write upgrade is promised. Namespace and inode
content gates are independent, not held while entering each other.

read_at retains ReadAccess over prepare -> independent device -> validated
completion. write_at/append/set_len/operate_range/update_metadata/xattr mutations
retain WriteAccess over the same boundaries they previously serialized. Reads
still exclude every content mutator, even for disjoint ranges; only read/read
concurrency expands. Legacy/oversized/unsupported-endpoint fallback remains
mount-serialized, with exact old full-result behavior.

Namespace link/unlink/rename retain existing short authoritative mount changes;
they do not mutate regular-file mapping or reclaim it until all InodeLifetime
owners disappear. Read completion updates current atime, never a stale links
snapshot. Existing cached-fill invalidation/publication owns page-cache races.
Mount admission still prevents shutdown from completing during actual read I/O.
No public core signature or raw device exposure is needed.

Contended access must remain visible as MutexWait through the existing profiler,
covering the full wait including retries and ending on failure or acquisition.
Do not claim reduced waiting merely because a new gate ceased reporting it.

## Risk and validation

High risk: changes concurrent ownership and wait failures. This independent
design needs maintainer approval before merge; local implementation is not that
approval. TaskWaiters can return WouldBlock/NoMemory; propagate typed errors,
do not spin, unwrap, swallow them or relax preemption checks.

Before runtime, finish all consumers and tests, then three static rounds:
ownership/error/profile audit; applicable strict combined Clippy plus xtask
matrices and actual8-CPU kernel build; formatting/source freeze/ELF-BIN identity.
Cover real same-inode alias nested reads, multiple reads in flight, pending
writer/read exclusion, cancellation and errors, no lost wakeups, counters,
metadata/namespace changes, EOF/holes/dirty-cache and compatibility fallbacks.
Retain existing tests, update only their lock-type assertions to the new
ownership contract. Then library regressions and full8c8g profiling.

Before-edit source archive: tmp/inode-read-sharing.FNhQ6c/sources-before.tar,
SHA57a7c52140589ce6075e4c4054d8d9b20f859a3376da7f1b4f58ea0adcecdcab.
Scope: ext4 adapter, cache, OS adapters, core file and owned API source trees;
not a whole-workspace commit. No new runtime test has started.
