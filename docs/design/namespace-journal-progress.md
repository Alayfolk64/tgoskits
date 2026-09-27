# Namespace mutation attempts and journal progress

## Status, evidence and scope

High-risk concurrency refactor in the preserved profiling worktree at
affddc3fecec02b31df94573063e4840433c9ebe. Independently reviewable design;
not maintainer approval or a merge-readiness claim. Full baseline and failure
evidence are in ../profiling/extent-read-ownership.md: 1297 s versus 1218 s,
CPU active 38.94%, AccessGate::read 112705365216 ns, sync_core mutex wait
53485138912 ns. A mkdir wait alone reached 19153882144 ns. Durations overlap;
the extent refactor cannot be credited as a full-build improvement.

Directory create/symlink/link/unlink/rmdir/rename currently acquire namespace
rights outside with_writeback_progress. A staging pause or typed journal-space
retry consequently retains those rights while another commit does device I/O.
A queued topology writer then excludes new lookup readers throughout the mount.
The workload is the unchanged 8-core/8-GiB QEMU-only full tg-xtask build.

Success means unrelated lookup can complete at a real journal I/O boundary,
failed atomic attempts publish no namespace/lifetime result, retries observe
current names and parent lifetime state, durability/errors remain intact, and
the next identical full workload improves. No timing claim before measurement.

## Prior art and alternatives

Existing boundaries retained: Namespace, AccessGate, mount admission, the
InodeLifetimeTracker zero-link/reap protocol, Ext4FileSystem metadata snapshots,
JBD2 scoped-handle rollback, detached commit and explicit sync policies.
Inspected internal history: ed3d4a1e6, c0b606fc7, 5d13ce881, 669e90adf,
828538f64. Open ext4 PR search on 2026-09-13 still lists #2015 at
6d5cc09f45a073680a270ae0b6047b24fd9eaff5; its previously inspected batching
scope does not implement this private namespace retry ownership boundary.
This is not a comprehensive review of that PR.

Linux source 980ab36ae5972c83f683b939e50c469c4947229e:

- [Directory locking](https://docs.kernel.org/filesystems/directory-locking.html)
  separates directory i_rwsem from cross-directory rename serialization.
- [lock_rename](https://github.com/torvalds/linux/blob/980ab36ae5972c83f683b939e50c469c4947229e/fs/namei.c#L3782-L3842)
  does not take the filesystem rename mutex for same-parent rename.
- [ext4_create](https://github.com/torvalds/linux/blob/980ab36ae5972c83f683b939e50c469c4947229e/fs/ext4/namei.c#L2813-L2847)
  reserves transaction credits and may retry allocation under the VFS parent
  lock. Linux does NOT establish that every journal wait releases inode locks.
- [may_create_dentry](https://github.com/torvalds/linux/blob/980ab36ae5972c83f683b939e50c469c4947229e/fs/namei.c#L3730-L3743)
  rejects a removed parent with ENOENT. Dropping namespace rights cannot be
  justified solely by retaining the parent's inode allocation.

Keep the current broad gate: safe baseline but retains the measured fan-out.
Replace the entire namespace protocol with Linux ancestor-ordered locks: wider
topology/alias changes than needed, and journal waits still require analysis.
Reserve a new namespace transaction owner before locking: existing reserved
handles are useful prior art, but would add portable public API and couple
adapter mutation kinds to core journal credit contracts. Selected instead:
reuse existing atomic namespace transactions, scope guards to one attempt,
and run existing pressure/abort completion only after guards drop.

## Ownership and state transitions

```
retained parent(s) + mount admission
  -> namespace guards -> mount guard -> one atomic mutation attempt
  -> release mount guard -> release namespace guards
  -> success / original error / journal progress -> fresh attempt
```

Split existing writeback orchestration into an explicit MutationAttempt result
and its completion. The attempt owns dirty/staging decisions and the existing
core call; completion owns retry classification, commit waiting and abort
persistence. Existing file mutation callers retain content exclusion across
the entire retry loop: partial extent writes/reap are NOT safe to replay as
new namespace operations and are not migrated.

One private InodeLifetime namespace method owns mount admission for the whole
operation and reacquires namespace rights for each attempt. Migrate every real
directory mutation caller together, including special files, symlinks, hard
links, unlink/rmdir, replace/exchange/whiteout rename. Per-attempt closures must
redo all name/type/cycle checks; do not carry a resolved victim across a wait.
Child retention and zero-link publication stay under the successful mount
guard. Reaping and VFS wrapper allocation/destruction stay outside the guards.

Retained parent allocation prevents reuse, not removal. Reuse the adapter's
existing canonical zero-link tracker to reject a removed retained parent on
each attempt, without new inode-table I/O or a duplicate version cache. Rename
also checks its retained destination parent. The lower core keeps the existing
writability/type/flag/transaction checks; no new raw inode entry point is public.

Staging means no operation has run. Namespace primitives use one
with_metadata_transaction (no restart_metadata_transaction): create, mkdir,
link, unlink/rmdir and rename roll back filesystem caches and JBD2 images on
error. Only their existing requires_journal_progress errors are retried.
Success lookup/retention happens before namespace release. Unexpected errors
are not converted to retry, abort persistence still occurs and the original
error survives. Synchronous mount/inode/directory policies still wait before
returning success. No journal ordering/barrier/cache policy changes.

## Validation and limitations

Complete refactor and deterministic cases first. Three static rounds before
runtime: ownership/consumer/error audit; strict combined-feature Clippy plus
xtask matrix and actual AArch64 kernel build; formatting/diff/source identity.
Regression must demonstrate failure on the original broad-guard arrangement,
then pass unchanged on the new arrangement. If RED requires temporarily
restoring the old arrangement, repeat applicable static gates before execution
and restore the candidate with fresh gates. No QEMU until GREEN.

Use the existing memory-device fixture's actual detached flush callback, not
timing sleeps or probabilistic threads. During forced staging and real journal
pressure, call real inode lookup; verify the target name is still absent or
unchanged, competing namespace work can proceed, and retry revalidates names
and removed parents. Cover ordinary errors, commit failure, mount admission,
both directory/topology scopes, replacement/exchange, and successful lifetime
publication. Existing full adapter/core and synchronous-policy tests remain.
The deterministic pressure interleaving belongs at this private boundary;
an external syscall test cannot select it without extra kernel fault-injection
ABI. No complete Linux syscall compatibility claim is made by this scoped work.

After gates and host regressions, run the unchanged full QEMU workload once,
without concurrent host builds/source edits or live-image reads. Preserve
before sources, all logs/graphs, ended rootfs and hashes; compare all 181 units
and output bytes. Do not accept another end-to-end regression as a speedup.
