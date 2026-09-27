# Cache writeback ownership: implementation and validation

## Measured input and current status

Baseline is the fully verified 1217-second, 181-unit / 175-name
`namespace-journal-progress` run, not a truncated flame graph. The two cache
fault entries wait 77.725440256 seconds specifically on `io_lock`; all raw PCs
were checked against the saved ELF. This is cumulative wait, not a predicted
wall-time saving. See [design](../design/cache-writeback-ownership.md) and
[previous full-run evidence](namespace-journal-progress.md).

Artifacts: `tmp/cache-writeback-ownership.m2tahG/`.
Candidate source archive SHA-256:
`00ad2b3372d8eaac5998928f4836522dbf2ebb79cb80e32a5e5182b887485827`.
The original-production RED test failed as expected; unchanged candidate GREEN,
all 302 adapter tests and all 55 8-core/8-GiB QEMU kernel tests passed. The fresh
full compilation completed in 1253 seconds with exit 0. This is 36 seconds
(2.958%) slower than the immediate 1217-second baseline: end-to-end performance
acceptance has NOT passed, despite a substantial reduction in the targeted wait.

## Connected change

- A finite writeback round pins selected physical pages and retains the
  existing writeback/truncate owner. Each page is Idle, Active or Redirtied.
- Protection callbacks, backing writes and backing sync do not retain cached
  I/O, index, listener or published-page byte locks.
- Contiguous owned batches copy at most 1 MiB once. Completion checks physical
  identity, dirty generation and absence of redirty. Errors preserve dirty
  suffixes and always release reservations; explicit empty sync still commits.
- Eviction and clean reclaim skip reserved pages, including completed clean
  batches while later I/O is still active. This prevents old writes from
  overwriting newer dirty eviction output.
- Stable fault pin/publication uses the existing index + updating contract.
  Final publication never waits for I/O ownership while holding AddrSpace;
  tentative updates and stale identity/EOF request a fresh fault transaction.

## Static ownership audit (round 1)

Read all changed cache paths, their fill/update/resize/retirement callers, the
real backing short-write loop, COW fault commit and private cache listener,
shared mapping listener and range writeback. No new unsafe block, public trait,
dependency, on-disk format, syscall flag or workload-specific branch is added.

| Boundary | Checked invariant |
| --- | --- |
| Selection | I/O owner excludes dirty membership changes between counting and reservation. All fallible result/selection allocation precedes callbacks and writes. |
| Snapshot | Bounded byte/version capacity is allocated before I/O/index locks. Physical pin outlives index borrowing; page byte guard ends before backing I/O. |
| Redirty | Dirtying during protection remains Redirtied even if snapshot sees the new generation. Dirtying after snapshot cannot be acknowledged by old completion. |
| Partial failure | Only completely written batches are cleaned. Zero/overlong writes propagate Io; no failure clears an unwritten suffix. |
| Eviction/reclaim | Reserved pages cannot be persisted or freed by a competing victim path. Missing-slot readers drop index before waiting for I/O. |
| Resize | Existing mutation -> writeback -> I/O ordering excludes concurrent old writeback through truncate/rollback and delayed mapping retirement. |
| Fault publication | Index and Acquire observation of updating exclude tentative bytes. Content changes take index after publishing updating; detached pages cannot accept stale publication. |
| Callback/Drop | Protection and sync hold only writeback ownership. Round Drop takes I/O then index only after all batch/callback guards end. |
| Kernel lifetime | PTE publication retains physical pins through existing invalidation; private mappings never grant file WRITE. Listener invalidation does not wait for AddrSpace under cached I/O. |

The unchanged syscall surfaces include buffered I/O, mmap faults, msync,
fsync/fdatasync, truncate and cache reclaim. Private backing callbacks are the
deterministic injection layer; no user fault-injection ABI is added. This is
an author-side concurrency audit, not a complete syscall compatibility review.
Existing kernel cache/PTE regressions must run against the final source.

## Regression protocol

Fifteen new tests exercise actual CachedFile and NodeOps paths, covering all
writeback entries, empty sync, buffered/mmap redirty before/after snapshot,
failed protection/write/sync, bounded/disjoint/partial-EOF batches, failed second
batch, active clean reclaim, dirty eviction and stable/tentative publication.
Existing short-write, zero-progress, truncate rollback and physical identity
tests remain unchanged.

For RED, restore the original page, mapping, populate, reclaim and writeback
production implementations from `sources-before.tar`; the old writeback module
is placed at its new `writeback/mod.rs` path without changing its contents.
Only a test-only accessor of the original writeback boolean is added so all
new test bodies compile. Candidate batch child files are not declared by RED.
Run only `every_writeback_entry_releases_io_before_real_backing_write`; its
first assertion fails before any nested cache call, with no timing dependency.
Do not execute the other new tests on RED or boot a RED kernel.

Round 1 for RED checks the restored scopes against the saved original and the
first assertion against the actual NodeOps callback. Round 2 is strict combined
lib/tests Clippy. Round 3 is fmt, diff and archived source identity. Restore the
byte-identical candidate afterwards, repeat all three static rounds, then run
unchanged GREEN and the complete adapter/kernel regressions.

## Development failures retained

1. Combined Clippy exited 101: E0282/E0283 on a closure's ambiguous Result error
   type before transpose. Fixed with explicit `VfsResult<Vec<u32>>` return type.
2. Retry exited 101: two test-only callback fields triggered type_complexity.
   Fixed by the private `BeforeBackingIo` alias, without any allow attribute.

Both full command/output logs are saved and original diagnostics were printed.
The next strict combined Clippy exited 0; this is compilation, not test execution.

## Original-production RED and exact restoration

All three RED static rounds passed before execution; see `red-static.log`.
RED archive SHA-256:
`2e29591bc793246ed41aecae91aaed388203b5236286c53771198a5f057f0bcf`.
The unchanged named regression exited 101 in 0.00 seconds, 0 passed / 1 failed,
with `writeback retained cached io_lock across backing I/O`. Full original
output is in `red-test.log`. It failed at the first callback lock assertion,
before trying any cache operation that would wait on that old lock.

Restoration used the saved candidate bytes, not a newly adjusted implementation.
All 58 archived regular files were compared byte for byte and matched.
`tar -df` returned 1 only because the five restored files have newer mtimes;
the subsequent full content comparison exited 0. A prior read-only query of a
nonexistent build-directory pattern exited 2; original output and confirmed
cause are retained in `path-query-error.log`. Neither condition changed code.

Final round 1: rechecked the restored candidate against the ownership table and
the regression's real callback path. No RED-only production or accessor remains.
Final round 2: strict combined lib/tests Clippy exited 0; project feature matrix
passed 8/8; actual AArch64 profile build exited 0 with 14232 kallsyms.
Final round 3: fmt and diff checks exited 0, all 58 candidate files matched byte
for byte, and an independently derived ELF-to-BIN conversion matched the saved
binary. `final-static.log` records all three rounds before candidate runtime.

## Candidate runtime validation

The unchanged named regression passed in 0.00 seconds. Full adapter library
tests passed 302/302 in 0.88 seconds, no ignored or filtered cases. Its initial
tool output was truncated in the middle; the saved log explicitly notes that
limitation and includes the complete terminal totals, rather than claiming a
complete per-test transcript.

`cargo xtask ktest qemu -p starry-kernel --test axtest_kernel --arch aarch64`
passed 55/55, no skip, exit 0. The saved full `kernel-tests.log` confirms 8 cores,
8 GiB, NVMe and snapshot disk mode. Real cache ownership/PTE, shared writeback,
truncate invalidation and file EOF tests ran. Allocation/I/O/shootdown failure
messages are deliberate existing regression injections, all corresponding
cases passed. This is correctness validation, not the measured compilation.

Saved profile ELF SHA-256:
`4876be49ff3026887fc789881ea130bb97318bdc80a15181dfa2a634541dd54b`.
Saved profile BIN SHA-256:
`2dc26f87fb567e2ceed1f99d71df80b6ab0c13c9f8d081a60a761e5e21d0a6d8`.
The kernel regression used its separate test ELF, not these profile artifacts.

## Space preparation

Freed approximately 5.5 GiB by removing only the old duplicate
`rootfs-profile-bitmap-writeback-window.img`, after comparing all 16 GiB against
its existing zstd archive and checking neither file was in use. Read-only `.zst`
backup remains, with raw/archive hashes and full comparison command/output in
`cleanup.log`; Python 3.14's zstd reader performs this without shell pipelines
or another decompressed temporary image. Available space rose to 9.6 GiB before
copying a fresh full-build disk. No base or full-run result image was deleted.

## Full profile completed and verified

Fresh disk matched the frozen base before replacing only the runner. Pre/post
readonly fsck exited 0; guest runner bytes and reusable tg-xtask hash matched.
Prepared disk SHA-256:
`a047e66c42778a293062996e1b7af9af9c694f4bccec06aa8752d7aa7a899799`.
QEMU booted at 2026-09-12 18:44:55 UTC, PID 1294228, with the saved profile
kernel and 8 cores / 8 GiB / NVMe. Runner invoked once after observing prompt.
Run directory: `target/profiling/arceos-helloworld/starry/cache-writeback-ownership`.
No timeout; no host build, source mutation or live-image inspection during the
measurement. The runner finished naturally, build_completed=true, command_rc=0;
QEMU exited 0. PID 1294228 and terminal 49059 are no longer live. Runner start
was observed between 18:45:10 and 18:45:59 UTC; an exact timestamp was not recorded.

Post-exit readonly fsck returned 0. Eight guest files were individually exported
with debugfs dump, without ownership errors. All six guest manifest hashes
passed. The 181 (name, version) compilation multiset, with 175 distinct names,
exactly matches the preceding Starry run, the earlier inode-read-sharing run,
and the matched Linux run. All 49 known startup instruction probes also match.
All 58 archived candidate source files remained byte-identical after execution.

Output ELF SHA-256:
`2792c35275d91f57a38a847be87c4d2b333d7b88cd548bf9e06633288c7dba21`.
Explicit byte comparisons with previous Starry and Linux both returned 0.
Ended image is now `rootfs.img` in the run directory, mode 0444, SHA-256
`524b980721c5bd0312649625fca4bea1f5764ec5aa7828b7f310a7c35f5316c2`.
The former working-image path is absent; no live image remains.

The complete raw profile contains 9696 CPU rows and 17215 wait rows (26911 total),
one phase definition and nine event definitions. All records are accounted for,
phase 1 only, all eight CPUs present, with every dropped/skipped counter zero.
All summary/folded totals were independently checked against every raw record.
Eleven SVGs were rendered; mutex wait, mount hold, active CPU and block read were
actually viewed. `integrity.json`, `integrity.log`, `comparison.log`, fsck,
export, checksum and identity logs are saved alongside the inputs.

The first PNG-preview command exited 1 because it requested `mutex_wait.svg`
instead of the renderer's `mutex-wait.svg`. The complete original Python
traceback was printed. File discovery established the correct names; the
corrected four-image conversion exited 0. No raw data or SVG was changed.

## Full-run outcome and remaining hot spots

| Metric | Previous namespace ownership | Cache writeback ownership |
| --- | ---: | ---: |
| Full build | 1217 s | 1253 s |
| CPU active samples / all samples | 43.418% | 41.037% |
| All mutex wait | 217.349 s | 157.914 s |
| pin_read_page mutex leaf | 50.637 s | 12.445 s |
| with_current_read_page mutex leaf | 28.441 s | 7.946 s |
| Ext4Guard::acquire mutex leaf | 65.573 s | 72.349 s |
| Dedicated ext4 mount wait | 66.528 s | 72.922 s |
| Mount hold, all owners | 54.692 s | 57.443 s |
| Mount hold, create owner | 15.235 s | 17.889 s |
| Block read cumulative | 497.819 s | 689.171 s |
| Block write cumulative | 83.041 s | 95.936 s |
| Block flush cumulative | 239.779 s | 124.996 s |

The two targeted fault-entry leaves fell from 79.079 to 20.391 seconds (74.21%).
The old exact I/O-lock subset was 77.725 seconds; the candidate methods no longer
take that I/O lock on stable resident hits. Do not label the remaining index
contention as the old I/O wait. The largest individual mutex leaf is now mount
state acquisition, 45.82% of mutex wait. Create owns 31.14% of mount hold.

The largest resolved kernel CPU leaf is still MemorySet::find_free_area:
1979 samples, 4.954% of active samples. User-space leaves account for 57.290%.
PreparedFileRead contributes 450.827 seconds, 65.42% of block-read cumulative
time. These are different kinds of measurements and may overlap across tasks;
they cannot be added to infer wall-clock savings or prove why the run slowed.
No host-device queue/critical-path evidence was collected that isolates the
36-second regression. Linux's matched full build remains 735 seconds.

The ownership regression is closed by deterministic RED/GREEN and production
kernel tests; a build-speed improvement is not established. Keep this candidate
and its baseline recoverable while addressing the remaining measured bottleneck.
Do not merge or present this candidate as an accepted throughput improvement.
