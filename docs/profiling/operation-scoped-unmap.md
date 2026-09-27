# Operation-scoped unmap: evidence and validation

## State

Connected candidate implemented; all three final static rounds passed. Host
components and the 8-core / 8-GiB QEMU kernel suite passed. Full-build profiling
is running; no throughput gain is claimed.
The accepted comparison baseline remains the complete 1087-second VMA-gap-index
run, 181 compile units, 8 cores / 8 GiB / NVMe, with reusable rootfs tg-xtask.

See [design](../design/operation-scoped-unmap.md) for measured hotspot attribution,
pinned Linux prior art, alternatives and lifetime requirements.

## Implemented boundary

- PageTable owns a bounded multi-range UnmapSession. Capacity, errors, finish
  and Drop flush before releasing physical/table owners. A synchronous table
  handoff drains first and adopts a replacement allocator only while empty.
- MemorySet exposes callback unmap/clear using the existing fragment state
  machine. Metadata order, partial errors, whole-area panic and gap reconciliation
  are preserved. Existing APIs delegate; MappingBackend itself is unchanged.
- Starry owns detached COW frames, RSS charge identity and cache registrations
  until invalidation completes. Whole unmap, clear and private discard share
  the session. Other backends keep synchronous lifetime completion.

The architecture's existing flush_batch implementation and capacity/thresholds
are unchanged. No filesystem, driver, boot, syscall argument or errno behavior
is intentionally changed. Indirect consumers include munmap, replacement mmap,
mremap cleanup/shrink, brk shrink, madvise discard, exec image replacement and
final address-space destruction. This is not a complete syscall compatibility
review or a maintainer-approved merge decision.

## Static findings retained

Artifacts: `tmp/operation-unmap.4U8Sgf/`. Every failure below was printed in full.

1. Incorrect test-directory and old xtask source paths returned rg exit 2 / cat
   exit 1. Actual paths were discovered and read; no source or data was lost.
2. Strict page-table all-targets Clippy exited 101 on existing fixtures in mocks,
   flags, map, translate and loongarch64_simulation. All five files were compared
   byte-for-byte with the before archive via tar -d, exit 0. No allow or unrelated
   test cleanup was added. Project library/copy-from checks and the affected
   retirement test target pass strict Clippy. Broad old-test lint cleanliness
   is not claimed.
3. Rustdoc initially warned about the new PageTable::unmap_owned link: the method
   belongs to PageTableRef. The link was corrected. Preexisting frame/LEVEL_BITS
   links and MemorySet README bare URL were also reported, not silenced.
4. Actual kernel Clippy/build report existing future-incompatibility notices in
   memchr and toolchain core; their commands returned 0.

Deterministic tests now cover cross-range grouping, bounded capacity, malformed
and partial-huge errors, preparation failure, Drop, inaccessible/huge leaves,
table replacement and allocator provenance; MemorySet fragment/error ordering;
130 real COW VMAs with fork owners; cache listener retention during real eviction;
and synchronous Shared-backend handoff.

## Completed admission and regression

Final admission log: `tmp/operation-unmap.4U8Sgf/final-static.log`.
Every command's terminal exit status was checked before admitting the next step.

1. Lifecycle/call-chain audit, targeted formatting and diff checks; project
   Clippy for page-table-generic, ax-memory-set and ax-hal (13 configurations);
   strict Clippy for the actual profiling kernel and AArch64 kernel-test target.
2. Strict retirement-target Clippy with copy-from, MemorySet all-target Clippy,
   documentation, actual profiling kernel build including kallsyms/BIN refresh,
   and byte comparison of all 84 archived source files.
3. Final formatting/diff/source checks, saved ELF/BIN identity, independent BIN
   derivation from the saved ELF and SHA-256 recording.

A counterfactual eager-flush mutation underwent its own three static rounds.
The unchanged disjoint-range regression then failed deterministically, exit 101:
`range ended the surrounding operation`. This is a mutation RED, not a claim
that byte-identical original production code was executed. The candidate was
restored byte-identically and all three final admission rounds repeated before
the accepted tests below. Logs: `mutation-static.log`, `mutation-red.log`.

- MemorySet: 30 tests and 1 documentation test passed, exit 0.
- Page-table-generic with copy-from: all 101 tests passed, including 21 retirement
  tests and the 8 new session scenarios, exit 0.
- Actual QEMU kernel suite: `AXTEST_SUMMARY pass=56 fail=0 skip=0 total=56`,
  `AXTEST_SUITE_OK`, exit 0. The new mapping-owner test was discovered and passed.
  Configuration: AArch64, 8 cores, 8 GiB, NVMe, snapshot test rootfs.

Host logs: `component-tests.log`. The first kernel attempt built successfully
but failed before QEMU while fetching the image registry, exit 1:
`client error (Connect): tls handshake eof`. Its complete output was displayed
and saved as `kernel-registry-failure.log`. A read-only request returned HTTP 200;
the unchanged original command then passed. The TLS EOF's underlying cause is
not established. Retry terminal chunk `c3f5d7` contains the suite summary and
exit 0, but 141 tokens of middle output were truncated by the terminal tool;
there is no claim of a complete separately captured retry log.

After the tests, all 84 selected source files and the final profiling ELF/BIN
were compared again, all exit 0. Final artifacts (not the earlier static build):

- `starryos-final.elf`: `ded4dc497c21c661c6553068a4b2bbe791c4b3acb825c8bc4ac05fa00e8a9a5a`
- `starryos-final.bin`: `e2916eaf07dbe3cc8508c5cd36e2ca082b8c79104128814e029582800e09deaa`
- `sources-admitted.tar`: `8444798c0c35bd81ae3eb7534bb8592f9499b06a90283846ade92cafcf7a87f0`

The profiling run directory is
`target/profiling/arceos-helloworld/starry/operation-scoped-unmap`.
Its saved kernel and selected-source archive match these hashes. The frozen
base was copied to the normal working image path. The complete hash matched
before replacing only the fixed runner; readonly fsck before/after returned 0.
Runner dump matched all 8657 bytes, mode 0755; tg-xtask SHA-256 stayed identical.
Prepared rootfs SHA-256:
`7db74bb4d1cdccf023f06bcbb3fbaec40e64d461347854bcb5be24887acdfad4`.
All preparation commands finished before QEMU booted at 20:54:25 UTC on
2026-09-12. PID 1327426, terminal session 27621; the runner was sent once after
the shell prompt, between 20:54:42 and 20:54:43 UTC. No deadline, second QEMU,
concurrent host build or live-image offline inspection is permitted in this run.

## Recoverable disk cleanup

The completed cache-writeback-ownership run's 16-GiB logical ended image was
compressed with zstd -T2 -3 --keep. All 17179869184 original/decompressed bytes
were compared and SHA-256 matched its recorded ended image:
524b980721c5bd0312649625fca4bea1f5764ec5aa7828b7f310a7c35f5316c2.
The compressed copy is mode 0444. fuser returned 1 (no users), then only the
verified uncompressed duplicate was removed. All profiler/output evidence stays.
Recovery uses `zstd -d --keep rootfs.img.zst` from that run directory.
Compressed SHA-256:
`af90982f61767c2e03fc7a718c8440de8d2e27c2d9b78c389ac2309bd52455c6`.
