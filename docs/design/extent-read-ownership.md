# Independent file mapping and read publication

## Evidence and scope

The complete, verified1218s run in [inode-read-sharing](../profiling/inode-read-sharing.md)
has mount-lock waiting109409064896ns/40.1314% of reported mutex waiting.
read_inode owns32359739008ns/37.3340% of ext4 hold time. Cold extent child
reads contribute23997756768ns in the synchronous-read graph. These overlapping
figures identify a path, not additive wall savings. Shared inode reads removed
99.4492% of their old exclusion wait without a significant full-build gain.

PreparedFileRead::prepare still allocates/zeros the data buffer, walks extent
nodes and snapshots cached bytes while Ext4Filesystem::read_inode holds mount
state. Only its subsequent ordinary data I/O is currently independent. The
next connected refactor moves mapping reads, parsing, private-buffer work and
final byte copying outside that global critical section. Inode/mount admission
remains held throughout. Success requires real cold extent I/O under these
owners with unrelated inode progress, unchanged data/error/coherence behavior,
and a matched full8c8g run after static3 and lowest-layer runtime coverage.

Non-goals: new syscall/ABI/disk format, removing barriers, new extent cache,
parallel writes, scheduler or VMA changes, replacing ext4 parsers, and blanket
claims that every metadata mutation or legacy fallback no longer performs I/O.
Atime still uses the existing canonical mutation/writeback-progress path.

## Prior art and alternatives

Linux checkout980ab36ae5972c83f683b939e50c469c4947229e was verified again.
[ext4_map_blocks](https://github.com/torvalds/linux/blob/980ab36ae5972c83f683b939e50c469c4947229e/fs/ext4/inode.c#L679-L828)
first consults extent status, then queries under the inode's shared i_data_sem;
it does not hold one filesystem-wide mutex across every cold mapping read.
[ext4 readpage mapping](https://github.com/torvalds/linux/blob/980ab36ae5972c83f683b939e50c469c4947229e/fs/ext4/readpage.c#L250-L345)
reuses mapped runs and treats holes separately before BIO submission.
[VFS mapping locks](https://docs.kernel.org/filesystems/locking.html#address-space-operations)
and [read(2)](https://man7.org/linux/man-pages/man2/read.2.html) checked2026-09-12.
Reuse the ownership separation and batching, not Linux's ABI or entire cache.

Internal prior art: owned/directory_read already provides inode snapshots,
geometry/system-zone/checksum context, a read-only MetadataBlockRead walker,
short cache/journal visibility callbacks, independent coherent endpoints and
version-validated results/errors. initialized_runs_with_reader already exists.
Cold live-inode loading already publishes without dirty eviction. Reuse these
boundaries for files and directories, preserving old directory public aliases.
Relevant histories d4ebf0652,fad09ebd3,828538f64 were inspected; open ext4 PR
search still finds #2015 at6d5cc09f45a073680a270ae0b6047b24fd9eaff5, not this
ownership protocol. This is scoped prior-art research, not a PR review.

Keeping the status quo retains the measured global hold. Pre-reading guessed
extent blocks into another cache duplicates coherence/invalidation knowledge.
Dropping the lock around borrowed filesystem/device state breaks ownership.
A second file-specific mapping walker duplicates validation and corruption
rules. The selected approach promotes the existing directory block-read
ownership into a shared private mounted-read boundary with two real consumers.

## Connected design and invariants

The shared private boundary owns a snapshot of the inode, mount identity,
existing InodeLoadVersion, superblock and shared system zones, plus one coherent
forked endpoint. Its opaque visibility request contains only owner-produced
physical addresses and a mapping/data kind. A short mount callback returns the
canonical data-cache image first for data, then the journal-visible image, or
an authoritative miss. Mapping blocks never consult/enter the directory data
cache. Physical reads, file-buffer allocation/zeroing and parser callbacks run
after the mount callback returns. Bounded visibility-vector allocation and
cloning journal-owned bytes into retained images still occur in the callback;
ordinary data-cache images only clone their Arc. Bounds, checksum, tree depth/order and system-zone validation
remain in the existing walkers; no raw device capability escapes publicly.

Directory lookup retains its namespace/lifetime policy and compatibility names.
File preparation returns an explicit cold-inode, ready-read, empty or serialized
phase. Cold inode execution reuses PreparedLiveInodeRead; publication can return
a fresh phase or retry. Ordinary ready reads run the existing range/coalescing
plan over the shared metadata source and short data visibility callbacks.

Every result and I/O/parse error remains private until the original mount and
inode version are validated. Superseded work requests retry without touching
the caller's output. Current errors remain typed; only the documented legacy,
oversized, overflowing or UnsupportedCapability cases select serialization.
After validation, capacity checking and existing current-inode atime progress,
a borrowed validated byte view permits copying outside mounted exclusion.
The view borrows completed storage, not mutable mount state. The adapter keeps
inode/mount admission through final copying; no public mutable buffer escapes.

Version validation supplements, never replaces, retained allocation and shared
inode/namespace exclusion. Atime/link metadata updates may invalidate another
reader and cause a safe retry; rollback and failed mutations also invalidate.
Do not ignore invalidation just because it appears unrelated to file contents.
The existing journal progress callback remains outside mount locking and may
retry metadata completion. No new unsafe code, lock class or driver contract.

## Risk, migration and validation

High risk: shared ownership and public owned-read phase changes. This design is
independently reviewable and needs maintainer agreement before any merge; no
merge/commit/PR is authorized here. Migrate every active core/adapter/test
consumer together. Keep historical unwired files and unrelated dirty edits.
The source-before archive is tmp/extent-read-ownership.iQN3IJ/sources-before.tar,
SHA6b3eb116dab296999eb55b79f21f96a318447f8199282e34f69695fda1e8985f;
scope core rsext4 source, ext4 adapter and file cache, not the whole kernel.

Finish the connected implementation and deterministic tests before runtime.
Three static rounds: ownership/error/coherence/consumer audit; strict combined
Clippy plus targeted xtask matrices and actual8CPU kernel compilation; format,
diff, source freeze and independent ELF/BIN identity. Then core/adapter tests
and matched full profiling. Renew gates after any production correction.

Coverage must include real external extent nodes, cold inode phase, dirty and
journal-only images, batch preservation, holes/unwritten/EOF and untouched
output tails, short buffers, errors before publication, foreign mount, invalid
versions/rollback, metadata changes during I/O, mount unlocked during real
mapping reads, retained shared inode ownership, and all existing directory
lookup behavior. Performance remains unmeasured until the new full run.
