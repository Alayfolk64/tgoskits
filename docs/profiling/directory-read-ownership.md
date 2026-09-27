# Directory read ownership: validation record

## Current result

The production refactor is connected and its complete 181-unit run passes at
**1252 s**, versus the preceding Starry run at **1323 s** and matching Linux run
at **735 s**. This single matched run saves 71 s / 5.3666%; it is not a statistical
repeatability claim. CPU active sampling rises from 38.0622% to 41.2991%, so low
CPU utilization remains unresolved. In the preceding run the largest mutex wait was
`Ext4Guard` (69.36%); lookup accounts for 33.63% of ext4 hold events, including
both inode-table and directory/HTree reads. These are overlapping sampled event
durations, not directly subtractable wall time. See the
[design and full baseline evidence](../design/directory-read-ownership.md).

The new path separates parent inode loading, selected-path mapping/directory
reads, validated child lifetime acquisition, and child inode loading. All physical
reads in these phases use independent endpoints outside the mount lock. Short
mount sections still own authoritative cache/journal visibility and publication.
Unsupported fork capability retains the serialized compatibility path; I/O and
allocation errors do not select fallback. Writes and readdir are not parallelized
by this change.

## Frozen input

- Workspace: `/home/wuxun/Projects/tgoskits`, branch `profiling/manual-20260907`.
- HEAD: `affddc3fecec02b31df94573063e4840433c9ebe`, with preserved worktree changes.
- Before-refactor snapshot: `tmp/directory-read-owner.dvQgri/sources-before.tar`,
  SHA-256 `f434e34e4ae7a945e055c8f810258c0b9d924710a32346b059bb65e8122e368a`.
- Final-static source snapshot: `tmp/directory-final-static.ORckrk/sources.tar`,
  SHA-256 `63b255d36d5e05eb710014d87486dc1a0c8a2249614ad96b843faf4c883fea14`.
  It covers the rsext4 source tree, ext4 adapter tree and VFS directory tree;
  it is not a clean commit or a whole-workspace source archive.
- Pinned toolchain: nightly-2026-07-15, repository formatting configuration.

## Three static rounds, before any runtime test

### 1. Ownership, concurrency and compatibility inspection

Completed for the connected read refactor on 2026-09-12:

- `InodeLifetime` is acquired with the authoritative result under mount state,
  moved into the wrapper without a second registration, and dropped outside
  mount/cache guards. The existing zero-link claim/reap protocol remains owner.
- Lookup retains admission, a parent reference and shared topology/directory
  guards. Create/link/non-directory unlink acquire topology shared then parent
  exclusive; rename/rmdir use topology exclusive. No path waits for these gates
  while holding mount state. Child metadata loading follows guard release.
- Waiter registration precedes its predicate recheck. Writer cancellation
  restores admission; reader/writer releases publish state before waking outside
  IRQ and mount exclusion. Existing operation admission drains reads at shutdown.
- Pending inode versions invalidate before mutation/reinitialization/eviction and
  on rollback. Current canonical records win over old bytes. Clean publication
  never writes a dirty victim; an all-dirty cache can decline residency and still
  return a validated inode. Directory completion checks origin/version before
  exposing a parse or I/O failure.
- Directory data uses cache then journal visibility; extent/indirect mapping uses
  journal visibility without publishing into the directory data cache. Immutable
  images survive cache eviction/checkpoint. Physical range, system-zone, depth,
  checksum, collision and fallback validation stays in the existing parsers.
- Compared the extent read traversal against the before-refactor archive, not
  only the much larger dirty diff against HEAD: the traversal/range selection is
  preserved and mutable JBD2 access is replaced by the read capability. Legacy
  pointer writes stay specialized on the mutable journal owner.
- VFS calls backend lookup outside its cache mutex, rechecks its mutation
  generation, preserves the winning positive owner, and drops losing owners
  outside the cache guard. Name validation, permission and symlink traversal
  remain in their existing callers; no syscall number/argument layout changes.

This is a scoped refactor inspection, not a full syscall ABI or merge review.
Path-based operations and metadata consumers (open/stat/access/exec, directory
mutation, metadata/xattr operations, Unix pathname sockets and mount traversal)
can reach the changed helper. The full per-syscall Linux compatibility matrix is
not claimed complete. Open issue #2351 was read: Unix bind/connect can call the
filesystem with preemption disabled; this existing caller defect is neither
fixed nor excused by the new blocking read boundary. Safe-point checks are not
relaxed. The open ext4 PR inventory was refreshed; #2015 remains at
`6d5cc09f45a073680a270ae0b6047b24fd9eaff5`. Its full semantic overlap and maintainer
approval remain requirements before merge, not evidence supplied here.

### 2. Strict compilation and the actual kernel consumer

All commands below completed with exit 0 after the final production edit:

- `cargo xtask clippy --package rsext4`: 3/3, finished 17:18:15 +0800.
- `cargo xtask clippy --package ax-fs-ng`: 8/8, finished 17:18:33 +0800.
- `cargo clippy --package rsext4 --lib --tests --features host-test,USE_MULTILEVEL_CACHE -- -D warnings`.
- `cargo clippy --package ax-fs-ng --lib --tests --features host-test,ext4,vfs,profile -- -D warnings`.
- `cargo xtask starry build --config apps/starry/macos-selfbuild/build-aarch64-unknown-none-softfloat.toml`:
  build stage 14.27 s, AArch64 release, SMP=8, NVMe, guest-profile, frame pointers.
  No QEMU or rootfs preparation was invoked.

The xtask Clippy matrix compiles separate features; `host-test` alone does not
enable the ext4 adapter tests. Its command generation and the `xtask test --help`
entry were inspected: targeted combined features are therefore checked with
native Cargo instead of claiming the matrix alone covers them. `--lib --tests`
compiles test code without executing it. Subsequent targeted runtime Cargo tests
are likewise needed because the std xtask entry has only a whole-whitelist or
`--since` selector, not a package/combined-feature selector.

The kernel build retains the existing future-incompatibility warning for pinned
Rust `core` and `memchr 2.8.3`; the original warning was displayed. No warning was
silenced with a new `allow`.

Commands use `TMPDIR=/home/wuxun/Projects/tgoskits/tmp`; adapter/kernel checks also
set `TGOS_IMAGE_DOWNLOAD_DIR=/home/wuxun/Projects/tgoskits/tmp/tgosimages` and
`AIC8800_FIRMWARE_DIR=/home/wuxun/Projects/tgoskits/tmp/ext4-static-firmware.pha7d1/firmware`.

### 3. Frozen source, formatting and artifact consistency

Completed with exit 0 before runtime validation:

- `cargo fmt --package rsext4 --package ax-fs-ng --package axfs-ng-vfs --check`.
- `git diff --check`.
- `tar -df tmp/directory-final-static.ORckrk/sources.tar`: archived source bytes
  and metadata still match the inspected files.
- Test module wiring and the actual combined-feature Clippy compilation include
  the new 13 core directory cases, 6 adapter physical inode/directory I/O cases,
  namespace gate tests and existing inode-load/cache/lifetime tests.
- Independently converted the finished ELF using `rust-objcopy --strip-all -O binary`
  to `tmp/directory-final-static.ORckrk/verified.bin`; `cmp` with the generated
  kernel BIN returned 0.

Kernel SHA-256:

- ELF `620eb21891e8a4a34a763b241371c2464a474b8ab23ae6e18ec331ec09b1b8b3`.
- BIN `3d31314f812b16874ea194e40372300b99b602f9d55597ce13670547f3bc4be3`.

The source freeze excludes this evidence document. A production correction
requires new applicable static checks and a new frozen-source/artifact record
before further runtime tests. Passing static gates is not a performance result.

## Development failures retained

All diagnostic output was displayed in full when each failure occurred:

- Earlier inode-load compilation: E0308, borrowed journal bytes supplied where
  an owned vector was required; changed to an owned snapshot.
- First read-capability test compilation: old HTree tests still used the old
  argument list; migrated the reader construction without changing assertions.
- First connected directory Clippy, exit 101: unused legacy single-block wrapper;
  removed it after confirming no callers, without an `allow`.
- New directory tests, exit 101: derived Debug imposed an unnecessary device
  Debug bound; implemented Debug over the already printable prepared phases.
- Several source queries returned exit 2 for guessed old file paths. Actual
  module paths were enumerated and used; these were inspection mistakes, not
  compiler or runtime failures. Expected `rg` exit 1 means no matches.

## Runtime validation

The first directory suite ran only after the three rounds above: 10 passed,
3 failed, exit 101. Full raw output is retained in
`tmp/directory-final-static.ORckrk/core-directory-first.log` and was displayed.
The three failures expected physical reads but observed zero additional reads;
the injected physical I/O failure therefore also returned a successful lookup.

Confirmed fixture cause: `sync_filesystem_with_observer` invokes
`commit_for_filesystem_sync`, which deliberately retains committed images in the
checkpoint queue. Clearing the data cache did not make those blocks cold. Core
fixtures now also checkpoint; adapter fixtures cleanly unmount and remount the
same real memory image before constructing fresh inode owners. No production
code or original assertion was changed to make these tests pass.

Before retrying, the fixture correction received three static steps:

1. Source/archive comparison reports exactly the three changed test files
   (`owned/tests/inode_load.rs`, `owned/tests/directory_read.rs`, adapter
   `fs/tests/inode_load.rs`). `tar -df` exit 1 is expected here; its full
   difference report was displayed. No production byte changed.
2. Both actual combined-feature strict Clippy commands passed again, exit 0.
3. Format/diff checks and a fresh archive comparison passed, exit 0. New snapshot
   `tmp/directory-final-static.ORckrk/sources-cold-fixture.tar`, SHA-256
   `88e899a22457506790ccb1aa21ca40c9210320fcd7844331a7ce3412ba526e86`.
   The production kernel and its source remain those validated above.

The corrected core directory suite passes 13/13; its inode-load suite passes
9/9. The full core library suite then passes 394/394, including canonical cache,
extent/legacy traversal, journal, directory and error-path coverage.

The first adapter inode-load suite reports 4/6, exit 101. Both failures are
`cold inode I/O retained the mount lock`. Separate backtrace runs confirm each
failure occurs **after** the cold read and unlink assertions, when dropping the
last reference enters `reap_unlinked_inode -> flush_reap_metadata -> bitmap flush`.
The test's read probe incorrectly remained active during this existing writer.
The probe now ends after the read/reference assertions and before final reap;
the final no-pending-reap assertion is retained. Reap's serialized write I/O is
not claimed optimized. Original output and both backtraces are preserved in
`adapter-first.log`, `adapter-unlink-backtrace.log`, `adapter-child-backtrace.log`
under the static artifact directory. Retry passes 6/6, exit 0.

The first full adapter suite reports 259/260, exit 101. The old metadata-miss
test expects one mount acquisition (`left: 261`, `right: 260`), whereas the new
protocol intentionally acquires twice: prepare, then validated publication.
Its exact count is updated to two and the test renamed accordingly; subsequent
cached metadata and length queries still must succeed while mount state is held
and must not request that lock. This is an explicit changed protocol assertion,
not a weakened physical I/O or metadata correctness assertion.

Each adapter-only test correction repeated its three scoped static steps before
runtime: archive/source inspection (only its test file differs, expected tar
exit 1), combined-feature strict Clippy (exit 0), then fmt/diff and fresh archive
comparison (all exit 0). Production kernel bytes are unchanged. Final fixture
archive: `tmp/directory-final-static.ORckrk/sources-runtime-fixtures.tar`, SHA-256
`c123f2c2b26a3a7f659b38615d262032403215b03de1bc80dd2070d695064844`.

Full host results, all exit 0 and no ignored/filtered tests:

| Command (with TMPDIR/environment above) | Passed | Raw log under static directory |
| --- | ---: | --- |
| `cargo test --package rsext4 --lib --features host-test,USE_MULTILEVEL_CACHE` | 394 | `core-full.log` |
| `cargo test --package ax-fs-ng --lib --features host-test,ext4,vfs,profile` | 260 | `adapter-full-retry.log` |
| `cargo test --package axfs-ng-vfs --lib --features host-test` | 49 | `vfs-full.log` |

The adapter total includes namespace gates, inode lifetime, real memory-device
cold reads, metadata, writeback, shutdown and existing block/cache tests. These
703 library cases do not replace full QEMU or Linux syscall validation.

Full 8c8g QEMU profiling follows
with the frozen 181-unit workload and reusable rootfs tg-xtask. No benchmark has
been shortened and no new speedup is asserted.

## Full-run preparation

The two successful non-performance regression disks
`tmp/private-cache.Lz1bM2/current-occupied-clean-case/rootfs.img` and
`tmp/private-cache.Lz1bM2/linux-case/rootfs.img` were compressed with `zstd -1`.
Both 1.69 GiB archives passed `zstd -t` over all 17179869184 decoded bytes before
their original sparse files were removed. Their `.zst` files, kernels and logs
remain; they can be restored. No baseline or failed-state image was deleted.
Available host space rose to about 12 GiB, then 6.5 GiB after the fresh base copy.
`lsof` warned that tracefs could not be statted; that original warning was shown.
Independent fuser/QEMU/loop-device checks found no target users. Exit 1 from
pgrep/fuser here denotes no matches, not a hidden build failure.

The fresh 16 GiB sparse working disk hashes to frozen base `99680045...` in
full before injection. Only `/opt/starry-macos-run.sh` is replaced, using
`guest-tg-xtask-profile.sh` SHA-256 `c94b86694e6f7e8b2c99b84c7c79ca2b1626323526add691d1636a238cd6f392`.
Read-only fsck before and after exits 0; optional extent-tree narrowing is
declined and no repair performed. The exported script compares byte-for-byte,
and its executable mode is confirmed. Prepared rootfs SHA-256:
`3acac076b3d449f1fdfa037aa29fa443bd2ec855de845045896b9de05df5548a`.
Hash processes have all ended before QEMU launch.

The new run directory is
`target/profiling/arceos-helloworld/starry/directory-read-ownership`.
Its `input.meta`, saved ELF/BIN, source archive and pre/post-injection logs
record actual inputs. The persistent runner is invoked through the saved-kernel
helper with a PTY and patched MTTCG; no host build or source edit is allowed in
the measured interval. This preparation is not itself a measured outcome.

## Completed full-run result, 2026-09-12

QEMU boots at 17:43:37 +0800; the real workload starts at 17:44:13 and finishes
around 18:05. PTY session 21453 / PID 1202532 exits naturally with 0. A user
continuation arrives later; the existing terminal result is collected, not a
second run. The terminal pass marker is:

```text
===STARRY-ARCEOS-HELLOWORLD-PROFILE-WINDOW-PASS elapsed=1252 rc=0 build_completed=true tg_xtask_reused=true===
```

Cargo reports 20m35s, axbuild's build phase 1245.94s, the runner 1252s, and the
kernel interval 1252597972768ns. These are different boundaries. No debugger,
second QEMU, host build, source edit or offline image access was initiated during
the measured interval. The inherited core/memchr future-incompatibility warning
was retained and displayed. All 49 startup IllegalInstruction warning addresses
and frequencies match the successful baseline and the previously documented
[Cargo/OpenSSL instruction probes](cargo-instruction-probes.md); no warning was
hidden or capability forcibly enabled.

Post-processing verifies:

- True post-exit read-only fsck returns 0 without pending journal recovery. No
  repair or extent optimization is performed. The ended disk is moved into the
  run directory as read-only `rootfs.img`; the fixed working-image path is absent.
  Complete disk SHA-256:
  `4f1daf2a4b3ae722b772dd021d9b0df91330084a8412f88d2907b2d43e2ed7f7`.
- All eight guest artifacts are exported individually, all six manifest hashes
  pass, and all 181 sorted name/version units compare identically with both
  `private-file-cache-backing` and Linux `full-build-rcsc`.
- The final ELF compares byte-for-byte with Linux, SHA-256
  `2792c35275d91f57a38a847be87c4d2b333d7b88cd548bf9e06633288c7dba21`.
- The source archive/fingerprint, reused tg-xtask, pinned guest toolchain and
  ArceOS SMP1 target inside the 8-CPU host are unchanged. Saved kernel hashes and
  selected source archive comparison still match after the run.
- All 26830 CPU/wait records are rendered with the matching saved ELF. All 11
  nonempty SVGs exist; mutex-wait, ext4-lock-hold and cpu-active PNGs are visually
  inspected. All dropped/skipped counters are zero. User execution is aggregated
  as `[user-space]`, not rustc/LLVM function symbolization.

PNG diagnostics: neither rsvg-convert nor cairosvg is in the active PATH; system
Python's import exits 1 with `ModuleNotFoundError: No module named 'cairosvg'`.
The original traceback is displayed. The existing
`tmp/profile-svg-env/bin/cairosvg` renders all three previews with exit 0; no
global dependency is installed.

### Full-profile comparison and new hotspot

| Metric | Previous | Directory owner |
| --- | ---: | ---: |
| Full build, seconds | 1323 | 1252 |
| CPU total / active samples | 102942 / 39182 | 97312 / 40189 |
| CPU active percentage | 38.0622% | 41.2991% |
| Mutex aggregate wait, ns | 1998565186336 | 1236373598912 |
| Ext4 lock aggregate wait, ns | 1387389660592 | 749497540064 |
| Ext4 lock hold, ns | 418947462256 | 254545792800 |
| Lookup-chain inclusive hold, ns | 140877707632 | 9251557232 |
| Page-cache inclusive, ns | 1646263274496 | 1278189824512 |
| Block read inclusive, ns | 650210768480 | 709661952080 |
| Block write inclusive, ns | 134682590272 | 148111705472 |
| Block flush inclusive, ns | 558037677440 | 339599478320 |

These event totals overlap and use different sampling rates; they are not
directly subtractable wall time. Overall ext4 wait falls 45.9779%, hold falls
39.2416%, and lookup-chain hold falls 93.4329%. The new lookup figure includes
every folded stack containing `InodeLifetime>::lookup`, not only its immediate
caller bucket (7841691600ns). The old equivalent uses every `lookup_locked`
stack. Block read/write totals increase, so this is chiefly an ownership and
concurrency change, not evidence of less device work. Absence of explicitly
named `namespace::` off-CPU frames does not prove zero directory-gate waits:
inlining and finite capture depth constrain that observation.

The current largest mutex owner remains `Ext4Guard` at 748491731856ns,
60.5393% of mutex waits. The dominant ext4 holder is now the background page
writeback chain:

```text
ext4-commit -> writeback_filesystem_pages -> writeback_dirty_for_global_sync
            -> writeback_page_runs -> Inode::write_at -> write_locked
            -> with_writeback_progress -> Ext4Guard
```

The write caller holds 148560955328ns / 58.3632% of ext4 hold events. Its block
write subset is 141231344064ns; its block-read subset is only 163854624ns.
The entire ext4-commit worker holds 148852100848ns. These full-folded-stack
aggregates and the visually dominant writeback tower identify **regular file
data write I/O while holding mount state** as the next ownership refactor,
not another directory-cache tweak or a guess about page faults. Read preparation
is the next hold bucket (14.7227%), then set_len (9.0313%).

The largest resolved kernel CPU leaf is independently `find_free_area`:
1854 samples / 4.6132% of active samples. `spin_release` has 1823, `flush_batch`
1267. A sampled leaf is not automatically the exact operation consuming that
time; instruction/caller inspection is required before optimizing it.

The new result is still 1.7034 times the fixed Linux 735s. Retain it as a
positive single matched measurement; repeat before claiming stable speedup.
Next work must preserve ordered-data/journal ordering, dirty-cache ownership,
inode lifetime, partial-block contents, failure/retry and shutdown semantics
when moving writes outside the mount lock. Another production refactor again
requires the three final static rounds before runtime tests.
