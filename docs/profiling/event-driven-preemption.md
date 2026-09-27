# Event-driven preemption refactor: validation record

## Baseline and scope

Design: `docs/design/event-driven-preemption.md`. Complete measured baseline:
`starry/full-build-rcsc` 2298 seconds versus `linux/full-build-rcsc` 735 seconds,
8c8g on the same RCsc-fixed QEMU. This candidate completed its full run in
2089 seconds, exit 0: 209 seconds (9.09487%) less than the single Starry baseline.
No guest workload, tg-xtask, rootfs base, compiler or profiling setting changed.

Pre-edit copies and logs: `tmp/preemption-refactor.5NL1NQ/`. These are evidence,
not a build source. Existing user changes in api.rs, run_queue.rs and rsext4
remain; the repository-wide dirty diff is not attributed to this refactor.

The frozen baseline ELF's `exit_preemption` begins at `ffffffff802bec2c`.
`baseline-exit.asm` shows repeated `current_area`, `current_context`, task policy
and pending publication calls before the depth transition, plus unconditional
DAIF save/mask. This supports changing the event boundary, not merely inlining
or renaming that function. The candidate assembly is inspected below.

## Static round 1 — ownership and call-chain audit

Completed 2026-09-11 before tests:

- All normal request setters were replaced by current-only publication:
  local wake, timer expiry, failed immediate preemption and deterministic test.
- All forced request setters use the same event boundary: host IPI model,
  IRQ reschedule deferral and switch-tail deferred wake. Remote delivery still
  uses the existing IPI protocol. No scheduler queue or wake-handoff atomics
  were weakened.
- Clearing the incoming normal policy flag remains separate. Resumed scheduler
  completion, first entry and unused preemption entry still reconcile policy
  under IRQ exclusion; first entry consumes the baton before IRQ enable.
- Final pending tokens retain depth one until runtime baton claim. Ordinary
  IRQ-disabled release cannot schedule; IRQ return retains its distinct right
  to schedule. Nested interrupts cannot consume the outer reserved depth.
- Context-owned tokens retain the owner. x86 resolves the current anchor
  before adopting a migrated switch depth and never touches the old CPU word.
- Compiler fences cover the lock-independent guard boundary; the existing
  atomic compare-exchange preserves concurrent interrupt pending updates.
- Public syscall numbers, flags, errno, credentials and resource sharing are
  not changed. This is a scheduling-mechanism refactor, not a claim of complete
  Linux syscall compatibility. Memory-management behavior still requires runtime
  validation because its guards use this mechanism.
- The ext4 regression invokes real mkfile, directory growth, extent encoding,
  inode persistence and remount. Real file payloads force fragmentation without
  orphaning allocated blocks. It is not a whole-disk fsck test. The previous
  measurement disk remains untouched and production accounting is still buggy
  until the required RED result is captured.

## Static round 2 — formatting and compiler checks

Completed. `cargo fmt --package cpu-local --package ax-runtime --package
ax-task --package rsext4` and `git diff --check` passed. Targeted
`cargo xtask clippy --package cpu-local` passed 3/3 checks (base, tls,
host-test including test compilation). `ax-task` passed 70/70, `ax-runtime`
25/25, and `rsext4` 3/3. The final axtask rerun finished at 15:28:08 with 70/70.
Native targeted cpu-local clippy also passed AArch64, RISC-V and LoongArch;
the xtask selector has no target override for that package's host-only metadata.
Clippy compiles but does not execute tests. Logs retain every invocation.

## Static round 3 and execution gates

Completed 2026-09-11 15:29 before the first test:

- Real Starry build returned 0 using the unchanged profiling build TOML;
  AArch64, release, SMP=8, guest-profile, 14069 kallsyms. Build only, no QEMU.
- Candidate `__RuntimePreemption_exit` at `ffffffff802b9df8` validates the
  opaque token and calls `finish_preemption`; its usual return path has no
  DAIF instruction, CPU pin, task query or pending write. Pending exits branch
  to `exit_pending_preemption`. IRQ return still verifies DAIF. The depth
  implementation retains LDXR/STXR compare-exchange and nesting checks.
- Candidate ELF SHA256 is
  `6e6b8f2f6e5413e9e57e94e98685a2c24cb3a600551a601c208b5569aeadb14a`;
  BIN is `24039ed4c44d1cefa154ce99dc7f9c4bad5f3ceb7b197bc4630e62822df7b539`.
  This intermediate image still contains the unfixed ext4 accounting bug and
  must not be used as an accepted performance result.
- Rechecked the final diff, compiler fences, pending interleavings, unchanged
  scheduler baton and actual HAL IRQ-return caller. fmt and diff checks pass.
- The task tests include nested pending, first entry, IRQ-disabled deferral
  and affinity migration through every available CPU. Axtask's new package-local
  build/QEMU TOMLs parse and agree on 8 CPUs, 8192 MiB and MTTCG. The runner's
  package-config discovery was inspected; other architecture configs are unchanged.
- HEAD remains `affddc3fecec02b31df94573063e4840433c9ebe`. No commit or push.

These gates permit the deterministic filesystem RED test. Production accounting
repair must then pass three final static rounds before GREEN and QEMU. No test
was started before these gates and none of these static results establishes
performance or runtime correctness.

## Directory accounting RED and repair

At 15:29 the unmodified accounting implementation failed the new public-path
regression with cargo exit 101: `left: 48`, `right: 64`. The missing 16 sectors
are exactly the two extent-tree blocks observed in the original filesystem
failure. Full original output is in `directory-accounting-red.log` and was
displayed directly. This is a deterministic production-code failure, not a
source-text assertion or a post-fix-only test.

The fix accounts the data block before `insert_extent`, so the extent tree's
own allocations increment the current count instead of being overwritten by
an old value. The same order already exists in classic directory growth.
Overflow preflight remains before allocation; allocation and inode cache
rollback remain owned by `with_metadata_transaction`. No disk is repaired.

The targeted native cargo test is used because the inspected `cargo xtask test`
command selects the std whitelist and has no package/test-name selector. The
QEMU tests and kernel builds continue to use xtask.

## Post-repair three static rounds

Completed at 15:31, before GREEN or QEMU execution:

1. Re-read every indexed directory append caller, its local inode copy and the
   metadata transaction rollback. Data accounting now precedes extent insertion,
   matching the classic directory path. Preemption production code is unchanged
   from the three preceding static rounds.
2. Formatted rsext4 and passed its 3/3 targeted clippy checks again
   (`clippy-rsext4-green.log`). Final formatting checks for all four packages
   and `git diff --check` returned 0.
3. Rebuilt the actual profiling Starry image, exit 0 in 14.03 seconds with
   14069 kallsyms (`starry-build-green.log`). Reviewed `final-exit.asm`:
   the ordinary path still has no DAIF save/mask or policy lookup. Final ELF
   SHA256 is `38bf7ef0f66442a898d3b561e50d22c01bd9762d2540932f5126468223094ddd`;
   BIN is `287bd9da5adfd0adb7aaca07e54c98e78f068101dc3f668c218758c02351c5e5`.
   Both hashes were independently checked again at 15:39.

These final gates permit GREEN and runtime tests. They do not establish runtime
correctness or a performance improvement; those results must be recorded below.

## First runtime checks and test-fixture correction

The exact directory accounting regression passed GREEN. Directory operations
(11), extent restart (10), and clean unmount (1) all passed, exit 0.
CPU-local's 24 state unit tests passed, including both new pending-interleaving
cases. Its broader command exited 101 in the final-image feature-tree integration
test: x86 Axvisor enables arm-el2. `ax-hal/Cargo.toml` in HEAD already has the
unconditional hv -> ax-cpu/arm-el2 edge, and neither that manifest nor the test
is changed here. The full command is not reported as passed.

The host ax-task command with ipi exited 101 before execution because the existing
IPI test calls missing `crate::tests::run_in_test_scheduler`. The same call is in
HEAD. The ordinary host combination without ipi passed 59/59; this is not an IPI
validation. Both failures remain in the logs and were displayed directly.

The actual 8c8g AArch64 QEMU suite completed 19 cases: 18 passed, including
first entry, nested requests, IRQ-disabled deferral, all-CPU migration, floating
point switches and IRQ notifications. The one failure was the old global FIFO
completion assertion: `spawn_raw` distributes tasks over independently executing
queues, which do not guarantee a global order. Its input fixture now enqueues
the entire batch on one CPU under a preemption guard before allowing execution;
the complete ordering assertion remains. Migration stays a separate all-CPU test.
No production change was made in response to this test failure.

Before rerunning the corrected fixture, three static rounds completed: reviewed
the enqueue/current-CPU lifetime and guard release before join; fmt and 70/70
targeted clippy passed at 15:41; final diff, public API resolution, unchanged
Starry image hashes and retained all-CPU migration assertions were checked.
The first QEMU command exited 1. The corrected fixture rerun passed 19/19,
`AXTEST_SUITE_OK`, command exit 0. Both complete logs are retained.

## Full profiling candidate

Run directory: `target/profiling/arceos-helloworld/starry/event-driven-preemption`.
The saved final ELF/BIN hashes match those above. `kernel-change-sources.tar.gz`
preserves the changed kernel inputs; the earlier frozen source archive records
the surrounding baseline. These archives are evidence only, not build paths.

The new root disk was copied from the same read-only base and its complete hash
matched `9968004595dec398480417e210d64f9ed509801d4325d4ab22f43951983f5ea3`
before offline writes. Only the formal guest runner was replaced and its dumped
bytes matched the repository script. Read-only fsck before and after injection
returned 0. The preboot hash is
`3d3cac0c3c5c73bcc4cf4fdc3cf02ba61d43ee16375d6080092cae062a68d283`.
The run uses the same RCsc-fixed QEMU and unchanged 8c8g profiling TOML.
No concurrent build, QEMU or large-file verification may run during measurement.

## Sampling attribution cross-check during the run

Re-reading the prior `fresh-page-zero.md` caveat prompted an exact PC check of
the complete baseline, not an extrapolation from that old window. Across the
full `exit_preemption` symbol range `ffffffff802bec2c..ffffffff802bef88`, 6405
active samples comprise 6278 at `ffffffff802bee88`, 82 at the entry, 39 at
`ffffffff802bec60`, and 3 each at `ffffffff802bec48`/`ffffffff802bec50`.
`baseline-exit.asm` shows `msr DAIFClr, #2` at `ffffffff802bee84` immediately
before the dominant PC. Thus 98.02% land just after IRQ enable.

This invalidates interpreting the 8.28% leaf width as exclusive execution time
or predicted savings. Delayed timer IRQ delivery can charge earlier masked
work to that boundary. The candidate removes real repeated operations, but a
smaller flame at this leaf alone cannot justify retaining it. The raw PC counts
are in `baseline-preemption-pcs.txt`; end-to-end time, total active samples and
all relocated guard/IRQ leaves must be compared after natural completion.

## Completed full run — 2026-09-11 16:18

The command and QEMU exited naturally with status 0. Cargo reported 34m22s,
axbuild 2082.41s; the outer measured interval is 2089 seconds. All 181 sorted
package/version entries and the final ELF compare byte-for-byte equal to both
frozen baselines. The final ELF SHA256 remains
`2792c35275d91f57a38a847be87c4d2b333d7b88cd548bf9e06633288c7dba21`.
The guest reused tg-xtask and source, with the same frozen compiler and inputs.

All six artifact checksums passed. The raw profile covers 2089523443840 ns,
with profiling disabled at export and all dropped/skipped counters zero.
Read-only post-shutdown fsck returned 0; unlike the baseline, there are no
directory block-accounting errors. The completed disk is preserved read-only
as this run's `rootfs.img`, SHA256
`eb594781dd35635badb88dbf35d74a20ee72a101ce6904087252d508f0e2b846`.
Neither the cold base nor any previous ended disk was repaired or overwritten.

| Metric | Prior Starry | Candidate |
| --- | ---: | ---: |
| Complete measured build | 2298 s | 2089 s |
| Active / total CPU samples | 77339 / 178447 | 74094 / 161966 |
| Active sample fraction | 43.34004% | 45.74664% |
| Mutex-wait accumulated duration | 3586.702 s | 2645.181 s |
| Ext4-lock-wait accumulated duration | 2085.214 s | 1503.894 s |
| Ext4-lock-hold accumulated duration | 726.173 s | 564.377 s |

These overlapping waits cannot be summed or equated with wall time. The
Linux baseline remains 735 seconds, making this single-run ratio 2.84218.
Different profiler overheads and run-to-run variability remain limitations;
this is not a stable or exclusively preemption-attributed speedup claim.

The complete CPU, mutex-wait and ext4-lock-hold SVGs were rendered successfully
and their PNG views inspected. User execution is 40049 active samples (54.05%).
The largest kernel leaf is now `unmap_range_recursive`: 3651 (4.92752%),
followed by fresh-page zeroing 3210 (4.33233%), `map_range_recursive` 2995
(4.04216%), `spin_release` 2779 (3.75064%), and `find_free_area` 2437 (3.28907%).
Page-fault stacks contain 12881 samples (17.38468%); ext4 stacks contain 1863
(2.51437%). CPU percentages are of non-idle samples, not exclusive wall time.
Mutex waits inside page-fault stacks still accumulate 1058.717 seconds.
Ext4's largest hold caller is lookup (211.393 s, 37.45609%); it remains a
serialization problem, not the largest kernel CPU leaf.

The next candidate targets range-based unmapping and invalidation-before-release
ownership. It must preserve the opaque-PTE architecture boundary and actual SMP
invalidation; calling the existing local-only full-TLB helper is not sufficient.
No runtime result for that next candidate is implied by this measurement.
