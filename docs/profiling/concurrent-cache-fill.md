# Concurrent file-cache fill: complete profiling result

## Candidate and evidence

The complete preceding `anonymous-zero-backing` run is 1676 s / 181 units;
its 1.06% difference from 1694 s is not an established independent speedup.
The new candidate implements the
[concurrent fill design](../design/concurrent-cache-fill.md). It targets the
largest file-fault mutex stacks (422386610400 ns I/O owner and
344292109856 ns cache-index owner), not the required anonymous clear itself.
The complete cold run finishes in **1684 s**, versus 1676 s before this
change: 8 s / 0.48% slower, not an accepted speedup. Both compilation and
QEMU exit 0 with 181 units / 175 distinct crates. The implementation remains
a validated candidate, not evidence that low CPU utilization is solved.

Changes cover bounded in-flight windows, coalesced completion, one-way
invalidation before content updates, off-lock preparation, validated
publication, and off-index single-page loading/eviction. Private mappings
still receive copied private backing. Writeback serialization is unchanged.
An admission-capacity waiter waits for space, not the unrelated owner's I/O
result. Readahead inserts the demand page last so the same run cannot evict
it under a smaller retention target. Canceled candidates release owned pages
before completing/removing their reservation.

## Three final static rounds

All three rounds below completed by 23:14 on 2026-09-11, before any runtime
test for this refactor:

1. Ownership/caller audit: checked read, update, all single-page callers,
   resize rollback, eviction/reclaim, writeback, mapping listener contracts,
   and pending completion. Storage reads own private buffers, not cache
   guards; admission/publication use I/O exclusion. Content-update entry
   invalidates every outstanding ticket before mutation. Pending wait and
   completion notify execute outside I/O/index locks. Reclaim retains its
   nonblocking I/O reservation. Faultable Writers receive only snapshots.
   New deterministic cases cover independent/overlapping misses, stale
   write/truncate, cancellation, queue bounds, allocation failure and victim
   retention; existing tests retain their assertions except the intentional
   `bool` to optional-copy-count internal return-type adaptation.
2. Compiler/lint: `cargo xtask clippy --package ax-fs-ng` passes 8/8 strict
   configurations, exit 0; host lib/test strict clippy with
   `host-test,ext4,vfs,profile` exits 0. `cargo xtask starry build --config
   apps/starry/macos-selfbuild/build-aarch64-unknown-none-softfloat.toml`
   builds the actual AArch64 profiling consumer, exit 0. It retains SMP=8,
   frame pointers and kernel profiling. Upstream future-incompatibility
   warnings remain visible; no lint suppression was added.
3. Final artifact/configuration audit: package fmt check and `git diff
   --check` exit 0; source archive comparison exits 0. Formal ELF is AArch64
   ET_DYN, no PT_TLS, with the new pending-wait/owned-publication symbols.
   Existing inherited RWE LOAD is not claimed as hardware read-only image
   protection. Explicit `qemu/test-concurrent-mmap` discovery selects the
   system wrapper; its build/runtime retain 8 CPUs and 8 GiB. No physical
   board or new runtime configuration is involved.

Logs: `tmp/cache-fill.dbJ0bd/{production-clippy,host-clippy,profile-build}.log`.
Saved candidate: `target/profiling/arceos-helloworld/starry/concurrent-cache-fill/`.

| Artifact | SHA-256 |
| --- | --- |
| ELF | `cc8b3d57a1f6b9e156c1a4a18bd7ca8c553c313b8f4fe067f05e26d6052255f8` |
| BIN | `34564fdd986e2e052e109e8855852c24e427359995dbed07a3c63b0c95239d10` |
| Source archive | `f38ff793c8bca0ca20c8d0fc5660b79af1e1dd3544cec921f1e0740f2b64a01b` |

The archive records selected modified filesystem/MM and frozen build inputs,
not a clean whole-repository commit. No project commit/push was performed.

## Retained pre-gate diagnostics

- First implementation patch was rejected before application because it
  deleted and added `populate.rs` in one patch. Applied as separate file
  operations without changing unrelated contents.
- Strict clippy first rejected constant `chunks_exact` and an Option
  let-else; replaced with `as_chunks` and `?`. Next test compilation caught
  the old bool assertion and the nightly-deprecated `fetch_update`; adapted
  the assertion and used `try_update`. Production base-feature clippy then
  caught the still-gated crate-root error converter export; moved the same
  existing converter into the unconditional internal export boundary.
  Every failure's original diagnostic was displayed; all final checks above
  pass after these corrections.
- Three path queries returned status 2 for nonexistent `components/ax-sync`,
  `test-pagecache-cap`, and `starry/cli.rs`. Actual paths were discovered as
  `os/arceos/modules/axsync`, `syscall-test-pagecache-cap`, and `starry/args.rs`.
  An exact demangled-name filter produced zero rows; matching `read_buf_at`
  and inspecting full stacks/ELF established the two exact wait sites above.

## Runtime status

After all three static rounds, the host library test command with the same
four features exits 0: **242 passed, 0 failed/ignored**, including all 11 new
fill tests. It verifies coalesced success/failure and exact backing-read
count, allocation rollback, stale content rejection, shortened EOF, victim
ownership and notification completion. These are compiled production-path
fixtures, not measurements of kernel speed or real SMP hardware ordering.

`cargo xtask starry test qemu --arch aarch64 -c qemu/test-concurrent-mmap`
exits 0: 12 assertions, one binary, 0.086 s. Its test file is in guest tmpfs;
it verifies mapping/COW integration, not cold disk miss concurrency.
`cargo xtask starry test qemu --arch aarch64
-c qemu/syscall-test-pagecache-cap` exits 0: all 1400 disk-backed files
created/mapped, observed memory delta 9341 KiB, one binary in 27.567 s.
This existing case samples mapped bytes; it is not a full-file data proof.
Both run the unchanged 8c8g system configuration with the patched QEMU.
Neither was run concurrently with the other. Test-image injection emits
four existing pre-delete `ext2_lookup` diagnostics per command, all shown
in full; actual tests then pass. No regex or expected behavior was relaxed.

The next formal working disk is a new sparse cold copy, full SHA matching
`9968004595dec398480417e210d64f9ed509801d4325d4ab22f43951983f5ea3`.
Pre/post runner-injection read-only fsck and runner dump/cmp exit 0. The
frozen runner SHA is `c94b86694e6f7e8b2c99b84c7c79ca2b1626323526add691d1636a238cd6f392`.
Source tar comparison and saved ELF/BIN SHA checks pass again after tests.
All regression/hash processes have ended. Prepared-disk SHA is
`58ae796fc12f6ff1d3f6707604b7e66c481eba9c6db73dd9a15bcc7f4c727c04`.
The prelaunch process check exits 1 with only
`PID ELAPSED COMMAND`, meaning no QEMU exists, not a guest failure.
This is not a full syscall compatibility audit. Old 1676/1694 results and
their read-only disks are unchanged. Formal QEMU session 11103 booted at
23:18:36 on 2026-09-11; the runner was sent once at 23:24. The GDB socket
remained unconnected, and no builds, edits or disk hashes ran during measurement.

## Complete-run acceptance and limits

The runner naturally reports `WINDOW-PASS elapsed=1684 rc=0` after all
181 units; QEMU PID 1066228/session 11103 exits 0. All six guest-generated
checksums pass. The complete sorted name/version multiset matches seven
previous Starry runs and the frozen Linux run, not just a count. The final
ELF matches Linux byte-for-byte, SHA-256
`2792c35275d91f57a38a847be87c4d2b333d7b88cd548bf9e06633288c7dba21`.
Post-shutdown read-only fsck exits 0; extent-tree narrowing suggestions were
displayed and no repair performed. The ended disk is preserved read-only at
`starry/concurrent-cache-fill/rootfs.img`, SHA-256
`d1e3a9800335099e0ed35f3610c424b7f99097a275c5eaeaee2f88f8ea2e2e00`.

The snapshot contains 22629 records, `prebuild_ns=1684150932432`, with
`dropped_cpu`, `dropped_wait`, `dropped_pending`, and `skipped_cpu` all zero.
All populated event SVGs were rendered; the CPU-active, mutex-wait and
ext4-lock-hold PNGs were also rendered and visually inspected. Source and
workload fingerprints remain the frozen values above. Linux remains 735 s;
the new single-run time ratio is 2.29116, not a repeated stable OS ratio.

| Full-run metric | Previous 1676 s | Concurrent fill 1684 s |
| --- | ---: | ---: |
| Active / total CPU samples | 56874 / 130028 | 54750 / 130696 |
| Active CPU sample fraction | 43.73981% | 41.89111% |
| memcpy leaf / active samples | 2563 / 4.50645% | 2380 / 4.34703% |
| try_zero_page leaf / active samples | 2847 / 5.00580% | 2354 / 4.29954% |
| find_free_area leaf samples | 1951 | 1855 |
| Aggregate mutex wait (ns) | 2434642484224 | 2411932033328 |
| Aggregate page-cache inclusive (ns) | 2362741774336 | 2361163035136 |
| Aggregate ext4 lock wait (ns) | 1398801779808 | 1427056498240 |
| Aggregate ext4 lock hold (ns) | 486435335584 | 486602683904 |
| Aggregate block-read sync (ns) | 754123205872 | 857085968640 |

Event aggregates overlap and can sum across tasks; they are not independent
wall-time components. Lower sample counts alone do not establish lower work
or a speedup, especially when active utilization also falls.

## Exact hotspot attribution

The largest resolved kernel CPU leaf is now memcpy (2380 samples), closely
followed by mandatory anonymous first-write clearing (2354). Of memcpy
samples, 1832 include `CowBackend::prepare_missing`, 1894 include CachedRead,
and 1936 include the full user-page-fault handler. One complete private-file
fault stack alone accounts for 1814 samples, with memcpy PC
`0xffffffff80544fd8` and CachedRead caller `0xffffffff8044f028`.
This is a cache-page-to-private-frame copy, not ext4 CPU execution.

The mutex caller ranking splits differently after inlining changes:
read_inode totals 388176746224 ns (16.09402%), CachedRead 362204680288 ns,
and FileBackend::read_buf_at 340991056032 ns. Comparing only those names
would misleadingly suggest the old read_buf_at wait disappeared. Exact
saved-ELF disassembly and complete raw stacks establish:

- `0xffffffff8044dc80` in CachedRead waits on the cache-index mutex at
  shared-object offset `0xb0`; its dominant private-fault stack is
  339069934784 ns. The prior index stack was 344292109856 ns.
- `0xffffffff803c612c` in the active Inode `io` module waits on the inode
  I/O mutex; its dominant private-fault stack is 359096513056 ns. Thus some
  serialization remains below the newly unlocked fill boundary.
- The largest ext4 hold caller remains lookup_locked: 203512351008 ns /
  41.82310% of ext4 hold. write_locked is next, 118692536208 ns / 24.39208%.

No end-to-end performance gain is accepted for this candidate. The next
investigation is ownership of private file-fault backing: Linux can map
read-only page-cache backing and copy on the first private write, whereas
the current CowBackend copies during every initial private read fault.
This requires complete lifetime, invalidation, fork, COW, forced-kernel-write,
EOF and rollback design; simply returning an unowned physical address is unsafe.
