# Extent read ownership

## Current state

Connected file/directory ownership refactor completed its full run in 1297 s,
79 s (6.486%) slower than the verified 1218 s shared-inode-read baseline.
This is a performance regression, not an accepted speedup. Both runs compiled
181 units / 175 distinct crate names and returned the identical final ELF.
The decision and prior art are in [the design](../design/extent-read-ownership.md).
The active goal and fixed 8-core/8-GiB QEMU workload remain unchanged.

## Static round 1: ownership, consumers and error paths

Source audit performed after the connected core, adapter and test migration:

- Inode::read_at retains shared content access across every phase and copying.
  Ext4Filesystem retains mount admission; its retained inode lifetime prevents
  reclaim. No broad lock spans independent inode/extent/data endpoint I/O.
- File and directory consumers share the existing versioned inode snapshot,
  canonical data-before-journal visibility and coherent endpoint. Existing
  extent parsers still validate bounds, checksums, ordering and system zones.
- The batched callback takes one mount lock per contiguous data run. Buffer
  allocation, zeroing and copying occur outside that callback; bounded image
  vector allocation and journal-image cloning remain inside it.
- Final validation precedes exposing bytes/errors. Invalidated snapshots retry;
  foreign mounts and current errors remain typed. Existing journal-progress
  handling surrounds current-inode atime completion, not independent data I/O.
- Oversized/overflowing/legacy/unsupported fallback remains explicit. Zero
  length has an explicit no-I/O phase; EOF and output tails are preserved.
- Whole-repository Rust consumer search found only migrated core/adapter/tests.
  Directory compatibility names remain aliases; lower PreparedFileRead retains
  its original public prepare/read/complete interface. No new dependencies,
  unsafe code, warning suppression or driver changes in this connected phase.
- Deterministic tests cover cold external extent nodes and inode-table phases,
  physical read/visibility batching, dirty and journal images, superseded bytes
  and errors, rollback, foreign mount, capacity, malformed images, holes and
  unwritten/EOF. Adapter tests identify actual extent-node device addresses on
  a cleanly remounted file and assert mount release, shared content protection,
  another inode's write progress and failed-read cleanup.

This is an author-side scoped ownership audit, not a complete syscall ABI or PR
merge review. Existing syscall dispatch and VFS error translation are unchanged;
data coherence, EOF and failure-publication behavior are the relevant risks.
High-risk design still needs maintainer review before any future merge.

## Static rounds 2 and 3 passed

All checks below completed before any new runtime test, 2026-09-13 +0800:

- Strict rsext4 Clippy, lib+tests, host-test with default multilevel cache: 0.
- Strict ax-fs-ng Clippy, lib+tests, host-test,ext4,vfs,profile: 0.
- cargo xtask clippy --package rsext4 --package ax-fs-ng: 11/11, 0 failed,
  00:31:40–00:31:52. Includes core no-default-features host tests.
- Fixed AArch64 8-CPU profiling kernel: 0, release 13.57 s, 14241 kallsyms;
  final BIN refreshed after kallsyms. Existing core/memchr future compatibility
  warning preserved, no warning suppression.
- cargo fmt --package rsext4 --package ax-fs-ng --check: 0.
- git diff --check: 0. Frozen source tar comparison: 0.
- Saved final ELF independently converted by rust-objcopy --strip-all -O binary;
  cmp against saved kernel BIN: 0.

Logs and frozen artifacts are under tmp/extent-read-ownership.iQN3IJ:

| Artifact | SHA-256 |
| --- | --- |
| sources-connected.tar | 69acde3ab75cd3bce866807f3f1f09f2ff4a0a3fac90d28d00856e92f1132352 |
| starryos.elf | ece779487086e718d0e418d6705d0e4a1d4379046b3ccd2fe36d8d925262005a |
| starryos.bin | 50992a390cfdcbfccf82c3ffa18ae73e98316484c93226191a6f7162ef1637fb |

Source archive scope is core source, ext4 adapter and file cache, not the whole
kernel. No runtime test has run at this entry; the three static gates now allow
host regression tests. Any production correction requires renewed gates.

## First runtime and fixture correction

First core library run discovered 424 tests: 423 passed, 1 failed, exit 101.
Full command/output is core-full-first.log. Failure happened at the fixture
precondition before the owned read executed:

```text
thread 'ext4::owned::tests::read::coherence::journal_only_file_images_override_home_bytes_without_device_reads' (1263929) panicked at fs/rsext4/src/ext4/owned/tests/read/coherence.rs:46:5:
assertion failed: mount.device.visible_block_image(physical).is_some()
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace
```

Cause: DataBlockCache::flush_all selects ordinary direct writeback, not journal
metadata publication. The fixture now explicitly uses flush_metadata, preserves
the journal-image and zero-home-read assertions, and additionally reads the raw
endpoint first to prove home still contains the old hello bytes. This tests the
journal visibility boundary; it does not claim ordinary file writes are journaled.

Renewed static checks for this test-only correction: fixture source audit,
strict core lib+tests Clippy, targeted formatting, diff check and new source
archive comparison all exit 0. Previous frozen source comparison exits 1 and
reports only tests/read/coherence.rs size/mtime; no production bytes changed.
New source archive sources-fixture-corrected.tar SHA
4e832d81224eadc0e1506d9566274197993b3dfd00b086a0ebf8ab101357c254.
Kernel/adapter matrix and ELF/BIN identity evidence remain unchanged.

## Host runtime passed

After the renewed checks, full core library tests pass 424/424 (0 ignored or
filtered, 0.32 s), including all 15 new file-read cases. Full combined-feature
adapter library tests pass 275/275 (0 ignored or filtered, 0.71 s), including
both real cold extent I/O cases. Original commands, complete stdout and exit 0
are preserved in core-full.log and adapter-full.log. Production sources have
not changed since the verified kernel build. QEMU has not started yet.
Public file-operation integration tests additionally pass 49/49, 0.67 s,
including all existing PreparedFileRead coherence, EOF, failure and fallback
cases; full command/output in file-operations.log.

## Space and next full run

Available space was only 5.6 GiB. The unused old window image
tmp/axbuild/rootfs/rootfs-profile-clean-cache-preread-window.img occupied
5.0 GiB physically. It was made read-only, compressed losslessly with zstd,
and zstd --test decoded/verified all 17179869184 logical bytes, exit 0.
Original SHA 9f87deecf0d4333497c062e3ce981fed00edad526f850a6ffc55e1a22c67f0fd;
retained .img.zst SHA 79d994a4c1b486e2fed5ca501f2d45e9c0a757bd30bda7f10a5b17088fd1e650.
Only the original raw copy was removed after checking no process used it;
the 1.23-GiB archive remains recoverable. Logs/flamegraphs and other images
were preserved. No QEMU/build was active at image preparation.

New run directory: target/profiling/arceos-helloworld/starry/extent-read-ownership.
The working disk was absent. Saved verified kernel/source files installed;
fresh sparse base-image copy started. Finish copy and full base hash before
runner injection, fsck and prepared-image hashing, then invoke the runner once.

Copy and full base SHA completed with the expected 9968004595dec398480417e210d64f9ed509801d4325d4ab22f43951983f5ea3
before injection. Both read-only e2fsck -fn checks exit 0 (no optimizations
accepted), installed runner is 8657 bytes/0755 and dump/cmp exits 0. Saved-source
tar comparison is 0 and copied ELF/BIN match the frozen hashes. Prepared-image
full hash is running; no QEMU or host build is active. Collect the existing hash
session, do not alter the disk until it finishes.

Prepared-image hash finished 0, SHA26e60f3a0c5e33ab61b5bfce27121382f57fb86d54965cda77f50bd89446aa86.
QEMU started once, PID1267869/PTY37189, boot2026-09-12T16:41:57Z; runner
invoked once after the prompt. Guest reports 8 CPUs, affinity0–7, reused tg-xtask
and source, unchanged archive/fingerprint, enabled kernel profiling. All 49
initial instruction probes match previous addresses and counts. Latest observed
progress61s/13 compilation units. Process confirmed live, not a completed run.
Continue the same session through natural exit; no live-image access or host
build/source changes during measurement. Goal remains active.

## Investigation failures

All original diagnostics were displayed in the conversation. No source file was
changed by the failed reads. Paths corrected using the actual file inventory:

```text
cat: fs/ax-fs-ng/src/fs/ext4/rsext4/fs/tests/read/probe.rs: No such file or directory
cat: fs/rsext4/src/ext4/owned/tests/directory/coherence.rs: No such file or directory
rg: fs/rsext4/src/ext4/owned/inode: No such file or directory (os error 2)
cat: fs/ax-fs-ng/src/fs/ext4/rsext4/fs/tests/device.rs: No such file or directory
cat: fs/rsext4/src/ext4/owned/extent.rs: No such file or directory
rg: scripts/xtask/src: No such file or directory (os error 2)
```

The cat commands exited 1 and rg commands with missing paths exited 2. Separate
rg checks that returned 1 with no output mean no matching unsafe/allow/unwrap/
expect tokens in the scoped production read modules, not an execution failure.

## Full-run verification and next hotspot

QEMU PID1267869 / PTY37189 naturally exited 0; command_rc=0 and
build_completed=true. Measurement was uninterrupted: no host build, kernel
source edits, live-image inspection or second runner invocation. After exit,
the image had no users; read-only e2fsck -fn returned 0. All seven guest
artifacts were exported, their SHA256SUMS verified 6/6, and the final executable
compared byte-for-byte equal against both the 1218 s run and Linux 735 s.
All three complete name/version compilation multisets match (181 entries,
181 distinct name/version pairs, 175 distinct names).

The ended rootfs is run/rootfs.img, mode 0444, SHA-256
ba66044a22c9326e4b5aa5d3d7848fcc8b8d500110fdbf80d7a909cbd49526a1.
Final ArceOS executable SHA-256 is
2792c35275d91f57a38a847be87c4d2b333d7b88cd548bf9e06633288c7dba21.
The working rootfs path is now absent. Renderer exited 0 using the saved ELF.
All 26961 records parsed (9846 CPU rows, 17115 wait rows), all phase 1, all
eight CPUs represented, no dropped/skipped records. Summary and folded event
totals agree. Eleven SVGs exist; mutex-wait, ext4-lock-hold and cpu-active were
converted and visually inspected. Raw graphs, comparison.json, integrity.json,
compile-units.txt and fsck/hash logs remain under the run directory.

| Metric | Previous 1218 s | Current 1297 s |
| --- | ---: | ---: |
| Active CPU samples / all samples | 40022 / 94554 | 39297 / 100907 |
| CPU active fraction | 42.3271% | 38.9438% |
| All mutex wait ns | 272626758240 | 394909196288 |
| Namespace/inode AccessGate::read wait ns | 16872207824 | 112705365216 |
| Commit gate sync_core wait ns | 5483938256 | 53485138912 |
| Mount lock wait ns (dedicated event) | 110156612272 | 104142597824 |
| Mount hold ns | 86676276128 | 68888928384 |
| read_inode direct mount hold ns | 32359739008 | 5494604176 |
| Page-cache inclusive ns | 770822259712 | 1315826039296 |
| Block read ns | 665519813648 | 687243719616 |
| Block write ns | 146172060416 | 180908529984 |
| Block flush ns | 203089027152 | 336414875136 |

These aggregate, overlapping intervals are not additive wall time. Read
ownership removed most of its targeted mount hold, but is not sufficient to
establish an end-to-end gain. The largest mutex leaf is now AccessGate::read
(28.54%); current stacks lead through Namespace::lookup. A mkdir commit-gate
wait alone accounts for 19153882144 ns. Namespace mutations retain their
directory/topology exclusion across with_writeback_progress -> sync_core.
An already queued topology writer also blocks newly arriving lookup readers.
This is a concrete broad blocking boundary for the next connected refactor.

Flush latency increased despite fewer flush events (666 -> 624), with the
maximum increasing from 16.119 s to 28.678 s. The graph does not distinguish
host backing-store latency from guest scheduling or establish that the extent
change caused this increase. Available host disk space is only 4.2 GiB; no
host load or disk-policy changes may be hidden inside a performance claim.
Read retries from inode-version invalidation remain an unproven hypothesis.
The leading active kernel CPU leaf remains find_free_area: 1890 samples,
4.8095% of active samples. No claim that all remaining overhead is ext4.

Postprocessing initially counted distinct name/version pairs as distinct crate
names. Its assertion failed before comparing the logs (exit 1):

```text
Traceback (most recent call last):
  File "<stdin>", line 36, in <module>
AssertionError: ('current', 181)
```

Corrected the checker, not the workload; complete verification above passed.
A read of the old access.rs path also exited 1; actual path is access/mod.rs:

```text
cat: fs/ax-fs-ng/src/fs/ext4/rsext4/fs/access.rs: No such file or directory
```

The similarly old fs/sync.rs is not wired by fs/mod.rs. Production sync lives
in fs/writeback.rs; no conclusions or edits use the unwired implementation.
