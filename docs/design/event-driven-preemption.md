# Event-driven preemption safe points

## Problem and measured scope

The complete 8-core, 8-GiB AArch64 QEMU cold compilation in
`docs/profiling/full-build.md` took Starry 2298 seconds and Linux 735 seconds.
`ax_runtime::preempt::exit_preemption` is the largest resolved kernel CPU leaf:
6405 of 77339 active samples (8.28%). Callers include allocation, IRQ-context
queries, atomic-context diagnostics, task identity and COW page operations.
Page-fault stacks include 17.90% of active samples. Ext4 remains a separate
wait bottleneck; its 3.43% inclusive CPU share does not explain this CPU leaf.

Important sampling limitation, checked against the complete baseline while the
candidate was running: 6278/6405 exit-preemption samples (98.02%) land at
`ffffffff802bee88`, the instruction immediately after `msr DAIFClr, #2`.
This is an IRQ-delivery boundary, so the 8.28% is not measured function self
time. Some samples represent delayed work done with IRQs masked. Removing the
common IRQ mask can redistribute samples without saving the same amount of
CPU time. The decision remains a testable hot-path simplification, not proof
that this function itself is the largest time consumer or an 8.28% saving.
Only the complete end-to-end comparison can establish a benefit.

Every old guard exit masks IRQs, validates a CPU pin and owner handoff, reads
task policy, updates the pending mirror, and only then decrements the depth.
Those operations are performed even for nested guards and no-work exits.

The users are all ArceOS runtime preemption guards, including Starry allocation
and memory-management paths. Success requires eliminating that repeated work
without losing scheduler requests, IRQ-return semantics or migrated switch
ownership, and reducing the measured complete workload time. Performance is
not established by code size or unit tests. No syscall ABI, scheduler policy,
workload, rootfs tg-xtask, compiler or profiling frequency changes are intended.

## Prior art and alternatives

Internal baseline: `8386094e6` established the scheduler-frame baton;
`7d4fc2723` established the scheduler-neutral CPU-local boundary. Their linear
tokens, first-entry completion and runtime baton must remain. No second current
pointer or replacement task system is needed.

Linux source at `980ab36ae5972c83f683b939e50c469c4947229e`:

- `kernel/sched/core.c::__resched_curr` publishes the local preemption request
  when policy requests a reschedule, and uses an IPI for a remote CPU.
- `arch/arm64/include/asm/preempt.h::__preempt_count_dec_and_test` separates
  the usual decrement from scheduling. Linux's count/flag representation and
  non-atomic operations are not copied: Rust retains the existing atomic word
  and the runtime's depth-one baton reservation.

Keeping the old adapter leaves the measured common cost. Optimizing individual
allocator/COW call sites misses other users. Removing IRQ and owner protection
without changing request publication can lose a pending request. A new scheduler
or a second per-CPU task pointer would duplicate an established boundary.
The selected change extends the existing runtime capability: publish requests
on events, complete ordinary tokens directly, and reserve IRQ masking and baton
work for a final pending exit. Full source inspection was performed locally;
the attempted raw GitHub URL returned `Cache miss` and is not evidence of a
remote source fetch. No PR or merge is submitted by this change.

## Ownership and ordering

1. Task policy owns normal and forced reschedule flags. Only current-task
   request operations set them. An IRQ-save scope spans current selection,
   policy publication and the runtime pending notification. Remote requests
   still arrive through the existing target-CPU IPI handler.
2. The runtime translates that notification to the architecture-selected
   pending bit. Clearing policy for an incoming task remains distinct from
   current publication. Scheduler completion and first entry reconcile the
   selected pending bit after current has changed, while IRQs remain disabled.
3. `cpu-local` consumes a linear token. Context-owned architectures use the
   captured owner, which survives migration. CPU-owned x86 resolves its current
   anchor before consuming the equivalent switch depth: a migrated suspended
   guard has inherited that depth on the destination CPU. No old-CPU state is
   read or modified. This operation introduces no new current-state source.
4. A nested or non-pending final exit performs the atomic depth transition and
   returns without selecting a CPU pin or invoking task policy. Compiler fences
   keep protected operations inside the preemption boundary. This word does
   not publish cross-CPU protected data; actual locks retain their ordering.
5. A final pending exit retains depth one. An intervening IRQ can nest but
   cannot schedule this context. The runtime masks IRQs, claims its baton when
   the origin permits scheduling, releases depth, clears the pending mirror,
   and invokes task policy. An ordinary IRQ-disabled exit defers scheduling;
   the final IRQ-return exit is explicitly allowed to schedule.

The atomic compare-exchange in the depth transition preserves a request that
arrives between observation and decrement. A request arriving after a final
non-pending decrement is handled at that IRQ's return boundary. No callback or
wakeup occurs while a broad filesystem or allocator lock is owned by this
adapter. A released token must never be used again, including after a possible
context switch immediately following depth zero.

## Validation and rollout

Before any test starts, complete three static rounds: ownership/request-site
audit; formatting, relevant clippy and feature/target checks; final diff,
generated code, regression coverage and frozen input audit. New findings reset
the affected final review gates. Then run real CPU-local state tests and the
runtime scheduler-frame, nested-request, IRQ-disabled and migration regressions.
Use the existing `cargo xtask` runners, not a new perf facility.

The prior full run also exposed lost ext4 directory extent metadata accounting.
Add its deterministic regression before repairing production code; run it RED
only after the static gates, then fix and repeat final static gates before
GREEN. The failed disk stays read-only. Filesystem correctness is required for
accepting the next performance measurement.

Finally build the new Starry kernel and run the original cold compilation to
natural completion, using the same repaired QEMU as both baselines. Save raw
profiling, output hashes, compile identities, exit status and read-only fsck.
No compile-count or duration cutoff is allowed. The previous kernel and cold
bases remain available for rollback; neither the ended measurement disks nor
guest compiler inputs are rewritten. Until these checks pass, the refactor is
an unvalidated performance candidate, not a measured improvement.
