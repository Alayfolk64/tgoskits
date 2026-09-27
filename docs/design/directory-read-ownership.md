# Directory read ownership: ext4 lookup without mount-wide I/O exclusion

## Status and scope

Work in progress on `profiling/manual-20260907`, based on
`affddc3fecec02b31df94573063e4840433c9ebe` plus the preserved profiling worktree.
This is **not an accepted performance result**. The production read path now
connects independent directory/mapping I/O, parent and child inode loading,
namespace and mount admission, and atomic child lifetime acquisition. Its
development Clippy checks are not final static acceptance or runtime evidence.

The caller is StarryOS compiling the frozen arceos-helloworld workload using the
rootfs tg-xtask under the existing patched QEMU, 8 CPUs and 8 GiB. No xtask perf,
board run, workload reduction, compiler-cache change or durability relaxation is
part of this work. The positive 1323-second full run remains the baseline.

This is high-risk shared/concurrency work. This document makes the design
independently reviewable; it does not claim maintainer approval or merge readiness.

## Measured problem

Source: `target/profiling/arceos-helloworld/starry/private-file-cache-backing/`.
The complete run compiled all 181 units, returned zero, and produced the same
ArceOS executable as Linux. Its saved kernel ELF, not the current build output,
is the symbol authority:
`9d2980c8cad07dc40ed283eb1c32bba82d3896dd9b1753e5771d7681ad339a43`.

The current full-run flame graphs and raw folded events show:

| Measurement | Aggregate sampled time |
| --- | ---: |
| All mutex waits | 1998565186336 ns |
| Mutex waits at `Ext4Guard::acquire` | 1386202358880 ns (69.36%) |
| All ext4 holds | 418947462256 ns |
| `lookup_locked` ext4 holds | 140877707632 ns (33.63%) |
| Block reads under `lookup_locked` | 130919106480 ns |
| Of those, inode-table `InodeCache::ensure_loaded` | 62390635872 ns |
| Of those, directory/HTree lookup | 68528470608 ns |

The final three rows were reduced over **every** line in
`rendered/block-read-sync.folded`, not the truncated top-stack JSON display.
Classification first selects `lookup_locked`, then the inode loader, then
directory/HTree frames; the unclassified remainder is zero. The exact command
executed on 2026-09-12 was:

```sh
awk '/lookup_locked/ { total += $NF; if (index($0, "InodeCache>::ensure_loaded")) inode += $NF; else if (index($0, "find_named_entry_in_parent") || index($0, "hashtree")) directory += $NF; else other += $NF } END { printf "lookup_block_read_ns=%.0f\ninode_load_ns=%.0f\ndirectory_ns=%.0f\nother_ns=%.0f\n", total, inode, directory, other }' target/profiling/arceos-helloworld/starry/private-file-cache-backing/rendered/block-read-sync.folded
```

`$NF` is the aggregate duration in each folded record. There is no shell pipe.
These overlapping, sampled event durations are not wall time, and independent
event totals do not prove an exact 130.92-second saving. They do establish that
both inode-table and directory reads belong in the lock-boundary redesign.
CPU activity remains only 38.06%; this issue is not solved by the previous speedup.

## Internal and external prior art

Existing VFS positive/negative directory caching, `InodeMetadataReader`, deferred
inode reap, independent ordinary-file reads, coherent block endpoints, and
background journal ownership are retained. Increasing the 128-entry inode cache
or adding another name cache does not remove cold-read serialization.

The inspected local history includes `ed3d4a1e6`, `c0b606fc7`, `5d13ce881`,
`669e90adf`, and `1ca87a306`. On 2026-09-12 the open `ext4` PR search returned
#2015 at `6d5cc09f45a073680a270ae0b6047b24fd9eaff5` among unrelated consumers.
Its file inventory concerns batching, caches and write I/O; this inventory alone
is not a full semantic overlap review. Open issue search `ext4 lookup` returned
#2351 (filesystem calls inside Unix-socket spinlock/preemption guards). The new
read boundary does not excuse or claim to fix that separate caller constraint.
Before a final implementation is accepted, repeat the relevant overlap check.

Linux source is pinned to `980ab36ae5972c83f683b939e50c469c4947229e`:

- [`ext4_lookup`](https://github.com/torvalds/linux/blob/980ab36ae5972c83f683b939e50c469c4947229e/fs/ext4/namei.c#L1760-L1813)
  resolves a directory record and then obtains an inode via `ext4_iget`, without
  an all-purpose ext4 mount mutex spanning both I/O operations.
- [Path lookup's directory and per-name protection](https://github.com/torvalds/linux/blob/980ab36ae5972c83f683b939e50c469c4947229e/Documentation/filesystems/path-lookup.rst#L246-L322)
  explains shared directory exclusion and coordination of same-name misses.
- [Directory locking](https://docs.kernel.org/filesystems/directory-locking.html)
  distinguishes per-inode protection from cross-directory rename exclusion.
- [path_resolution(7)](https://man7.org/linux/man-pages/man7/path_resolution.7.html)
  is the public pathname-resolution contract; this change does not move or
  remove existing pathname, credential, permission or symlink checks.

Borrow the ownership separation, not Linux's RCU implementation or unproven lock
equivalences. MOSS's profiling-driven method remains the methodology; there is
no claim that this rsext4 mechanism is copied from its lwext4 implementation.

## Alternatives and selected direction

| Alternative | Assessment |
| --- | --- |
| Keep the global lock | Baseline; cold inode and directory reads serialize unrelated tasks |
| Increase caches or add a second dentry cache | Does not fix cold misses and duplicates existing ownership |
| Replace the outer mutex with an RW lock | Invalid while lookup requires mutable cache/device/mount access |
| Copy an entire filesystem or materialize every directory block per lookup | Would turn indexed lookup into work proportional to the full directory/cache; not selected |
| Reimplement an independent HTree parser | Duplicates collision, checksum, fallback and corruption semantics; rejected |
| Separate read algorithms, then give their I/O an independent owner with protected publication | Selected; retain one parser and the existing mutation/journal owner |

The read algorithm receives only `DirectoryBlockRead`: a paired inode identity
and snapshot, filesystem geometry, mapping queries, and immutable block images.
It cannot reach through the mount into allocation, journal mutation or namespace
publication. The initial `MountedDirectoryRead` implements this capability using
the current exclusive mount borrow; it is the compatibility adapter, **not the
finished optimization**.

HTree root/index/leaf verification, collision continuation, indexed readdir and
insertion's read-only probe use this same capability. The ordinary parent scan
also uses it, retaining its physical ordering, duplicate-block removal, legacy
mapping path and current error translation. Existing malformed-record behavior
is not silently tightened in a refactor; any separately identified behavior bug
needs deterministic RED evidence before a fix.

Immutable `Arc<Vec<u8>>` block owners reuse `CachedBlock::data`. This removes the
extra full-block clone in HTree's old `read_block_data`, without borrowing a
reusable device buffer or allowing cache mutation to change an acquired image.
No new public API, dependency, unsafe code, on-disk format, cache size or default
mount policy is introduced by this capability extraction.

## Required ownership and publication protocol

The following are mandatory implementation and validation requirements:

1. Mount admission spans preparation, independent I/O and completion. Shutdown
   cannot publish a clean unmount while one of these reads remains in flight.
2. Parent inode lifetime is retained throughout lookup. Directory extent/index
   blocks cannot be reclaimed or reused while their physical addresses are
   consumed. A copied block number or change attribute alone is not a pin.
3. Cold parent **and child inode-table loads** leave global exclusion. The child
   must be protected against unlink/reap/reuse before its metadata is loaded.
4. Cache/journal images take precedence over home blocks, including uncommitted
   metadata. A detached endpoint is not permission to read stale home bytes.
5. Namespace changes cannot make a stale found/missing result authoritative.
   Protection must cover rename/exchange, unlink/rmdir, aliases and failed
   transactions; completion must validate before exposing parsing or I/O results.
6. Validated lookup and live-reference acquisition share one publication
   boundary. Constructing and dropping VFS wrappers must not recursively acquire
   the global mount lock; errors and abandoned reads release all owned state.
7. Preserve HTree's on-demand lookup complexity. Do not walk the whole extent
   tree or clone every journal/cache entry just to release the lock.
8. Keep the mutation owner serialized where required by rsext4/JBD2. Any new
   per-directory exclusion must define order for multiple directories and must
   not wait behind inode locks while holding global state.

The independent mapping reader now uses the same extent and legacy traversal
through `MetadataBlockRead` and immutable `BlockMapContext`. Namespace coordination uses a
sleepable shared topology gate followed by a shared per-directory gate for
lookup. Ordinary create/link/non-directory unlink take a shared topology gate
and an exclusive parent gate. Rename and rmdir take exclusive topology access:
they can change a moved directory's `..` or detach a directory discovered during
the core operation. This conservative boundary avoids performing new preflight
lookups that reorder read-only/mount-failure/name/error checks. It does not copy
Linux's full ancestor-ordered rename locking and does not promise concurrent
rename versus unrelated lookup. Ordinary inode/data I/O takes neither gate.

The order is topology gate -> directory gate -> mount state. Gate state is
protected briefly by IRQ-aware locks; waiting and wake callbacks happen outside
those locks and outside mount state. Waiting writers exclude newly arriving
readers, and cancelling a waiter restores admission before waking others.
Directory aliases resolve to one weakly registered gate per live inode. The
parent lifetime and these guards remain until the directory result and child
reference are published together; child inode metadata I/O then needs only its
allocation reference. Reap cannot free a directory with a live lookup owner.

All eight requirements remain final review and runtime release conditions.
The serialized lookup compatibility path is selected only when the device
explicitly reports unsupported fork capability, not on I/O or allocation errors.

## Implemented ownership boundaries

### Inode demand loads

The adapter now carries a non-cloneable `InodeLifetime` from authoritative
lookup/create through VFS publication and final release. Its destructor retains
the existing orphan/reap protocol and must run outside mount exclusion.

Cold live-inode inspection uses a per-inode pending-load version, not a
filesystem-wide sequence. Mutating, reinitializing, explicitly evicting or
rolling back that inode invalidates its pending read. Updates to unrelated
inodes do not invalidate the read. Pending registrations are weak and retired
when no request remains, rather than an unbounded inode-version history.

Preparation resolves immutable table geometry and captures journal-visible
bytes before considering a coherent independent home read. Completion checks
mount/cache identity and the pending version before decoding or exposing an
I/O failure. If a current canonical record is available, it wins. Otherwise a
valid completion may enter the canonical cache by evicting a **clean** record;
when all entries are dirty, return the validated metadata without caching it.
A read completion must never trigger dirty-eviction I/O under the mount lock.

This live-inode interface requires the caller's existing allocation reference;
it is not an unchecked replacement for inspecting arbitrary inode numbers.
The child lookup path must acquire that reference before independent inspection.
The directory owner can consume the validated raw parent even when the clean
cache refuses admission, without retrying a successful read indefinitely.

### Directory traversal

`InodeLifetime::lookup` retains mount admission and parent namespace exclusion.
`lookup_admitted_child` drives `DirectoryLookupPreparation`: a cold parent read,
then the independent lookup, then validated result publication and child pinning
in one mount critical section. It releases namespace exclusion before loading
child metadata, so unlink can remove a name while the child's allocation stays
alive. Final completion checks mount identity and parent version before exposing
either found/missing or an I/O/parser error; superseded attempts restart.

Each mapping or directory block read asks `DirectoryReadCache` for an immutable
visible image through a short mount critical section. Directory entry blocks
honor the data cache, then journal images; mapping blocks honor the journal but
are not inserted into the data cache because extent/indirect mutation has a
different owner. Misses read the coherent independent endpoint outside the mount
lock. Clean residency remains the endpoint's policy rather than adding a second
metadata cache. HTree keeps its selected-path traversal and existing checksum,
collision and fallback parsers. Warm parent reads retain the original LRU touch.

Directory enumeration and namespace mutations remain mounted operations. This
change removes cold `lookup` I/O from global exclusion; it does not claim that
all ext4 I/O or the similarly large write hotspot has been parallelized.

## Validation and release gates (complete refactor only)

The connected production refactor has completed the three final static rounds
and host behavior tests; its full 181-unit run passes at 1252 s versus the
preceding 1323 s. This is one matched measurement, not stable repeatability. See the
[validation record](../profiling/directory-read-ownership.md) for exact commands,
source and kernel hashes, failures, fixture corrections and observed results.
The required sequence is:

1. Review source and every caller for identity, cache visibility, locking,
   rollback, errors, shutdown and publication; map affected syscall helpers.
2. Run relevant strict Clippy and build-only feature/target combinations,
   including actual 8-CPU AArch64 Starry profiling configuration.
3. Recheck final diff/format, test discovery, source snapshot and matching
   ELF/BIN configuration. Any production correction requires fresh final gates.

Only then run deterministic behavior tests. Required observations include real
inode-table and directory device reads with the mount lock available; another
directory/inode progressing during that read; exact-name/negative results;
checksum/I/O failures; HTree collisions/fallback; hard links; rename/unlink/reap
interleavings; dirty metadata and detached commits; shutdown admission; and no
lost/reused inode reference. Preserve existing semantic assertions; if an old
assertion encodes the intentionally replaced locking protocol, explicitly record
the changed expectation and pair it with the new lock-external I/O checks.
New syscall-semantic defects additionally require the project Starry case and
the same Linux input, not only a host mock. No complete syscall ABI review is
claimed by this design document.

Finally run the entire fixed 181-unit workload on QEMU 8c8g, with the reusable
rootfs tg-xtask, no deadline, direct kernel profiling, matching symbols, artifact
hashes and clean offline fsck. Compare against 1323 seconds and Linux's matched
735-second run, preserving all prior baselines. Repeat positive measurements
before declaring stable speedup. No disk image is changed by this source phase.

## Development evidence, 2026-09-12

Before editing, the affected core/adapter/VFS source trees were archived in
`tmp/directory-read-owner.dvQgri/sources-before.tar`, SHA-256
`f434e34e4ae7a945e055c8f810258c0b9d924710a32346b059bb65e8122e368a`.
This archive is a dirty-worktree snapshot, not a clean commit.

First `cargo xtask clippy --package rsext4`: base and multilevel-cache checks
passed; the host-test compile failed with two E0277 and two E0061 diagnostics.
Two old tests still supplied the five-argument `fallback_to_linear_search` API.
All raw diagnostics were displayed. Only their reader construction/call sites
were migrated; their missing-entry and corrupted-extent assertions were retained.
After formatting, the same command passed all 3 checks at 16:10:14 +0800,
exit 0; full output is `tmp/directory-read-owner.dvQgri/capability-clippy.log`.
At that development checkpoint no runtime test had started. Subsequent final
static and host runtime evidence is recorded in the linked validation document;
it does not yet establish a performance gain.
