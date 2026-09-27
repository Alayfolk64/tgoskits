# Batched mapping retirement: evidence

Design: `docs/design/batched-mapping-retirement.md`. Baseline is the completed
2089-second event-driven-preemption run. That run's sources, ELF, profile and
read-only disk remain frozen; it is not a measurement of this candidate.

## Regression admission, before production changes

The initial static review examined these three areas:

1. The regression calls the real generic `unmap_page` with real allocated
   backing tables. A transparent PTE wrapper records actual clear operations,
   the metadata capability records invalidation completion, and the allocator
   records actual frees. A four-level, one-page mapping requires three table
   retirements. A thread-local log avoids inter-test interference. The assertion
   allows one flush to cover several clears, so it does not prohibit batching.
2. Package fmt and xtask clippy passed (2/2 base and copy-from). Targeted native
   test clippy is needed because the xtask package plan does not compile this
   host integration test. That test-only command failed with 10 old-mock
   dead-code/style diagnostics; its raw output was displayed. It did not pass.
3. Re-read the callback/lifetime fixture and compared PR #2009's actual diff.
   Generic/ax-cpu production files are still unchanged, fmt and diff checks pass.
   The command batch incorrectly started RED despite the test-only clippy
   failure. This violated the admission gate and was explicitly reported.
   The test did fail on the intended production ordering, but that run is not
   counted as compliant validation. The oversized old mock dependency was
   replaced by a minimal recording PTE/allocator with real backing pages;
   no warning suppression was added. Repeat all three gates before RED again.
   No QEMU has been started. The whole production refactor needs three further
   final rounds before GREEN/QEMU.

The corrected fixture then passed three fresh rounds: (1) read the complete
minimal PTE/allocator and ownership log, including zero representation and
huge/invalid-leaf bits; (2) fmt, package clippy 2/2 and targeted test clippy all
returned 0 with no warnings; (3) final full fixture review, diff/fmt checks and
confirmation of unchanged production files. These gates admit the repeated
RED command only; they do not admit a new kernel performance test.

The repeated RED returned 101 without compiler warnings. It recorded
`[Clear, Flush, Clear, Free, Clear, Free, Clear, Free]`, proving that the first
parent unlink was not covered before freeing its table. Full original output
is in `tmp/preemption-refactor.5NL1NQ/retirement-red-verified.log` and was shown.

## Final refactor static rounds

### Round 1: complete ownership and boundary audit

Completed after the generic engine, AArch64 batch operation, both Starry
consumers and all new regressions were written. Read the range walker, legacy
unmap, frame representation, COW reference table, RSS accounting, MemorySet
callers and file-cache invalidation/retirement ownership together.

- Each occupied leaf is prepared before clear. No fallible step separates
  clear from queue insertion: every owner/table also consumes one of the 64
  address slots, and capacity is drained before mutation. Missing subtrees
  advance to the next actual level boundary, not one 4 KiB hole at a time.
- A child is detached only after its recursive call returns and its slice
  borrow ends. `Frame` has no implicit freeing Drop. Queued table frames are
  never active recursion frames when a capacity-driven drain frees them.
  Root ownership is unchanged. Errors drain the removed prefix and preserve
  the failing descriptor; an attached but empty table after a later error
  remains owned and is reclaimed by a subsequent teardown.
- COW keeps the original mapping reference counted until retirement, so an
  address-space sibling cannot prematurely treat its frame as exclusive.
  The lookup's global frame-table lock is released before the per-frame lock;
  retirement retains the existing per-frame -> global-table lock order.
  RSS removal still runs under the original exclusive address-space access.
- Shared-file cache owners are independent of a PTE. Cache retirement keeps
  discarded pages until every listener acknowledges invalidation. A concurrent
  listener cannot pass the held address-space mutex while the range is being
  removed. This change neither drops that owner nor adds a cache/aspace lock
  inversion.
- Reversed/unaligned/out-of-width ranges fail before mutation. Partial huge
  leaves are rejected, occupied non-present leaves are included, and opaque
  PTE configuration stays architecture-owned. Existing single-page consumers
  remain on the legacy API; its `flush: false` now defers only leaf flushes,
  not the barrier required for immediate table-frame reuse. Public docs were
  updated accordingly.
- Direct syscall signatures, flags, permissions and dispatch are unchanged.
  This is an ownership/lifetime audit of the shared teardown helper, not a
  claim of complete Linux ABI conformance. The deterministic clear/flush/free
  violation is tested at the real page-table allocator boundary because a
  userspace syscall cannot deterministically observe that hardware race.
  Real COW fork ownership/RSS and file-cache regressions remain in kernel
  axtest. Existing MemorySet error handling is not redesigned here.
- AArch64 uses baseline ARMv8 IS broadcasts with publication/completion
  barriers. Other architectures retain existing per-address invalidation
  scope and are compile-checked only; this does not certify their remote
  shootdown or software-refill contracts. No board or new architecture support
  is claimed. The architecture skill's debugging reference records that limit.

Re-read the pinned Linux mmu-gather and arm64 batching code, and MOSS's
owned UnmapGather. This refactor reuses their ownership ordering, not their
whole MM implementation or MOSS's 8192-record heap allocation policy.

### Round 2: format, lints and static compilation

Completed on 2026-09-11 at 16:55 +0800. Logs are under
`tmp/preemption-refactor.5NL1NQ/`. This round alone does not admit tests.

| Check | Actual terminal result | Log |
| --- | --- | --- |
| `cargo fmt -p page-table-generic -p ax-cpu -p starry-kernel`, then `--check` | exit 0 | terminal |
| `git diff --check` | exit 0 | terminal |
| `cargo xtask clippy --package page-table-generic` | exit 0, 2/2 | `retirement-generic-final-clippy.log` |
| `cargo clippy -p page-table-generic --test retirement -- -D warnings` | exit 0, no diagnostics | `retirement-fixture-final-clippy.log` |
| `cargo xtask clippy --package ax-cpu` | exit 0, 33/33 | `retirement-cpu-clippy.log` |
| `cargo xtask clippy --package starry-kernel` with existing firmware cache | exit 0, 110/110, 6m10s | `retirement-starry-clippy-retry.log` |
| Actual AArch64 `axtest_kernel` clippy, `cfg(axtest)` and SMP=8 | exit 0, 19.26s | `retirement-axtest-clippy.log` |
| Actual profiling-config Starry build | exit 0, 13.82s, 14039 symbols | `retirement-starry-build.log` |

The axtest static command uses native clippy because ktest has no build-only
entry. Its features match the package build TOML plus required `axtest,smp`;
`AX_ARCH`, `AX_TARGET`, `AX_LOG`, `SMP=8`, `build-std=core,alloc` and
`cfg(axtest)` match ktest. Clippy checks code without linking/running it; the
normal xtask build separately validates the profiling kernel's PIE link.
Existing future-incompatibility notices concern toolchain core/memchr, not
new clippy warnings; they remain visible in the logs.

The first Starry matrix stopped at check 22/110 because firmware download
could not resolve raw.githubusercontent.com (exit 1, build-script exit 101).
Its full original error was displayed and remains in
`retirement-starry-clippy.log`. Reusing the pre-existing verified firmware
cache through `AIC8800_FIRMWARE_DIR` is an environment-only retry, not a driver
change or a suppressed failure.

### Round 3: final source and artifact audit

Completed on 2026-09-11 at 16:58 +0800, before GREEN or QEMU execution.
Final documented-source profiling build returned 0 (14.13s, 14039 symbols).
Final package fmt check and whole-worktree whitespace check both returned 0.
Rechecked queue/error/root invariants, actual `axtest_kernel` discovery and
export of the 130-page fork/RSS case, and public deferred-leaf documentation.

Read the saved final ELF, not the earlier build: it is AArch64 ET_DYN with no
PT_TLS/.tdata/.tbss. `batched-tlb.asm` shows `dsb ishst` at
`ffffffff80380910`, `vmalle1is` at `...091c`, `vaae1is` at `...0934`,
then `dsb ish; isb` at `...093c/0940`. The actual COW gather calls it at
`ffffffff800b3dd8`, before table free at `...3e00` and frame-reference drop
at `...3eac`. Shared-file capacity drains likewise call it before table free.
The small/large choice is 32 entries and needs no FEAT_TLBIRANGE. Empty batches
return without barriers. Generic tests cover structural ordering independently
of these architecture instructions.

An exact comparison with the earlier symbol table returned 1 at byte 1299,
line 25; its original output was displayed. The first difference is an LLVM
internal suffix (`4921251078493087253` vs `1788086934486685833`). The two
builds are not claimed identical. All new artifacts and disassembly are tied
to the final saved ELF instead.

Artifacts: `target/profiling/arceos-helloworld/starry/batched-mapping-retirement/`.

- ELF: `7a40b7261151c17d2db0f473b1704d671d63a105935461581f9336ef4a4ee01c`.
- BIN: `29080f2f927c03076b02ac48e35cebdd257163ef68887dc62bbd7b4d8bbf1151`.
- `retirement-sources.tar.gz`: `5c89e90fa1e62878941fc963815ba83da3dffb17af1bd92bd93262045293aeac`.
  Contains the full three affected crates plus design/architecture reference;
  earlier worktree changes remain captured by event-driven-preemption's source
  archive over repository HEAD `affddc3fecec02b31df94573063e4840433c9ebe`.
- Copied cold image matches frozen base `9968004595dec398480417e210d64f9ed509801d4325d4ab22f43951983f5ea3`.
- After replacing only the guest runner: preboot disk
  `e7a09de553c8628870d08c0a01b1056e03fcae6a87435cf97e02d1f42e53f935`;
  installed runner cmp and read-only filesystem checks returned 0. Full hash
  finished before QEMU. The previous run's ended disk remains read-only.

All three static rounds are complete. GREEN and the existing 8c8g kernel
ktest entry are now admitted; performance is still unmeasured for this refactor.

## Runtime validation

After all three static rounds, `cargo test -p page-table-generic --test
retirement` returned 0, 7/7 passed, none skipped. This includes the original
deterministic parent-unlink ordering RED now turning GREEN. The complete
`cargo test -p page-table-generic --features copy-from` command also returned 0.
Logs: `retirement-green.log`, `retirement-generic-suite.log`.
The full suite totals 76 passed, 0 failed/ignored. Existing unrelated mock
dead-code and unused-variable warnings remain in that log; the new standalone
retirement test clippy is warning-free. No warning suppression was added.

The next command is the existing package-owned 8c8g runner:

```sh
cargo xtask ktest qemu -p starry-kernel --test axtest_kernel --arch aarch64
```

The environment sets workspace `TMPDIR` and prepends the verified patched
QEMU directory to `PATH`. This test uses its own runtime rootfs with discard
writes, not the new persistent compilation-measurement disk. Actual result:
exit 0, `AXTEST_SUMMARY pass=52 fail=0 skip=0 total=52`, `AXTEST_SUITE_OK`;
QEMU portion 1.84s. The new fork-owner case and existing file-cache retirement
case are both explicitly reported as passed in `retirement-starry-ktest.log`.

The historical generated image config hard-coded `/tmp/tgosimages`, overriding
TMPDIR. This was discovered during asset setup and reported. After completion,
the two exact files generated by this invocation (registry and verified Alpine
archive) were moved to workspace `tmp/tgosimages/`; no unrelated `/tmp` files
were removed. Future xtask image consumers must additionally set
`TGOS_IMAGE_DOWNLOAD_DIR=/home/wuxun/Projects/tgoskits/tmp/tgosimages`.
The persistent compile-profile disk is separate and unchanged by this test.

## Complete cold-build result, 2026-09-11 17:35 +0800

The 8c8g Cortex-A53/GICv3/MTTCG measurement completed naturally. QEMU and
its saved-kernel wrapper returned 0; PTY session 65278 is closed. This is
the whole frozen tg-xtask build, not a timed window or a compile-count cutoff.

| Check | Result |
| --- | --- |
| Measured duration / command status | 1908 s / 0 |
| Cargo / axbuild internal build duration | 31m21s / 1898.74 s |
| Compile units / distinct package names | 181 / 175 |
| Source and tg-xtask reused | both true |
| Compile-unit name/version list | byte-identical to both full-build-rcsc baselines and event-driven-preemption |
| Final arceos-helloworld ELF | byte-identical to all three reference artifacts |
| Original artifact SHA256SUMS | 6/6 passed |
| Saved kernel SHA256SUMS | ELF and BIN passed |
| Read-only ended filesystem check | exit 0; no repair performed |
| Profile interval | 1908172087152 ns |
| Dropped CPU/wait/pending and skipped CPU | all 0 |
| Offline renderer | exit 0; complete CPU, mutex-wait and ext4-lock-hold PNGs inspected |

The final program SHA-256 is
`2792c35275d91f57a38a847be87c4d2b333d7b88cd548bf9e06633288c7dba21`.
The ended disk was moved from the fixed working path into this run's
read-only `rootfs.img`; its complete SHA-256 is
`3b60eefee6560f0e09a3698b7b91339f22a082ba2af98a546e19d93986b8d7ec`.
The original cold base and previous ended disks were not modified.

The single measured duration decreased from 2089 to 1908 s: 181 s, or
8.66443%. Relative to the original 2298 s baseline this is 16.97128% less
time. The frozen Linux measurement remains 735 s (current Starry/Linux
ratio 2.59592). These are single-run, profiling-enabled comparisons, not
statistical speedup guarantees or proof of one instruction's exclusive cost.
Linux and Starry's profiler overhead/stack visibility still differ.

### New complete hotspots

There are 147934 CPU samples: 66408 active and 81526 idle. Active fraction
is 44.89029%, compared with 45.74664% previously. Faster completion did not
solve low CPU utilization. Percentages below use active samples, not all CPU
samples and not wall time.

| Active CPU leaf | Samples | Percent |
| --- | ---: | ---: |
| User space | 36910 | 55.58065% |
| `try_zero_page` | 3495 | 5.26292% |
| `map_range_recursive` | 2919 | 4.39555% |
| `spin_release` | 2591 | 3.90164% |
| `memcpy` | 2103 | 3.16679% |
| `find_free_area` | 2014 | 3.03277% |
| COW `clone_map` | 1412 | 2.12625% |
| Architecture `flush_batch` | 1219 | 1.83562% |
| COW retirement `finish` | 660 | 0.99386% |
| COW `remove_range` | 350 | 0.52704% |

The old recursive-unmap leaf is no longer the largest kernel CPU leaf.
Do not compare its disappearance alone with the new walker: work also moved
into batch invalidation and retirement. The complete duration includes those
costs. Page-fault-inclusive CPU stacks contain 12422 samples (18.70558%);
ext4-inclusive stacks contain 1760 (2.65028%).

Of 3495 zeroing samples, 3433 are at `ffffffff80381614`, the actual
`dc zva, x0` instruction in the saved ELF (98.22604%). This is not the old
DAIFClr-boundary attribution problem. `zero-hotspot.asm` and
`zero-hotspot-pcs.txt` preserve the evidence. Of these zeroing samples, 3463
come through user page faults: 2615 through the locked `populate` path,
848 through unlocked private-file preparation. The remaining 32 are other
paths. Stack shape identifies the path, not the original fault's read/write
flags; it cannot prove that a shared zero page would remove all this work.

Mutex waits total 2641.041 s across tasks, including 1074.239 s in fault
stacks. Ext4 lock wait is 1447.399 s and lock hold is 543.123 s; lookup
accounts for 223.106 s (41.07827%) of hold. Page-cache inclusive time is
2017.953 s, of which private `prepare_frame` accounts for 1657.251 s
(82.12534%). Block read/write/flush totals are 809.013/99.586/636.038 s.
Off-CPU totals are 137140.447 s across tasks, with futex 57.99001%; they
are not elapsed build time. These nested/sampled event totals overlap and
must not be added. Ext4 remains a waiting bottleneck even though it is not
the largest CPU leaf.

The next candidate is the complete private-fault preparation/commit boundary:
anonymous zeroing and resident COW copying still hold the address-space lock,
and private-file preparation currently zeroes even bytes subsequently read
from the file. Compare Linux `mm/memory.c` at the pinned revision:
`do_anonymous_page` allocates before the PTE lock, and `wp_page_copy` retains
the old page, copies, then revalidates the PTE before commit. MOSS's owned
`CowFaultPlan` follows the same prepare/execute/revalidate pattern. New design,
ownership regressions and three final static rounds are required before its
runtime tests; none of that new production refactor is included in this run.

### Offline diagnostic errors retained

`debugfs rdump` returned 0 but printed `dump_file: Operation not permitted
while changing ownership of ...` for the eight exported files and the
directory. The unprivileged host cannot reproduce guest root ownership;
the actual exported contents subsequently passed all six original hashes.
A separate final-program dump initially used the mistyped image basename
`rootfs-aarchos-helloworld-profile.img`. It printed `No such file or directory
while trying to open ...` and `dump: Filesystem not open`, also returning 0.
The corrected exact image path exported the final ELF successfully, followed
by the SHA and three baseline byte comparisons above. All original diagnostic
output was shown directly; neither initial command was treated as successful
validation. Future Rust compatibility warnings for core/memchr remain in the
complete guest log. SVG rasterization emitted the existing get_pixbuf
deprecation warning and returned 0.
