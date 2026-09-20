use alloc::{
    collections::{BTreeMap, btree_map::Entry},
    sync::Arc,
    vec::Vec as AllocVec,
};
use core::{
    mem,
    ops::Bound::{Excluded, Unbounded},
    sync::atomic::{AtomicBool, Ordering},
};

use axfs_ng_vfs::VfsResult;
use heapless::Vec as InlineVec;

use super::{CachedFileShared, PageCache};

const MAX_RECLAIM_BATCH: usize = 256;
const REGISTER_PRUNE_INTERVAL: usize = 64;
const REGISTER_PRUNE_BATCH: usize = 64;

struct ReclaimGuard;

impl Drop for ReclaimGuard {
    fn drop(&mut self) {
        RECLAIM_IN_PROGRESS.store(false, Ordering::Release);
    }
}

struct CachedFileRegistry {
    state: ax_sync::SpinRwLock<CachedFileRegistryState>,
}

struct CachedFileRegistryState {
    files: BTreeMap<usize, Arc<CachedFileShared>>,
    next_prune_after: Option<usize>,
    registrations_until_prune: usize,
}

impl CachedFileRegistryState {
    const fn new() -> Self {
        Self {
            files: BTreeMap::new(),
            next_prune_after: None,
            registrations_until_prune: REGISTER_PRUNE_INTERVAL,
        }
    }
}

impl CachedFileRegistry {
    const fn new() -> Self {
        Self {
            state: ax_sync::SpinRwLock::new(CachedFileRegistryState::new()),
        }
    }

    #[cfg(feature = "ext4")]
    fn release(&self, file: &Arc<CachedFileShared>) {
        let removed = self
            .state
            .write()
            .files
            .remove(&cached_file_registry_key(file));
        drop(removed);
    }

    fn register(&self, file: &Arc<CachedFileShared>) -> usize {
        let key = cached_file_registry_key(file);
        let should_prune = {
            let mut registry = self.state.write();
            match registry.files.entry(key) {
                Entry::Occupied(_) => return 0,
                Entry::Vacant(entry) => {
                    entry.insert(file.clone());
                }
            }
            registry.registrations_until_prune -= 1;
            if registry.registrations_until_prune == 0 {
                registry.registrations_until_prune = REGISTER_PRUNE_INTERVAL;
                true
            } else {
                false
            }
        };

        if should_prune {
            self.prune_some(REGISTER_PRUNE_BATCH)
        } else {
            0
        }
    }

    fn prune(&self) {
        self.prune_with(|| {});
    }

    fn prune_with(&self, before_restore: impl FnOnce()) {
        // Cached-file destruction can take a sleepable filesystem lock.
        let mut files = {
            let mut registry = self.state.write();
            registry.next_prune_after = None;
            mem::take(&mut registry.files)
        };
        files.retain(|_, cached| Arc::strong_count(cached) > 1 || cached.has_dirty_pages());
        before_restore();
        for (_, file) in files {
            self.restore_after_prune(file);
        }
    }

    fn prune_some(&self, max_scan: usize) -> usize {
        let scan_len = self.state.read().files.len().min(max_scan);
        let mut scanned = 0;
        for _ in 0..scan_len {
            let Some(file) = self.take_next_prune_candidate() else {
                break;
            };
            scanned += 1;
            if Arc::strong_count(&file) > 1 || file.has_dirty_pages() {
                self.restore_after_prune(file);
            }
        }
        scanned
    }

    fn take_next_prune_candidate(&self) -> Option<Arc<CachedFileShared>> {
        let mut registry = self.state.write();
        let key = registry
            .next_prune_after
            .and_then(|cursor| {
                registry
                    .files
                    .range((Excluded(cursor), Unbounded))
                    .next()
                    .map(|(&key, _)| key)
            })
            .or_else(|| registry.files.first_key_value().map(|(&key, _)| key))?;
        registry.next_prune_after = Some(key);
        registry.files.remove(&key)
    }

    fn restore_after_prune(&self, file: Arc<CachedFileShared>) {
        let mut pending = Some(file);
        {
            let mut registry = self.state.write();
            let file = pending.as_ref().expect("prune candidate must be present");
            // Unlink and retirement publish their flags before taking this
            // registry lock. Either this recheck observes the transition, or
            // the later release removes the restored entry by its stable key.
            if !file.unlinked.load(Ordering::Acquire) && !file.retired.load(Ordering::Acquire) {
                let key = cached_file_registry_key(file);
                if let Entry::Vacant(entry) = registry.files.entry(key) {
                    entry.insert(pending.take().expect("prune candidate must be present"));
                }
            }
        }
        // Cached-file destruction may take sleepable locks, so the last Arc is
        // always dropped after releasing the registry spin lock.
        drop(pending);
    }
}

static GLOBAL_CACHED_FILES: CachedFileRegistry = CachedFileRegistry::new();
static RECLAIM_IN_PROGRESS: AtomicBool = AtomicBool::new(false);

fn cached_file_registry_key(file: &Arc<CachedFileShared>) -> usize {
    Arc::as_ptr(file) as usize
}

fn visit_registered_cached_file_after<R>(
    after: Option<usize>,
    visit: impl FnOnce(&Arc<CachedFileShared>) -> R,
) -> Option<(usize, R)> {
    // Retain registry read ownership instead of cloning an Arc that could
    // become the last file owner during concurrent pruning. The visitor only
    // uses try-lock clean eviction and never allocates or invokes callbacks.
    let registry = GLOBAL_CACHED_FILES.state.try_read()?;
    let (&key, file) = match after {
        Some(cursor) => registry.files.range((Excluded(cursor), Unbounded)).next()?,
        None => registry.files.first_key_value()?,
    };
    Some((key, visit(file)))
}

/// Reclaims clean disk-backed cache pages without holding listener callbacks
/// under the page-cache lock.
pub fn page_cache_reclaim(num_pages: usize) -> usize {
    if RECLAIM_IN_PROGRESS.swap(true, Ordering::AcqRel) {
        return 0;
    }
    let _guard = ReclaimGuard;

    let mut reclaimed = 0;
    let target = num_pages.max(16).saturating_mul(2);
    let mut visited_files = 0;
    let scan_len = {
        let Some(registry) = GLOBAL_CACHED_FILES.state.try_read() else {
            return 0;
        };
        registry.files.len()
    };
    let mut cursor = None;

    // Borrow each registry owner while trying the file locks; pressure reclaim
    // cannot become a cached file's final owner or allocate a snapshot Vec.
    // Ordered keys let the cursor advance without repeated index walks.
    // Concurrent pruning may remove an entry; reclaim is a best-effort scan,
    // so a later allocator retry can revisit anything skipped here.
    for _ in 0..scan_len {
        let Some((key, freed)) = visit_registered_cached_file_after(cursor, |file| {
            file.try_evict_clean_pages(target - reclaimed)
        }) else {
            break;
        };

        cursor = Some(key);
        reclaimed += freed;
        visited_files += 1;
        if reclaimed >= target {
            break;
        }
    }
    // The remaining quota goes to the block-layer cache trees; like the
    // page cache above, only clean folios are reclaimable here.
    #[cfg(any(feature = "ext4", feature = "fat"))]
    if reclaimed < target {
        let freed = crate::block::cache::reclaim_clean_folios(target - reclaimed);
        if freed > 0 {
            debug!("page_cache_reclaim: evicted {freed} clean block-cache folios");
        }
        reclaimed += freed;
    }

    if reclaimed > 0 {
        debug!(
            "page_cache_reclaim: evicted {} clean pages across {} files",
            reclaimed, visited_files
        );
    }
    reclaimed
}

pub(super) fn register_cached_file(file: &Arc<CachedFileShared>) {
    GLOBAL_CACHED_FILES.register(file);
}

/// Drops reclaim ownership after inode reaping or final mount-cache writeback.
///
/// The removed `Arc` is dropped only after releasing the registry spin lock:
/// destroying a cached file can take its sleepable page-cache lock.
#[cfg(feature = "ext4")]
pub(super) fn release_cached_file(file: &Arc<CachedFileShared>) {
    GLOBAL_CACHED_FILES.release(file);
}

pub fn sync_all_cached_files(_data_only: bool) -> VfsResult<()> {
    sync_cached_files(None)
}

fn sync_cached_files(filesystem: Option<&dyn axfs_ng_vfs::FilesystemOps>) -> VfsResult<()> {
    let files: AllocVec<_> = GLOBAL_CACHED_FILES
        .state
        .read()
        .files
        .values()
        .cloned()
        .collect();
    let mut first_error = None;
    for file in &files {
        if let Some(filesystem) = filesystem
            && !file.backing.as_ref().is_some_and(|backing| {
                super::filesystem_key(backing.filesystem()) == super::filesystem_key(filesystem)
            })
        {
            continue;
        }
        if let Err(error) = file.writeback_dirty_for_global_sync()
            && first_error.is_none()
        {
            first_error = Some(error);
        }
    }

    drop(files);
    prune_cached_files();
    first_error.map_or(Ok(()), Err)
}

/// Writes back cached files belonging to one filesystem before its unmount.
pub fn sync_filesystem_cached_files(filesystem: &dyn axfs_ng_vfs::FilesystemOps) -> VfsResult<()> {
    sync_cached_files(Some(filesystem))
}

fn prune_cached_files() {
    GLOBAL_CACHED_FILES.prune();
}

impl CachedFileShared {
    /// Scans the LRU and evicts up to `max` clean pages.
    ///
    /// This allocator-pressure path is allocation-free and only detaches pages
    /// from files without a live mapping endpoint.
    fn try_evict_clean_pages(&self, max: usize) -> usize {
        self.try_evict_clean_pages_with(max, || {})
    }

    fn try_evict_clean_pages_with(&self, max: usize, before_detach: impl FnOnce()) -> usize {
        // Hold endpoint exclusion until every victim has left the cache.
        // A Weak with zero strong refs cannot be resurrected; inspecting it
        // avoids acquiring an Arc whose last Drop might run arbitrary code in
        // allocator-pressure context. Leave tombstone cleanup to normal I/O.
        let Some(installed) = self.mapping_endpoint.try_lock() else {
            return 0;
        };
        if installed
            .as_ref()
            .is_some_and(|endpoint| endpoint.strong_count() != 0)
        {
            return 0;
        }
        before_detach();

        let limit = max.min(MAX_RECLAIM_BATCH);
        let mut pending: InlineVec<PageCache, MAX_RECLAIM_BATCH> = InlineVec::new();
        let Some(mut cache) = self.page_cache.try_lock() else {
            return 0;
        };
        let mut to_pop = [0u32; MAX_RECLAIM_BATCH];
        let mut count = 0;
        for (&pn, page) in cache.iter().rev() {
            if !page.dirty && page.pins == 0 && count < limit {
                to_pop[count] = pn;
                count += 1;
            }
        }
        for &pn in &to_pop[..count] {
            if let Some(page) = cache.pop(&pn) {
                // There is one push per selected key and count <= capacity.
                if pending.push(page).is_err() {
                    unreachable!("reclaim batch exceeds its selected victim count");
                }
            }
        }

        let evicted = pending.len();
        drop(cache);
        drop(installed);
        drop(pending);
        evicted
    }
}

#[cfg(test)]
struct ReclaimTestEndpoint {
    invoked: Arc<AtomicBool>,
}

#[cfg(test)]
impl super::CacheMappingEndpoint for ReclaimTestEndpoint {
    fn publish(&self, _event: super::CacheMappingEvent) -> super::CacheMappingResult {
        self.invoked.store(true, Ordering::Release);
        super::CacheMappingResult::Retired
    }
}

#[cfg(test)]
fn pressure_reclaim_is_allocation_free_and_skips_live_mappings_for_test() -> bool {
    const RECLAIM_PAGES: usize = 32;

    let file = Arc::new(CachedFileShared::new_unbounded(
        (RECLAIM_PAGES * crate::os::memory::PAGE_SIZE) as u64,
    ));
    for page_number in 0..RECLAIM_PAGES as u32 {
        file.page_cache
            .lock()
            .put(page_number, PageCache::detached_for_test());
    }

    let invoked = Arc::new(AtomicBool::new(false));
    let endpoint: Arc<dyn super::CacheMappingEndpoint> = Arc::new(ReclaimTestEndpoint {
        invoked: Arc::clone(&invoked),
    });
    *file.mapping_endpoint.lock() = Some(Arc::downgrade(&endpoint));

    let protected = file.try_evict_clean_pages(RECLAIM_PAGES);
    let protected_pages = file.page_cache.lock().len();
    drop(endpoint);
    let reclaimed = file.try_evict_clean_pages(RECLAIM_PAGES);
    protected == 0
        && protected_pages == RECLAIM_PAGES
        && reclaimed == RECLAIM_PAGES
        && !invoked.load(Ordering::Acquire)
}

#[cfg(test)]
mod tests {
    use std::{
        alloc::{GlobalAlloc, Layout, System},
        cell::Cell,
    };

    use super::*;

    std::thread_local! {
        static ALLOCATIONS: Cell<Option<usize>> = const { Cell::new(None) };
    }

    struct ObservedAllocator;

    // SAFETY: all allocation semantics are delegated unchanged to System.
    // The const thread-local Cell cannot allocate or observe another thread.
    unsafe impl GlobalAlloc for ObservedAllocator {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            let _ = ALLOCATIONS.try_with(|count| {
                if let Some(value) = count.get() {
                    count.set(Some(value + 1));
                }
            });
            // SAFETY: preserve the caller's GlobalAlloc layout contract.
            unsafe { System.alloc(layout) }
        }

        unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
            // SAFETY: this allocation came from System with the same layout.
            unsafe { System.dealloc(ptr, layout) }
        }
    }

    #[global_allocator]
    static ALLOCATOR: ObservedAllocator = ObservedAllocator;

    #[test]
    fn pressure_reclaim_endpoint_race_never_reallocates_cache_nodes() {
        let file = CachedFileShared::new_unbounded(4096);
        file.page_cache
            .lock()
            .put(0, PageCache::detached_for_test());
        let endpoint: Arc<dyn super::super::CacheMappingEndpoint> = Arc::new(ReclaimTestEndpoint {
            invoked: Arc::new(AtomicBool::new(false)),
        });
        let mut installed_during_reclaim = false;
        ALLOCATIONS.with(|count| count.set(Some(0)));
        let reclaimed = file.try_evict_clean_pages_with(1, || {
            if let Some(mut installed) = file.mapping_endpoint.try_lock() {
                *installed = Some(Arc::downgrade(&endpoint));
                installed_during_reclaim = true;
            }
        });
        let allocations = ALLOCATIONS.with(|count| count.replace(None).unwrap());
        assert_eq!(
            allocations, 0,
            "allocator-pressure rollback must not allocate new LRU nodes"
        );
        assert!(
            !installed_during_reclaim,
            "endpoint publication must be excluded until detachment is complete"
        );
        assert_eq!(reclaimed, 1);
        assert!(file.page_cache.lock().is_empty());
    }

    #[test]
    fn allocator_pressure_reclaim_skips_mapped_pages_without_callbacks() {
        assert!(pressure_reclaim_is_allocation_free_and_skips_live_mappings_for_test());
    }

    #[test]
    fn registry_registration_uses_bounded_prune_batches() {
        let registry = CachedFileRegistry::new();
        let mut live_files = Vec::new();
        for _ in 0..REGISTER_PRUNE_INTERVAL - 1 {
            let cached = Arc::new(CachedFileShared::new_unbounded(0));
            assert_eq!(registry.register(&cached), 0);
            live_files.push(cached);
        }

        let last = Arc::new(CachedFileShared::new_unbounded(0));
        assert_eq!(registry.register(&last), REGISTER_PRUNE_BATCH);
        assert_eq!(registry.state.read().files.len(), REGISTER_PRUNE_INTERVAL);

        // Re-registering the same cache owner is O(log n), idempotent, and
        // does not advance the bounded-prune cadence.
        assert_eq!(registry.register(&last), 0);
        assert_eq!(registry.state.read().files.len(), REGISTER_PRUNE_INTERVAL);
        drop(live_files);
    }

    #[cfg(feature = "ext4")]
    #[test]
    fn registry_does_not_restore_retired_cache_owners_after_pruning() {
        // Force retirement while pruning temporarily owns the registry entries.
        for unlinked in [true, false] {
            let registry = CachedFileRegistry::new();
            let cached = Arc::new(CachedFileShared::new_unbounded(0));
            let lifetime = Arc::downgrade(&cached);
            registry
                .state
                .write()
                .files
                .insert(cached_file_registry_key(&cached), cached.clone());
            registry.prune_with(|| {
                if unlinked {
                    cached.mark_unlinked();
                } else {
                    cached.retired.store(true, Ordering::Release);
                }
                registry.release(&cached);
            });
            drop(cached);
            assert!(
                lifetime.upgrade().is_none(),
                "pruning restored retired cache ownership: unlinked={unlinked}"
            );
        }
    }
}
