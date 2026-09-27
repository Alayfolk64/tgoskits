# File write ownership: implementation record

## Status

In progress. The mount-lock-external ordinary extent write path is connected,
including growth, partial blocks and unwritten conversion. Final static rounds
are in progress; no runtime test or new QEMU run has started in this phase.
The retained measured baseline remains the complete
181-unit directory-owner run at 1252 s. See the
[design](../design/file-write-ownership.md) and
[baseline evidence](directory-read-ownership.md).

The current adapter enters `write_locked -> write_extent_inode`, executing
ordinary file-data I/O outside mount exclusion. This is an implementation fact,
not an asserted performance gain. The earlier serialized intermediate stage
below is retained as history and is superseded by the connected-stage record.

## Frozen starting point

- Directory: `/home/wuxun/Projects/tgoskits`.
- Branch: `profiling/manual-20260907`.
- HEAD: `affddc3fecec02b31df94573063e4840433c9ebe`, preserving the dirty tree.
- Before-edit source archive:
  `tmp/file-write-owner.VHVeZy/sources-before.tar`.
- SHA-256: `a4037b4f03117e64bff8fbb54aa097eea9086e3014e5b4d300bf175d3be18524`.
- Archive scope: rsext4 source, ext4 adapter, VFS file-cache and node sources;
  not a whole-workspace snapshot or clean commit.
- Available host disk remains about 6.3 GiB. No additional image was copied or
  deleted. No QEMU session is active from the completed baseline.

## Implemented state separation, 2026-09-12

Ordinary file writes move from `file/io.rs` into a private `file/write/`
domain, preserving the flat `file::write_inode_data` public entry. Separate
modules now own input/range validation and orchestration, full/partial data
writes, legacy allocation rollback, and unwritten prepare/data/completion.

The production unwritten path now constructs a `PreparedUnwrittenWrite`,
executes it into a `CompletedUnwrittenWrite` on either success or error, then
finishes that receipt. The preparation owner retains the target, inode,
prepared runs, journal reservation and extent-leaf rollback snapshots together.
Data failure releases the reservation without converting the extent; successful
data proceeds through the same reservation/conversion/rollback algorithms.
The new `WriteTarget` groups validated inode/old-size/logical-range inputs.

This first phase intentionally retains serialized execution and its previous
allocation, full-run grouping, partial-block contents and metadata behavior.
It has not yet made the prepared inode safe against concurrent link/orphan
updates. Before detached use, completion must load the current inode and merge
only the write-owned state; its old inode image must never erase those updates.

The remaining `file/io.rs` still contains existing range/resize/read/allocation
logic (2458 lines). It is not claimed to meet the final module-size/ownership
review merely because the extracted write modules are now small. Review that
remaining boundary as the write/mapping refactor proceeds; do not disguise its
existing mixed responsibilities as a single finished domain.

## Intermediate static evidence

These checks compile code; they do not execute tests and do not constitute the
three **final** static rounds required after the full refactor is connected.

- `cargo fmt --package rsext4`: exit 0.
- First `cargo xtask clippy --package rsext4`: 3/3, exit 0,
  19:56:53–19:56:58 +0800.
- After explicit prepared/completed types:
  `cargo xtask clippy --package rsext4`: 3/3, exit 0,
  19:58:32–19:58:35 +0800. Raw log:
  `tmp/file-write-owner.VHVeZy/core-phase-clippy.log`.
- `cargo clippy --package rsext4 --lib --tests --features host-test,USE_MULTILEVEL_CACHE -- -D warnings`:
  exit 0. Raw log:
  `tmp/file-write-owner.VHVeZy/core-phase-combined-clippy.log`.
  Native Cargo covers the combined feature set unavailable through xtask's
  separate-feature matrix, as already established in the baseline validation.
- `git diff --check`: exit 0.
- Before-archive comparison returns expected exit 1 with only `file/io.rs`
  and `file/mod.rs` reporting changed time/size among archived existing files.
  New `file/write/` files are absent from that archive by definition.
- Phase snapshot `tmp/file-write-owner.VHVeZy/sources-phase.tar` matches the
  current selected sources with `tar -df` exit 0, SHA-256
  `520458ff99f7c0c0bc175d85770109b0b5c6208ac2a837bcff7c63222252c5c4`.
- Final intermediate `cargo fmt --package rsext4 --check`: exit 0.
- Source lookup confirms callers retain one `write_inode_data` implementation
  via the existing flat re-export. This is static call-wiring evidence, not a
  behavioral test or an assertion of unchanged on-disk results.

All Cargo commands set `TMPDIR=/home/wuxun/Projects/tgoskits/tmp`.
No warning was silenced, no test assertion removed, and no test invoked.

## Inspection errors retained

The following commands failed during source discovery. Their full diagnostic
output was displayed when they occurred; neither was a compiler/runtime error.

`cat fs/ax-fs-ng/src/fs/ext4/rsext4/fs/mutation.rs`, exit 1:

```text
cat: fs/ax-fs-ng/src/fs/ext4/rsext4/fs/mutation.rs: No such file or directory
```

`rg` with the guessed journal commit filename, exit 2:

```text
rg: fs/rsext4/src/blockdev/journal/commit.rs: No such file or directory (os error 2)
```

Cause: these names do not exist in the current module layout. The actual
directories were enumerated; adapter mutation orchestration is in
`fs/writeback.rs`, and synchronous journal commit is in `journal/sync.rs`.
The initial AGENTS.md file search under fs/docs returned 1 with empty output
(no nested files); this was explicitly treated as no matches.

The earlier fixed-commit raw Linux web fetches reported cache misses; existing
local Linux source at the verified commit was used, and the official journal
documentation was fetched successfully. No remote source was guessed or edited.

## Original next-phase checklist (superseded below)

Do not restart the completed benchmark. Continue from the write types above:

1. Produce a validated immutable physical-data plan, including initialized
   partial-block preservation, unwritten zero fill and full-run batching.
2. Reuse coherent endpoints and shared-cache ownership; invalidate or account
   for overlapping private/held images without losing dirty data on failures.
3. Connect extent growth as well as overwrites through an admitted, pinned
   inode owner. Preserve legacy/capability compatibility explicitly.
4. Separate data completion from retryable metadata completion. Audit whether
   and where journal/data admission must be tracked; do not retain a reserved
   finish handle across an interval in which the worker must make progress.
   Avoid a commit gate waiting for completion that itself needs that gate.
5. Merge current inode metadata at publication, retain receipts over journal
   pressure, handle errors/abandonment/shutdown, and add deterministic real
   core/adapter tests before the final static gates.
6. Only after the complete refactor and three final static rounds, run runtime
   tests and the full 8c8g QEMU/Linux-matched profiling workflow.

## Connected stage and final static round 1

2026-09-12: connected ordinary file writes through prepared/completed owners,
coherent independent endpoints, mapping leases and adapter admission/RAII cleanup.
The design now records the actual commit ordering and retry mechanism. Completion
loads the current inode rather than overwriting link/orphan/xattr metadata with
the pre-I/O snapshot. The 2462-line I/O remainder was split by mapping ownership;
public file entry points remain stable. No portable driver, syscall dispatch,
rootfs runner or QEMU configuration was changed in this stage.

Final source inspection round 1 covers:

| Invariant | Source boundary / evidence |
| --- | --- |
| Data owns no mount borrow | `PreparedInodeWrite::execute` uses only `FileDataEndpoint`, the physical plan and immutable input. |
| Mapping cannot be reused during data I/O | `InodeWriteOwners`; mounted read/write/truncate/resize/range/reap guards; adapter retains shared inode lock and lifetime. |
| Partial/new blocks preserve their boundaries | Full request grouping, initialized edge image/home-read, unwritten zero buffer in `write/detached/plan.rs`. |
| No premature initialized mapping | Preparation reuses KEEP_SIZE unwritten allocation; only successful completed receipt converts. |
| Namespace changes survive completion | Current inode is reloaded for conversion and rollback; link count/orphan fields are not read from the old snapshot. |
| Journal progress cannot replay data or deadlock on it | No finish reservation across I/O; commit gate does not drain data; receipt retries metadata only. |
| Failure and abandonment remain visible | Original data cause returned; terminal completion releases lease; progress failure uses adapter completion guard; unexecuted abandonment stays Busy. |
| Cache images cannot shadow successful writes | Clean private image transferred/removed; dirty image rejected; held read image discarded at preparation and successful completion. |
| Clean shutdown cannot outrun an owner | Mount admission drains before final clean publication; core remount/unmount requires drained mapping leases. |
| Compatibility remains explicit | Empty/private/nonregular/legacy/unsupported fork selects serialized compatibility before allocation; actual I/O/NoMemory errors do not fall back. |
| Algorithm movement is scoped | All non-import/nonconstant nonblank lines of old `io.rs` assigned exactly once to domain files before formatting; only required private visibility/exports changed. |
| ABI claims are bounded | Existing [per-syscall impact matrix](ext4-syscall-impact.md) remains the scope inventory, not a full ABI approval. Dispatch/permission/buffer validation is unchanged; no complete errno or errseq_t claim. |

Linux comparison uses the fixed commit in the design. The current man-pages
6.19 entries for [write](https://man7.org/linux/man-pages/man2/write.2.html),
[fsync](https://man7.org/linux/man-pages/man2/fsync.2.html),
[truncate](https://man7.org/linux/man-pages/man2/truncate.2.html) and
[unlink](https://man7.org/linux/man-pages/man2/unlink.2.html) were refreshed.
Successful data I/O is not itself a durability claim; fsync still drains VFS
pages before the backing metadata commit. Existing all-or-error backing writes,
full syscall differential coverage and historical-write-error semantics are not
silently upgraded to complete Linux compatibility by this refactor.

Added production-path coverage comprises 15 core tests and 7 adapter tests.
They include multi-request growth, initialized/unwritten edges, same-inode
exclusion, unrelated writes, open-unlinked writes, foreign/repeated/cancelled
receipts, a failed later data run, remount before/after conversion, external
extent leaves plus concurrent links/owner/xattrs, private-cache overlap, fork
errors and progress-failure cleanup. **Not executed yet.** Core combined Clippy
and adapter combined Clippy have passed intermediate compilation. Final round 2
will repeat all relevant matrices after the last production/test edits and build
the actual AArch64 8-CPU kernel; round 3 must precede runtime tests.

### Connected-stage failures retained

Full raw output was displayed in the session. These failures were not hidden:

- First connected core matrix: exit 1, E0432/E0425, private `BlockDev` inaccessible.
  Introduced narrow crate-private `FileDataEndpoint`, not a public raw device.
- First new core test matrix: exit 1, unused must-use `UnlinkOutcome` twice.
  Tests now assert `requires_reap()` rather than ignoring the outcomes.
- One patch context mismatch: rustfmt had placed `PreallocationMode::KeepSize`
  on one line. Read current files, verified no partial edit and reapplied exactly.
- Adapter combined Clippy: exit 101, E0624 at test lines 91/176/190 (`io_lock`
  and `number` private). Tests now use existing `NodeOps::inode` and the existing
  filesystem inode-lock lookup; retry exited 0.
- Core combined Clippy after new coherence test: exit 101, E0425/E0433 at test
  lines 55/59 (missing `AbsoluteBN` import). Added the domain import; retry 0.
- Source discovery `cat fs/ax-fs-ng/src/fs/ext4/rsext4/fs/progress.rs`, exit 1:

```text
cat: fs/ax-fs-ng/src/fs/ext4/rsext4/fs/progress.rs: No such file or directory
```

  Actual progress helper was located in `fs/writeback.rs` and inspected.

## Final static rounds 2 and 3, before runtime

All completed after the connected production and test edits, on 2026-09-12:

- Core xtask Clippy: 3/3, exit 0, 20:40:13–20:40:18 +0800.
- Adapter xtask Clippy: 8/8, exit 0, 20:40:15–20:40:24 +0800.
- Both combined-feature Clippy commands from the baseline protocol: exit 0.
- Actual `cargo xtask starry build --config apps/starry/macos-selfbuild/build-aarch64-unknown-none-softfloat.toml`:
  exit 0, release build 14.28 s, AArch64 SMP=8, NVMe, guest-profile and frame
  pointers. Started 20:40:57 +0800. The pinned core/memchr future-incompatibility
  warning remains visible; no kernel/test runtime was started by this command.
- `cargo fmt --package rsext4 --package ax-fs-ng --check`: exit 0.
- `git diff --check`: exit 0.
- `tar -df tmp/file-write-owner.VHVeZy/sources-connected.tar`: exit 0.
- Independently re-converted saved ELF with `rust-objcopy --strip-all -O binary`;
  `cmp` against the saved build BIN: exit 0.
- Newly compiled test-module inventory: core 10+2+2+1=15, adapter 7. Runtime
  discovery/results are recorded separately below; source counting is not a test.

Frozen artifacts under `tmp/file-write-owner.VHVeZy/`:

| Artifact | SHA-256 |
| --- | --- |
| `sources-connected.tar` | `78f7e8207db340e087d6139234f28521711ea8a69699a6794a9a99c6c842c55d` |
| `starryos.elf` | `332ea2c0cfe8b06ea3dfd6ff3e4c961d05ae6635fdce7212819618b244c86a6b` |
| `starryos.bin` | `9ae1825213daa5ebabb60b29ecaef9becbd029f5257a4497b9753d61921693a7` |
| Build configuration | `031da98e3181df1f262e63aee92ec43e391ba36190fc156662edc384aa9b2d66` |
| 8c8g QEMU configuration | `458234d28407fe37ac89916285be744b5928d81481ad973db537ab3d66a17794` |

Matrix post-yield logs are explicitly named `*-final-matrix-tail.log`; full
combined adapter and kernel build outputs are saved as `adapter-final-combined.log`
and `kernel-build.log`. The complete matrix command starts remain in the session
transcript. Source archive scope is the same selected FS/VFS boundary as before,
not a claim of a clean or whole-workspace commit. Three static rounds are now
complete; runtime tests may start. Any further production fix requires renewed
static inspection before another runtime attempt.

## Runtime regression and experiment preparation

After all three static rounds, the new core filter discovered and passed exactly
15 tests and the adapter filter exactly 7 (no skipped tests). Full library runs
then passed 409 core, 267 adapter and 49 VFS tests, total **725**, all exit 0,
zero failures/ignored/filtered in the full runs. Complete logs are in the snapshot
directory as `core-targeted.log`, `adapter-targeted.log`, `core-full.log`,
`adapter-full.log`, `vfs-full.log`. No production or test fix was needed after
runtime began. This is correctness evidence, not a compilation speedup.

Host disk preparation found only about 6.3 GiB free. Two obsolete decompression
verification copies were independently compared byte-for-byte with their retained
originals, both `cmp` exit 0; `fuser` returned 1 with empty output (no users),
and no matching QEMU/loop device existed. Removed only:

- `tmp/axbuild/rootfs/host-config-baseline-archive-verify.img`
- `tmp/axbuild/rootfs/inode-writeback-archive-verify.img`

Each used about 5.1 GiB. The corresponding `rootfs-profile-*-window.img` original
and `.zst` remain, so each removed verification copy is recoverable. Free space
rose to about 17 GiB before copying the fresh frozen base. No baseline, failed
state, reusable tg-xtask or source was removed.

New run directory: `target/profiling/arceos-helloworld/starry/file-write-ownership`.
It holds the verified saved ELF/BIN and selected source snapshot. QEMU/runner
hashes match the 1252 s baseline; full cold-image verification/injection precedes
the actual measured interval. Do not treat a prepared directory as a completed run.

Preparation correction: the first full-disk SHA-256 was still being collected
when the runner replacement began. Although it returned the expected base hash,
read/write overlap invalidates that identity evidence. No QEMU had started. After
collecting its terminal exit 0 and confirming the working disk had no users,
the disposable working copy was re-copied from the unchanged frozen base.
The second complete SHA-256 finished with exit 0 and matched `99680045...` before
any injection. Then pre-injection fsck, runner replacement, dump/cmp, executable
mode check and post-injection fsck all passed. The old hash is not used as proof.
The QEMU-name process match was an old `tmux: server` command line, confirmed with
`ps`, not a live QEMU; `fuser` exited 1 with empty output (no disk users).

## Completed full run, 2026-09-12

QEMU boots at 20:52:51 +0800, workload starts at 20:53:23; PTY 79418,
PID 1232212 exits naturally with 0. No host build, source edit, debugger or live
disk access was initiated in the measured interval. The full pass marker is:

```text
===STARRY-ARCEOS-HELLOWORLD-PROFILE-WINDOW-PASS elapsed=1222 rc=0 build_completed=true tg_xtask_reused=true===
```

Cargo reports 20m01s; axbuild's build phase 1212.06s; runner 1222s; kernel interval
1221896282528ns. These boundaries differ. All 181 name/version compile units
(175 distinct crate names) match both preceding Starry and fixed Linux runs.
The final ELF compares byte-for-byte with Linux, SHA-256
`2792c35275d91f57a38a847be87c4d2b333d7b88cd548bf9e06633288c7dba21`.
Six guest manifest checks pass. All 25959 CPU/wait records are rendered with the
saved ELF; dropped/skipped counters are zero. Eleven nonempty SVGs and three
visually inspected PNGs (mutex-wait, ext4-lock-hold, cpu-active) are retained.
All 49 IllegalInstruction warning addresses/frequencies exactly match the
preceding successful baseline's documented Cargo/OpenSSL probes. The pinned
core/memchr future-incompatibility warning remains visible.

Post-exit read-only fsck returns 0 with no journal replay or repair. The ended
disk is moved to read-only `rootfs.img` in the run directory; complete SHA-256:
`890da3b4e9778d5fb61664802e018847c63b4055941699b39837071f1759a683`.
The fixed working-image path is now absent.

Export warning: `debugfs rdump` exited 0 but printed
`Operation not permitted while changing ownership` for all eight files and
their directory: unprivileged host extraction cannot restore guest root
ownership. Full raw output was displayed. All eight files exist; all six content
hashes pass, so root ownership is not needed for offline analysis. This is not
recorded as a warning-free export.

Analysis-command failures: reading the 411175-byte baseline JSON through a
bounded tool output produced a truncated-output warning instead of full JSON,
so JavaScript parsing failed (`Unexpected token 'W'`). A subsequent inline
Python reduction command exited 1 because one closing dictionary brace was
missing (`closing parenthesis ')' does not match opening parenthesis '{'`).
Both original errors were displayed. A smaller, stepwise JSON reduction passed;
the original profile/summary files were not modified by either failure.

### Comparison and next measured hotspot

| Metric | Directory owner baseline | File write owner |
| --- | ---: | ---: |
| Full build seconds | 1252 | 1222 |
| Active CPU samples / all samples | 40189 / 97312 | 39990 / 94892 |
| CPU active percentage | 41.2991% | 42.1426% |
| Mutex aggregate wait, ns | 1236373598912 | 318674503120 |
| Ext4 aggregate lock wait, ns | 749497540064 | 94474594592 |
| Ext4 aggregate hold, ns | 254545792800 | 74929037136 |
| Page-cache inclusive, ns | 1278189824512 | 826484569088 |
| Block read inclusive, ns | 709661952080 | 479927038352 |
| Block write inclusive, ns | 148111705472 | 105527276176 |
| Block flush inclusive, ns | 339599478320 | 267879256400 |

Single matched run: 30s / 2.3962% faster, still 1.6626 times fixed Linux735s.
This does not establish statistical repeatability. Inclusive event durations
overlap and are not directly subtractable wall time. Ext4 wait drops87.3949%,
hold70.5636%, total mutex wait74.2251%; active CPU improves only0.8435 points.
The low-CPU problem remains.

All folded stacks containing `write_locked` now hold6060707520ns, compared with
148682596848ns before (the older148560955328ns number was its immediate caller
bucket, not the entire inclusive chain). The new `write_extent_inode` chain
holds5414249888ns while100589147392ns of block writes execute outside it.
The entire ext4-commit worker now holds6243982112ns, down from148852100848ns.
This confirms the intended data-I/O ownership change, not a skipped workload.

The new largest mutex leaf is `Inode::read_at`,121319296256ns /38.0700%,
with121310650432ns attributed to `populate_page_window`. The dominant visible
tower is rustc page fault -> CowBackend::prepare_missing -> pin_read_page ->
populate_page_window -> Inode::read_at. The next mount-lock leaf is
93369088000ns /29.2992%. Within ext4 hold, read_inode is now largest:
25780589040ns /34.4067%, then create16029711040ns /21.3932%.
The largest resolved kernel CPU leaf remains find_free_area,
1863 samples /4.6587% of active samples; it is distinct from blocking waits.

Next investigation: same-inode read exclusion and off-lock read-plan metadata
ownership, including write/truncate/range safety and cache publication. Current
read_at takes the same exclusive inode I/O mutex as writers across all physical
reads. Compare Linux's shared mapping/read boundary before selecting the next
connected refactor; do not merely swap a lock without auditing every mutator.
