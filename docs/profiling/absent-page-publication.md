# Absent-page publication: complete-run result

## Latest result: 1694 seconds

The frozen 8c8g QEMU workload naturally completed on 2026-09-11 at about
21:29:45: **1694 s, 181 compile units / 175 distinct crates, workload and QEMU
rc 0**. This is 9.36330% shorter than the accepted 1869 s control and
14.18440% shorter than the preceding 1974 s candidate. Linux remains 735 s;
Starry takes 2.30476 times as long. This is the new best observed complete
configuration; one run does not isolate host I/O variation or attribute every
saved second to absent publication. In particular the retained cached-read
refactor did not independently pass its earlier speed comparison.

All six raw artifact hashes passed; all 181 sorted name/version entries match
six controls, including Linux. The final workload ELF is byte-identical to
Linux and both recent Starry controls, SHA
`2792c35275d91f57a38a847be87c4d2b333d7b88cd548bf9e06633288c7dba21`.
Read-only fsck returned 0. The ended disk is preserved as read-only `rootfs.img`,
SHA `0ba106ab5304f825ae1845e15f6c8b0d6a1a24faa74063112de10a7b91f351c7`.
Post-run ELF/BIN checks passed 2/2. The fixed working disk path is absent.
All build, QEMU, renderer and disk-hash sessions have exited; no guest was
paused or cut off. Every SVG was rendered, and CPU-active, mutex-wait and
ext4-lock-hold PNGs were visually inspected.

The full profile has 131441 CPU samples, 56992 active and 74449 idle:
**43.35938% active**. No dropped/skipped CPU/wait/pending records were reported.
Low CPU occupancy is not solved; the 1869 s control was 45.40550% active.

| Full active-CPU leaf counts | Previous 1974 s | Current 1694 s |
| --- | ---: | ---: |
| `map_range_recursive` | 2802 | 59 |
| `try_zero_page` | 2513 | 2691 |
| `memcpy` | 2445 | 2402 |
| `find_free_area` | 2064 | 1911 |
| `spin_release` | 2000 | 1839 |
| `flush_batch` | 1245 | 1500 |

Full folded-stack aggregation, not only the renderer's top 30, gives 91
`install_missing`, 22 `install_absent_page` and 24 `install_entry` leaf samples.
`install_missing` inclusive samples are 423; user-fault inclusive samples are
8254/56992 (14.48274%). Inclusive stacks overlap and must not be added.

The new largest kernel leaf is page clearing, 2691/56992 (4.72172%). Exact
raw-PC counting finds 2649 at `0xffffffff8037f614`, the actual `dc zva` loop,
39 at loop exit, and three in entry/geometry checks. This is not an IRQ-enable
instruction misattributed to clearing. 2654 samples follow user fault ->
`CowBackend::prepare_frame` -> `allocate_frame`; 34 follow locked population,
and three have an inlined allocation parent. Linux already uses DC ZVA too;
any next optimization must target initialization/ownership policy rather than
claiming an unimplemented faster instruction. Of 2402 memcpy samples, 1866
follow resident `read_buf_at` -> `CachedRead::read_page`.

Waiting remains substantial: mutex 3272.56733 aggregate task-seconds, ext4 lock
wait 1942.40392, ext4 lock hold 621.02953, block read 859.39831, block write
131.35668 and flush 642.07111. These overlap across tasks and events, not
wall-clock contributions. The largest ext4 lock holder is lookup,
263.04709 s (42.35661%); dirty mutation follows at 144.92302 s (23.33593%).
The CPU leaf ranking does not prove ext4 waiting is irrelevant.

During offline export, `debugfs rdump` printed `Operation not permitted while
changing ownership` for the eight guest-root-owned files and their directory.
The complete raw warnings were directly displayed. Exported contents passed
all six checksums; only host ownership restoration was denied. No host
permission change or filesystem repair was performed. A post-exit `ps` check
returned 1 because the completed QEMU PID no longer existed; its execution
session independently returned 0. These are not workload failures.

## Complete-run evidence and decision

The previous `starry/cached-read-destinations` run completed all 181 compile
units (175 distinct crates), rc 0, in 1974 s. It is **not accepted as a speed
improvement** against `starry/private-fault-transactions` at 1869 s. All six
raw hashes, complete sorted name/version lists against four Starry controls
and Linux, final workload ELF equality and read-only fsck passed. See
[the complete report](cached-read-destinations.md).

Of 64229 active CPU samples, `map_range_recursive` has 2802 (4.362515%).
Full raw-PC counting against the saved ELF finds 2749 at
`0xffffffff80046f1c`: `dsb sy` immediately follows `tlbi vaae1is` at `...6f18`.
Thus 98.10849% of this leaf samples the TLBI/completion region. QEMU interrupt
delivery can place a sample after an expensive helper; this is not a measured
physical-CPU DSB latency. The distinct ax-mm monomorphization at
`0xffffffff80330138..80330580` has zero samples; selecting it by name alone
would give the wrong conclusion. Saved disassembly:
`tmp/preemption-refactor.5NL1NQ/cached-read-map.asm`.

2727 of the 2802 mapper samples have `PrivateFault::install_missing` as the
nearest non-recursive parent. ext4 still contributes substantial lock waiting,
but this highest kernel CPU leaf is not an ext4 operation. Do not sum inclusive
CPU stacks or overlapping task wait durations as wall time.

## Implemented boundary

See [design and fixed-version prior art](../design/absent-page-publication.md).
The new generic `install_absent_page` transaction builds missing table branches
off-tree with rollback ownership, preserves opaque configurations, and rejects
occupied/non-present/huge conflicts. Successful installation calls one distinct
architecture publication operation. AArch64 uses `dsb ishst; isb`, without
TLBI; other architectures keep conservative invalidation. Only revalidated
private missing faults opt in. Replacement, write revocation and retirement
retain their strong invalidation boundaries.

Static auditing found a prerequisite: old `unmap_page` did not complete the
leading PTE-store barrier if a neighbor kept its child table alive. Address
moves and other backends can reach that boundary before a subsequent private
mapping. Single-page unmap now reuses `unmap_owned`, retains the original
physical/configuration/page-size result and checks end-address overflow.
It reclaims only table frames; the caller retains mapped data through return.

## Deterministic RED evidence

Before fixing single-page unmap, three host-RED admission rounds completed:

1. Source/ownership review, including every new branch failure, opaque and
   non-4K geometry, missing-fault revalidation and previous-removal callers.
   The known single-unmap gap admitted only the host reproducer, never QEMU.
2. Final production clippy: page-table-generic 2/2, ax-cpu 33/33, Starry
   110/110, all rc 0. Final host fixture clippy rc 0 with existing warnings
   visible; fmt/diff check and exact profile build rc 0.
3. Final caller/test discovery, source tar compare rc 0 and actual ELF review:
   ET_DYN/no PT_TLS, install leaf/parent release ordering, rollback calls,
   `install_missing` calls install then `dsb ishst; isb`, without TLBI.

The real-table neighbor-preserving test then failed deterministically, rc 101:

```text
assertion `left == right` failed: single-page removal bypassed descriptor publication/completion
  left: [Invalidate(Some(VA:0x200000))]
 right: [Retire(VA:0x200000)]
```

Full compiler warnings, assertion, panic context and status are preserved in
`tmp/preemption-refactor.5NL1NQ/absent-unmap-red.log` and were directly displayed.
The completion hook additionally observes that the actual removed descriptor
is absent. The old source archive SHA is
`f8e619ac620c0f47a2173e321ec7f77400c4244b74000d6aad15256d3972a103`;
the RED profile ELF is
`fb3be871535b87216b3d159f7e13f9ad59b66c0cd80062e1e7b371c9dd7e4684`,
both under `tmp/preemption-refactor.5NL1NQ/absent-red/`.
That ELF lacks the prerequisite fix and must not be benchmarked.

Earlier build failures remain in `absent-red-*.log`: E0283 required explicit
`T,A` at branch/frame construction; strict host-test clippy then exposed ten
pre-existing shared mock issues. No `allow` was added. Production matrices are
strict; host fixture typechecking preserves those warnings and must not be
reported as a warning-free strict pass.

## Current final validation

The prerequisite fix and complete transaction are implemented. Final three
rounds completed on the fixed sources at 20:58 on 2026-09-11: source and
ownership audit; generic 2/2, ax-cpu 33/33 and Starry 110/110 strict clippy,
host fixture clippy with existing warnings, fmt/diff and profile/axtest builds;
then final caller/test discovery, source archive comparison and saved-ELF
instruction/identity checks. Every command exited 0. The known host mock
warning limitation above remains; no warning-free host check is claimed.
The unchanged RED case then passed within the complete generic host suite
(90/90, rc 0). Actual 8c8g Starry kernel axtests then passed 53/53, rc 0,
including private missing/COW preparation and interleavings. The same frozen
181-unit workload subsequently completed in patched QEMU; the measured result
and limitations are recorded above.

The candidate is saved in
`target/profiling/arceos-helloworld/starry/absent-page-publication/`:

- ELF: `01e79a74d95b6c02bba351bab7d672a2848747c89fa40ed6e6b727a39f6319f6`.
- BIN: `8d36396b7f547618c55823a3a32dda929ebddac3d7c8796251e83002cf1c3ff8`.
- Source archive: `fb1a3f2e0a81ab8e0fe05b1f9fb9132bf3c48c3e9f6eab30ebd6bda24054c974`.
  It contains the affected paging/CPU/Starry/fs/axio sources, manifests,
  toolchain, app runner/configs and design; it is not a clean whole-workspace
  checkout or a replacement for preserved earlier dirty-input archives.
- Build-only axtest ELF before kallsyms postprocessing:
  `49176cfb411828d8637469f0c92173e65e5b88449034a081b6f0357467f480d2`.
- Cold copy before runner replacement:
  `9968004595dec398480417e210d64f9ed509801d4325d4ab22f43951983f5ea3`.
- Prepared working disk:
  `03bd3db8a4ef5ab31f5038d6732c76c899e9fb3c8135fbaebf3f4a049fc84a56`.

In this final profile ELF, `install_missing` at `0xffffffff8025b9b4` calls
`install_absent_page` at `0xffffffff80029528`. Its success path has `dsb ishst`
at `0xffffffff80029608`, then `isb`, without TLBI. `install_entry` releases
child/backing initialization before the leaf/parent stores. `unmap_page` calls
`remove_range`, then `flush_batch` before the first allocator free. The actual
batch implementation at `0xffffffff8037e904` retains leading `dsb ishst`,
inner-shareable TLBI, trailing `dsb ish; isb`. Disassembly is preserved in
`tmp/preemption-refactor.5NL1NQ/absent-*.asm`. These are static facts supported
by the subsequent QEMU result, not evidence of physical-board performance.
