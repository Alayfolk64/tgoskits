# Anonymous zero backing: complete 1676 s run

## Current status

The complete candidate run finishes naturally in **1676 s**, 181 units / 175
distinct crates, workload and QEMU exit 0. This is 18 s (1.06257%) below the
1694 s comparison, but does **not** establish a stable speedup. Clearing did
not shrink in the sampled profile. Keep the candidate evidence and do not
promote this single small difference as an independently accepted optimization.
The 1694 s run remains the stronger preceding comparison; Linux is 735 s
(this candidate is 2.28027 times its wall time).

The complete prior run is 1694 s with 181 compile units. Its largest resolved
kernel CPU leaf is `try_zero_page`: 2691/56992 active samples (4.72172%),
2649 exactly on DC ZVA. This candidate changes anonymous backing ownership,
not the zeroing instruction or ext4. See
[`anonymous-zero-backing` design](../design/anonymous-zero-backing.md).
Its measured result and limitations are recorded below.

Private 4 KiB non-write faults use one immutable zero page; first user or
kernel writes acquire private zeroed backing. Typed backing references cover
fault transactions, population, fork, rollback, unmap and RSS. Files, shared
anonymous mappings, huge pages and syscall argument validation are unchanged.

## Three final static rounds, before any runtime test

1. Ownership and caller audit: read both fault paths, physical-alias kernel
   writes, clone/protection/rollback, completed retirement, accounting and
   existing/new tests. Zero backing never enters the allocator frame index;
   both write paths replace it before a store. Existing counted source pins
   and break-before-make replacement remain in use. Whole-page/ownership and
   deterministic OOM/interleaving checks cover the new state transitions.
2. Formatting/build/lint: `cargo fmt --package starry-kernel --check` and
   `git diff --check` exit 0. `cargo xtask clippy --package starry-kernel`
   completes 110/110 strict configurations, exit 0 at 21:59:24 on 2026-09-11.
   The actual axtest target also passes native matched strict clippy, exit 0;
   native build-only Cargo is necessary because ktest has no build-only mode.
   Formal profiling and axtest builds exit 0. Host CMake and static AArch64
   compilation of the C system case pass `-Wall -Wextra -Werror`; neither
   executable was run during this round. Upstream core/memchr future-version
   diagnostics remain visible; no warning suppression was added.
3. Final source/image/discovery audit: archive comparison, final fmt/diff and
   both kernel hashes pass. The actual axtest ELF contains 54 descriptors,
   including the new ownership test. Explicit system subcase discovery exits
   0 and selects the system wrapper. Formal ELF is ET_DYN without PT_TLS.
   Its zero object is exactly 4096 aligned bytes at `0xffffffff80566000`,
   file offset `0x576000`; all 4096 file bytes were checked as zero. Actual
   machine code selects the no-allocation path, removes user WRITE, translates
   the image address through HAL, and skips its private RSS charge. The locked
   first-write path calls a cleared allocation. The kernel LOAD is inherited
   RWE: this is Rust-immutable backing and read-only user mapping, not a claim
   of hardware-enforced kernel image read-only protection.

All three rounds completed before the first runtime test (22:04, 2026-09-11).
Runtime tests and the complete cold compile are pending at this checkpoint.
QEMU absence check returned status 1 with only the `PID ELAPSED COMMAND` header:
there was no matching process, not a failed guest. Earlier discovery-log rg
returned 1 with no output because the command prints a tree, not a flat path;
explicit discovery above succeeds. These are retained diagnostic statuses.

Static logs and disassembly: `tmp/preemption-refactor.5NL1NQ/zero-*`.
Frozen candidate directory:
`target/profiling/arceos-helloworld/starry/anonymous-zero-backing/`.

| Artifact | SHA-256 |
| --- | --- |
| Formal ELF | `9d9c3443ab50240ddf0bc71836ba2d30aa36d33f13b3d8ceb861ca86fb9ac22d` |
| Formal BIN | `2028a25eb1af27daaa8cba3e878db5550a2f9c2f456a760dc70a88e4f348fab7` |
| Source archive | `d90e7be18e6252937c0aa421951e49fbebfda661fc63647c4097ba32d8e42a28` |
| Build-only axtest ELF | `c03688c98bb2dc2ce9df8902e3b89e0cc5a2ceaacb58f87129ed24ee6ce7504c` |
| Static AArch64 C fixture | `bed7022020e1d8e90334883d3aca9c6d89a779b6500dd1e3e84268af15746f9e` |

The source archive captures the modified MM and adjacent frozen profiling
inputs, not a clean repository commit. Baseline HEAD remains
`affddc3fecec02b31df94573063e4840433c9ebe` on the dirty profiling branch.
No project commits or external submissions were made. Full Linux syscall ABI
compatibility and runtime physical translation are not established by static
checks; focused QEMU behavior validation follows.

## Runtime gates and frozen formal input

After the three static rounds, `cargo xtask ktest qemu -p starry-kernel
--test axtest_kernel --arch aarch64` exits 0: 54/54, fail 0, skip 0. The new
zero test is case 3 and passes all internal scenarios. Its QEMU is 8c8g,
Cortex-A53, default virt/GICv2. `cargo xtask starry test qemu --arch aarch64
-c qemu/test-anonymous-zero-backing` exits 0: exactly one selected system
binary, 0.055 s, grouped pass; this fixture uses 8c8g/GICv3 plus system devices.
These test configurations are not identical to each other.

The same runner-produced musl binary is extracted from the injected cache
image, SHA `e945e3fbf6c18e27f0e823b9cd3db406e035cbcb26677def95aadf1902b4ffa1`,
then installed into an independent Linux disk copy. Linux 6.18.35-0-virt on
the frozen profiling hardware prints `ANONYMOUS_ZERO_BACKING_PASSED`, returns
0, and reports that same SHA. Linux source prior art in the design is a
separately pinned newer source tree, not the runtime kernel version. Linux
root is remounted read-only before shutdown. Its regression directory is
`target/profiling/arceos-helloworld/linux/anonymous-zero-regression/`.

The old BusyBox image path was a legacy directory. Before the current image
tool could replace it, it was moved intact to
`tmp/axbuild/rootfs/rootfs-aarch64-busybox.legacy-directory`; no old contents
were deleted. The system runner then prepared its regular-file image.
Injection prints four original `rm: File not found by ext2_lookup while
trying to resolve filename` diagnostics because the existing overlay tool
unconditionally removes targets before writing new files. All four raw
diagnostics were displayed; injection and actual test pass, not just its
debugfs exit status. The runner cleans its build directory; the retained
cache image supplied the exact binary for Linux, explaining the empty
post-test filename searches.

The formal cold working copy's full SHA before runner replacement is
`9968004595dec398480417e210d64f9ed509801d4325d4ab22f43951983f5ea3`, matching
the preserved base. The frozen full-build runner SHA is
`c94b86694e6f7e8b2c99b84c7c79ca2b1626323526add691d1636a238cd6f392`;
offline dump/cmp and read-only fsck pass. Prepared disk SHA is
`42457f76d70f7ec49a393530720bc4b6f65dfcf3587462bb1d72b58ac73e17be`.
Formal ELF/BIN 2/2 and source archive comparison pass again after the tests.
All builds, regressions and full-disk checks must end before formal timing.
The QEMU binary SHA remains
`a1d9a6080e1f025038a81749baa9d9552ae89321ea163abbf704206034f0a6a4`.
The next complete run retains the 8c8g/TCG/GICv3/NVMe hardware and original
guest workload; no new performance result exists at this checkpoint.

## Complete-run integrity

The run booted at 22:09:36 on 2026-09-11 (QEMU PID 1045638, execution session
15990), reached the full-build marker with 8 logical CPUs, affinity 0-7,
source/tg-xtask reuse true, and naturally exited 0. No debugger was connected,
no cutoff was applied, and no source change/build/full-disk hashing ran in
the measurement window. The guest interval is 1676515593328 ns at 10 Hz;
CPU/wait/pending dropped and skipped CPU counters are all zero.

- 22852 raw records; six raw SHA checks pass.
- The full sorted 181 name/version list matches seven saved comparisons,
  including Linux, 1694 s, 1869 s and 1974 s runs.
- Final ArceOS ELF is byte-identical to Linux, 1694 s and 1869 s. SHA:
  `2792c35275d91f57a38a847be87c4d2b333d7b88cd548bf9e06633288c7dba21`.
- Read-only fsck exits 0; no disk repair. Direct per-file debugfs dumps avoid
  the earlier rdump ownership warnings. Original kernel ELF/BIN hashes pass
  2/2 again after the run.
- Ended rootfs moved to the candidate directory and made read-only, SHA
  `8729f23cb4eb0e32ffefa74fb4c87d190847b8feb672714d0e663d746399f281`.
  The fixed working-disk path is absent; the next measurement needs a new
  cold copy, not this completed disk.
- All nonempty event SVGs rendered successfully. CPU-active, mutex-wait and
  ext4-lock-hold PNGs were rendered with the existing `tmp/profile-svg-env`
  environment and actually inspected. Renderer session 55840 and end-disk
  hash session 21647 both exit 0.

## Complete hotspot results

CPU: 56874 active / 130028 total samples = **43.73981%**, 73154 idle.
The prior run was 43.35938%; low CPU utilization is not solved.

| Active CPU leaf | Samples | Active share |
| --- | ---: | ---: |
| User space (not a kernel function) | 32014 | 56.28934% |
| `try_zero_page` | 2847 | 5.00580% |
| `memcpy` | 2563 | 4.50645% |
| `find_free_area` | 1951 | 3.43039% |
| `spin_release` | 1832 | 3.22116% |
| `flush_batch` | 1526 | 2.68312% |
| `FrameReference::share_mapping` | 1318 | 2.31740% |

The complete raw PC audit locates 2790/2847 clear samples at
`0xffffffff80381614` (DC ZVA), 52 at loop exit, five at entry/geometry.
2727 follow user fault -> `prepare_missing` -> cleared allocation (including
five inlined allocator frames); 88 follow the user-fault path without that
caller frame, and 32 use locked population. Source inspection shows the
private 4 KiB non-write branch cannot perform that clear; file preparation
uses an uninitialized allocation. The 2727 path therefore remains anonymous
write preparation, not redundant clearing of file-read buffers. No raw
read/write fault counter exists, so these are sampled path attributions,
not counts or a population-wide read/write ratio.

`memcpy` has 2097 samples from `CachedRead::read_page`, 2037 specifically on
the private file-fault path through `FileBackend::read_buf_at` and
`prepare_missing`. The largest exact memcpy PC is `0xffffffff80541f10`
(2397 samples overall). The whole user page-fault chain has 8743 active
samples (15.37258%); it includes clear, copy, allocation and mapping work.
`find_free_area` mostly follows mmap placement (1945/1951).
1316/1318 share-mapping samples have exact PC `0xffffffff80237c64`, the branch
immediately after `msr daifclr, #2` at `0xffffffff80237c60`. This is an IRQ
reenable sampling boundary, not evidence of slow refcount arithmetic.

Wait values below are overlapping cross-task totals, not additive wall time:

| Event | Total ns |
| --- | ---: |
| mutex wait | 2434642484224 |
| page-cache inclusive | 2362741774336 |
| ext4 inclusive | 2149626489600 |
| ext4 lock wait | 1398801779808 |
| block read | 754123205872 |
| block flush | 537395617952 |
| ext4 lock hold | 486435335584 |
| block write | 114744554640 |
| off CPU | 88080661492352 |

The largest mutex-wait caller remains `FileBackend::read_buf_at`:
771263158752 ns (31.67870%), followed by ext4 lookup 365478170992 ns
(15.01157%). Within ext4 hold, lookup is 222462743072 ns (45.73326%) and
write 129163626400 ns (26.55309%). Ext4 remains important waiting work;
the evidence does not make it the largest resolved CPU leaf or establish
that replacing ext4 alone will solve the workload.

The immutable zero path is functionally validated, but cannot remove clears
required by first writes. Further work must address the actual remaining
write-preparation / private-file-fault ownership and cache-wait boundaries,
not repeat a global memset change or infer a major speedup from this run.

Postprocessing failures were retained: an initial Python summary command
missed a closing brace (SyntaxError, exit 1), then succeeded after correction;
`rsvg-convert` was absent, and system Python lacked cairosvg. The existing
project-local rendering environment was found and all three PNG commands
passed. An rg over old logs had no matches (status 1, empty output). None of
these failures damaged or changed the measured guest or raw artifacts.
