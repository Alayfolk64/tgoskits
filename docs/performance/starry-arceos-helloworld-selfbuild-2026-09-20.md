# StarryOS ArceOS Hello World Self-build Optimization Report

Date: 2026-09-20

## Status

This document records an in-progress performance investigation of compiling
`arceos-helloworld` with `tg-xtask` inside StarryOS. It is also the handoff
record for the temporary WIP branch. The branch is not ready for a pull
request: the latest board run exposed another block-cache locking defect, and a
deterministic regression test for that defect is intentionally failing until
the implementation is completed.

The latest fully successful StarryOS result measured during this investigation
is 457.577 seconds. It predates the latest unverified WIP changes and must not
be presented as the performance of the current branch tip.

## Reproducibility boundary

- Upstream base: `4a398bd61` (`upstream/dev`)
- WIP history before the checkpoint:
  - `3244fe721 refactor(ext4): move data and journal I/O to owned requests`
  - `821e13aeb perf(starry-kernel): prune VMA free-area searches`
  - `7b8d3cb3e feat(cqhci-host): add RK3588 eMMC command queue`
  - `44ecc1c26 perf(starry-kernel): map private file reads from page cache`
  - `25b82ca4c wip(profile): preserve OrangePi optimization state`
- Board: OrangePi 5 Plus, 8 CPUs and 8 GiB RAM
- QEMU configuration: 8 vCPUs and 8 GiB RAM
- Source archive SHA-256:
  `7de46545454e3f5562d78a1a065bf2ed1a736cf807f4cbf05b30d98ab2725147`
- `tg-xtask` SHA-256:
  `e6823ab3b6a6d944266fc31d5e54814f463466f88949849ba139b6c575f7e6f1`
- Profiling method: direct in-kernel sampling and wait instrumentation. The
  `xtask perf` wrapper was not used as the profiler.

Linux and StarryOS used the same source snapshot and `tg-xtask` input. The
available successful figures are individual cold runs rather than three-run
medians. Three cold runs per side are still required before publishing a stable
benchmark claim.

## Measured results

| Environment or configuration | Result | Wall time | Interpretation |
| --- | --- | ---: | --- |
| Board Linux baseline | success | 78.925 s | Full cold build, exit status 0 |
| StarryOS VMA gap-index build | success | 607.595 s | Earlier successful baseline in this work |
| StarryOS with in-kernel profiler | success | 607.35 s | Profiling overhead was negligible in this run |
| StarryOS block-stage build | success | 561 s | Intermediate block-stack optimization |
| StarryOS I/O-phase build | success | 457.577 s | Best fully successful result so far |
| Board profile run 2 | failed | 788 s | `command_rc=1` |
| Board profile run 3 | failed | 1058 s | `command_rc=1`; profile data was captured |
| Board run 10 | timeout | 1200 s | No final result metadata |
| Board run 11 | failed | about 616 s | Profile exists, no final result metadata |
| Board run 13 | panic | 245/246 units | PI-mutex diagnostic found geometry lookup under an unsafe blocking context |
| Board run 14 | watchdog reset | 244/246 units | Reached the 20-minute recovery limit |
| Board run 15 | panic | 245/246 units | PI-mutex diagnostic found block-cache state-lock contention |
| Current full QEMU run | incomplete | over 3060 s | Stalled at 80 units with repeated `ResourceBusy` writeback errors |

The best successful StarryOS run is 150.019 seconds, or 24.69%, faster than
the 607.595-second StarryOS baseline. It remains 5.80 times slower than the
78.925-second board Linux result.

Only wall time, exit status, compiler progress, and kernel profiling counters
are trusted. Some StarryOS `/usr/bin/time -v` process-usage fields were invalid,
including impossible CPU percentages and page-fault counts, so those fields
are excluded from conclusions.

## Hotspot evolution

### Initial complete kernel profile

The profile with 86,928 CPU samples recorded 62,295 active samples and 24,633
idle samples. Its largest leaves were:

- idle wait: 24,628 samples;
- `CowBackend::alloc_new_frame_sized`: 13,204 samples;
- `dma_alloc_pages`: 12,722 samples;
- global allocator allocation: 9,463 samples;
- page allocation: 7,550 samples;
- PI mutex owner spinning: 1,438 samples.

The sampled wait ranking was led by page-cache waits, mutex waits, block reads,
completion waits, and ext4 lock holds. These are scaled sampled totals, not
independent wall-clock durations, and therefore must not be added together as
elapsed time.

The block counters reported 476,268 submissions, a largest batch of one, and a
peak in-flight depth of one. This proves that adding a command queue alone did
not create I/O parallelism: the upper block/filesystem path still serialized
each request.

### Later watchdog profile

Run 10 recorded 6,121 retained CPU samples. `FilePageIndex::prune_stale`
accounted for 2,980 samples, or 48.68% of all retained samples and 55.39% of
active retained samples. This motivated the subsequent file-page-index
refactor.

This profile must be interpreted cautiously. It retained only CPU 0 and
reported `dropped_cpu=22190` and `dropped_wait=37553`. It is strong evidence of
a pathological algorithmic hotspot, but it is not a balanced all-CPU flame
graph.

Its I/O counters were:

- 41,187 reads covering 537,287 sectors;
- 160,640 writes covering 1,285,192 sectors;
- 201,762 DMA-pool hits and 65 misses;
- largest block batch one and peak in-flight depth one.

The DMA pool was effective, but the one-request submission model remained.

### Current limiting defect

The latest board run reached 245 of 246 compilation units and then failed with:

```text
ARCEOS_PANIC_EMERGENCY
panicked at components/ax-task/src/sync/mutex/mod.rs:606:35:
validate PI mutex blocking context at fs/ax-fs-ng/src/block/cache/device.rs:241:43 failed: operation requires a scheduler safe point
```

The immediate contention point is `self.shared.state.lock()` in
`BufferedBlockDevice::read_block`. The per-device cache-state lock is shared by
foreground reads and periodic writeback. The caller can reach this lock while
the scheduler context is temporarily non-blocking, so waiting on the PI mutex
is invalid. This is now a concrete block-cache synchronization problem, not a
generic claim that ext4 itself is slow.

An earlier failure at the same layer was caused by immutable folio geometry
being read under the mutable cache-state lock. Moving that geometry into an
immutable `FolioGeometry` object made `matches_block_size` and request splitting
lock-free. The deterministic geometry-contention regression passed after that
change.

The next deterministic test,
`atomic_read_does_not_wait_for_contended_cache_state`, currently fails against
the unfinished implementation with:

```text
called Result::unwrap() on an Err value: Timeout
test result: FAILED. 0 passed; 1 failed
```

This red test is preserved deliberately in the WIP checkpoint. The required
fix is a context-aware cache-state acquisition contract and explicit
`WouldBlock` propagation to the filesystem boundary, not another timeout or a
disabled diagnostic.

## Implemented optimization areas

### Filesystem and block I/O

- Refactored ext4 data and JBD2 journal operations into owned requests so I/O
  can be issued outside broad filesystem-state locks.
- Split large filesystem and journal modules along lifetime, admission,
  writeback, namespace, and I/O ownership boundaries.
- Added detached write preparation and explicit completion stages.
- Moved writeback work out of broad locks and added synchronization-policy
  regression coverage.
- Added reusable DMA buffers and batched block-runtime interfaces.
- Added the RK3588 CQHCI queue and completion path.

The remaining structural gap is visible in the counters: submissions still
arrive one at a time, so the queue cannot expose hardware concurrency yet.

### Memory management and page faults

- Added indexed VMA gap searches rather than repeatedly scanning the complete
  mapping set.
- Mapped private file reads from the shared page cache and retained COW for
  private modifications.
- Added in-flight page coordination and reworked stale file-page indexing after
  profiling identified `FilePageIndex::prune_stale` as the dominant leaf.

### Scheduler ordering

- Added an explicit post-spinlock SMP barrier to the wake path, following the
  ordering role of Linux `smp_mb__after_spinlock()` in `try_to_wake_up()`.
- Routed the relevant wake requests through that barrier.
- Confirmed the new AArch64 image contains the expected `dmb ish`; the old image
  did not.
- A focused futex regression passed 30 rounds under the standard system QEMU.

The full QEMU compilation is not complete. Its latest recorded state was 80
compilation units and 77 distinct crates after more than 3060 seconds, with
repeated periodic-writeback `ResourceBusy` errors. The focused regression is
evidence for the ordering fix, but it is not a full-build performance result.

### Board recovery

- Added a reusable DesignWare watchdog driver and a typed watchdog interface.
- Added RK3588 discovery, clock/reset preparation, and StarryOS lease feeding.
- Restored Linux as the default boot target before running the compiler.
- Demonstrated automatic reset and Linux recovery after board failures.

The watchdog is development/recovery infrastructure. It should not be mixed
into the final performance pull requests unless the application integration is
reviewed independently.

## Validation state

Completed at earlier stable points:

- `ax-task` targeted diff, formatting, and clippy checks;
- the standard-QEMU futex regression for 30 rounds;
- `ax-fs-ng` diff, formatting, and clippy checks after the lock-free geometry
  change;
- relevant StarryOS kernel feature clippy checks;
- block-geometry contention regression.

Completed immediately before the WIP checkpoint:

- unstaged `git diff --check`;
- staged `git diff --cached --check`.

Not complete on the current WIP tip:

- `atomic_read_does_not_wait_for_contended_cache_state` is intentionally red;
- formatting and clippy have not been rerun after all latest edits;
- a complete current-tip board build has not passed;
- a complete current-tip 8-vCPU/8-GiB QEMU build has not passed;
- three cold Linux and StarryOS comparison runs have not been collected.

## Continuation order

1. Keep the red atomic-read contention test unchanged.
2. Implement context-aware cache-state acquisition and typed `WouldBlock`
   propagation through `ax-fs-ng` and the ext4 adapter.
3. Make the red test pass, then rerun the existing block-cache and journal
   regression suites.
4. Perform the required three static gates: diff check, formatting check, and
   targeted clippy for every modified crate.
5. Run one full 8-vCPU/8-GiB QEMU build and one full OrangePi build.
6. Capture a complete in-kernel profile from a successful current-tip run.
7. Use the new profile to decide whether the next large change belongs in
   block submission batching, writeback admission, or memory management.
8. After correctness stabilizes, collect three cold Linux runs and three cold
   StarryOS runs and compare medians.

## Pull-request split after stabilization

The WIP checkpoint is intentionally not a pull request. The final clean series
should contain three reviewable parts:

1. ext4, block-cache, DMA, and RK3588 command-queue ownership and concurrency;
2. VMA search, private file-backed page cache, COW, and file-page indexing;
3. scheduler wake ordering and narrowly scoped board recovery support.

Raw profiles, flame graphs, temporary run scripts, and `target/` artifacts must
not be included in those optimization pull requests. This report can remain as
the durable record of measurements, limitations, and handoff state.
