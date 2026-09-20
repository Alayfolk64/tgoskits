# ext4 inode hot-metadata publication

## Problem and evidence

The complete run20 StarryOS self-build profile shows that the next ext4
contention point is not block completion or the NVMe driver. Sampled
`ext4_lock_wait` time is 409.209 seconds, while block-completion draining is
0.939 seconds. The largest individual waiter is the private file-fault path:

```text
handle_page_fault_result -> CowBackend -> CachedFile::file_len
    -> Inode::len -> InodeLifetime::metadata
    -> read_live_inode_info -> ext4 mount-state lock
```

That path accounts for about 143.021 seconds of sampled wait. The same
mount-wide metadata lookup is also used by `writeback_policy()` after ordinary
writes and contributes another 13.180 seconds of sampled wait. Both values are
scaled sampled totals, not independent wall-clock durations.

Linux commit `980ab36ae5972c83f683b939e50c469c4947229e` keeps `i_size` in the
shared in-memory inode. On 64-bit SMP systems, `i_size_read()` performs an
acquire load and `i_size_write()` performs a release store while the caller
holds the inode write lock. Page-fault bounds checks therefore do not enter a
filesystem-global metadata lock.

## Success criteria and non-goals

- Repeated regular-file `len()` and inode writeback-policy reads do not acquire
  ext4 mount state after one authoritative inode load, even after the bounded
  core inode-table cache evicts that record.
- Cached and direct handles, including hard links, observe one shared size
  because the state is owned by the existing per-inode runtime object.
- Every successful size-changing operation publishes its exact resulting
  size before releasing per-inode content-write ownership.
- Failed mutations never publish a speculative size.
- Full `metadata()` remains an authoritative ext4 read, and directory sizes
  continue to use that path because directory mutations use namespace rather
  than regular-file content exclusion.
- On-disk format, journal ordering, syscall return values, errno mapping,
  permissions, timestamps, and file data semantics remain unchanged.

This change does not cache complete `stat` metadata, weaken content exclusion,
change ext4 transaction scope, or claim a board-level speedup without a later
attended measurement.

## Alternatives

| Alternative | Result |
| --- | --- |
| Keep loading inode metadata under mount state | Rejected: preserves the measured page-fault and writeback lock convoy. |
| Store length only in each `CachedFile` | Rejected: direct I/O and independently opened or hard-linked handles can change size, creating multiple facts. |
| Cache complete inode metadata | Rejected: mode, owner, timestamps, link count, and directory state have different mutation and ordering rules. |
| Publish hot immutable/mutating fields in the shared inode runtime | Selected: one identity-scoped state source with Linux-like read/write ordering. |

## Ownership and ordering

The filesystem already interns one `AccessGate` per live inode number and all
`InodeLifetime` owners retain that object. Its inode-scoped instances therefore
own two additional hot fields:

- regular-file size, initialized to an unknown sentinel;
- writeback policy, initialized to an unknown sentinel.

An authoritative metadata read initializes an unknown value with compare and
exchange. It never unconditionally overwrites a value: if a writer publishes a
new size after the disk/cache read, the initializer loses the compare-exchange
and retains the writer's newer value.

Regular-file writers hold the existing content write access across old-size
observation, the complete ext4 mutation, and publication. A successful write,
append, truncate, or range operation stores the exact new size with Release
ordering. Readers load with Acquire ordering, matching the visibility role of
Linux `i_size_write()` and `i_size_read()`. Errors leave the previous value
unchanged. Zero-length writes preserve size.

Inode flags used by `WritebackPolicy` are immutable through the current VFS
metadata update API, so they are decoded once and published with the same
initialize-if-unknown rule. If an inode-flags mutation API is added later, it
must update or invalidate this field in the same change.

The unknown size sentinel is safe because ext4's supported file-size range is
strictly below `u64::MAX`. Non-regular inode metadata does not initialize the
size field, so directory and special-node `len()` retain the authoritative
path.

## Compatibility and risk

The Starry syscall guideline applies indirectly to `read`, `write`, `pwrite`,
`writev`, `truncate`, `ftruncate`, `fallocate`, `mmap`, `stat`, and their
shared VFS helpers. No ABI or error precedence changes: the optimization only
changes how an already published regular-file size and immutable flags are
read. The existing ext4 mutation remains authoritative and publication occurs
only after it succeeds.

The principal risk is stale or speculative size publication. Deterministic
tests must cover lock-free reads while mount state is unavailable, all
size-changing operations, zero-length writes, failed mutations, and independent
handles sharing the same inode runtime.

## Validation and rollback

Before runtime validation, run `git diff --check`, repository-wide rustfmt
check, and targeted `ax-fs-ng` clippy. Then run the focused regression, the
complete applicable `ax-fs-ng` host-test matrix, and the 8-vCPU/8-GiB AArch64
Starry QEMU axtest suite. No unattended physical-board operation is permitted.

The change has no persistent representation or migration. It can be reverted
independently; rollback restores mount-state metadata reads without changing
disk state.

The completed implementation passed:

- `git diff --check`, repository-wide rustfmt check, and all seven targeted
  `ax-fs-ng` clippy configurations;
- the deterministic cold-inode-cache contention regression, which failed on
  the old implementation and passes on the hot-metadata implementation;
- all 336 applicable `ax-fs-ng` unit tests and all three root-selector tests;
- all 205 Starry kernel axtests in AArch64 QEMU with 8 vCPUs, 8 GiB RAM, and a
  snapshot root filesystem.

No physical-board boot, deployment, reset, or filesystem write was performed
while the board was unattended. Board-level performance remains unmeasured.
