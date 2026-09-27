# Namespace journal progress

## Current state

Connected implementation and twelve deterministic adapter tests are complete.
After the final three static rounds, the unchanged GREEN regression passed,
then all287 adapter library tests passed (0failed/0ignored,0.80s). The full QEMU
run is now complete and verified:1217s,181units175names,exit0. This recovers80s
versus the immediate1297s run but is effectively level with the1218s earlier
baseline, not a demonstrated improvement beyond it.
Design: ../design/namespace-journal-progress.md.
Before archive tmp/namespace-progress.m0n7Zy/sources-before.tar has SHA-256
9a1849e1e5c325a714a3923231ac4c15fcb58dede2e924c7cdfee39ecaf30a4c.
The 1297 s full extent-read run remains a regression, not an accepted gain.

## Static ownership audit

- Core create/mkdir/link/unlink/rmdir/rename use scoped metadata transactions;
  their retryable errors restore filesystem and journal images. None of those
  migrated calls is a restartable partial file-write operation.
- New fs/mutation.rs separates attempt classification from pressure/abort
  completion without duplicating journal error policy. Existing content-write
  callers retain the same exclusion through the whole loop. No driver or
  memory-management changes, public core API, unsafe code or new locks.
- All five adapter entry groups (create, symlink, link, unlink/rmdir, rename)
  use InodeLifetime::mutate_namespace. Each attempt reacquires topology then
  directory then mount, and repeats name/type/cycle checks. Pressure/abort work
  happens after all namespace/mount guards drop, with mount admission retained.
- Parent allocation references survive the wait. Existing zero-link publication
  identifies removed retained parents; both rename parents are checked. No
  duplicate version registry or inode-table I/O is added for that check.
- Successful child retention and orphan publication remain in the successful
  critical section; VFS wrapper creation and orphan reap remain outside.
  Explicit sync, directory-sync and fallback policies remain unchanged.
- Test cases cover six create kinds, symlink/hardlink, unlink/rmdir, rename
  replace/exchange, staging and genuine core checkpoint admission pressure,
  same-name/source replacement and parent removal during real device flush,
  non-retry errors, flush failure and mount-admission cleanup. TLS callbacks
  are taken before execution, permitting reentrant real namespace operations.
- This is scoped author-side ownership validation, not a complete syscall or
  PR review. The private pressure interleaving is deterministically selected at
  the existing host memory-device boundary; no syscall fault-injection ABI is
  introduced. Repeated runtime evidence and the full workload remain required.

## Development static check

Initial strict combined ax-fs-ng lib/tests Clippy exited 101: three new test
calls used private Inode::number. Full original diagnostics were displayed and
saved in development-clippy-first.log. Tests now use public NodeOps::inode and
checked InodeNumber construction; no production visibility was expanded.
The corrected strict check exits 0 (1.41 s), saved in
development-clippy-corrected.log. Native Cargo is used only because the checked
xtask Clippy flow tests individual features, not this combined host-test/ext4/
vfs/profile lib+test target. cargo fmt --package ax-fs-ng and git diff --check
have returned 0. These development checks are not the final runtime gates.

## RED/GREEN protocol

The refactor is complete before runtime. To verify the regression test fails
on the original behavior, preserve the candidate and temporarily move the
namespace guard back outside the attempt loop (the old broad-guard ownership).
Audit that exact arrangement, compile it with strict Clippy, then verify
format/diff/source identity before executing only the named regression.
Expected failure is a real lookup blocked during detached flush, not a timeout.
Restore the candidate, repeat three static rounds, then execute unchanged tests.
No QEMU may use the deliberately restored broad-guard variant.

RED static gates completed before runtime: (1) exact source audit confirmed only
namespace guard lifetime moved outside the loop, restoring old wait ownership;
candidate archived SHA fc198ca1ee2d7f9929df6422aa7d7692cf062176a2e94602fa4b30f20e4a7dd5;
(2) strict combined lib/tests Clippy exit 0, 0.88 s;
(3) targeted fmt --check, git diff --check and saved RED source tar comparison
all exit 0. The deliberate ownership regression is not a candidate for QEMU.

RED ran exactly one unchanged regression: exit101, 0passed/1failed/286filtered,
0.02s, original output saved in red-test.log. Real lookup at the flush boundary
failed with `namespace lookup blocked while journal I/O progresses: BadState`;
the host fixture has no blocking runtime, so the retained gate fails immediately
instead of hiding behind a timeout. Restored only the guard's candidate scope.
Fresh ownership audit confirms the previously documented attempt/completion
and reference boundaries; production was not otherwise changed after RED.

## Final three static rounds passed

After restoring the candidate, before GREEN/runtime:

1. Scoped ownership/consumer/error audit above repeated; all47 candidate
   archived files compare byte-for-byte equal. No partial content caller moved
   into the namespace retry protocol.
2. Strict combined lib/tests Clippy exits0 (0.79s). cargo xtask clippy --package
   ax-fs-ng passes8/8,01:31:20–01:31:25+0800. Fixed8CPU AArch64 profile kernel
   build exits0,12.55s release,14249kallsyms, final BIN refreshed. Existing
   core/memchr future-incompatibility warning remains visible, no allow added.
3. Targeted cargo fmt --check, git diff --check, final source archive comparison
   all exit0. Saved ELF independently converted with rust-objcopy --strip-all
   -O binary; cmp against saved BIN exits0.

Saved sources-final.tar SHA
cd84d3cc48566deaec31e13bfc68198d4472f685721a1ec722c3696d59db586d.
Saved ELF SHA f790d3a0a7fd72abf890b63b3149d265541330ea6a634ad2bfad862eb4a5ad36;
BIN SHA d0ac50a7911c7102995e197fdc8d5b8c86a87f0d3e10efeb0badf8368b464719.
The active xtask Rust postprocessor generates kallsyms; the old shell
starry-kallsyms.sh is not executed by this flow. No QEMU has started.

## Host runtime evidence

The unchanged named GREEN regression exits0,1passed/286filtered,0.10s; it
exercises all six create kinds. Full combined host-test/ext4/vfs/profile library
suite exits0,287passed/0failed/0ignored/0filtered,0.80s. Logs are green-test.log
and adapter-full.log in tmp/namespace-progress.m0n7Zy. All twelve new tests ran,
including reentrant create/rename/rmdir during real detached device flush,
core checkpoint pressure, and error/admission cleanup. No runtime source
changes were needed after the final static gates. The flush-failure test checks
Io propagation, abort state and released guards/admission; its name alone is
not evidence of an additional post-abort child-lookup assertion.

## Recoverable space cleanup and run preparation

The old unused rootfs-profile-cached-inode-metadata-window.img had an existing
.img.zst archive. Decoded all17179869184bytes and compared every byte against
the raw image, exit0; raw SHA d0853a7a31eb603f646deddbc3b579088aeaf28237c651595de33d714f6b830a.
Archive SHA 0874fa030ce5b5d0fe7f0d54582e9d90c60cd74143081b994f830f1c8b711791.
fuser returned1 with empty output (unused). Only the duplicate raw was removed;
the archive, original logs and graphs remain recoverable. Freed about5.5GiB,
available space4.2GiB ->9.5GiB before creating the fresh working image.
Exact byte-comparison command/output is in cleanup.log. The new image is copied
from the unchanged readonly host-config base, not an ended workload disk.

## Full QEMU outcome and evidence

Run: target/profiling/arceos-helloworld/starry/namespace-journal-progress.
Fixed8cores/8192MiB,TCG multi-thread,cortex-a53,NVMe; unchanged guest tg-xtask,
source archive/fingerprint,toolchain and profiling instrumentation. All input
hashes are in input.meta. Boot2026-09-12T17:42:58Z,runner invoked once at
17:43:27Z. PID1281594/PTY38598 naturally exited0 after full1217s. No timeout,
concurrent hostbuild,kernel source edits,live-image inspection or reinvocation.
Guest reported build_completed=true,tg_xtask_reused=true,source_reused=true.

After exit,fuser had no users. Readonly e2fsck -fn exits0;17 optional extent
narrowing suggestions were declined,not filesystem errors or repairs. Exported
guest artifacts pass all6 SHA256SUMS entries. The181 name/version compile
multisets match the immediate previous run,the1218s run,and Linux. Final ELF
cmp exits0 against allthree; SHA2792c35275d91f57a38a847be87c4d2b333d7b88cd548bf9e06633288c7dba21.
Ended disk is preserved as rootfs.img,0444,SHA
7c0b5c8136108519cab71ad4e66c32243433c198a7ebbd5d5ed7412fe65e413b.
The former working rootfs path is absent. No QEMU remains running.

Saved matching ELF symbolizes all27050 records:10056CPU+16994wait. Explicitly
verified phase/event definitions,phase1 rows,all8CPUs,no dropped/skipped samples,
all summary and folded totals. All49 instruction-probe address/type/count
entries match the prior run.11SVGs generated; mutex-wait,ext4-lock-hold and
cpu-active viewed. Exact checks and raw outputs are in integrity-check.log,
integrity.json,comparison.json,fsck/hash logs and qemu.log.

| Metric | Earlier baseline | Immediate previous | Namespace progress |
| --- | ---: | ---: | ---: |
| Full build seconds |1218|1297|1217|
| Active CPU samples / total |42.327%|38.944%|43.418%|
| Mutex wait aggregate seconds |272.627|394.909|217.349|
| Namespace/read-gate leaf wait seconds |16.872|112.705|10.791|
| sync_core commit-gate wait seconds |5.484|53.485|24.356|
| Dedicated ext4 mount-lock wait seconds |110.157|104.143|66.528|
| ext4 mount-lock hold seconds |86.676|68.889|54.692|
| Page-cache inclusive seconds |770.822|1315.826|683.734|
| Block read aggregate seconds |665.520|687.244|497.819|
| Block write aggregate seconds |146.172|180.909|83.041|
| Block flush aggregate seconds |203.089|336.415|239.779|
| Block flush count |666|624|699|

Durations overlap and are not additive wall-time savings. Relative to1297s,
wall time improves6.168%; relative to1218s,the1s difference is insufficient
evidence of a new end-to-end gain. Linux remains735s. The deterministic guard
test and read-gate wait reduction support the local ownership fix. Different
flush latency/counts and scheduling mean this single run cannot causally assign
every I/O or wall-time difference to the patch.

## Largest remaining hotspots and next boundary

Largest individual mutex leaf is Ext4Guard::acquire65.573s (30.17% of mutex
wait); the dedicated mount-lock event is66.528s. Largest mount owner is create
15.235s (27.86% of54.692s hold); set_len6.474s,lookup6.290s,readdir6.035s.
This does not prove that replacing all ext4 is the next best change.

The two private-file fault cache entries pin_read_page50.637s and
with_current_read_page28.441s account for79.079s combined mutex wait,36.38%.
Both methods take the inode-wide cached io_lock and cache index. Source
writeback.rs holds io_lock across backing writes and sync; mapping.rs requires
the same owner for pinning and PTE publication. Ordinary buffered read hits
already use the updating flag + index boundary in update.rs. Next diagnosis
must resolve sampled lock sites and every mutation/retirement consumer before
changing the cache publication/writeback ownership as a connected refactor.
Retain current EOF,physical identity,dirty generations,truncate rollback,
mapping invalidation and explicit sync error semantics; never simply drop
those guards. No implementation or performance benefit of that next phase is
claimed here.

Largest non-user CPU leaf remains MemorySet::find_free_area,1991samples,
4.851% of active samples. spin_release1678 and flush_batch1278 follow. Thus
the full profile still contains memory-management/scheduling work beyond ext4.

## Postprocessing errors retained

debugfs rdump returned0 but could not restore guest root ownership as this host
user; full diagnostics were displayed. All exported file contents and ELF were
independently verified; host ownership is not a workload result. Future exports
should use individual debugfs dump calls when ownership preservation is unwanted.
Default Python lacked cairosvg,exit1; the existing tmp/profile-svg-env/bin/python
successfully generated allthree previews. Initial integrity checker incorrectly
expected only one header before samples and asserted,exit1; the format also has
one phase and nine event definitions. Corrected checker verifies those exact
definitions plus every sample,exits0. Original outputs and confirmed causes are
preserved in postprocessing-errors.log; no source/workload change or QEMU rerun
was made for these host-side issues.
