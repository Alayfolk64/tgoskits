mod readahead;
#[cfg(feature = "vfs")]
mod reclaim;
mod resize;
mod writeback;

use alloc::{
    collections::BTreeMap,
    sync::{Arc, Weak},
    vec::Vec,
};
use core::{
    array,
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
};

use ax_io::prelude::*;
use axfs_ng_vfs::{CachedWriteGuard, FileNode, FilesystemOps, Location, VfsError, VfsResult};
use lru::LruCache;
use readahead::ReadAheadState;
#[cfg(feature = "vfs")]
pub use reclaim::{page_cache_reclaim, sync_all_cached_files, sync_filesystem_cached_files};

use super::page::PageCache;
use crate::os::{
    memory::PAGE_SIZE,
    sync::{SleepMutex as Mutex, SleepMutexGuard},
};

type CachedFileKey = (usize, u64);
type InodeCacheIndex = BTreeMap<CachedFileKey, Weak<CachedFileShared>>;
type PageCacheShard = LruCache<u32, PageCache>;

/// Per-inode cache-index partitions selected by page number.
///
/// Linux separates the address-space index from folio ownership so unrelated
/// faults do not serialize on one mutable LRU lookup. Our cache pages are still
/// value-owned, so use bounded index sharding as the capability boundary until
/// page objects gain independent lifetime and lock state. Callers must never
/// hold two shard guards simultaneously; operations requiring a coherent
/// whole-file view use `io_lock` or the mapping-layout boundary above them.
const PAGE_CACHE_SHARD_COUNT: usize = 16;

struct PageCacheIndex {
    shards: [Mutex<PageCacheShard>; PAGE_CACHE_SHARD_COUNT],
}

impl PageCacheIndex {
    fn new() -> Self {
        Self {
            shards: array::from_fn(|_| Mutex::new(LruCache::unbounded())),
        }
    }

    const fn shard_index(page_number: u32) -> usize {
        page_number as usize % PAGE_CACHE_SHARD_COUNT
    }

    fn lock_page(&self, page_number: u32) -> SleepMutexGuard<'_, PageCacheShard> {
        self.shards[Self::shard_index(page_number)].lock()
    }

    fn lock_shard(&self, index: usize) -> SleepMutexGuard<'_, PageCacheShard> {
        self.shards[index].lock()
    }

    #[cfg(feature = "vfs")]
    fn try_lock_shard(&self, index: usize) -> Option<SleepMutexGuard<'_, PageCacheShard>> {
        self.shards[index].try_lock()
    }

    fn len(&self) -> usize {
        (0..PAGE_CACHE_SHARD_COUNT)
            .map(|index| self.lock_shard(index).len())
            .sum()
    }
}

static CACHED_FILE_BY_INODE: ax_lazyinit::LazyLock<Mutex<InodeCacheIndex>> =
    ax_lazyinit::LazyLock::new(|| Mutex::new(BTreeMap::new()));

/// Stable identity of one page-cache ownership domain.
///
/// Clones and independently opened handles that resolve to the same
/// [`CachedFileShared`] carry the same identity.  A newly created cache owner
/// always receives a fresh identity, so users such as shared-futex lookup do
/// not need to turn an `Arc` address into a public integer key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CachedFileIdentity(u64);

impl CachedFileIdentity {
    fn allocate() -> Self {
        static NEXT_ID: AtomicU64 = AtomicU64::new(1);
        let Ok(identity) = NEXT_ID.try_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
            current.checked_add(1)
        }) else {
            panic!("cached-file identity space exhausted");
        };
        Self(identity)
    }

    /// Returns the numeric component of this opaque identity.
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// Physical identity of a frame currently owned by one [`CachedFile`].
///
/// This is an observation token, not ownership.  The cache keeps the matching
/// [`PageCache`] indexed, pinned, or detached in a local eviction batch while a
/// mapping endpoint processes the event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CachedFrameIdentity(usize);

impl CachedFrameIdentity {
    const fn new(paddr: usize) -> Self {
        Self(paddr)
    }

    pub const fn paddr(self) -> usize {
        self.0
    }
}

/// Exact file-cache page named by a mapping lifecycle event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CachePageIdentity {
    file: CachedFileIdentity,
    file_epoch: u64,
    page_number: u32,
    frame: CachedFrameIdentity,
}

impl CachePageIdentity {
    const fn new(
        file: CachedFileIdentity,
        file_epoch: u64,
        page_number: u32,
        frame: CachedFrameIdentity,
    ) -> Self {
        Self {
            file,
            file_epoch,
            page_number,
            frame,
        }
    }

    pub const fn file(self) -> CachedFileIdentity {
        self.file
    }

    pub const fn file_epoch(self) -> u64 {
        self.file_epoch
    }

    pub const fn page_number(self) -> u32 {
        self.page_number
    }

    pub const fn frame(self) -> CachedFrameIdentity {
        self.frame
    }
}

/// Mapping operation published after releasing page-cache and cached-I/O locks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheMappingEvent {
    Evict(CachePageIdentity),
    WritebackProtect(CachePageIdentity),
}

impl CacheMappingEvent {
    pub const fn page(self) -> CachePageIdentity {
        match self {
            Self::Evict(page) | Self::WritebackProtect(page) => page,
        }
    }

    const fn no_endpoint_result(self) -> CacheMappingResult {
        match self {
            Self::Evict(_) => CacheMappingResult::Retired,
            Self::WritebackProtect(_) => CacheMappingResult::Protected,
        }
    }
}

/// Completion state returned by the sole mapping owner for a cached file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheMappingResult {
    Retired,
    Protected,
    Busy,
    /// The PTE was unpublished but its TLB obligation is not acknowledged yet.
    Quarantined,
    /// The endpoint could not prove either completion or an exact rollback.
    Failed,
}

/// Why a best-effort file-cache pageout could not reclaim every candidate.
///
/// The cache retains every candidate it could not reclaim.  Linux
/// `MADV_PAGEOUT` deliberately does not expose these transient reclaim
/// failures as syscall errors, while internal callers still need a typed fact
/// for metrics, retry policy, and invariant auditing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CachePageoutDeferred {
    Writeback(VfsError),
    Eviction(VfsError),
}

/// Result of one bounded, best-effort pageout request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CachePageoutResult {
    reclaimed: usize,
    deferred: Option<CachePageoutDeferred>,
}

impl CachePageoutResult {
    const fn complete(reclaimed: usize) -> Self {
        Self {
            reclaimed,
            deferred: None,
        }
    }

    const fn deferred(reclaimed: usize, reason: CachePageoutDeferred) -> Self {
        Self {
            reclaimed,
            deferred: Some(reason),
        }
    }

    pub const fn reclaimed(self) -> usize {
        self.reclaimed
    }

    pub const fn deferred_reason(self) -> Option<CachePageoutDeferred> {
        self.deferred
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct InvalidateCleanOutcome {
    invalidated: usize,
    busy: bool,
}

/// Reverse-mapping boundary owned by the VM subsystem using this page cache.
///
/// One live endpoint is installed per [`CachedFileIdentity`].  Implementations
/// must not enter cached I/O from `publish`; the cache invokes it without any
/// page-cache, cached-I/O, or endpoint-publication lock held. A mapping-layout
/// serialization guard may remain held so buffered I/O cannot republish stale
/// cache contents before the event either commits or rolls back.
pub trait CacheMappingEndpoint: Send + Sync {
    fn publish(&self, event: CacheMappingEvent) -> CacheMappingResult;
}

/// A transient, typed pin on one page-cache frame.
///
/// The cache index remains the physical owner.  The pin only prevents reclaim
/// or truncate from detaching that owner while a caller publishes a PTE; it
/// never exposes mutable page data outside the cache lock.
pub struct CachedPagePin {
    shared: Arc<CachedFileShared>,
    page_number: u32,
    paddr: usize,
}

impl CachedPagePin {
    pub const fn paddr(&self) -> usize {
        self.paddr
    }
}

impl Drop for CachedPagePin {
    fn drop(&mut self) {
        let mut cache = self.shared.page_cache.lock_page(self.page_number);
        let Some(page) = cache.get_mut(&self.page_number) else {
            // Reclaim/truncate must reject pinned entries, so disappearance is
            // an ownership protocol violation.  The underlying frame cannot be
            // repaired here because its cache owner is already unknown.
            warn!(
                "pinned cached page {} disappeared before pin release",
                self.page_number
            );
            return;
        };
        if page.pins == 0 {
            warn!("cached page pin underflow for page {}", self.page_number);
            return;
        }
        page.pins -= 1;
    }
}

/// Serializes a file mapping-layout change against buffered cache publication.
///
/// Linux uses `address_space::invalidate_lock` for the same boundary: buffered
/// cache population takes the shared side while truncate and hole-punch take
/// the exclusive side. `ax-sync` does not yet provide a sleepable RW lock, so
/// cached I/O and layout mutation use this exclusive sleepable lock. Faults do
/// not acquire it while holding address-space state; they observe the atomic
/// publication barrier and retry instead.
struct MappingUpdateGuard<'a> {
    shared: &'a CachedFileShared,
    _layout: SleepMutexGuard<'a, ()>,
}

impl Drop for MappingUpdateGuard<'_> {
    fn drop(&mut self) {
        self.shared
            .mapping_update_in_progress
            .store(false, Ordering::Release);
    }
}

#[derive(Clone, Copy)]
enum PageLoadState {
    Loading,
    Complete(VfsResult<()>),
}

/// One backing-read owner shared by every fault targeting its page window.
///
/// The owner acquires `state` before publishing this object in `page_loads` and
/// keeps the guard while the backing read is in progress. Contenders clone the
/// object without holding `page_loads`, then sleep on this mutex. This gives the
/// page cache Linux-like per-page I/O ownership without making the filesystem
/// layer depend directly on a scheduler wait-queue type.
struct PageLoad {
    state: Mutex<PageLoadState>,
}

impl PageLoad {
    const fn new() -> Self {
        Self {
            state: Mutex::new(PageLoadState::Loading),
        }
    }

    fn wait(&self) -> VfsResult<()> {
        match *self.state.lock() {
            PageLoadState::Loading => Err(VfsError::BadState),
            PageLoadState::Complete(result) => result,
        }
    }
}

struct CachedFileShared {
    identity: CachedFileIdentity,
    page_cache: PageCacheIndex,
    /// Active backing reads indexed by every page covered by their window.
    ///
    /// Never wait on a [`PageLoad`] while holding this index lock. The load
    /// owner may need the index briefly to retire its published entries before
    /// releasing waiters.
    page_loads: Mutex<BTreeMap<u32, Arc<PageLoad>>>,
    mapping_layout_lock: Mutex<()>,
    io_lock: Mutex<()>,
    mapping_endpoint: Mutex<Option<Weak<dyn CacheMappingEndpoint>>>,
    backing: Option<FileNode>,
    len: AtomicU64,
    /// Generation of mapping-changing file operations (truncate, hole punch,
    /// collapse/insert).  File-backed VMAs snapshot this value so stale page
    /// objects cannot be mistaken for the current file view.
    mapping_epoch: AtomicU64,
    mapping_update_in_progress: AtomicBool,
    unlinked: AtomicBool,
    #[cfg(feature = "vfs")]
    retired: AtomicBool,
}

impl CachedFileShared {
    pub fn new(len: u64, backing: FileNode) -> Self {
        Self {
            identity: CachedFileIdentity::allocate(),
            // Linux limits the page cache globally under memory pressure, not
            // with a small per-inode ceiling. Disk-backed owners are registered
            // with the global reclaimer below, which evicts clean pages while
            // filesystem writeback turns dirty pages into reclaim candidates.
            page_cache: PageCacheIndex::new(),
            page_loads: Mutex::new(BTreeMap::new()),
            mapping_layout_lock: Mutex::new(()),
            io_lock: Mutex::new(()),
            mapping_endpoint: Mutex::new(None),
            backing: Some(backing),
            len: AtomicU64::new(len),
            mapping_epoch: AtomicU64::new(0),
            mapping_update_in_progress: AtomicBool::new(false),
            unlinked: AtomicBool::new(false),
            #[cfg(feature = "vfs")]
            retired: AtomicBool::new(false),
        }
    }

    pub fn new_unbounded(len: u64) -> Self {
        Self {
            identity: CachedFileIdentity::allocate(),
            page_cache: PageCacheIndex::new(),
            page_loads: Mutex::new(BTreeMap::new()),
            mapping_layout_lock: Mutex::new(()),
            io_lock: Mutex::new(()),
            mapping_endpoint: Mutex::new(None),
            backing: None,
            len: AtomicU64::new(len),
            mapping_epoch: AtomicU64::new(0),
            mapping_update_in_progress: AtomicBool::new(false),
            unlinked: AtomicBool::new(false),
            #[cfg(feature = "vfs")]
            retired: AtomicBool::new(false),
        }
    }

    fn len(&self) -> u64 {
        self.len.load(Ordering::Acquire)
    }

    fn update_len_max(&self, len: u64) {
        let mut current = self.len();
        while len > current {
            match self
                .len
                .compare_exchange_weak(current, len, Ordering::AcqRel, Ordering::Acquire)
            {
                Ok(_) => break,
                Err(observed) => current = observed,
            }
        }
    }

    fn prepare_mapping_epoch(&self) -> VfsResult<u64> {
        self.mapping_epoch
            .load(Ordering::Acquire)
            .checked_add(1)
            .ok_or(VfsError::ValueOverflow)
    }

    fn publish_mapping_epoch(&self, epoch: u64) {
        self.mapping_epoch.store(epoch, Ordering::Release);
    }

    fn prepared_mapping_epoch_is_current(&self, epoch: u64) -> bool {
        self.mapping_epoch.load(Ordering::Acquire) == epoch - 1
    }

    fn set_len(&self, len: u64) {
        self.len.store(len, Ordering::Release);
    }

    fn backing(&self) -> VfsResult<&FileNode> {
        self.backing.as_ref().ok_or(VfsError::InvalidInput)
    }

    /// Takes a strong snapshot of the endpoint capability and releases the
    /// publication lock before any VM code can run.
    fn mapping_endpoint(&self) -> Option<Arc<dyn CacheMappingEndpoint>> {
        let (endpoint, stale) = {
            let mut installed = self.mapping_endpoint.lock();
            match installed.as_ref().and_then(Weak::upgrade) {
                Some(endpoint) => (Some(endpoint), None),
                None => (None, installed.take()),
            }
        };
        // Dropping the final weak control-block owner can deallocate. Keep it
        // outside the endpoint publication lock just like detached pages.
        drop(stale);
        endpoint
    }

    fn publish_mapping_event(&self, event: CacheMappingEvent) -> CacheMappingResult {
        let Some(endpoint) = self.mapping_endpoint() else {
            return event.no_endpoint_result();
        };
        endpoint.publish(event)
    }

    fn cache_page_identity(&self, page_number: u32, paddr: usize) -> CachePageIdentity {
        CachePageIdentity::new(
            self.identity,
            self.mapping_epoch.load(Ordering::Acquire),
            page_number,
            CachedFrameIdentity::new(paddr),
        )
    }

    #[cfg(all(feature = "ext4", feature = "vfs"))]
    fn mark_unlinked(&self) {
        self.unlinked.store(true, Ordering::Release);
    }

    #[cfg(test)]
    fn invoke_writeback_protect_for_test(&self, pns: &[u32]) -> VfsResult<()> {
        self.protect_dirty_pages_before_writeback(pns)
    }

    #[cfg(test)]
    fn io_lock_is_free_for_test(&self) -> bool {
        self.io_lock.try_lock().is_some()
    }

    #[cfg(test)]
    fn mapping_layout_lock_is_free_for_test(&self) -> bool {
        self.mapping_layout_lock.try_lock().is_some()
    }

    #[cfg(test)]
    fn endpoint_lock_is_free_for_test(&self) -> bool {
        self.mapping_endpoint.try_lock().is_some()
    }

    #[cfg(test)]
    fn page_cache_lock_is_free_for_test(&self) -> bool {
        (0..PAGE_CACHE_SHARD_COUNT).all(|index| self.page_cache.shards[index].try_lock().is_some())
    }

    #[cfg(test)]
    fn page_cache_guard_for_page_for_test(
        &self,
        page_number: u32,
    ) -> SleepMutexGuard<'_, LruCache<u32, PageCache>> {
        self.page_cache.lock_page(page_number)
    }

    #[cfg(test)]
    fn page_cache_lock_for_page_is_free_for_test(&self, page_number: u32) -> bool {
        self.page_cache.shards[PageCacheIndex::shard_index(page_number)]
            .try_lock()
            .is_some()
    }
}

impl Drop for CachedFileShared {
    fn drop(&mut self) {
        if !self.unlinked.load(Ordering::Acquire) {
            return;
        }
        for index in 0..PAGE_CACHE_SHARD_COUNT {
            for (_, page) in self.page_cache.lock_shard(index).iter_mut() {
                page.dirty = false;
            }
        }
    }
}

/// A file handle with an LRU page cache for buffered I/O.
pub struct CachedFile {
    inner: Location,
    shared: Arc<CachedFileShared>,
    readahead: Arc<Mutex<ReadAheadState>>,
    in_memory: bool,
}

impl Clone for CachedFile {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
            shared: self.shared.clone(),
            readahead: self.readahead.clone(),
            in_memory: self.in_memory,
        }
    }
}

enum FileUserData {
    Strong(Arc<CachedFileShared>),
}

impl FileUserData {
    pub fn get(&self) -> Arc<CachedFileShared> {
        match self {
            FileUserData::Strong(strong) => strong.clone(),
        }
    }
}

fn filesystem_uses_unbounded_page_cache(name: &str) -> bool {
    matches!(name, "tmpfs" | "ramfs")
}

impl CachedFile {
    #[cfg(feature = "profile")]
    fn profile_scope(&self) -> ax_sync::ProfileScope {
        ax_sync::ProfileScope::new(
            ax_sync::ProfileEvent::PageCache,
            Arc::as_ptr(&self.shared) as usize,
        )
    }

    /// Returns an existing cached file for `location`, or creates a new one.
    pub fn get_or_create(location: Location) -> VfsResult<Self> {
        let in_memory = filesystem_uses_unbounded_page_cache(location.filesystem().name());

        let existing = {
            let guard = location.user_data();
            guard
                .get::<FileUserData>()
                .as_deref()
                .map(FileUserData::get)
        };
        if let Some(shared) = existing {
            #[cfg(feature = "vfs")]
            if shared.retired.swap(false, Ordering::AcqRel) {
                reclaim::register_cached_file(&shared);
            }
            return Ok(Self {
                inner: location,
                shared,
                readahead: Arc::new(Mutex::new(ReadAheadState::new())),
                in_memory,
            });
        }

        let len = location.len()?;
        let inode_key =
            should_share_cached_file_by_inode(&location).then(|| cached_file_key(&location));
        let candidate = if in_memory {
            Arc::new(CachedFileShared::new_unbounded(len))
        } else {
            let backing = location.entry().as_file()?.clone();
            Arc::new(CachedFileShared::new(len, backing))
        };
        let (created, owner_created) = if let Some(key) = inode_key {
            publish_inode_cached_file(key, candidate)
        } else {
            (candidate, true)
        };
        let user_data = FileUserData::Strong(created.clone());

        let shared = {
            let mut guard = location.user_data();
            if let Some(shared) = guard
                .get::<FileUserData>()
                .as_deref()
                .map(FileUserData::get)
            {
                shared
            } else {
                guard.insert(user_data);
                created
            }
        };

        // tmpfs and ramfs have no backing store, so evicting clean pages would
        // lose data. Only register disk-backed files for reclaim.
        #[cfg(feature = "vfs")]
        if !in_memory && (owner_created || shared.retired.swap(false, Ordering::AcqRel)) {
            reclaim::register_cached_file(&shared);
        }
        #[cfg(not(feature = "vfs"))]
        let _ = owner_created;

        Ok(Self {
            inner: location,
            shared,
            readahead: Arc::new(Mutex::new(ReadAheadState::new())),
            in_memory,
        })
    }

    /// Returns `true` if both handles refer to the same shared state.
    pub fn ptr_eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.shared, &other.shared)
    }

    /// Returns the stable identity of the shared page-cache owner.
    pub fn identity(&self) -> CachedFileIdentity {
        self.shared.identity
    }

    /// Returns the current cached file length.
    pub fn len(&self) -> u64 {
        self.shared.len()
    }

    /// Returns whether a file page currently has a page-cache object.
    ///
    /// This is a snapshot query for `mincore`; it does not update LRU order,
    /// perform I/O, or manufacture a cache entry.
    pub fn is_page_cached(&self, page_number: u32) -> bool {
        self.shared
            .page_cache
            .lock_page(page_number)
            .contains(&page_number)
    }

    /// Returns whether the current cached file length is zero.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Returns `true` if this file is backed by an in-memory filesystem (e.g. tmpfs).
    pub fn in_memory(&self) -> bool {
        self.in_memory
    }

    /// Returns the current length (in bytes) of the backing file.
    pub fn file_len(&self) -> VfsResult<u64> {
        self.inner.len()
    }

    /// Current generation of the file's page-to-offset mapping.
    pub fn mapping_epoch(&self) -> u64 {
        self.shared.mapping_epoch.load(Ordering::Acquire)
    }

    fn cache_page_identity(&self, page_number: u32, paddr: usize) -> CachePageIdentity {
        self.shared.cache_page_identity(page_number, paddr)
    }

    /// Installs the sole VM reverse-mapping endpoint for this cached file.
    ///
    /// Reinstalling the same endpoint is idempotent.  A different live endpoint
    /// is rejected because two independent mapping owners would make truncate,
    /// eviction, and writeback protection impossible to complete atomically.
    /// The cache stores only a `Weak` capability; dropping the VM domain proves
    /// that no mappings remain and allows a later owner to be installed.
    pub fn install_mapping_endpoint(
        &self,
        endpoint: &Arc<dyn CacheMappingEndpoint>,
    ) -> VfsResult<()> {
        let mut installed = self.shared.mapping_endpoint.lock();
        match installed.as_ref().and_then(Weak::upgrade) {
            Some(current) if Arc::ptr_eq(&current, endpoint) => Ok(()),
            Some(_) => Err(VfsError::AlreadyExists),
            None => {
                *installed = Some(Arc::downgrade(endpoint));
                Ok(())
            }
        }
    }

    /// Invokes the endpoint for an indexed page while holding a transient cache
    /// pin. A cache miss means no PTE can still own that frame and is therefore
    /// already retired.
    pub(crate) fn invalidate_page_mappings(&self, pn: u32) -> VfsResult<bool> {
        let Some(pin) = self.pin_cached_page_if_present(pn)? else {
            return Ok(true);
        };
        let event = CacheMappingEvent::Evict(self.cache_page_identity(pn, pin.paddr()));
        let result = self.shared.publish_mapping_event(event);
        drop(pin);
        match result {
            CacheMappingResult::Retired => Ok(true),
            CacheMappingResult::Busy | CacheMappingResult::Quarantined => Ok(false),
            CacheMappingResult::Protected | CacheMappingResult::Failed => Err(VfsError::BadState),
        }
    }

    /// Invalidates clean, unpinned cache pages in `[start_pn, end_pn)`.
    ///
    /// Candidates are detached from the cache index first. Mapping listeners
    /// are invoked only after releasing that lock, and a page is restored when
    /// any listener cannot retire all of its reverse mappings. Dirty or pinned
    /// pages make the request busy rather than being dropped behind an active
    /// writer or PTE publisher.
    fn invalidate_clean_pages_inner(
        &self,
        start_pn: u32,
        end_pn: u32,
    ) -> VfsResult<InvalidateCleanOutcome> {
        if start_pn > end_pn {
            return Err(VfsError::InvalidInput);
        }
        if start_pn == end_pn {
            return Ok(InvalidateCleanOutcome {
                invalidated: 0,
                busy: false,
            });
        }
        if self.in_memory {
            // The page cache is the backing store for tmpfs-like files.  With
            // no swap object to retain the contents, dropping a clean page
            // would lose file data rather than merely invalidate a cache copy.
            return Ok(InvalidateCleanOutcome {
                invalidated: 0,
                busy: false,
            });
        }

        let candidates = self.cached_pages_in(u64::from(start_pn), u64::from(end_pn))?;
        let mut pending = Vec::new();
        pending
            .try_reserve_exact(candidates.len())
            .map_err(|_| VfsError::NoMemory)?;
        let mut busy = false;
        for pn in candidates {
            let mut cache = self.shared.page_cache.lock_page(pn);
            let Some(page) = cache.get_mut(&pn) else {
                continue;
            };
            if page.dirty || page.pins != 0 {
                busy = true;
            } else {
                let page = cache.pop(&pn).ok_or(VfsError::BadState)?;
                pending.push((pn, page));
            }
        }

        let mut invalidated = 0;
        let mut first_error = None;
        for (pn, page) in pending {
            let result = match page.paddr() {
                Ok(paddr) => self.shared.publish_mapping_event(CacheMappingEvent::Evict(
                    self.cache_page_identity(pn, paddr),
                )),
                Err(error) => {
                    first_error.get_or_insert(error);
                    CacheMappingResult::Failed
                }
            };
            match result {
                CacheMappingResult::Retired => invalidated += 1,
                CacheMappingResult::Busy | CacheMappingResult::Quarantined => {
                    busy = true;
                    let replaced = self.shared.page_cache.lock_page(pn).put(pn, page);
                    drop(replaced);
                }
                CacheMappingResult::Protected | CacheMappingResult::Failed => {
                    first_error.get_or_insert(VfsError::BadState);
                    let replaced = self.shared.page_cache.lock_page(pn).put(pn, page);
                    drop(replaced);
                }
            }
        }

        first_error.map_or(Ok(InvalidateCleanOutcome { invalidated, busy }), Err)
    }

    pub fn invalidate_clean_pages(&self, start_pn: u32, end_pn: u32) -> VfsResult<usize> {
        let outcome = self.invalidate_clean_pages_inner(start_pn, end_pn)?;
        if outcome.busy {
            Err(VfsError::ResourceBusy)
        } else {
            Ok(outcome.invalidated)
        }
    }

    /// Writes back dirty candidates and then attempts typed rmap eviction.
    ///
    /// Operational writeback or eviction failures are returned as `Deferred`:
    /// no dirty cache owner is discarded, and a later reclaimer may retry.
    /// Ownership/protocol corruption remains an error rather than being hidden
    /// behind Linux's best-effort syscall semantics.
    pub fn pageout_pages(&self, start_pn: u32, end_pn: u32) -> VfsResult<CachePageoutResult> {
        if start_pn > end_pn {
            return Err(VfsError::InvalidInput);
        }
        if self.in_memory {
            return Err(VfsError::OperationNotSupported);
        }

        let dirty = self.dirty_pages_in_range(start_pn, end_pn)?;
        if !dirty.is_empty()
            && let Err(error) = self.writeback_pages(&dirty)
        {
            return match error {
                VfsError::BadState
                | VfsError::FilesystemCorrupted
                | VfsError::InvalidData
                | VfsError::InvalidInput
                | VfsError::ValueOverflow => Err(error),
                _ => Ok(CachePageoutResult::deferred(
                    0,
                    CachePageoutDeferred::Writeback(error),
                )),
            };
        }

        match self.invalidate_clean_pages_inner(start_pn, end_pn) {
            Ok(outcome) if outcome.busy => Ok(CachePageoutResult::deferred(
                outcome.invalidated,
                CachePageoutDeferred::Eviction(VfsError::ResourceBusy),
            )),
            Ok(outcome) => Ok(CachePageoutResult::complete(outcome.invalidated)),
            Err(
                error @ (VfsError::BadState
                | VfsError::FilesystemCorrupted
                | VfsError::InvalidData
                | VfsError::InvalidInput
                | VfsError::ValueOverflow),
            ) => Err(error),
            Err(error) => Ok(CachePageoutResult::deferred(
                0,
                CachePageoutDeferred::Eviction(error),
            )),
        }
    }

    fn prepare_cache_page(
        &self,
        file: &FileNode,
        pn: u32,
        read_backing: bool,
    ) -> VfsResult<PageCache> {
        let mut page = PageCache::new()?;
        if self.in_memory || !read_backing {
            page.data().fill(0);
        } else {
            // `PageCache::new()` does not zero the freshly allocated frame, and
            // `FileNodeOps::read_at` short-reads at EOF (rsext4/fat return only the
            // bytes actually read, leaving the rest of the buffer untouched). Zero the
            // tail beyond the read length so a partial last page never exposes stale
            // physical memory past EOF — POSIX/Linux require those bytes to read as 0
            // (e.g. an mmap of a 100-byte file must see `[100, PAGE_SIZE)` as zero).
            let read = file.read_at(page.data(), pn as u64 * PAGE_SIZE as u64)?;
            page.data()[read..].fill(0);
        }
        Ok(page)
    }

    /// Updates an existing page or publishes one fully prepared cache page.
    ///
    /// Frame allocation and backing I/O happen before taking the cache-index
    /// lock. The lock section only rechecks identity, mutates page bytes, and
    /// installs the prepared owner. A losing candidate or capacity eviction is
    /// dropped after unlocking, so frame reclaim cannot re-enter this index.
    /// Callers serialize cache insertion with `io_lock`.
    fn with_page_or_insert<R>(
        &self,
        file: &FileNode,
        pn: u32,
        read_backing: bool,
        update: impl FnOnce(&mut PageCache, bool) -> R,
    ) -> VfsResult<R> {
        let mut update = Some(update);
        {
            let mut cache = self.shared.page_cache.lock_page(pn);
            if cache.contains(&pn) {
                let page = cache.get_mut(&pn).ok_or(VfsError::BadState)?;
                return Ok(update.take().ok_or(VfsError::BadState)?(page, false));
            }
        }

        let mut prepared = self.prepare_cache_page(file, pn, read_backing)?;
        let (result, retired) = {
            let mut cache = self.shared.page_cache.lock_page(pn);
            if cache.contains(&pn) {
                let page = cache.get_mut(&pn).ok_or(VfsError::BadState)?;
                let result = update.take().ok_or(VfsError::BadState)?(page, false);
                (result, Some(prepared))
            } else {
                let result = update.take().ok_or(VfsError::BadState)?(&mut prepared, true);
                let retired = cache.push(pn, prepared).map(|(_, page)| page);
                (result, retired)
            }
        };
        drop(retired);
        Ok(result)
    }

    /// Loads one bounded contiguous cache window beginning at `pn`.
    ///
    /// A window publishes one [`PageLoad`] for every page it covers before
    /// entering the backing filesystem. A fault for the same page sleeps on
    /// that load, while faults for unrelated pages can submit independent I/O.
    /// Each prepared page is published under its cache-index shard after
    /// rechecking the mapping epoch. A concurrent mapping update either sees
    /// and invalidates that page or makes this publication retry, matching
    /// Linux's per-folio publication under the address-space invalidate
    /// boundary without a whole-file fault lock.
    fn populate_page_window(&self, file: &FileNode, pn: u32, window_pages: usize) -> VfsResult<()> {
        if self.in_memory {
            let _io = self.shared.io_lock.lock();
            self.with_page_or_insert(file, pn, false, |_, _| {})?;
            return Ok(());
        }

        if self
            .shared
            .mapping_update_in_progress
            .load(Ordering::Acquire)
        {
            return Err(VfsError::ResourceBusy);
        }

        // Snapshot the mapping generation before any file coordinate. A
        // truncate or range shift that overlaps planning or backing I/O makes
        // publication retry instead of installing data under stale offsets.
        let load_epoch = self.shared.mapping_epoch.load(Ordering::Acquire);
        let file_len = self.shared.len();
        let first_page = u64::from(pn);
        let file_pages = file_len.div_ceil(PAGE_SIZE as u64);
        if first_page >= file_pages {
            return Err(VfsError::InvalidInput);
        }

        let max_pages = window_pages.max(1);
        let candidate_end = first_page
            .saturating_add(max_pages as u64)
            .min(file_pages)
            .min(u64::from(u32::MAX) + 1);
        if self.shared.page_cache.lock_page(pn).contains(&pn) {
            return Ok(());
        }
        let active_load = {
            let loads = self.shared.page_loads.lock();
            loads.get(&pn).cloned()
        };
        if let Some(load) = active_load {
            return load.wait();
        }

        let load = Arc::new(PageLoad::new());
        // Acquire before publication. Any contender that finds this load must
        // block until this owner stores `Complete` below.
        let mut load_state = load.state.lock();
        let run_pages = {
            let mut loads = self.shared.page_loads.lock();
            if let Some(existing) = loads.get(&pn).cloned() {
                drop(loads);
                drop(load_state);
                return existing.wait();
            }

            if self.shared.page_cache.lock_page(pn).contains(&pn) {
                return Ok(());
            }
            let mut page = first_page;
            while page < candidate_end {
                let page_number = u32::try_from(page).map_err(|_| VfsError::InvalidInput)?;
                if self
                    .shared
                    .page_cache
                    .lock_page(page_number)
                    .contains(&page_number)
                    || loads.contains_key(&page_number)
                {
                    break;
                }
                page += 1;
            }
            let run_pages =
                usize::try_from(page - first_page).map_err(|_| VfsError::InvalidInput)?;
            for index in 0..run_pages {
                let page_number = pn
                    .checked_add(
                        u32::try_from(index).expect("bounded page-load window index fits in u32"),
                    )
                    .expect("bounded page-load window stays within u32 page space");
                loads.insert(page_number, load.clone());
            }
            run_pages
        };
        if run_pages == 0 {
            return Ok(());
        }

        let result = (|| {
            let run_len = run_pages
                .checked_mul(PAGE_SIZE)
                .ok_or(VfsError::InvalidInput)?;
            let mut data = Vec::new();
            data.try_reserve_exact(run_len)
                .map_err(|_| VfsError::NoMemory)?;
            data.resize(run_len, 0);
            let file_offset = first_page
                .checked_mul(PAGE_SIZE as u64)
                .ok_or(VfsError::InvalidInput)?;
            let readable = usize::try_from(file_len.saturating_sub(file_offset))
                .unwrap_or(usize::MAX)
                .min(run_len);
            file.read_at(&mut data[..readable], file_offset)?;

            let mut prepared = Vec::new();
            prepared
                .try_reserve_exact(run_pages)
                .map_err(|_| VfsError::NoMemory)?;
            for index in 0..run_pages {
                let page_number = pn
                    .checked_add(u32::try_from(index).map_err(|_| VfsError::InvalidInput)?)
                    .ok_or(VfsError::InvalidInput)?;
                let start = index * PAGE_SIZE;
                let mut page = PageCache::new()?;
                page.data().copy_from_slice(&data[start..start + PAGE_SIZE]);
                prepared.push((page_number, page));
            }

            let mut unused = Vec::new();
            unused
                .try_reserve_exact(run_pages)
                .map_err(|_| VfsError::NoMemory)?;
            let mut publication_error = None;
            for (page_number, page) in prepared {
                let mut cache = self.shared.page_cache.lock_page(page_number);
                if self
                    .shared
                    .mapping_update_in_progress
                    .load(Ordering::Acquire)
                    || self.shared.mapping_epoch.load(Ordering::Acquire) != load_epoch
                {
                    unused.push(page);
                    publication_error = Some(VfsError::ResourceBusy);
                    break;
                }
                if cache.contains(&page_number) {
                    unused.push(page);
                } else if let Some((_, retired)) = cache.push(page_number, page) {
                    unused.push(retired);
                }
            }
            drop(unused);
            publication_error.map_or(Ok(()), Err)
        })();

        *load_state = PageLoadState::Complete(result);
        {
            let mut loads = self.shared.page_loads.lock();
            for index in 0..run_pages {
                let page_number = pn
                    .checked_add(
                        u32::try_from(index).expect("bounded page-load window index fits in u32"),
                    )
                    .expect("bounded page-load window stays within u32 page space");
                if loads
                    .get(&page_number)
                    .is_some_and(|active| Arc::ptr_eq(active, &load))
                {
                    loads.remove(&page_number);
                }
            }
        }
        drop(load_state);
        result
    }

    /// Marks one cached mmap page dirty through the shared cached-I/O protocol.
    pub fn mark_mmap_dirty_page(&self, pn: u32) -> VfsResult<()> {
        if self.in_memory {
            return Ok(());
        }
        let _write = CachedWriteGuard::acquire(self.inner.filesystem())?;
        if self
            .shared
            .mapping_update_in_progress
            .load(Ordering::Acquire)
        {
            return Err(VfsError::ResourceBusy);
        }
        let _io = self.shared.io_lock.lock();
        if self
            .shared
            .mapping_update_in_progress
            .load(Ordering::Acquire)
        {
            return Err(VfsError::ResourceBusy);
        }
        let mut guard = self.shared.page_cache.lock_page(pn);
        guard.get_mut(&pn).ok_or(VfsError::BadState)?.mark_dirty();
        Ok(())
    }

    fn pin_page_or_insert_with_window(
        &self,
        pn: u32,
        use_readahead: bool,
    ) -> VfsResult<CachedPagePin> {
        #[cfg(feature = "profile")]
        let _profile = self.profile_scope();
        if self
            .shared
            .mapping_update_in_progress
            .load(Ordering::Acquire)
        {
            return Err(VfsError::ResourceBusy);
        }
        let window_pages = if self.in_memory || !use_readahead {
            1
        } else {
            let offset = u64::from(pn)
                .checked_mul(PAGE_SIZE as u64)
                .ok_or(VfsError::InvalidInput)?;
            let end = offset
                .checked_add(PAGE_SIZE as u64)
                .ok_or(VfsError::InvalidInput)?
                .min(self.shared.len());
            if end <= offset {
                return Err(VfsError::InvalidInput);
            }
            self.readahead.lock().plan(offset, end).window_pages
        };
        self.populate_page_window(self.inner.entry().as_file()?, pn, window_pages)?;

        self.pin_cached_page_for_mapping_if_present(pn)?
            .ok_or(VfsError::ResourceBusy)
    }

    /// Loads and transiently pins one cache page for PTE publication.
    ///
    /// Backing I/O happens without the page-cache index lock. The returned pin
    /// carries only frame identity, so callers cannot invoke unknown code while
    /// borrowing mutable cache state.
    pub fn pin_page_or_insert(&self, pn: u32) -> VfsResult<CachedPagePin> {
        self.pin_page_or_insert_with_window(pn, false)
    }

    /// Loads one mmap fault page and performs bounded sequential readahead.
    ///
    /// Page faults only pin the requested page. Neighboring pages are populated
    /// as clean, unpinned cache entries so subsequent faults avoid backing I/O.
    /// The sequence detector is updated even when a prefetched page is already
    /// resident, allowing a linear fault stream to grow the next I/O window.
    pub fn pin_page_for_mapping(&self, pn: u32) -> VfsResult<CachedPagePin> {
        self.pin_page_or_insert_with_window(pn, true)
    }

    fn begin_mapping_update(&self) -> VfsResult<MappingUpdateGuard<'_>> {
        let layout = self.shared.mapping_layout_lock.lock();
        self.shared
            .mapping_update_in_progress
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| VfsError::ResourceBusy)?;
        Ok(MappingUpdateGuard {
            shared: &self.shared,
            _layout: layout,
        })
    }

    /// Pins an already-resident cache page without performing backing I/O.
    ///
    /// Address-space retirement uses this after validating an installed PTE:
    /// the pin keeps the cache-owned frame alive until that PTE's tagged TLB
    /// receipt is acknowledged.  A missing entry is an ownership mismatch,
    /// not a request to fault the page back in while holding MM metadata.
    pub fn pin_cached_page(&self, pn: u32) -> VfsResult<CachedPagePin> {
        let mut cache = self.shared.page_cache.lock_page(pn);
        let page = cache.get_mut(&pn).ok_or(VfsError::BadState)?;
        let paddr = page.paddr()?;
        page.pins = page.pins.checked_add(1).ok_or(VfsError::ValueOverflow)?;
        Ok(CachedPagePin {
            shared: self.shared.clone(),
            page_number: pn,
            paddr,
        })
    }

    fn pin_cached_page_if_present(&self, pn: u32) -> VfsResult<Option<CachedPagePin>> {
        let mut cache = self.shared.page_cache.lock_page(pn);
        self.pin_cached_page_in(pn, &mut cache)
    }

    fn pin_cached_page_for_mapping_if_present(&self, pn: u32) -> VfsResult<Option<CachedPagePin>> {
        let mut cache = self.shared.page_cache.lock_page(pn);
        if self
            .shared
            .mapping_update_in_progress
            .load(Ordering::Acquire)
        {
            return Err(VfsError::ResourceBusy);
        }
        self.pin_cached_page_in(pn, &mut cache)
    }

    fn pin_cached_page_in(
        &self,
        pn: u32,
        cache: &mut PageCacheShard,
    ) -> VfsResult<Option<CachedPagePin>> {
        let Some(page) = cache.get_mut(&pn) else {
            return Ok(None);
        };
        let paddr = page.paddr()?;
        page.pins = page.pins.checked_add(1).ok_or(VfsError::ValueOverflow)?;
        Ok(Some(CachedPagePin {
            shared: self.shared.clone(),
            page_number: pn,
            paddr,
        }))
    }

    /// Reads data from the file at `offset` into `dst`.
    pub fn read_at(&self, mut dst: impl Write + IoBufMut, offset: u64) -> VfsResult<usize> {
        #[cfg(feature = "profile")]
        let _profile = self.profile_scope();
        let len = self.shared.len();
        let end = offset.saturating_add(dst.remaining_mut() as u64).min(len);
        if end <= offset {
            return Ok(0);
        }
        let window_pages = if self.in_memory {
            1
        } else {
            self.readahead.lock().plan(offset, end).window_pages
        };

        let file = self.inner.entry().as_file()?;
        let mut scratch = PageCache::new()?;
        let mut read = 0;
        let mut current = offset;
        'read_pages: while current < end {
            let pn = (current / PAGE_SIZE as u64) as u32;
            let page_start = u64::from(pn) * PAGE_SIZE as u64;
            let page_offset = (current - page_start) as usize;
            let chunk_len = loop {
                match self.populate_page_window(file, pn, window_pages) {
                    Ok(()) => {}
                    Err(VfsError::ResourceBusy) => {
                        // A truncate or range mutation owns the exclusive
                        // mapping-layout phase. Sleep behind it, then retry
                        // from the current file coordinates.
                        drop(self.shared.mapping_layout_lock.lock());
                        continue;
                    }
                    Err(VfsError::InvalidInput) => {
                        let _layout = self.shared.mapping_layout_lock.lock();
                        if current >= end.min(self.shared.len()) {
                            break 'read_pages;
                        }
                        return Err(VfsError::InvalidInput);
                    }
                    Err(error) => return Err(error),
                }

                let _layout = self.shared.mapping_layout_lock.lock();
                // A preceding user copy may have faulted or slept while a
                // truncate committed. Resample EOF before each cache snapshot.
                let visible_end = end.min(self.shared.len());
                if current >= visible_end {
                    break 'read_pages;
                }
                let chunk_len =
                    (visible_end - page_start).min(PAGE_SIZE as u64) as usize - page_offset;
                let _io = self.shared.io_lock.lock();
                let mut guard = self.shared.page_cache.lock_page(pn);
                let Some(page) = guard.get_mut(&pn) else {
                    // Reclaim may retire a clean page after its load completed
                    // but before this short cache snapshot. Retry the load
                    // rather than surfacing an internal race to userspace.
                    continue;
                };
                scratch.data()[..chunk_len]
                    .copy_from_slice(&page.data()[page_offset..page_offset + chunk_len]);
                break chunk_len;
            };

            // `dst` may point at user memory. Copy after releasing cached-file
            // locks so a user page fault can take AddrSpace without creating a
            // cached-I/O -> AddrSpace lock order.
            dst.write_all(&scratch.data()[..chunk_len])
                .map_err(crate::io_error_to_vfs_error)?;
            read += chunk_len;
            current += chunk_len as u64;
        }

        Ok(read)
    }

    fn write_at_locked(&self, mut buf: impl Read + IoBuf, offset: u64) -> VfsResult<usize> {
        let file = self.inner.entry().as_file()?;
        let end = offset.saturating_add(buf.remaining() as u64);
        let old_len = self.shared.len();
        if end > old_len {
            let next_epoch = self.shared.prepare_mapping_epoch()?;
            if !old_len.is_multiple_of(PAGE_SIZE as u64) {
                let page_number = (old_len / PAGE_SIZE as u64) as u32;
                let page_start = u64::from(page_number) * PAGE_SIZE as u64;
                self.zero_partial_page_locked(
                    file,
                    page_number,
                    (old_len - page_start) as usize,
                    (end - page_start).min(PAGE_SIZE as u64) as usize,
                )?;
            }
            file.publish_cached_write_size(end)?;
            self.shared.update_len_max(end);
            self.shared.publish_mapping_epoch(next_epoch);
        }

        let mut scratch = PageCache::new()?;
        let mut written = 0;
        let mut current = offset;
        while current < end && buf.remaining() > 0 {
            let pn = (current / PAGE_SIZE as u64) as u32;
            let page_start = pn as u64 * PAGE_SIZE as u64;
            let page_offset = (current - page_start) as usize;
            let chunk_len =
                ((PAGE_SIZE - page_offset).min(buf.remaining())).min((end - current) as usize);
            let n = buf
                .read(&mut scratch.data()[..chunk_len])
                .map_err(crate::io_error_to_vfs_error)?;
            if n == 0 {
                break;
            }
            self.shared.update_len_max(current + n as u64);

            let read_backing = page_start < old_len && !(page_offset == 0 && n == PAGE_SIZE);
            self.with_page_or_insert(file, pn, read_backing, |page, _| {
                page.data()[page_offset..page_offset + n].copy_from_slice(&scratch.data()[..n]);
                if !self.in_memory {
                    page.mark_dirty();
                }
            })?;

            written += n;
            current += n as u64;
        }

        Ok(written)
    }

    /// Writes `buf` to the file at `offset`.
    pub fn write_at(&self, buf: impl Read + IoBuf, offset: u64) -> VfsResult<usize> {
        #[cfg(feature = "profile")]
        let _profile = self.profile_scope();
        let _write = CachedWriteGuard::acquire(self.inner.filesystem())?;
        let _layout = self.shared.mapping_layout_lock.lock();
        let _io = self.shared.io_lock.lock();
        self.write_at_locked(buf, offset)
    }

    /// Appends `buf` to the end of the file. Returns `(bytes_written, new_end)`.
    pub fn append(&self, buf: impl Read + IoBuf) -> VfsResult<(usize, u64)> {
        #[cfg(feature = "profile")]
        let _profile = self.profile_scope();
        let _write = CachedWriteGuard::acquire(self.inner.filesystem())?;
        let _layout = self.shared.mapping_layout_lock.lock();
        let _io = self.shared.io_lock.lock();
        let len = self.shared.len();
        self.write_at_locked(buf, len)
            .map(|written| (written, len + written as u64))
    }

    pub fn writeback(&self) -> VfsResult<alloc::vec::Vec<u32>> {
        if self.in_memory {
            return Ok(alloc::vec::Vec::new());
        }
        self.shared.writeback()
    }

    pub fn writeback_pages(&self, pns: &[u32]) -> VfsResult<()> {
        if self.in_memory {
            return Ok(());
        }
        self.shared.writeback_pages(pns)
    }

    pub fn dirty_pages_in_range(
        &self,
        start_pn: u32,
        end_pn: u32,
    ) -> VfsResult<alloc::vec::Vec<u32>> {
        let _io = self.shared.io_lock.lock();
        let mut pages = self.cached_pages_in(u64::from(start_pn), u64::from(end_pn))?;
        pages.retain(|pn| {
            self.shared
                .page_cache
                .lock_page(*pn)
                .get_mut(pn)
                .is_some_and(|page| page.dirty)
        });
        Ok(pages)
    }

    pub fn clear_dirty_pages(&self, pns: &[u32]) {
        let _io = self.shared.io_lock.lock();
        for pn in pns {
            let mut guard = self.shared.page_cache.lock_page(*pn);
            if let Some(page) = guard.get_mut(pn) {
                page.dirty = false;
                page.dirty_generation = page.dirty_generation.wrapping_add(1);
            }
        }
    }

    /// Flushes all cached pages back to disk.
    pub fn sync(&self, data_only: bool) -> VfsResult<()> {
        if self.in_memory {
            return Ok(());
        }
        self.shared.sync(data_only)
    }

    /// Returns a reference to the underlying [`Location`].
    pub fn location(&self) -> &Location {
        &self.inner
    }
}

fn should_share_cached_file_by_inode(location: &Location) -> bool {
    matches!(location.filesystem().name(), "ext4" | "tmpfs" | "ramfs")
}

fn filesystem_key(filesystem: &dyn FilesystemOps) -> usize {
    filesystem as *const dyn FilesystemOps as *const () as usize
}

fn cached_file_key(location: &Location) -> CachedFileKey {
    (filesystem_key(location.filesystem()), location.inode())
}

fn publish_inode_cached_file(
    key: CachedFileKey,
    candidate: Arc<CachedFileShared>,
) -> (Arc<CachedFileShared>, bool) {
    let mut cache = CACHED_FILE_BY_INODE.lock();
    match cache.get(&key).and_then(Weak::upgrade) {
        Some(shared) => (shared, false),
        None => {
            cache.insert(key, Arc::downgrade(&candidate));
            (candidate, true)
        }
    }
}

/// Retires the reclaim ownership after the last mount and open file leave.
/// The filesystem serializes this operation against acquiring a new lease.
#[cfg(feature = "ext4")]
pub(crate) fn retire_filesystem_cache(filesystem: &dyn FilesystemOps) -> VfsResult<()> {
    let files = filesystem_cached_files(filesystem)?;
    let mut first_error = None;
    for file in files {
        if let Err(error) = file.writeback_dirty_for_global_sync() {
            first_error.get_or_insert(error);
            continue;
        }
        // Retain failed dirty owners for the existing global writeback path.
        // A concurrent registry prune must not restore successful retirees.
        #[cfg(feature = "vfs")]
        {
            file.retired.store(true, Ordering::Release);
            reclaim::release_cached_file(&file);
        }
    }
    first_error.map_or(Ok(()), Err)
}

/// Drains a finite snapshot of this filesystem's dirty pages without syncing
/// metadata. Callers must not hold its metadata lock or journal commit gate.
#[cfg(feature = "ext4")]
pub(crate) fn writeback_filesystem_pages(filesystem: &dyn FilesystemOps) -> VfsResult<()> {
    let files = filesystem_cached_files(filesystem)?;
    let mut first_error = None;
    for file in files {
        if let Err(error) = file.writeback_dirty_for_global_sync() {
            first_error.get_or_insert(error);
        }
    }
    first_error.map_or(Ok(()), Err)
}

#[cfg(feature = "ext4")]
fn filesystem_cached_files(
    filesystem: &dyn FilesystemOps,
) -> VfsResult<Vec<Arc<CachedFileShared>>> {
    let key = filesystem_key(filesystem);
    let index = CACHED_FILE_BY_INODE.lock();
    let entries = index.range((key, 0)..=(key, u64::MAX));
    let mut files = Vec::new();
    files
        .try_reserve(entries.clone().count())
        .map_err(|_| VfsError::NoMemory)?;
    // Unlinked open files retain their inode key until the final backing
    // owner is reaped. This index also remains visible during reclaim pruning.
    files.extend(entries.filter_map(|(_, cached)| cached.upgrade()));
    Ok(files)
}

#[cfg(feature = "ext4")]
pub(crate) fn forget_cached_file_key(filesystem: &dyn FilesystemOps, inode: u64) {
    if filesystem.name() == "ext4" {
        let cached = CACHED_FILE_BY_INODE
            .lock()
            .remove(&(filesystem_key(filesystem), inode))
            .and_then(|cached| cached.upgrade());
        #[cfg(feature = "vfs")]
        if let Some(cached) = cached {
            cached.mark_unlinked();
            reclaim::release_cached_file(&cached);
        }
        #[cfg(not(feature = "vfs"))]
        let _ = cached;
    }
}

impl Drop for CachedFile {
    fn drop(&mut self) {
        // Linux close(2) does not imply fsync(2). Disk-backed page cache is
        // retained by the inode user_data and written by explicit sync paths.
    }
}

#[cfg(test)]
mod tests;
