# Anonymous zero backing and first-write ownership

## Problem, evidence and scope

The complete 1694 s compilation has 2691/56992 active samples in fresh-page
clearing. 2649 exact PCs are the DC ZVA loop, and 2654 samples follow the
unlocked private-fault preparation path. Existing anonymous preparation always
allocates and clears a private page even for a read. The profile does not
record read/write fault counts, so it does not prove what fraction can be
avoided. The new complete run, not the leaf ranking alone, decides acceptance.

Introduce immutable anonymous zero backing for private 4 KiB non-write faults.
First writes allocate a zeroed private frame. Apply the same ownership model
to unlocked faults, locked population, kernel copies, fork, rollback, unmap,
discard, protection and resident accounting. File mappings, shared anonymous
memory, huge pages, generic memset, cache-zero instructions, disk formats,
syscall argument validation and architecture PTE encodings are non-goals.

This is high risk: physical sharing, COW, concurrency and observable RSS change.
This document precedes implementation. No merge or external submission is
authorized; independent maintainer approval remains necessary before merging.

## Prior art and alternatives

- Linux local commit `980ab36ae5972c83f683b939e50c469c4947229e`,
  [anonymous faults](https://github.com/torvalds/linux/blob/980ab36ae5972c83f683b939e50c469c4947229e/mm/memory.c#L5287):
  non-write faults install the special zero PFN, with no private RSS charge.
  [write faults](https://github.com/torvalds/linux/blob/980ab36ae5972c83f683b939e50c469c4947229e/mm/memory.c#L3853):
  zero backing requests a cleared allocation, skips copying the old page,
  revalidates the original PTE, invalidates before replacement, then charges
  anonymous RSS. These exact local functions were read; a web fetch of the
  fixed raw source returned Internal Error and is not source evidence.
- [mmap(2)](https://man7.org/linux/man-pages/man2/mmap.2.html) requires zero
  contents for anonymous mappings and private-write isolation. Storage of the
  initial zero contents need not be a separate physical allocation per VA.
- Linux arm64 clear_page.S already uses DC ZVA on this CPU class, as does the
  existing Starry implementation. MOSS scheduler-optimization worktree at
  `5a54e4413c9657bfb531cc32f6d688090465200d`, full
  `docs/profiling/riscv-buildstorm-zicboz-zero-20260725/zicboz-zero-report.md`,
  confirms the previous narrowly owned clearing optimization and rejects a
  global memset replacement. Do not copy that rejected global experiment.
- Internal search found no shared zero backing. COW history includes #1991
  refcount width, #1992 anonymous protection and #2096 clone rollback; preserve
  these contracts. Open-PR exact "zero page" search returned #1573 sysfs
  topology/meminfo, not a competing COW implementation. This is scoped discovery,
  not an all-PR merge review.

Keeping eager clear retains the measured work. Loop unrolling cannot avoid
the clear and already matches Linux's primitive. Background pre-zeroing would
move work rather than remove it, add worker/pool pressure and require new
reclaim policy. Anonymous read-zero sharing reuses the existing COW transaction
boundary and eliminates both allocation and clearing for pages never written;
read-then-write pages pay an extra fault. Retain it only if full-workload
correctness and performance justify that tradeoff.

## Ownership and transitions

Use one immutable, page-aligned, exactly 4 KiB zero object in the kernel image.
Its type has no writable API and its full lifetime is the kernel's. Runtime
HAL translation resolves the relocated image address; no guessed direct-map
subtraction, allocation, once lock or CPU-dependent initialization is needed.
Audit the final ELF object's alignment/size/section and actual QEMU physical
mapping. The normal RAM attributes match ordinary anonymous pages. The object
must never be passed to the allocator's free operation.

The private COW layer owns the distinction between immutable zero backing and
counted allocated backing. It must not add a page-table flag or generic-engine
exception. A typed frame reference centralizes lookup, cloning and retirement;
zero references have no mutable refcount or RSS charge. Ordinary frame owners
keep their existing count, source pin, overflow checks and release ordering.

| Transition | Backing and accounting |
| --- | --- |
| Private 4 KiB read/execute fault | Zero backing, PTE without WRITE, no RSS charge |
| Missing write fault | Cleared private allocation, original VMA permissions, one Anon charge |
| Zero-backed write fault | Retain zero source identity; clear private destination unlocked; revalidate; break-before-make replacement; charge once |
| Fork of zero backing | Share immutable page read-only; no frame count or RSS charge |
| mprotect(+W) | Keep COW PTE read-only; never make the zero object writable |
| Forced kernel write | Resolve backing under exclusive aspace lock; replace zero first, even if user VMA stays read-only |
| Unmap/discard/clone rollback | Complete normal PTE retirement; drop only counted frames and their charges |
| mremap | Move the PTE using existing completed removal; no nonexistent charge to move |

The prepared missing result is either zero backing or an unpublished allocated
frame. Only successful install transfers ownership; stale/error results cannot
leak a private allocation. A zero source is permanently pinned, whereas a
counted source acquires an additional physical reference before dropping the
aspace lock. First-write errors keep the original read-only zero PTE and no
RSS charge. A competing valid winner supersedes stale preparation/errors as
before. No global frame-index or frame IRQ lock spans zero/copy or file I/O
in unlocked preparation; locked fallback retains its established contract.

## Validation and acceptance

Complete the whole implementation and tests, then three final static rounds:
ownership/caller audit; fmt, strict Starry clippy, exact profile and build-only
axtest/C-test builds; final source/archive/ELF/test-discovery audit. No runtime
test starts before all three rounds pass. This is a new optimization, not a
claimed repair of anonymous zero-content correctness; do not invent an old
failing correctness run.

Kernel tests use real address spaces and allocation-failure/interleaving hooks:
zero reads share one physical page with zero RSS and no indexed frames; first
write isolates bytes and charges once; fork and kernel forced writes preserve
sibling zero data; protect/discard/move/unmap and clone rollback preserve
ownership; zero-write OOM, duplicate and VMA replacement retain the winner or
leave the old page unchanged. Whole-page byte checks and frame-index restoration
are required, not source-text tests. Keep all existing private/file/retirement
tests. A userspace system case covers raw mmap/mprotect/madvise/mremap/mincore,
fork isolation and kernel copy-to-user; run the same program on Linux.

Then run 8c8g kernel/system QEMU tests and one complete cold frozen tg-xtask
compile. Require 181 matching units, final ELF equality, six raw hashes and
read-only fsck. Compare 1694 s and Linux 735 s, preserving the old disk/source/
ELF. A zero-leaf reduction with a slower full build is not a speed acceptance.
Rollback only this backing model if disproved, preserving earlier dirty work.

RSS exposure through procfs/getrusage is intentionally closer to Linux: shared
zero backing is not a privately resident page. Tests at the exact accounting
boundary establish counts; noisy whole-process RSS deltas are not deterministic.
Full syscall compatibility is not claimed by these focused checks; the affected
entrypoints must retain their existing validation and failure priority.
