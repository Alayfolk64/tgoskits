# Private file-cache backing: complete-run evidence

The completed 8c8g QEMU run takes **1323 s**, versus the preceding **1684 s**:
361 s less, or **21.437%** in this single matched full-build comparison.
All 181 compile units/175 distinct names complete, the command and QEMU exit
0, and the final binary matches Linux byte-for-byte. This is a positive
single-run result, not a repeated statistical speedup. Full validation,
remaining bottlenecks and limitations are recorded below.

## Baseline and mechanism

The frozen complete `concurrent-cache-fill` run is 1684 s / 181 units.
Its largest resolved kernel CPU leaf is memcpy, 2380/54750 active samples;
1832 samples include private missing-page preparation. This does not mean
memcpy is the largest aggregate wait, nor that ext4 is no longer costly.
This was the pre-refactor baseline. See the
[complete baseline](concurrent-cache-fill.md) and
[ownership design](../design/private-file-cache-backing.md).

The candidate separates a cache entry's dirty state from its retained
physical allocation. Full aligned private read/execute faults can publish
the cache frame read-only after canonical identity/EOF revalidation. First
private writes, including forced kernel copies, allocate and copy using a
retained, byte-serialized source; even one cache mapping cannot take the
exclusive-private-frame permission shortcut. Unmap/fork rollback retire
counted references through their actual allocator/provider ownership.

Cache eviction/truncation notifications bind to one live address space.
Splits retain immutable file coordinates; fork and relocation create
separate notification owners. Each callback computes one virtual address
and checks its current VMA identity and exact physical page. It does not
scan the complete address space. Eviction tries the aspace nonblockingly;
truncate invokes blocking callbacks only after releasing cached I/O.
Completed mapping retirement precedes physical-reference release.

Partial EOF/ELF pages, unaligned prefixes, direct backing, initial write
faults, and unpublished locked-loader population retain owned copies.
Anonymous zero backing and existing retirement barriers are unchanged.
Dirty eviction snapshots a pinned victim before storage I/O so no published
page-byte guard waits for the device. Source-copy destinations use
`BorrowedCursor<u8>` over uninitialized private storage without claiming
those bytes initialized before the copy.

## Static and runtime status

The three final static rounds completed on 2026-09-12 before any runtime
test of this refactor. Logs are under `tmp/private-cache.Lz1bM2/`.

1. Ownership/caller review covers ordinary and forced faults, counted
   cache/private source copies, fork and failed clone prefixes, VMA
   split/relocation, cache fill/update/eviction, truncate retirement and
   writeback. Publication keeps aspace -> cache I/O -> index ordering;
   initial I/O and source copying retain no aspace/index/IRQ reference
   locks. Invalidation checks exact current backing and completes the
   removed prefix before retiring owners. Cache byte guards do not cross
   device I/O or callbacks; unpublished scratch buffers remain separate.
2. Compiler/lint: filesystem final matrix 8/8, host lib/test strict clippy
   with `host-test,ext4,vfs,profile`, kernel 110/110 configuration matrix,
   actual `cfg(axtest)` AArch64 strict clippy and build-only all exit 0.
   After final Debug/measurement-scope additions, filesystem matrix,
   host/profile combination and actual axtest strict clippy pass again.
   The actual profiling consumer builds through `cargo xtask starry build`
   with the unchanged app TOML, SMP=8 and frame pointers, exit 0.
   `ktest` has no build-only CLI, so its inspected target/features/linker
   and `cfg(axtest)` contract was matched with native Cargo for this gate.
   C11 syntax, host CMake build and AArch64 musl static build pass strict
   `-Wall -Wextra -Werror`; none of these commands executes a test.
3. Final artifact/configuration review: fmt check, `git diff --check`,
   source tar comparison and reconstructed-BIN byte comparison all exit 0.
   The saved final ELF is AArch64 ET_DYN without PT_TLS or kernel TLS
   sections. New cache-pin/publication/invalidation symbols are present;
   `flush_batch` retains `dsb ishst`, inner-shareable TLBI, `dsb ish`, ISB.
   The inherited RWE LOAD is not claimed as hardware read-only protection.
   The axtest ELF contains 55 descriptors including the new cache test.
   Explicit subcase discovery selects the system wrapper; its CMake glob
   and selector resolve the new installed binary. Kernel/system build and
   runtime configs retain 8 CPUs and 8 GiB; no board is involved.

The new cache-pin preparation path also records the existing PageCache
timing scope. Moving work out of `CachedRead` must not make unobserved
latency look like a measured gain. CPU sampling remains kernel-wide.

Saved candidate: `target/profiling/arceos-helloworld/starry/private-file-cache-backing/`.

| Artifact | SHA-256 |
| --- | --- |
| ELF | `39a1aab876665df7e7c56f4128d39ee011a8b3ec4fb39e4e5d8a18210f1d2d08` |
| BIN | `57ba4ecd41879fe6ef34365cf8b9acf52715505eb528d17253117bcfe8fd5000` |
| Selected source archive | `b00d42d5f55f94391ee478e83f082cd5748298c3c17c898510c83c6cd3f286c4` |
| Static AArch64 C case | `5e8aae433b526114e211d7e16aa739cb6b5f9bb5ea28494bff70381b53cd3ddc` |

The archive contains the changed filesystem/kernel/MM boundaries and
selected frozen build/test inputs, not a clean whole-workspace commit.

Filesystem tests add physical pin lifetime/exact release, serialized byte
copy and wrong-size rejection, complete-page eligibility, stale EOF and
replacement identity checks. Kernel axtest adds ten scenario groups over
real PTEs: shared read physical identity/no-private-allocation, independent
writes, exclusive/forked first writes and forced read-only writes, retained
source across truncate, clone conflict rollback, stale read publication,
eviction with a busy address space, EOF/ELF initialization, relocation,
and actual MAP_SHARED/private interoperability before and after COW.
The host library suite passes 246/246, exit 0, after the three gates.
The first QEMU kernel run fails at the new shared/private fixture's WRITE
assertion: the existing shared-disk backend first installs a clean read-only
PTE, then a subsequent write-protection fault marks it dirty and grants
WRITE. The fixture incorrectly assumed one first-write fault performs both
steps. It now explicitly read-faults then write-faults, retaining both
permission assertions, physical identity and private-isolation checks.
No shared-backend production behavior was changed to accommodate the test.

Before retry, caller/transition review, actual AArch64 axtest strict clippy
and build-only, fmt/diff and descriptor/source-snapshot checks pass again.
The test-only correction leaves the saved profiling ELF/BIN unchanged.
`sources.tar` preserves the initial harness; `sources-two-fault.tar` records
the correction, SHA-256
`090c20853e28bfa167f9615d3afb269404b996ce88f385297fd5fa76a852b060`.
The original panic and outer exit 1 are retained in `kernel-qemu.log` and
were displayed without suppressing failure. `kernel-qemu-two-fault.log`
is the retry, which exits 1 at the cache-fork rollback test's exact conflict
address assertion. The generic mapper's base-page and huge-page insertion
branches return the requested physical address in `existing_paddr` instead
of the occupied PTE address. The earlier anonymous fork rollback fixture
uses the same physical address for both and therefore cannot detect this
diagnostic error. The new fixture deliberately uses distinct backing pages.
Two generic host regressions check the exact error and preservation of
the original mapping. After source review, crate 2/2 strict clippy plus
strict test clippy, and final fmt/diff checks, both fail on the original
implementation: `existing_paddr=0x600000` instead of `0x400000`, exit 101.
`paging-conflict-red.log` records this deterministic RED run. Both mapper
branches now read the occupied descriptor (`paddr(true)` for a block,
`paddr(false)` for a base page); no mapping/rollback operation is changed.
The cache-fork assertion retains exact values and now prints the actual
returned error if it fails again. Post-fix review, crate 2/2 and retirement
strict clippy, actual cfg(axtest) strict clippy/build-only and profiling
consumer build all pass. Final fmt/diff/archive comparison, ELF/BIN byte
comparison, 55 test descriptors, no-TLS PIE and actual AArch64
`dsb ishst; tlbi ...is; dsb ish; isb` inspection complete the third gate.
Only then does the same retirement suite pass 12/12, including both RED
cases, exit 0 (`paging-conflict-green.log`). Kernel QEMU retry is
`kernel-qemu-paging-error.log`; it is not a performance run.
No passing kernel result or performance acceptance is claimed yet.

The additional strict clippy of the old `generic_contracts` integration
entry failed on ten pre-existing shared-mock dead-code/style diagnostics.
These were displayed in full; no suppression was added. The new regression
uses the existing minimal `retirement` fixture, which passes strict clippy,
without changing the old mock or its existing tests.

The regenerated candidate after the mapper diagnostic fix has ELF SHA-256
`499181d475e6d291ca4773c92be4c43bab8b351b92fece11fc376393a0263acf`
and BIN `fb3d7e4af0136a722c3848e1c363f58f462301a2481de74093bb73b0f1951966`.
Matching `sources-paging-conflict.tar` is
`5701ae4c37cb2e2dc169729329c064c5dde6334754f2e31eeb453f3f1f355995`.
The preceding unmeasured ELF/BIN remain under the validation directory as
`private-cache-before-paging-fix.elf/bin`; the previous table is historical.
This correction changes error payloads only, not successful mapping logic.

The system case is `qemu/test-private-cache-backing`, selected by the
existing grouped system discovery and CMake installation. It tests real
disk-backed private mappings, fork/mprotect, uname/pread destinations,
private msync/madvise, resident/missing mremap pages, truncate/regrow and
partial EOF. The same built binary is intended for Linux QEMU comparison.

## Occupied-page ownership corrections

The kernel retry `kernel-qemu-paging-error.log` exits 1 at the truncate
fixture: parent RSS is 1, expected 0. An inaccessible PROT_NONE cache page
is skipped because ordinary `query` requires hardware-present translation.
The page still owns its cache frame and must participate in retirement.
Before fixing this ownership boundary, a new fork regression is added and
passes three static rounds. `kernel-inaccessible-red.log` then exits 1:
the child's file RSS is 0, expected 1. Private clone skips the inaccessible
descriptor, and fork reconciliation also mistakes it for an absent page.
Both original panics and outer errors were displayed without suppression.

The saved 1684-second kernel was also booted with a stronger C fixture
(binary SHA-256 `38cde0e0ac640003ca0e800d3854d2a2d4ced5b622c68439112ef6829b0ef9be`).
It fails earlier at private-file MADV_DONTNEED: expected file byte 0x30,
actual private byte 0x69, command exit 1. This run did not reach its later
fork or mremap checks. The existing discard helper selects anonymous COW
only, despite the syscall's file-private reload contract. See
`tmp/private-cache.Lz1bM2/old-kernel-case/qemu.log`. QEMU later shuts down
normally; post-run read-only fsck exits 0.

The completed correction introduces one descriptor-ownership query in the
generic page table and uses it for cache invalidation, private fork and
rollback, RSS reconciliation, and relocation. Ordinary translation and
architecture present-bit definitions are unchanged. Private-file discard
now uses completed COW retirement while retaining its VMA and file origin;
MADV_FREE's existing anonymous-only validation is unchanged. Tests cover
inaccessible cache and private owners, failed clone prefixes, repeated
discard with an unaffected sibling, and relocation followed by truncate.
The kernel aggregate now has 13 scenario groups; the generic retirement
suite adds occupied 4 KiB/2 MiB geometry checks. The C case adds separate
inaccessible fork and single-VMA relocation checks and prints mismatched
bytes directly. Its final binary differs from the preceding old-kernel
RED binary and has not yet been run.

All three final static rounds pass for the correction. The source/caller
review covers the occupied-vs-access distinction and Linux move_ptes.
Generic crate 2/2 strict clippy, retirement strict clippy, actual cfg(axtest)
kernel strict clippy, full kernel 110/110 matrix, strict C syntax and
AArch64 static compilation, actual axtest/profile build-only all exit 0.
Final fmt/diff checks, tar comparison, 55 descriptors, no-TLS AArch64 PIE,
ELF/BIN comparison and actual publication/completion barriers pass. At
`ffffffff80386110` the saved profiling ELF has `dsb ishst`, followed by
`vmalle1is`/`vaae1is`, `dsb ish`, and ISB. No architecture translation
semantics were weakened. Only after these gates, generic retirement passes
13/13 (`paging-occupied-green.log`); kernel QEMU is now running in
`kernel-occupied-qemu.log`, with no result yet.

| Corrected candidate | SHA-256 |
| --- | --- |
| ELF | `9d2980c8cad07dc40ed283eb1c32bba82d3896dd9b1753e5771d7681ad339a43` |
| BIN | `1f7c6dae53becc0239d3fa0d30b1a1b15ac2f079a6ff480c28d781d2912472f0` |
| sources-occupied.tar | `f195424065cfff4c0581dcc77e696d493a5d2f143c3fb1778942cce7356d961d` |
| Static AArch64 C case | `3eb5ab664f7f6cdb9328ed043d79d08074163c39a7c0ec6f0772e307f0634917` |

Earlier ELF hashes describe preserved pre-correction candidates. The same
new C binary is injected into separate Starry/Linux regression disks;
dump/cmp and post-injection read-only fsck both pass. Linux's copy matched
the frozen base before injection. Neither injected disk is a formal cold
performance baseline. No performance acceptance or complete syscall
compatibility is claimed.

The corrected kernel's first runtime still exits 1 at `assert_missing`.
The helper now uses occupied-descriptor lookup, retains the assertion, and
uses `track_caller` with the full observed mapping. After test-only source
review, strict axtest clippy/build-only and final fmt/diff/descriptor/tar
checks, the diagnostic retry exits 1 at invalidation.rs:141, specifically
the eviction scenario. The existing cache policy grows its retention
target with file length, so filling 544 pages does not exceed a presumed
512-page capacity. This is an invalid new-fixture assumption, not evidence
that a production eviction callback ran and failed. The fixture now calls
the real memory-pressure reclaim entry, with the address space locked and
then unlocked, retaining PTE/RSS/refcount/physical-pin/data checks. No
production cache policy is reduced. The new diagnostic and both failures
remain in `kernel-occupied-qemu.log` and `kernel-occupied-diagnostic-qemu.log`.
The test-only correction passes caller/real-reclaim source review, strict
actual axtest clippy and build-only, then fmt/diff/55-descriptor/tar checks.
The retry `kernel-real-reclaim-qemu.log` passes 55/55, fail 0, skip 0,
QEMU execution unit exit 0. All 13 cache ownership scenario groups execute.
Injected allocation/I/O/timeout failures in other tests are expected checks,
not ignored suite failures. The profile ELF/BIN are unchanged by these
test-only edits; `sources-real-reclaim.tar` includes the final tests and
has SHA-256 `37834821ba6430ac9047c86f3fed8cc2d67fea85e1c3bb6e02433a97817e817f`.
The following completed validation supersedes the pending runtime states above.

## Final functional validation, 2026-09-12 01:52

The actual grouped system subcase passes through
`cargo xtask starry test qemu --arch aarch64 -c qemu/test-private-cache-backing`:
one selected/installed case, `PRIVATE_CACHE_BACKING_PASSED`, system 1/1,
grouped success and outer exit 0 (`system-case.log`). The injector's four
`rm: File not found by ext2_lookup while trying to resolve filename` messages
come from its existing remove-before-write operation on absent destinations;
the subsequent writes and actual guest execution succeed. This build replaces
`target/.../release/starryos` with the system-test kernel, not the saved
profiling kernel. Performance runs must use the saved ELF/BIN in the table.

The exact same static C binary `3eb5ab66...` also runs on all three saved
kernel comparisons, each using QEMU Cortex-A53/GICv3/MTTCG/NVMe, 8 CPUs/8 GiB:

| Kernel | Guest result | Log under `tmp/private-cache.Lz1bM2/` |
| --- | --- | --- |
| Previous 1684 s Starry kernel | `PRIVATE_CACHE_OLD_RC=1`, MADV_DONTNEED byte mismatch | `old-occupied-case/qemu.log` |
| Linux 6.18.35-0-virt | `PRIVATE_CACHE_BACKING_PASSED`, `PRIVATE_CACHE_LINUX_RC=0` | `linux-case/qemu.log` |
| Current saved profile ELF `9d2980c8...` | `PRIVATE_CACHE_BACKING_PASSED`, `PRIVATE_CACHE_CURRENT_RC=0` | `current-occupied-clean-case/qemu.log` |

The old kernel fails before the later fork/mremap checks:

```text
mapping mismatch: offset=0 expected=0x30 actual=0x69
PRIVATE_CACHE_BACKING_FAILED: file mapping bytes or private isolation (errno=0: No error information)
PRIVATE_CACHE_OLD_RC=1
```

Its subsequent shutdown does not complete. One mistakenly premature launch
of the current kernel is rejected by QEMU's existing disk write lock, exit 1:

```text
qemu-system-aarch64: -device nvme,drive=disk0,serial=tgoskits,max_ioqpairs=64,msix_qsize=65: Failed to get "write" lock
Is another process using the image [/home/wuxun/Projects/tgoskits/tmp/axbuild/rootfs/rootfs-aarch64-arceos-helloworld-profile.img]?
```

No second guest starts and no lock bypass or offline write is performed.
`old-occupied-post-fsck.log` was also mistakenly collected before the first
QEMU exited and is not a valid post-shutdown check. After verifying the
specific old process, it is stopped with SIGTERM; this is not a natural guest
shutdown, even though the outer helper exits 0. The true post-exit check in
`old-occupied-after-exit-fsck.log` exits 0 but warns that read-only checking
skips journal recovery; the superblock has `needs_recovery`. The original
disk is retained read-only as `old-occupied-case/rootfs-stalled.img`, never
repaired or reused. The shutdown stall's root cause is not established.

Linux completes sync, a confirmed read-only root remount and natural poweroff,
then post-exit fsck exits 0 with no pending journal warning. The current Starry
comparison uses another freshly copied base, verified before injecting only
the C executable. It passes, returns from a separate sync command, and exits
naturally on a separately issued poweroff, outer 0. Its post-exit fsck also
exits 0 without a pending journal warning. Both regression disks are sealed
read-only alongside their logs. They are not performance baselines.

The corrected production kernel now has generic retirement 13/13, kernel
55/55, actual system-case 1/1 and same-binary Linux/current-Starry evidence.
Final selected sources still match `sources-real-reclaim.tar` byte for byte.
This establishes the tested ownership transitions, not an end-to-end speedup.
A fresh frozen-base copy is prepared for full profiling. Its complete
pre-injection SHA is
`9968004595dec398480417e210d64f9ed509801d4325d4ab22f43951983f5ea3`.
The base intentionally retains the historical 600 s runner (`4f2580b3...`),
so comparing it directly with the full-build runner returns 1 at byte 68,
line 5. The complete diff was displayed; this is a preparation mismatch,
not a measured failure. As in the preceding complete runs, only the working
copy's `/opt/starry-macos-run.sh` is replaced with the frozen full-build
runner `c94b8669...`. Its exported copy compares byte-for-byte, and pre/post
injection read-only fsck both exit 0. The base, tg-xtask and guest source are
unchanged; no C regression executable is injected into the formal disk.
The prepared disk SHA is
`e9efce5f557842eec6d60c0c9c821145b2c0370f9b664a36ce34374128c86137`.
The saved-kernel helper boots at 2026-09-12 01:56:19 +08 (session 2754),
with the exact frozen inputs recorded in the run directory's `input.meta`.
This first launch omitted a PTY in the host terminal tool. Sending the guest
command failed before any workload or sampling began:

```text
write_stdin failed: stdin is closed for this session; rerun exec_command with tty=true to keep stdin open
```

The exact QEMU PID 1129212 was confirmed and SIGTERM-stopped; this is not a
natural guest shutdown or measured result. Its log/input manifest/checks and
read-only ended disk are preserved under
`tmp/private-cache.Lz1bM2/boot-closed-stdin/`. Its true post-exit fsck returns
0 but warns of pending journal recovery; the disk is not repaired or reused.
Another fresh frozen-base copy is prepared for an interactive PTY launch,
with the same runner-only replacement and full checks. A read-only capacity
query also used a nonexistent historical image filename and exited 1; actual
prior full-run images are stored beside their run logs. No data was removed.
Full compilation and its performance result remain pending.

The replacement cold copy again matches base `99680045...` before replacing
only the full runner. Export/cmp and both read-only fsck checks pass. Its
prepared SHA is `7ffc82b322020772d7ba8d1ccb7009c1a69a023dc341a72e95f3b3df3e916535`.
All hash processes end before launch. The corrected PTY session 80041 boots
at 2026-09-12 02:00:14 +08 and owns the run directory's new `qemu.log`.
The original non-PTY log remains separately preserved. The kernel and
selected production/test source archive remain unchanged.

## Completed cold full-build profile

The interactive run starts the actual build at 2026-09-12 02:01:06 +08 and
finishes at approximately 02:23 +08. Session 80041 / QEMU PID 1130900 exits
naturally with 0. The exact end marker is:

```text
===STARRY-ARCEOS-HELLOWORLD-PROFILE-WINDOW-PASS elapsed=1323 rc=0 build_completed=true tg_xtask_reused=true===
```

Cargo reports 21m47s; the inner axbuild build phase is 1318.00 s. The
runner's full interval is 1323 s and the kernel's interval is
1322921285344 ns. These are different boundaries, not interchangeable
timers. This run has no host deadline, debugger connection, concurrent
build, second QEMU, source edit, or offline disk operation in its measured
interval. Post-processing is outside that interval.

The ended filesystem is checked read-only only after session exit, with
status 0 and no pending journal warning. Optional extent-tree narrowing
suggestions are declined; no repair is performed. The image is moved to
the run's `rootfs.img`, made read-only, and fully hashed:
`214122d833996ce9553cc634f1055c27a0ad536726323e32a062b4cbb4443446`.
The fixed working-image path is now absent. Earlier baseline, failed-launch
and regression disks remain preserved.

All eight files under the guest artifact directory are exported through
read-only per-file debugfs dumps. All six entries of its `SHA256SUMS` pass.
The complete sorted name/version multiset, not just its count, compares
identically with `concurrent-cache-fill` and Linux `full-build-rcsc`.
The exported final ELF and Linux ELF compare byte-for-byte, SHA-256:
`2792c35275d91f57a38a847be87c4d2b333d7b88cd548bf9e06633288c7dba21`.
The frozen guest source archive/fingerprint, reused tg-xtask and toolchain
match the preceding comparison; the guest ArceOS SMP1 target is unchanged
inside the 8-CPU Starry host. The inherited core/memchr future-incompatibility
warning is retained in full `run.log`; it does not make this build fail.

The renderer consumes all 21616 CPU/wait records with the matching saved
ELF and produces all 11 nonempty CPU/event SVGs. The CPU-active, mutex-wait
and ext4-lock-hold SVGs are also rendered to PNG and visually inspected.
All reported dropped/skipped counters are 0. User execution is an aggregate
`[user-space]` category, not rustc/LLVM function symbolization.

| Metric | Previous 1684 s run | Current 1323 s run |
| --- | ---: | ---: |
| Total CPU samples | 130696 | 102942 |
| Active CPU samples | 54750 | 39182 |
| Active CPU percentage | 41.8911% | 38.0622% |
| memcpy active leaf samples | 2380 (4.3470%) | 291 (0.7427%) |
| find_free_area active leaf samples | 1855 | 1816 (4.6348%) |
| Mutex aggregate wait ns | 2411932033328 | 1998565186336 |
| Page-cache inclusive ns | 2361163035136 | 1646263274496 |
| ext4 global lock wait ns | 1427056498240 | 1387389660592 |
| ext4 global lock hold ns | 486602683904 | 418947462256 |
| Block flush inclusive ns | 470805964512 | 558037677440 |

The memcpy leaf count falls 87.773%, and page-cache inclusive time falls
30.277%. The comparison bundles cache-backed private reads with the
occupied-PTE/MADV_DONTNEED correctness corrections; it does not isolate each
change's share of the 361-second improvement. Mandatory anonymous first-write
zeroing remains; no disappearance of a symbol is evidence that zeroing was
removed. Similarly, 533 of the 556 allocate_frame leaf samples stop at
`ffffffff80160ef4`, immediately after IRQ re-enable, so they cannot be
interpreted as 533 expensive allocator operations.

The current largest resolved kernel CPU leaf is `find_free_area`, not
memcpy. Its two hottest PCs, `ffffffff800e9e84` (665 samples) and
`ffffffff800e9e78` (528), are the inlined BTree range-iteration loop in the
actual ELF. Source `memory/memory_set/src/set.rs:93` scans successive areas
for a fitting gap. The pinned Rust BTree range's `last()` already calls
`next_back()`; replacing that spelling would not remove this scan.
The next aggregate leaves include spin_release (1649), flush_batch (1261)
and frame sharing (1005); IRQ/barrier boundaries must be distinguished
from the apparent containing function before selecting another change.

Low guest CPU utilization is **not solved**: active percentage decreases
3.829 percentage points, despite lower total time. The largest mutex owner
is still ext4's global Ext4Guard, 1386202358880 ns / 69.3599% of mutex time.
Its lock-wait event is separately sampled and therefore is not numerically
identical. Directory lookup accounts for 486764528848 ns / 35.0849% of ext4
lock wait, followed by set_len 25.1346% and read_inode 21.3452%. The hold
breakdown is lookup 33.6266%, write 33.5679%, set_len 11.8196%, inode_info
10.1283%, and read_inode 5.3453%. These cross-task/overlapping totals must
not be added or interpreted as portions of the 1323-second wall clock.

Current source confirms that `Inode::lookup_locked` still takes the global
filesystem guard across child lookup, child metadata inspection, reference
accounting and entry creation. The portable core takes `&mut self` across
parent inode loading, directory lookup and child inode loading. This remains
a concrete coarse ownership boundary for subsequent Linux-guided work;
this private-page refactor does not claim to have removed it. Linux's pinned
`ext4_lookup` instead resolves a directory entry then obtains the inode via
`ext4_iget`; its VFS fast and slow lookup/locking contracts must be examined
before changing Starry's namespace concurrency.

The current full interval is exactly 1.8 times the frozen Linux 735-second
interval; different profiling overhead and single-run variation still
preclude treating that as an intrinsic OS ratio. Keep the current positive
candidate and its full evidence, but do not claim repeatability, solved
CPU utilization, complete syscall compatibility or readiness to merge.

## Limits and retained diagnostics

The existing user-fault signal adapter maps rejected faults to SIGSEGV;
this work has not established Linux SIGBUS-on-EOF compatibility. It also
does not claim complete truncate semantics for already copied private
pages, a full syscall ABI review, or non-AArch64 runtime validation. No
project commit, push or PR is authorized by this local experiment.

The initial filesystem clippy exits 1 with E0107: the pinned nightly's
`BorrowedCursor` requires its explicit element type. Adding `u8` resolves
the error; the retry passes all eight configurations without suppression.
The original diagnostic was displayed and remains in
`production-clippy.log`; the passing run is `production-clippy-retry.log`.

One combined source/document patch was rejected before applying because
its documentation context was not exact. Directory inspection established
no partial files were created, then source and documentation were applied
separately. Read-only path queries also reported the old `starry/test.rs`,
nonexistent `scripts/xtask`, an unmatched temporary-directory name, and
executable files mistaken for CMake build directories. Original errors
were displayed. Actual module paths, ktest code and saved build logs were
then read; no tests were launched to investigate those path errors.
