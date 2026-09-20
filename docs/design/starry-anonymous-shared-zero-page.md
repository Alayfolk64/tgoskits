# StarryOS Anonymous Shared Zero Page

## Problem

The complete Orange Pi self-build profile showed anonymous/COW page-fault
materialization as the largest CPU cost not attributable to the filesystem
lock convoys already removed. `GlobalAllocator::{alloc,alloc_pages,dealloc}`
accounted for 2,293 leaf samples, while zeroing and
`CowBackend::alloc_new_frame_sized` were also prominent.

StarryOS allocated and zeroed a private physical page for every first read of
anonymous memory. A writable VMA also received a writable PTE immediately, so
the system paid the allocation cost even when the page was never written.

## Users, success criteria, and non-goals

Anonymous private mappings are the direct users. A successful change must:

- map distinct anonymous 4 KiB read faults to one immutable zero frame;
- install read-only PTEs even when the VMA permits writes;
- allocate a private zeroed page on the first write fault without copying the
  shared frame;
- keep the shared frame out of anonymous RSS accounting;
- preserve reverse mappings, fork, unmap, rollback, and TLB retirement;
- never expose write permission to the shared frame.

This change does not introduce a shared 2 MiB huge zero page. Transparent huge
faults retain their existing allocation and fallback policy.

## Linux prior art

The reference is Linux commit
[`93f51579e7df248780214094418f205253383cc5`](https://github.com/torvalds/linux/commit/93f51579e7df248780214094418f205253383cc5),
retrieved on 2026-09-21. In
[`do_anonymous_page()`](https://github.com/torvalds/linux/blob/93f51579e7df248780214094418f205253383cc5/mm/memory.c#L5427-L5469),
Linux installs a special `ZERO_PAGE` PTE for an anonymous read fault instead of
allocating private storage. A later write enters the write-protect fault path
and allocates private anonymous storage. Linux also documents shared zero
folios as special mappings whose refcount and mapcount are not adjusted for
each user PTE in
[`__vm_normal_page()`](https://github.com/torvalds/linux/blob/93f51579e7df248780214094418f205253383cc5/mm/memory.c#L699-L708).

## Design

A fallible once-initialized `PageObject` owns one zeroed base-page frame for the
kernel lifetime. Anonymous 4 KiB read faults clone that typed owner rather than
allocating a frame. Address-space mapping slots retain the typed owner, but the
special immutable page does not publish per-PTE reverse maps or enter a
mapping-local physical-frame index. Tracking those entries would create one
global lock and an unbounded vector for a page that is never reclaimed.

Provider publication recognizes the global page as already complete. Publish,
cancel, identity restoration, and pending-index discard are no-ops for that
provider; physical lookup resolves its initialized frame explicitly. This
keeps rollback able to recover the typed owner without fabricating ownership
from a raw physical address.

All private read faults are installed without write permission. On a zero-page
write fault, exclusive-mapping reuse is forbidden: the backend allocates an
already-zeroed private page and remaps the PTE. Ordinary private and cached-file
pages retain their existing reuse/copy rules.

## User-visible compatibility

The `mmap` and `brk` contracts remain unchanged: newly faulted anonymous bytes
read as zero, and a permitted store succeeds after the internal COW fault. A
forked process initially observes the same immutable zero contents, while the
first writer receives an isolated page. `mprotect` permissions remain VMA
permissions; keeping the hardware PTE read-only is an internal mechanism and
does not reject a permitted write. Anonymous RSS no longer charges the shared
kernel-owned frame, matching Linux's special-page accounting. No syscall
number, argument, return value, errno, or structure layout changes.

## Alternatives

- Per-CPU page caches reduce buddy-lock traffic but still allocate, clear,
  index, account, and eventually free one frame for every untouched read page.
- Mapping a raw zero PFN would avoid allocation but violate StarryOS typed
  frame ownership and rollback invariants.
- A huge shared zero page could cover 2 MiB faults but adds split, accounting,
  and architecture policy that is not required for the measured 4 KiB path.

## Validation

The deterministic regression was added before the implementation and failed
because two anonymous read faults received distinct private pages. The final
coverage proves shared read-only mapping, zero RSS and rmap growth, fork
sharing, first-write isolation, split-THP discard/refault, receipt accounting,
and rollback/unmap cleanup.

The final tree passed these gates:

- `git diff --check`: exit status 0;
- `cargo fmt --all -- --check`: exit status 0;
- `cargo xtask clippy --package starry-kernel`: 104/104 target and feature
  checks passed;
- `TMPDIR=/home/wuxun/Projects/tgoskits-orangepi-profile/tmp cargo xtask ktest qemu -p starry-kernel --arch aarch64 --qemu-config tmp/qemu-aarch64-8c8g-axtest.toml`:
  `AXTEST_SUMMARY pass=206 fail=0 skip=0 total=206` on an 8-vCPU, 8-GiB QEMU
  guest.
