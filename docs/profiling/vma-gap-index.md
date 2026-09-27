# VMA gap-index refactor: evidence and validation

## Complete measured result

The connected address-allocation refactor completed its full 8-core/8-GiB QEMU
compile in 1087 seconds, 181 units / 175 distinct crate names, exit 0. This is
166 seconds (13.248%) less than the immediate 1253-second run and 130 seconds
(10.682%) less than the previous 1217-second best. Matched Linux remains 735
seconds; Starry/Linux ratio is 1.479. These are single-run, profiling-enabled
comparisons, not a statistical guarantee or exclusive attribution of every
saved second. This candidate is retained as a measured local improvement.

After corrected final static gates, all 24 MemorySet tests plus one doc test
passed, and the actual 8-core/8-GiB QEMU kernel suite passed 55/55, no skip,
exit 0. Complete performance evidence and remaining hotspots are below.

Input hotspot is the complete 1253-second cache-writeback-ownership run:
MemorySet::find_free_area is the largest resolved kernel CPU leaf, 1979 samples
(4.954% of active samples); 1976 are inside the successor/gap scan. Largest
individual mutex wait remains ext4 mount state, 72.349 cumulative seconds.
These measure different costs. See [preceding full profile](cache-writeback-ownership.md)
and [design and Linux comparison](../design/vma-gap-index.md).

## Implementation

MemorySet keeps its authoritative BTreeMap of mappings. A private address-ordered
AVL index of maximal free gaps stores subtree maximum lengths. First-fit search
prunes undersized subtrees, preserving lowest fitting address and hint/base
ordering. An implicit tail keeps empty construction allocation-free. No public
trait, dependency, lock, unsafe block, scheduler or filesystem change is added.

Map, replacement, extension, unmap, metadata-only unmap and clear maintain the
same gap ownership. A scoped unmap guard reconciles only affected old-area bounds
from actual survivors on early error as well as success. Metadata partitioning
does not create false holes. Existing backend partial-failure behavior is not
silently changed or presented as repaired.

The search also enforces its documented upper bound before returning an early
gap and rejects alignment overflow. Zero-size/non-power-of-two alignment requests
return None explicitly at the Rust API. Existing Starry syscall validation and
fallback remain unchanged. This is not a complete Linux syscall compatibility
review. No Starry production code changed in this experiment.

## Deterministic RED and GREEN

Candidate was completed and archived before any runtime. Original MemorySet and
lib.rs were restored byte-for-byte (old set.rs at the new set/mod.rs path); new
gap/mutation modules were undeclared. New public regressions were unchanged.
Three RED static rounds passed before running original production.

- Bound regression failed: returned Some(12288), expected None for a request
  whose aligned start is already at the exclusive search upper bound. Exit 101.
- Cost regression failed: 4107 comparisons against the less-than-200 limit with
  4096 mappings, 0.01 seconds, exit 101. This calls the real production search,
  not a recreated arithmetic model or timing threshold.

Candidate MemorySet production/tests were restored exactly. The accepted full
run includes those unchanged cases, all passing. Separate private-index tests
verify that one subtree-summary visit rejects 4096 too-small holes; exhaustively
compare all 256 small occupancy layouts and their bounded aligned queries; check
AVL height/maximum summaries during permuted insertion/removal; and compare mixed
real map/unmap operations against independent byte coverage. Success, error,
replacement, split, coalescing, protection and overflow paths are covered.

`cargo test --package ax-memory-set` passed 8 unit + 8 gap integration + 4 range
integration + 4 existing std integration + 1 doc test. Full untruncated output
is saved. `cargo xtask ktest qemu -p starry-kernel --test axtest_kernel --arch aarch64`
passed all 55 cases on the final implementation. It used a separate test ELF,
8 cores, 8 GiB and NVMe snapshot mode, not the measured build image. Existing
injected allocation/I/O/shootdown errors were expected and their cases passed.

## Static checks, failures and corrected gate handling

Artifacts and complete command/output logs: `tmp/vma-gap-index.6UocCb/`.
Project MemorySet Clippy 1/1, strict MemorySet all-targets Clippy, actual AArch64
8-core guest-profile Starry Clippy and project profile kernel build passed.
Native all-targets/component Clippy is needed because inspected xtask base checks
do not include these integration tests. Host-only Starry test Clippy is not the
supported kernel axtest configuration.

Failures retained and printed in full when encountered:

1. New test integer loops had ambiguous types for is_multiple_of (E0689,
   exit 101); explicitly typed usize loops fixed it without allow.
2. Host Starry lib-test Clippy exited 101 on existing axtest cfg mismatch for
   open_sync_flags_reach_write_completion and unused COW frame imports. These
   unrelated paths were not patched or silenced. Only the newly added duplicate
   host placement test was removed, restoring placement.rs exactly. The actual
   MemorySet upper-bound regression remains and was executed. This supersedes
   the design's proposed extra host placement test; real kernel integration
   coverage uses the supported QEMU suite.
3. A lookup of an incorrectly named old green-test.log exited 2; actual artifact
   names were subsequently discovered. No data or source was changed by it.
4. The first final-artifact comparison returned 1 for both ELF and BIN. An
   orchestration mistake nevertheless ran three passing host GREEN cases and
   initially wrote a false artifact-match statement. This violated the intended
   pre-runtime gate; it was immediately disclosed, the log corrected, and those
   results excluded from final acceptance. No QEMU had started.

All 15056896 BIN bytes were subsequently compared: changes were confined to the
8-MiB kallsyms section; all other load-image bytes matched. Old and final images
are separately retained. Final ELF/BIN were saved and checked against the final
build and an independently derived BIN. The complete three static rounds were
repeated (ownership audit, strict Clippy/build, fmt/diff/full-source/artifact
identity) before the accepted full MemorySet tests and any QEMU runtime.
`corrected-static.log` and the per-command logs supersede the invalid first gate.
Tool orchestration now checks every exit code before permitting runtime.

## Exact inputs and acceptance boundary

Host branch profiling/manual-20260907, HEAD affddc3fecec02b31df94573063e4840433c9ebe;
unrelated dirty worktree changes preserved. Original source archive SHA-256:
`2e500b64fd1edc0e072a5600e5dd340317c33f62c689006d9a5fc7ec916af659`.
Initial completed candidate archive SHA-256:
`75ce2c57825e29bcaace6327b8c5a26f3ce9153890c319d23d590d4348b7f431`.
Final archive SHA-256 (21 regular files, not the whole dirty kernel):
`69eb2246b187782ae783a9a2dcb820bf368779328445df266228761ad14b898a`.
The only difference between candidate/final is removing the new host-only test;
all MemorySet production and regression source is unchanged.

Final profile ELF:
`e907be91d46a4c8cb1dfb741ac428bf7661ae65c769be5a62550a76d78d06e11`.
Final profile BIN:
`6508f6258113708b6fe6b33d2976bcf203cadf7d7fd54504bc4ba2406c5b74d6`.
14269 kallsyms. Earlier pre-final ELF/BIN are not measurement inputs.

The subsequent acceptance run used a full 181-unit build on a fresh copy of the same frozen
rootfs, reused tg-xtask and source, same direct kernel sampler, 8c8g/NVMe. Compare
all output bytes/compilation units and full CPU/wait profiles, not only elapsed
time. Added gap-node allocation/mutation cost and alignment-heavy worst cases
remain real tradeoffs. Full results below establish local measured improvement,
not merge readiness or an independently reviewed design approval.

## Fresh measurement preparation

Removed only the old fresh-page-zero-window raw image after a complete 16-GiB
byte comparison with its existing zstd backup and an unused-file check. Backup
remains read-only and fully recoverable; cleanup.log records both hashes.
An initial metadata-stability assertion included atime and failed after the byte
comparison, with no deletion. The repeated full check explicitly compared
dev/ino/size/mtime/ctime and passed. It cannot retroactively prove which field
changed in the initial attempt, because those values were not printed then.
Space rose from 3.7 to 9.1 GiB before creating the fresh working copy.

The new run directory is
`target/profiling/arceos-helloworld/starry/vma-gap-index`; final kernel and source
archive are saved there. Full base-copy hash matched the frozen base; readonly
fsck before and after runner replacement returned 0. Guest runner is 8657 bytes,
0755, byte-identical; guest tg-xtask retains the matched hash and executable mode.
Prepared rootfs SHA-256:
`dee720b3ef765b91c83e58cdc8f79cf05da4b7bde73477c61e6b04a0600a2d7f`.

The full measurement used QEMU PID 1304938, boot 2026-09-12 19:44:46 UTC,
terminal 98551. Both are now terminal, exit 0. After observing the shell prompt, the runner was invoked once;
the send call is bounded by 19:45:08..19:45:09 UTC, not an exact guest workload
start timestamp. There was no timeout, concurrent host build, live-image
inspection or runtime-input edit during measurement.

## Full-build integrity and profile

Cargo finished release in 17m49s; axbuild reports 1080.09 seconds. Outer guest
measurement is 1087 seconds, direct kernel interval 1086937426352 ns. The
workload's generated ArceOS SMP=1 setting is unchanged; the compiling Starry
guest itself has 8 CPUs / 8 GiB, all CPUs visible in the profile.

- All five compilation name/version multisets match: this run, cache-writeback,
  namespace-journal-progress, inode-read-sharing and Linux, each 181 units.
- Actual final ELF compares byte-for-byte with previous Starry and Linux:
  SHA-256 `2792c35275d91f57a38a847be87c4d2b333d7b88cd548bf9e06633288c7dba21`.
- Eight individual debugfs exports returned 0 without ownership errors;
  all six guest manifest hashes passed. No rdump was used.
- All 9807 CPU rows and 16180 wait rows were parsed: 25987 records, one phase,
  nine event definitions. All folded/summary totals agree with the full raw
  input, every CPU is represented, and all four dropped/skipped counters are 0.
- All 49 startup exception signatures and multiplicities match the preceding
  successful run, including IP/FAR/kind/ESR/EC/ISS. No warning was suppressed.
- After QEMU exited and the image was unused, readonly fsck returned 0. Its
  extent-tree narrowing suggestions were declined; no repair was performed.
- The ended disk is read-only `rootfs.img` in the run directory, SHA-256
  `491373c5aa9895d7084dbe625ad4361ffa5e00e3dc8ca1efa50c9452b86b1c69`.
- All 21 selected final source files are unchanged; saved ELF/BIN/archive
  hashes match their prelaunch identities. The working rootfs path is now absent.
- Renderer returned 0 and generated 11 SVGs. Complete active-CPU, mutex-wait,
  ext4-hold and block-read PNGs were actually viewed.

Logs are `integrity.json`, `integrity.log`, `comparison.log`, `hot-pcs.log`,
`hot-asm.log`, `hot-callers.log`, `ended-fsck.log`, `ended-hash.log`, `export.log`
and `output-identity.log` under the run directory.

| Metric | Previous cache-writeback | VMA gap index |
| --- | ---: | ---: |
| Whole build | 1253 s | 1087 s |
| Active / all CPU samples | 39951 / 97353 | 38245 / 84247 |
| Active fraction | 41.037% | 45.396% |
| User-space samples | 22888 | 22704 |
| Aggregate mutex wait | 157.914 s | 161.305 s |
| Ext4 mount mutex leaf wait | 72.349 s | 82.944 s |
| Dedicated ext4 mount wait | 72.922 s | 83.614 s |
| Ext4 mount hold | 57.443 s | 64.336 s |
| Block reads | 689.171 s | 654.572 s |
| Block writes | 95.936 s | 140.366 s |
| Block flushes | 124.996 s | 203.720 s |

Wait and I/O totals overlap across tasks and categories; they are not wall
times, and their differences cannot be summed into predicted savings. This
refactor improves the full build without claiming to have solved ext4 locking
or low utilization. Activity is still only 45.4% across 8 CPUs on this workload.

Old first-fit-inclusive stacks contain 1990 samples, of which 1979 are the
linear search leaf. In the new profile, the gap search leaf has 14 samples.
Including *all* stacks containing either first-fit or gap-index code, including
new mutation, allocation and lock-release costs, gives 246 samples. This wider
candidate measure avoids hiding the added index-maintenance cost; it is not a
claim that old mutation costs were zero or that each sample is exclusive time.

## Remaining hotspot and next investigation

The largest resolved kernel CPU leaf is spin_release: 1758 samples (4.597% of
active). All 1758 PCs are immediately after `msr daifclr,#2`, at
0xffffffff8034f428. Likewise 1003/1007 share_mapping samples are immediately
after IRQ enable at 0xffffffff800fb9f0. Delayed IRQ delivery prevents treating
these widths as exclusive execution time in those leaf instructions.

The next large directly located operation is TLB invalidation, the CPU's cached
address translations: flush_batch has 1286 samples (3.363%). 1202 PCs are just
after per-address `tlbi vaae1is`, 81 just after full `vmalle1is`; 1141 samples
(88.725% of the leaf) come through COW unmap retirement. can_access_range still
has 968 samples (2.531%) in a full VMA-prefix scan on user-buffer validation.
Neither is an ext4 CPU leaf. Largest individual mutex wait remains ext4 mount
state (82.944 s); its biggest hold caller is create (17.114 s, 26.602% of hold).

Next investigation is operation-scoped unmap/TLB retirement versus Linux
mmu_gather, including whether current per-area retirement repeatedly ends small
batches. Do not weaken remote invalidation, free frames before completion, or
mistake IRQ-boundary samples for proof that unlocking should be removed. No
next-candidate runtime or throughput result is implied by this measurement.

Pinned Linux comparison remains 980ab36ae5972c83f683b939e50c469c4947229e:
mm/mmap.c exit_mmap uses tlb_gather_mmu_fullmm across unmap_vmas/free_pgtables
before tlb_finish_mmu and backend/VMA teardown. include/asm-generic/tlb.h
explicitly requires unhook -> invalidate -> free. Its tlb_end_vma normally
flushes at a VMA boundary; fullmm or CONFIG_MMU_GATHER_MERGE_VMAS skips that
boundary. Therefore "Linux always batches across VMAs" would be incorrect.
The [Linux TLB interface documentation](https://docs.kernel.org/core-api/cachetlb.html)
also distinguishes page, range and whole-mm operations and their SMP effects.
The next design must establish the actual Starry operation/owner boundary and
architecture capability instead of copying the Linux fullmm flag blindly.
