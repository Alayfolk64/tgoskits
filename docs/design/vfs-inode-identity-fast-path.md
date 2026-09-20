# VFS inode identity fast path

## Problem

Starry advisory-lock lookup and close-time cleanup key POSIX, OFD, and `flock`
state by `(device, inode)`. Regular files and directories currently obtain that
pair through `Location::metadata()`. On an ext4 inode-table cache miss this
loads complete inode attributes and can wait for mount-wide filesystem state,
although neither advisory-lock operation needs size, mode, timestamps, link
count, or blocks.

`Location::metadata()` already overwrites the backend's device field with the
mountpoint device identity. The requested pair is therefore exactly the
existing `Mountpoint::device()` plus `DirEntry::inode()` values.

## Semantics and Linux alignment

Linux associates advisory locks with the opened file's inode and superblock
identity. Bind mounts of the same filesystem must share the key, while distinct
mounted filesystems with equal inode numbers must not collide. TGOSKits already
implements that distinction: bind/clone mountpoints retain the source device,
and a new filesystem mount receives a new device identity.

The VFS exposes `Location::inode_identity()` as an infallible snapshot of those
two immutable identity fields. `metadata()` continues to provide complete,
fallible attributes for `stat` and other real metadata consumers. Starry file
and directory `inode_key()` implementations use only the identity method.

This does not change lock ownership, range conflict rules, wakeups, errno,
close ordering, bind-mount sharing, or persistent state. It only removes an
unrelated attribute load from identity lookup.

## Alternatives

| Alternative | Result |
| --- | --- |
| Keep using complete metadata | Rejected: retains unrelated ext4 I/O and lock waits on every lock lookup and close. |
| Cache a key separately in each Starry file wrapper | Rejected: duplicates VFS mount/inode identity and complicates mount construction. |
| Read mountpoint device and entry inode at each Starry call site | Correct but leaks VFS composition details and duplicates the invariant. |
| Add a typed VFS identity query | Selected: one boundary method with the same values already used by `Location::metadata()`. |

## Validation

A deterministic VFS test counts backend `metadata()` calls. It failed with one
call when identity was implemented through metadata and passes with zero calls
when identity reads the stable fields directly. The test also verifies the
returned mountpoint device and inode values.

The completed change passed:

- `git diff --check`;
- `cargo fmt --all -- --check`;
- all 3 `axfs-ng-vfs` clippy configurations;
- all 104 `starry-kernel` architecture/feature clippy configurations;
- all 11 `axfs-ng-vfs` host integration tests;
- all 205 Starry kernel axtests in an 8-vCPU/8-GiB AArch64 QEMU guest.

The QEMU root filesystem ran with `snapshot=on`. No physical-board action was
performed.
