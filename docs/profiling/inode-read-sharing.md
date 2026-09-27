# Inode read sharing: execution record

Baseline: verified1222s full run,181 units175 crate names; Linux735s. Largest
remaining mutex leaf is inode read_at121319296256ns/38.0700%. See
[design](../design/inode-read-sharing.md) and
[complete baseline](file-write-ownership.md). No new performance result yet.

The connected implementation reuses namespace's old shared-access state machine
at private fs/access and replaces the inode identity registry's exclusive mutex
with shared-reader/exclusive-writer admission. All content mutators use write
access; reads retain read access across owned mapping/data/completion. Existing
core/cache protocols and unrelated historical unwired files are unchanged.

## Static round 1: ownership and evidence

Inspected after all connected source/test edits:

- Registry lookup only obtains an Arc under registry/mount locks, never waits
  for content access. Every InodeLifetime keeps the same gate alive. Read and
  write wait outside mount/IRQ state; normal release wakes after dropping state.
- Fast acquisitions honor queued writers. Checked reader/writer counts reject
  overflow without mutation; waiting-writer Drop reverses registration on any
  error. No read-to-write upgrade or recursive acquisition contract was added.
- Read preparation snapshots mappings and private/journal bytes. Multiple
  readers cannot mutate data mapping; their atime completion uses current inode
  metadata under mount state. EOF/legacy/oversized/unsupported fork fallback,
  errors, output publication and core write-owner ledger remain unchanged.
- Namespace mutation changes links/parents, while retained inode lifetimes
  prevent data block reclamation. Current active directory implementation is
  inode/directory, not historical unwired inode/dir.rs. No new victim locking
  or old rename behavior was copied from the latter.
- Existing PendingFills owns overlap coalescing, content invalidation and
  generation-checked page publication. This change does not replace that with
  another cache ownership mechanism.
- Journal progress takes only commit/state admission, not inode-content gates.
  periodic_writeback flushes pages before taking the commit gate. Read atime
  progress can therefore run without waiting on its own content-write owner.
- Contended read/write access retains a MutexWait ProfileScope over retries
  and on failure. Namespace gates now also report previously unreported waits
  through that same category; future comparisons must distinguish their stacks
  instead of claiming the entire category has identical instrumentation.
- No public API, disk format, unsafe implementation, feature-boundary widening
  or new dependency. Typed task-wait failures remain explicit. This is a scoped
  concurrency review, not full Linux syscall compatibility or merge approval.

New coverage is three additional gate cases (overflow, all-reader drain with
spurious wake, actual TaskWaiters wake), two same-inode physical I/O cases, and
one contended acquisition profiling case. Existing real write-I/O and mutator
probes now also assert reader exclusion. Existing lifetime test asserts shared
gate identity. These tests have not run yet.

Before archive SHA57a7c52140589ce6075e4c4054d8d9b20f859a3376da7f1b4f58ea0adcecdcab
under tmp/inode-read-sharing.FNhQ6c. Its source comparison deliberately exits1:
the reported changed files are the connected adapter consumers/tests; the old
namespace/gate.rs and tests/read.rs are absent because they moved to access/
and tests/read/. Full raw output was displayed. New-file inventory was separately
checked; archive comparison alone cannot detect additions. Core/cache/OS adapter
bytes in that snapshot have no reported differences.

## Failures and limitations

Source lookup of inode/extents.rs exited1 with the original diagnostic:

```text
cat: fs/ax-fs-ng/src/fs/ext4/rsext4/inode/extents.rs: No such file or directory
```

The active extent adapter is a method in inode/mod.rs. A combined patch failed
validation on the old write-probe message: expected "data write lost inode
exclusion", actual "data write lost inode content exclusion". No partial patch
applied; verified paths/content and re-applied. The full error was displayed.

No runtime or QEMU is permitted until rounds2 and3 pass on the final sources.

## Static rounds 2 and 3 passed before runtime

2026-09-12, all after the connected change:

- Strict combined Clippy with lib+tests, features host-test,ext4,vfs,profile:0.
- cargo xtask clippy --package ax-fs-ng:8/8,21:31:37–21:31:43 +0800,exit0.
- Actual fixed Starry AArch64 kernel build starts21:32:12 +0800,exit0,
  release12.32s,8CPUs,NVMe,guest-profile,frame pointers;14222kallsyms and
  post-kallsyms BIN refresh. Existing core/memchr future warning remains visible.
- cargo fmt --package ax-fs-ng --check, git diff --check and source archive
  comparison:all0. The core/cache/OS adapter code was not changed this round.
- Independently converted the saved ELF with rust-objcopy and compared BIN:0.

Frozen under tmp/inode-read-sharing.FNhQ6c:

| Artifact | SHA-256 |
| --- | --- |
| sources-connected.tar | b987319db270a6b47bf8014e0b34f66bbe0d417511bff95ccd8c4b216465444c |
| starryos.elf | 16fae63c2879a26a8559d4fe08062d97d6676c84346e304f71900e649fa59b46 |
| starryos.bin | 5d22aaadab6e3819777e75f6aa1b42e3f4e78ccb9193a20524fbe294d5e121b6 |

Full combined/matrix/build logs saved beside artifacts. No new compile failure
or warning suppression. Three static gates now permit runtime; a source
correction requires renewed applicable gates before another runtime attempt.

## Host runtime passed

After all three gates, the full combined-feature ax-fs-ng library suite passes
273/273,exit0,0ignored/filtered/failed,0.70s runtime. Exactly six additional
cases were discovered and passed, including real waiter wake and physical
same-inode read overlap. No production/test edits or assertion relaxation were
needed after runtime began. Full command/output saved as adapter-full.log.
Core409/VFS49 results remain those from the preceding write-owner phase; those
unchanged crates were not rerun here. This is not a measured speedup.

Next run directory: target/profiling/arceos-helloworld/starry/inode-read-sharing.
Fresh frozen-base copy, complete hash, script-only injection, readonly fsck,
dump comparison and prepared-disk full hash all finished before QEMU started.

## Complete QEMU result and next hotspot

Full run completed1218s,181 units175 distinct crate names,command0,QEMU0.
Cargo reports20m00s; axbuild1211.26s; kernel prebuild1218298080544ns.
These are different timing boundaries, not conflicting measurements. Reused
tg-xtask/source,8CPUs/8GiB,NVMe,10Hz kernel profiling, no timeout/debugger or
host builds/source edits during measurement. Six guest manifest hashes pass;
sorted complete name/version multiset equals previous Starry and Linux.
Final ELF is byte-identical to frozen Linux:
2792c35275d91f57a38a847be87c4d2b333d7b88cd548bf9e06633288c7dba21.

Post-exit readonly fsck0; no repair/optimization accepted. Individual debugfs
dumps avoid the earlier rdump ownership warning. Ended rootfs is readonly in
the run directory, SHA191af79811bbd10409b7df74a4e62212b91ad152bb7619226c928dc9ec09c51e.
9593CPU records+16104wait records=25697, all rendered; dropped/skipped0.
11SVG and three actually viewed PNG previews retained beside summary.json and
comparison.json. All49 IllegalInstruction PC counts exactly match the earlier
known Cargo/OpenSSL probes. The unchanged core/memchr future warning was shown.

| Metric | Previous1222s | Shared readers1218s |
| --- | ---: | ---: |
| Active CPU samples | 39990/94892 (42.1426%) | 40022/94554 (42.3271%) |
| Inode content read wait ns, all matching stacks | 121319296256 | 668269264 |
| Total reported mutex wait ns | 318674503120 | 272626758240 |
| Ext4 mount wait ns | 94474594592 | 110156612272 |
| Ext4 mount hold ns | 74929037136 | 86676276128 |
| Page-cache inclusive ns | 826484569088 | 770822259712 |
| Block read inclusive ns | 479927038352 | 665519813648 |
| Block write inclusive ns | 105527276176 | 146172060416 |
| Block flush inclusive ns | 267879256400 | 203089027152 |

Inode read exclusion wait fell99.4492%, but wall time only4s/0.3273% in one
matched run. CPUactive rose0.1845percentage points. This does not establish
significant/repeatable speedup and does not resolve low CPU utilization.
Linux remains735s (~70.8084% active sampling), ratio1.6571. Overlapping and
sample-scaled waits cannot be added as wall savings. Newly reported namespace
gate waits total30699611008ns; do not compare total MutexWait as unchanged
instrumentation or misclassify those gates as inode content readers.

Current largest mutex leaf is mount state lock109409064896ns/40.1314%.
Cache pin_read_page63013796656ns/23.1136% follows. Largest mount holder is
read_inode32359739008ns/37.3340%, including29665245312ns in pin_read_page ->
populate_page_window -> read_at. The full synchronous-block-read graph shows
extent child-node parsing23997756768ns; source read_plan preparation still
walks cold extent metadata while holding mounted exclusion. Continue by
reusing the independent directory mapping/visibility protocol for complete
file mapping/data ownership, checking Linux's corresponding read locking.

The separate largest resolved CPU leaf remains find_free_area1968samples/
4.9173%active. Reading the pinned Rust BTreeMap Range implementation disproves
the hypothesis that its last() scans forward: it calls next_back(). No memory
source was changed. A larger free-range redesign needs its own evidence and
must not be conflated with removing extent I/O under mount state.

Additional diagnostic path mistakes, both original output shown and corrected:
Linux progress uses artifacts/linux-profile-artifacts, not the Starry folder
(cat exit1, No such file or directory); Starry source is os/StarryOS, not
os/starry (rg exit2, No such file or directory). Large display requests were
truncated by tool output limits, not corrupted on-disk data; compact JSON and
whole-file aggregation are used for statistics. No benchmark rerun or source
fix was needed after these read-only diagnostic mistakes.
