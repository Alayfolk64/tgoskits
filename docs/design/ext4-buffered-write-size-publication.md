# Ext4 Buffered-Write Size Publication

## Problem

The shared page cache previously called `FileNodeOps::set_len()` before every
write that extended a file. For ext4, `set_len()` is the explicit truncate
boundary: it enters mount-wide state, constructs an `InodeResize`, evaluates
mapping removal and journal requirements, and completes metadata persistence.
Compiler workloads grow many object and archive files incrementally, so this
made ordinary buffered writes serialize on the ext4 mount lock.

The complete Orange Pi self-build profile in `tmp/board-runs/run20` attributed
about 36.3 scaled seconds of ext4 lock ownership to
`CachedFile::write_at_locked -> FileNodeOps::set_len -> InodeResize`. The same
lock convoy contributed to about 61.1 scaled seconds waiting below ext4
writeback.

## Users and success criteria

The direct user is the disk-backed page cache used by StarryOS regular files.
The change succeeds when:

- extending a cached file publishes its visible size without executing ext4's
  persistent truncate path;
- `len()` and `metadata().size` immediately observe the buffered size;
- explicit truncate, range mutation, direct I/O, and non-ext4 filesystems keep
  their existing behavior;
- sync writes the cached bytes and final size to storage, and a fresh mount can
  read them;
- invalid ext4 sizes fail before publication without taking mount-wide state.

## Prior art

Linux commit `980ab36ae5972c83f683b939e50c469c4947229e` separates buffered
size publication from persistent truncate. `generic_write_end()` in
`fs/buffer.c` calls `i_size_write()` while the folio remains locked, then marks
the inode dirty after releasing the folio. `ext4_write_end()` and
`ext4_da_write_end()` in `fs/ext4/inode.c` follow the same model; they do not
invoke the truncate path for each buffered extension.

## Design

`FileNodeOps::publish_cached_write_size()` is the capability boundary between
the generic cache and a filesystem. Its default calls `set_len()`, preserving
the old semantics for filesystems without a separate in-memory inode size.

Ext4 overrides the capability. It takes per-inode content-write ownership,
validates the prospective size using the same extent or legacy mapping limits
as persistent growth, and publishes the size through `AccessGate`. The gate
caches the inode's immutable mapping format alongside its hot size, so the
steady-state path performs no mount-state read. Ext4 metadata replaces only a
regular file's disk size with this hot value; all other inode fields remain
authoritative core metadata.

Dirty page ownership remains in `CachedFile`. Writeback already writes dirty
runs through `FileNodeOps::write_at()`, which allocates mappings, persists the
resulting size, and completes ext4 metadata handling before `sync()` returns.

## Alternatives

- Batching repeated `set_len()` calls would reduce frequency but retain the
  wrong truncate boundary and mount-lock convoy.
- Teaching ext4 `set_len()` to detect page-cache callers would mix explicit
  truncate semantics with buffered-write ownership.
- Publishing without validating the mapping limit could expose a size that
  writeback can never persist.

## Validation

The deterministic cache regression first failed because one buffered growth
called `set_len()`. After the implementation:

- the same regression passes with zero `set_len()` calls and one buffered-size
  publication;
- lock-independent publication and fresh-mount persistence regressions pass;
- all 340 ext4 and 238 FAT `ax-fs-ng` host tests pass;
- all 658 `rsext4` unit and integration tests pass;
- all 205 Starry kernel tests pass under QEMU with 8 CPUs and 8 GiB of memory.
