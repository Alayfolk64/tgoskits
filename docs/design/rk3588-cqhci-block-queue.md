# RK3588 eMMC CQHCI block queue

## Problem

The RK3588 DWC MSHC driver currently exposes one in-flight request and one
request per submission batch.  On Orange Pi 5 Plus, Linux enables the eMMC
Command Queue Engine and exposes 31 data tags.  StarryOS therefore serializes
independent filesystem reads before the device can schedule them.

The initial target is the StarryOS self-hosted `arceos-helloworld` build on the
Orange Pi 5 Plus eMMC root filesystem.  Success means that the RK3588 path:

- enables CQE only when both firmware and EXT_CSD advertise it;
- publishes the card's bounded data-tag depth through `rdif-block`;
- accepts and commits multiple owned DMA requests as one batch;
- completes requests by tag from an acknowledged IRQ;
- never returns DMA ownership before the corresponding task is complete or
  the engine is demonstrably quiesced;
- retains the existing legacy SDHCI path for SD cards and unsupported eMMC.

This change does not add SD CQE, inline crypto, packed commands, or multiple
physical hardware queues.

## Evidence and prior art

The board's running Linux system reports `cmdq_en=1`, `nr_requests=31`, HS400
Enhanced Strobe at 200 MHz, and a `supports-cqe` property on
`mmc@fe2e0000`.  Linux's current `sdhci-of-dwcmshc.c` selects the RK35xx
CQHCI operations for RK3588 and initializes CQHCI when that firmware property
is present.  Linux's `cqhci-core.c` uses 32 slots, reserves tag 31 for direct
commands, publishes a task-list DMA address, rings one doorbell bitmap, and
consumes the task-completion notification bitmap from IRQ context.

References:

- Linux `drivers/mmc/host/sdhci-of-dwcmshc.c`, current upstream source
- Linux `drivers/mmc/host/cqhci-core.c`, current upstream source
- Linux `drivers/mmc/host/cqhci.h`, current upstream source
- JEDEC JESD84-B51 command queue fields and CQHCI register semantics

## Alternatives

Increasing only `QueueLimits::max_inflight` is rejected because the legacy
SDHCI path owns one command state and one ADMA table.  Concurrent callers would
race the same registers and DMA descriptors.

Keeping hardware depth one while enlarging the software channel could reduce
producer blocking, but it cannot overlap requests in the eMMC device and does
not match the board's working Linux configuration.

Filesystem-only readahead and metadata-cache changes remain useful, but the
current path already coalesces adjacent blocks and submits multi-block I/O.
They cannot remove the single-request controller boundary.

The selected design adds a portable CQHCI core and a small RK3588 adapter.  It
reuses `rdif-block` request ownership and `dma-api` buffers instead of adding a
second block API.

## Boundaries and ownership

`cqhci-host` owns the standardized CQHCI register state, coherent task and
transfer descriptor tables, tag allocator, in-flight DMA values, completion
bitmaps, and recovery state.  It has no FDT, scheduler, filesystem, or RK3588
dependencies.

`sdmmc-protocol` parses command-queue capability and depth from EXT_CSD and
performs the card-side `CMDQ_MODE_EN` transition through its existing command
state machine.  It does not access CQHCI MMIO.

`sdhci-host` provides the legacy initialization transport and an IRQ endpoint.
The RK3588 adapter supplies the Vendor Area 2 register base and the Rockchip
pre-enable/disable hooks described by the Linux driver.

`rdif-block` remains the capability boundary consumed by the OS runtime.  The
hardware queue has one maintenance-task owner.  Hard IRQ code only reads and
acknowledges status into atomics; descriptor mutation, DMA ownership transfer,
and request completion remain in task context.

## State transitions

1. Start with the existing IRQ-driven SD/MMC initialization path.
2. Preallocate CQHCI descriptor ownership and provision the runtime for at
   most 31 tags before card discovery.  A non-CQE card shrinks back to the
   legacy depth-one limits before becoming ready.
3. If the card is eMMC with `CMDQ_SUPPORT`, issue `CMD6` for
   `CMDQ_MODE_EN=1` and wait for transfer state.
4. Program the preallocated task-list base, RCA, transfer mode, and interrupt masks, then
   publish queue limits and mark initialization ready.
5. Stage an ordered request prefix into free tags.  Descriptor writes happen
   before a single doorbell write in `commit_submissions`.
6. On IRQ, acknowledge CQHCI status and TCN in the top half.  The maintenance
   task consumes completed tags, transitions each `InFlightDma` back to
   `CompletedDma`, and reports terminal requests.
7. On an engine error, halt and clear all tasks before returning DMA.  If the
   controller cannot be proven quiescent, quarantine the DMA values instead.

Flush is a queue barrier.  It waits until data tags are idle and uses the
reserved direct-command slot for `FLUSH_CACHE` when the eMMC cache is enabled;
otherwise it uses `SEND_STATUS` as a non-mutating completion barrier.

## Concurrency and memory ordering

Only the queue maintenance task writes descriptors or owns in-flight request
objects.  The hard IRQ endpoint publishes acknowledged status through atomics.
Descriptor writes and DMA preparation precede the doorbell with a release
fence.  Completion status is observed with acquire ordering before DMA is
returned to CPU ownership.

## Validation

Deterministic tests cover EXT_CSD capability parsing, task and transfer
descriptor encoding, batch tag allocation and doorbell publication,
out-of-order completion, and flush serialization.  Static checks are
formatting, targeted clippy for every changed crate, and the applicable
unit-test builds.  Only after all three pass is the board image deployed.

Board validation first performs read-only filesystem work, then the full
self-hosted build and kernel profiling.  Linux boots afterward to run the
normal root filesystem health checks.  The feature is accepted only if output
hashes match and profiling shows lower block dispatch waiting without new CQE,
DMA, or ext4 errors.
