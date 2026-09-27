# Absent-page installation and publication

## Evidence and scope

The complete cached-read-destinations build finished in 1974 s, slower than
the accepted 1869 s control. Its largest kernel CPU leaf is map_range_recursive
(2802/64229 active samples). Exact saved-ELF symbolization finds 2749 samples
at 0xffffffff80046f1c, the DSB SY immediately after TLBI VAAE1IS. That is
98.10849% of this leaf, not evidence that recursive index arithmetic dominates.
2727/2802 samples have PrivateFault::install_missing as the nearest
non-recursive caller. The full source, ELF, profile and disk remain frozen.

Separate an absent base-page install from replacement/protection/retirement.
The high-risk shared paging interface must express why invalidation can be
omitted, preserve physical ownership, and publish fully initialized subtrees.
Only the revalidated Starry private missing-fault caller will opt in initially.
Other existing mapping APIs, file/shared backends, huge pages, COW replacement,
fork protection and owned retirement retain their established invalidation behavior.
Single-page unmap must join that completed retirement boundary before this opt-in.
No new lock-free page-table mutation, ASID scheme, syscall or disk format is
introduced. ext4 lock waiting remains a separate measured bottleneck.

Success requires deterministic absent/conflict/OOM/ownership/publication tests,
actual architecture instructions, and a complete identical-workload cold build
with lower elapsed time. A cheaper hook alone is not an accepted speed result.

## Prior art and alternatives

- Linux 980ab36ae5972c83f683b939e50c469c4947229e:
  [memory.c](https://github.com/torvalds/linux/blob/980ab36ae5972c83f683b939e50c469c4947229e/mm/memory.c#L426)
  pmd_install orders child initialization before parent publication; the
  non-present install path does not invalidate an old translation.
  [arm64 pgtable.h](https://github.com/torvalds/linux/blob/980ab36ae5972c83f683b939e50c469c4947229e/arch/arm64/include/asm/pgtable.h#L360)
  separates PTE stores/cache maintenance from TLB invalidation;
  update_mmu_cache_range at line 1574 permits a spurious user refault.
  The local files were read, including callers and completion helpers. A web
  fetch of the fixed raw URL returned `Cache miss`; the local pinned checkout
  remains the exact implementation evidence, not an invented online response.
- Arm Learn the Architecture, Memory Management, 101811_0100_00_en:
  [translation caching rules](https://developer.arm.com/-/media/Arm%20Developer%20Community/PDF/Learn%20the%20Architecture/LearnTheArchitecture-MemoryManagement-101811_0100_00_en.pdf?revision=1fdc3375-d81c-4457-b786-04fb98557de0)
  exclude translations that result in a translation fault. This does not
  excuse stale valid translations left by an incomplete earlier removal.
- MOSS 5a54e4413c9657bfb531cc32f6d688090465200d,
  vendor/page_table_multiarch/src/bits64.rs:249-305: initialize then CAS-publish
  child tables and absent 4K leaves; replacement has separate invalidation.
  Reuse the operation distinction, not its concurrent raw-entry model: Starry
  still excludes software mutation with the address-space lock.
- Current history 4737af90a unified generic page-table execution. Current
  page-table-generic already owns opaque PTE configurations and completed
  retirement; extend this boundary instead of writing PTE bits in Starry.
  Open PR #2009 at d026ff43e1375cc23009d38916462e9c5e81eb24 was read in full;
  it concerns intermediate-table retirement, not absent installs, and the
  local retirement implementation already has stronger deferred-flush safety.
  #2000 at 6c1b006a852a458ad577f92ea8a5978a56fb4e59 changes fault-in windows,
  not publication. The title search is not an exhaustive all-PR review.

Keeping map_page retains the demonstrated broadcast cost. Setting flush=false
at one caller gives no distinct publication/ownership contract. Changing every
map_page caller would silently assume all prior removals completed, including
users of deferred flush. Rewriting all recursive range walking targets only
53 of these 2802 samples. Choose a bounded, explicit absent-install transaction.

## Transaction and capabilities

PageTableRef::install_absent_page is an unsafe, base-page-only entry. The caller
must own initialized backing memory, exclude other software table mutations,
retain the backing until a successful unmap completes, and establish that no
previous valid translation to this virtual address remains on any permitted
hardware user. An unused descriptor alone cannot prove the last condition.
Addresses are checked for alignment, overflow and the metadata's width policy.
Non-present occupied leaves and huge mappings are conflicts, never overwritten.

The structural install descends existing valid tables. At the first missing
child it owns an unpublished branch: allocate/zero and build the entire lower
chain before linking its parent. Allocation failure drops only that unpublished
branch; the visible table and caller's data owner remain unchanged. A release
fence orders child/data initialization before a new parent/leaf descriptor.
No broad lock, callback, new architecture bits or data-page ownership enters
the generic engine. A private RAII branch owner releases table frames, never
the caller's mapped data frame.

TableMeta::publish_new_mapping is a distinct completion capability, used once
after successful structural insertion. Its default calls flush_batch for
architectures that have not opted into a narrower guarantee. AArch64 completes
PTE stores with DSB ISHST and local context synchronization with ISB, without
TLBI. This is deliberately more conservative than Linux's user no-op; it does
not change existing flush_tlb, replacement or retirement semantics, nor claim
new remote-shootdown support on other architectures.

PrivateFault::commit still rechecks VMA identity, access and PTE under the same
address-space lock. Only FaultSource::Missing uses the new entry; COW replacement
retains break-before-make and both invalidation boundaries. Previous Starry COW
unmaps complete unmap_owned/flush_batch before returning and releasing ownership.
Single-page removals also enter unmap_owned: mremap, shared/file/linear backends
and clone rollback must not leave a stale translation when an address is later
reused by a private mapping. The former unmap_page called flush(Some) without
the leading descriptor-publication barrier when a neighbor kept the child table
alive. An unused PTE alone was therefore not sufficient. Keep its original
physical/configuration/page-size return values, alignment and error order;
the existing retirement engine owns table reclamation, while the caller keeps
the data owner until return. A checked end address preserves overflow rejection.
This strengthens completion rather than introducing a new syscall ABI; it does
not certify all mmap/mremap/fork semantics or remote shootdown on other targets.
Successful installation transfers PreparedFrame to its PTE and records the
already-checked empty RSS charge. Errors retain RAII ownership and no charge.

## Validation and rollback

Before any runtime test, complete three rounds: ownership/source review; fmt,
targeted component and Starry clippy plus exact profile/axtest builds; then final
source/caller/test discovery and saved-ELF instruction consistency checks.
Host tests must exercise real tables: publication sees the installed descriptor,
exactly one completion, conflict/non-present/huge preservation, all intermediate
OOM positions with unchanged visible root/no leak, neighbor reuse, root geometry,
alignment/overflow, and conservative default completion. Keep existing
protection/remap/retirement tests and actual Starry missing/COW interleavings.
Architecture code is checked in the final ELF, not by parsing Rust spelling.

Then run relevant host suites and 8c8g Starry ktest, and the entire frozen
tg-xtask workload in the same patched QEMU. Require raw SHA, all 181 name/version
entries, final ELF equality and read-only fsck. The cached-read 1974s candidate
is not an accepted baseline; compare with both it and accepted 1869s, and retain
all evidence. Rollback only this transaction/caller opt-in, never prior unrelated
dirty changes. No project commit/push/merge or physical board run is authorized.
