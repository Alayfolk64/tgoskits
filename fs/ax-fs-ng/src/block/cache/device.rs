//! `FsBlockDevice` adapter routing traffic through the shared per-device
//! cache, the role the bdev page cache plays for Linux filesystem metadata
//! (`fs/buffer.c`).
//!
//! Requests inside one folio take the buffered path: reads are served
//! from folios when possible (`bread`), writes only mark slots dirty
//! (`mark_buffer_dirty`) and reach the device at writeback. Requests
//! spanning multiple folios take the device-direct path (the analog of
//! direct IO): overlapping dirty slots are written back first so the
//! device is current, the request is submitted unchanged, and the result
//! is overlaid onto cached folios. The buffered/direct split at folio
//! granularity replaces Linux's filesystem-declared metadata/data split
//! because the `FsBlockDevice` boundary only observes request shapes;
//! see the module documentation for the full mapping.

use alloc::{boxed::Box, sync::Arc};
use core::{
    array,
    sync::atomic::{AtomicUsize, Ordering},
};

use super::{
    address_space::{BLOCK_CACHE_FOLIO_CAP, BlockAddressSpace, FolioGeometry},
    barrier::IoBarrier,
};
use crate::{
    BlockError, BlockResult,
    block::FsBlockDevice,
    os::{
        runtime_ops,
        sync::{SleepMutex, SleepMutexGuard},
    },
};

/// Independent folio-index domains within one block-device address space.
/// Sequential frames distribute evenly, while 64 shards leave enough local
/// cache capacity for the eight-CPU board workload without rebuilding the
/// global block runtime around a cache implementation detail.
const BLOCK_CACHE_SHARDS: usize = 64;
const BLOCK_CACHE_FOLIOS_PER_SHARD: usize = BLOCK_CACHE_FOLIO_CAP / BLOCK_CACHE_SHARDS;

/// Acquires a sleepable cache lock without entering the scheduler from an
/// atomic context. File-backed page faults can reach the block cache while a
/// short non-sleeping MM guard is active; a contended lock must be retried by
/// that caller instead of parking the task while the guard is held.
fn lock_for_current_context<T>(lock: &SleepMutex<T>) -> BlockResult<SleepMutexGuard<'_, T>> {
    if runtime_ops().is_ok_and(|runtime| runtime.can_block()) {
        return Ok(lock.lock());
    }
    lock.try_lock().ok_or(BlockError::WouldBlock)
}

/// Device cache state whose lifetime is independent from a writeback endpoint.
///
/// The durability barrier is shared by ordinary one-folio operations and
/// exclusive to direct I/O and flush. Each shard owns disjoint frame indices,
/// so unrelated folios retain separate sleepable locks while a flush can still
/// drain every admitted operation before publishing the device barrier.
pub(super) struct BlockCacheState {
    geometry: FolioGeometry,
    barrier: IoBarrier,
    shards: [Arc<SleepMutex<BlockAddressSpace>>; BLOCK_CACHE_SHARDS],
}

impl BlockCacheState {
    fn new(geometry: FolioGeometry) -> Self {
        const _: () = assert!(BLOCK_CACHE_FOLIO_CAP.is_multiple_of(BLOCK_CACHE_SHARDS));
        const _: () = assert!(BLOCK_CACHE_SHARDS.is_power_of_two());
        const _: () = assert!(BLOCK_CACHE_SHARDS <= u64::BITS as usize);
        Self {
            geometry,
            barrier: IoBarrier::new(),
            shards: array::from_fn(|_| {
                Arc::new(SleepMutex::new(BlockAddressSpace::with_capacity(
                    geometry,
                    BLOCK_CACHE_FOLIOS_PER_SHARD,
                )))
            }),
        }
    }

    fn shard_index(frame: u64) -> usize {
        frame as usize & (BLOCK_CACHE_SHARDS - 1)
    }

    fn shard_for_block(
        &self,
        geometry: FolioGeometry,
        block: u64,
    ) -> &SleepMutex<BlockAddressSpace> {
        &self.shards[Self::shard_index(geometry.frame_of(block))]
    }

    fn shard_mask_for_range(geometry: FolioGeometry, first: u64, count: u64) -> u64 {
        let Some(last) = count
            .checked_sub(1)
            .and_then(|span| first.checked_add(span))
        else {
            return 0;
        };
        let first_frame = geometry.frame_of(first);
        let last_frame = geometry.frame_of(last);
        if last_frame - first_frame >= BLOCK_CACHE_SHARDS as u64 - 1 {
            return u64::MAX >> (u64::BITS as usize - BLOCK_CACHE_SHARDS);
        }
        let mut mask = 0;
        for frame in first_frame..=last_frame {
            mask |= 1 << Self::shard_index(frame);
        }
        mask
    }

    fn for_each_shard_in_mask(
        &self,
        mut mask: u64,
        mut visit: impl FnMut(&mut BlockAddressSpace) -> BlockResult<()>,
    ) -> BlockResult<()> {
        while mask != 0 {
            let index = mask.trailing_zeros() as usize;
            mask &= mask - 1;
            let mut shard = lock_for_current_context(&self.shards[index])?;
            visit(&mut shard)?;
        }
        Ok(())
    }

    fn writeback_range<T: FsBlockDevice + ?Sized>(
        &self,
        device: &mut T,
        range: Option<(u64, u64)>,
    ) -> BlockResult<()> {
        let mask = range.map_or(u64::MAX, |(first, count)| {
            Self::shard_mask_for_range(self.geometry, first, count)
        });
        self.for_each_shard_in_mask(mask, |shard| {
            if shard.has_dirty() {
                shard.writeback_dirty(device, range)?;
            }
            Ok(())
        })
    }

    fn apply_direct(
        &self,
        first: u64,
        count: u64,
        data: &[u8],
        preserve_dirty: bool,
    ) -> BlockResult<()> {
        let mask = Self::shard_mask_for_range(self.geometry, first, count);
        self.for_each_shard_in_mask(mask, |shard| {
            shard.apply_direct(first, count, data, preserve_dirty);
            Ok(())
        })
    }

    fn invalidate_range(&self, first: u64, count: u64) -> BlockResult<()> {
        let mask = Self::shard_mask_for_range(self.geometry, first, count);
        self.for_each_shard_in_mask(mask, |shard| {
            shard.invalidate_range(first, count);
            Ok(())
        })
    }

    fn sync_to_device<T: FsBlockDevice + ?Sized>(&self, device: &mut T) -> BlockResult<()> {
        let _barrier = self.barrier.exclusive()?;
        self.writeback_range(device, None)?;
        device.flush()
    }

    #[cfg(feature = "vfs")]
    pub(super) fn reclaim_clean_folios(&self, target: usize) -> usize {
        let Ok(Some(_io)) = self.barrier.try_shared() else {
            return 0;
        };
        let mut reclaimed = 0;
        for shard in &self.shards {
            if reclaimed >= target {
                break;
            }
            let Some(mut shard) = shard.try_lock() else {
                continue;
            };
            reclaimed += shard.reclaim_clean_folios(target - reclaimed);
        }
        reclaimed
    }

    #[cfg(test)]
    fn first_shard(&self) -> Arc<SleepMutex<BlockAddressSpace>> {
        Arc::clone(&self.shards[0])
    }
}

/// The shared per-device cache shards and their global-writeback endpoint.
///
/// All filesystem instances on one physical device share coherent folios, but
/// unrelated frames no longer serialize their device I/O. The independent
/// consumer count excludes temporary global-sync references and elects exactly
/// one last wrapper to perform drop-time writeback.
pub(crate) struct BlockCacheShared {
    device_key: usize,
    geometry: FolioGeometry,
    consumers: AtomicUsize,
    // Allocator reclaim may retain this data without retaining the device
    // endpoint, whose final drop can wait for IO or scheduler work.
    state: Arc<BlockCacheState>,
    endpoint: SleepMutex<Box<dyn FsBlockDevice>>,
}

impl BlockCacheShared {
    pub(crate) fn new(
        device_key: usize,
        geometry: FolioGeometry,
        endpoint: Box<dyn FsBlockDevice>,
    ) -> Self {
        Self {
            device_key,
            geometry,
            consumers: AtomicUsize::new(0),
            state: Arc::new(BlockCacheState::new(geometry)),
            endpoint: SleepMutex::new(endpoint),
        }
    }

    /// Whether the tree was built for `block_size`; a registry hit with a
    /// different size means the device key collides across geometries.
    pub(crate) fn matches_block_size(&self, block_size: usize) -> bool {
        self.geometry.block_size() == block_size
    }

    fn acquire_consumer(&self) -> BlockResult<()> {
        self.consumers
            .try_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                count.checked_add(1)
            })
            .map(|_| ())
            .map_err(|_| BlockError::InvalidState)
    }

    fn release_consumer(&self) -> bool {
        let previous = self
            .consumers
            .try_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                count.checked_sub(1)
            })
            .expect("each cache wrapper acquires exactly one consumer reference");
        previous == 1
    }

    /// Writes back every dirty slot through `endpoint`, then issues its
    /// flush barrier; used by global sync when no wrapper device drives
    /// the tree.
    pub(crate) fn sync_to_device_with(&self, endpoint: &mut dyn FsBlockDevice) -> BlockResult<()> {
        self.state.sync_to_device(endpoint)
    }

    /// Writes back through the endpoint owned by the shared cache tree.
    pub(crate) fn sync_to_registered_device(&self) -> BlockResult<()> {
        let mut endpoint = lock_for_current_context(&self.endpoint)?;
        self.sync_to_device_with(&mut **endpoint)
    }

    /// Publishes a data-only capability for allocator reclaim. Its last
    /// release frees cache storage without running a device destructor.
    #[cfg(feature = "vfs")]
    pub(super) fn reclaim_state(&self) -> alloc::sync::Weak<BlockCacheState> {
        Arc::downgrade(&self.state)
    }
}

impl Drop for BlockCacheShared {
    fn drop(&mut self) {
        super::registry::unregister_cache(
            self.device_key,
            core::ptr::from_ref::<BlockCacheShared>(self),
        );
    }
}

/// A [`FsBlockDevice`] wrapper whose buffered traffic is cached and shared
/// by every consumer of the same underlying device.
pub(crate) struct BufferedBlockDevice<T: FsBlockDevice> {
    inner: T,
    shared: Arc<BlockCacheShared>,
}

impl<T: FsBlockDevice> BufferedBlockDevice<T> {
    /// Wraps `inner`, resolving the shared cache tree registered under
    /// `device_key` (the identity of the runtime device handle);
    /// `endpoint` is an equivalent device the registry keeps for global
    /// writeback.
    ///
    /// # Errors
    ///
    /// Returns [`BlockError::InvalidRequest`] when the device block size is
    /// zero or not a power of two, [`BlockError::InvalidState`] when the
    /// registered tree for `device_key` was built for a different block
    /// size, and [`BlockError::NoMemory`] when the registry cannot grow.
    pub(crate) fn with_device_key(
        device_key: usize,
        endpoint: Box<dyn FsBlockDevice>,
        inner: T,
    ) -> BlockResult<Self> {
        let block_size = inner.block_size();
        let shared = super::registry::shared_cache_for(device_key, block_size, endpoint)?;
        shared.acquire_consumer()?;
        Ok(Self { inner, shared })
    }

    /// Writes back every dirty slot, then issues the device flush barrier
    /// (`sync_dirty_buffers` followed by the cache flush). The barrier
    /// ordering is what keeps journal commit sequences crash-safe when
    /// block writes are deferred into this layer.
    fn sync_to_device(&mut self) -> BlockResult<()> {
        self.shared.state.sync_to_device(&mut self.inner)
    }

    /// Splits a request into `(first_block, block_count)`, validating the
    /// buffer geometry against the device block size.
    fn split_request(&self, block_id: u64, buf_len: usize) -> BlockResult<(u64, u64)> {
        let block_size = self.shared.geometry.block_size();
        if block_size == 0 || buf_len == 0 || !buf_len.is_multiple_of(block_size) {
            return Err(BlockError::InvalidRequest);
        }
        let count = u64::try_from(buf_len / block_size).map_err(|_| BlockError::InvalidState)?;
        Ok((block_id, count))
    }

    #[cfg(all(test, feature = "vfs"))]
    pub(super) fn reclaim_from_allocator_while_state_locked_for_test(&self) -> usize {
        let state = self.shared.state.first_shard();
        let _state = state.lock();
        super::registry::reclaim_clean_folios(usize::MAX)
    }

    #[cfg(all(test, feature = "vfs"))]
    pub(super) fn unregister_while_registry_locked_for_test(&self) {
        super::registry::unregister_while_locked_for_test(
            self.shared.device_key,
            Arc::as_ptr(&self.shared),
        );
    }

    #[cfg(test)]
    pub(super) fn cache_state_for_test(&self) -> Arc<SleepMutex<BlockAddressSpace>> {
        self.shared.state.first_shard()
    }

    #[cfg(test)]
    pub(super) fn split_request_for_test(
        &self,
        block_id: u64,
        buf_len: usize,
    ) -> BlockResult<(u64, u64)> {
        self.split_request(block_id, buf_len)
    }
}

impl<T: FsBlockDevice> FsBlockDevice for BufferedBlockDevice<T> {
    fn name(&self) -> &str {
        self.inner.name()
    }

    fn num_blocks(&self) -> u64 {
        self.inner.num_blocks()
    }

    fn block_size(&self) -> usize {
        self.inner.block_size()
    }

    #[cfg(feature = "ext4")]
    fn physical_block_size(&self) -> usize {
        self.inner.physical_block_size()
    }

    #[cfg(feature = "ext4")]
    fn is_read_only(&self) -> bool {
        self.inner.is_read_only()
    }

    #[cfg(feature = "ext4")]
    fn supports_flush(&self) -> bool {
        self.inner.supports_flush()
    }

    #[cfg(feature = "ext4")]
    fn supports_fua(&self) -> bool {
        self.inner.supports_fua()
    }

    #[cfg(feature = "ext4")]
    fn fork_io(&self) -> BlockResult<Box<dyn FsBlockDevice>> {
        let inner = self.inner.fork_io()?;
        // Detached I/O must observe dirty buffered blocks and update the same
        // cache as its parent. Each endpoint owns one drop-time consumer vote.
        self.shared.acquire_consumer()?;
        Ok(Box::new(BufferedBlockDevice {
            inner,
            shared: Arc::clone(&self.shared),
        }))
    }

    fn read_block(&mut self, block_id: u64, buf: &mut [u8]) -> BlockResult<()> {
        let (first, count) = self.split_request(block_id, buf.len())?;
        if self.shared.geometry.spans_one_folio(first, count) {
            let _io = self.shared.state.barrier.shared()?;
            let shard = self
                .shared
                .state
                .shard_for_block(self.shared.geometry, first);
            return lock_for_current_context(shard)?.read_buffered(
                &mut self.inner,
                first,
                count,
                buf,
            );
        }
        let _barrier = self.shared.state.barrier.exclusive()?;
        // Direct read: write overlapping dirty slots back first so stale
        // device bytes cannot bypass newer cached data.
        self.shared
            .state
            .writeback_range(&mut self.inner, Some((first, count)))?;
        self.inner.read_block(block_id, buf)?;
        self.shared.state.apply_direct(first, count, buf, true)?;
        Ok(())
    }

    fn write_block(&mut self, block_id: u64, buf: &[u8]) -> BlockResult<()> {
        let (first, count) = self.split_request(block_id, buf.len())?;
        if self.shared.geometry.spans_one_folio(first, count) {
            let _io = self.shared.state.barrier.shared()?;
            let shard = self
                .shared
                .state
                .shard_for_block(self.shared.geometry, first);
            return lock_for_current_context(shard)?.write_buffered(
                &mut self.inner,
                first,
                count,
                buf,
            );
        }
        let _barrier = self.shared.state.barrier.exclusive()?;
        // Direct write: the device must absorb overlapping dirty slots
        // before the newer direct bytes land, then the folios are overlaid.
        self.shared
            .state
            .writeback_range(&mut self.inner, Some((first, count)))?;
        match self.inner.write_block(block_id, buf) {
            Ok(()) => {
                self.shared.state.apply_direct(first, count, buf, false)?;
                Ok(())
            }
            Err(error) => {
                // The device contract reports no completed prefix. Some
                // blocks may already be durable, so every overlapping folio
                // must be refetched before it can become authoritative again.
                self.shared.state.invalidate_range(first, count)?;
                Err(error)
            }
        }
    }

    #[cfg(feature = "ext4")]
    fn write_block_fua(&mut self, block_id: u64, buf: &[u8]) -> BlockResult<()> {
        let (first, count) = self.split_request(block_id, buf.len())?;
        let _barrier = self.shared.state.barrier.exclusive()?;

        // FUA is a durability request, never a deferred buffered write. Older
        // dirty bytes in the same range must reach the device first; then the
        // FUA request is sent unchanged and the shared cache is refreshed from
        // the completed image.
        self.shared
            .state
            .writeback_range(&mut self.inner, Some((first, count)))?;
        match self.inner.write_block_fua(block_id, buf) {
            Ok(()) => {
                self.shared.state.apply_direct(first, count, buf, false)?;
                Ok(())
            }
            Err(error) => {
                self.shared.state.invalidate_range(first, count)?;
                Err(error)
            }
        }
    }

    fn flush(&mut self) -> BlockResult<()> {
        self.sync_to_device()
    }
}

impl<T: FsBlockDevice> Drop for BufferedBlockDevice<T> {
    fn drop(&mut self) {
        // The consumer count is independent from temporary strong refs held
        // by global sync. Exactly one concurrent wrapper drop observes the
        // transition to zero and flushes while the tree remains upgradeable,
        // so a same-key creator cannot race a stale tree against a new one.
        if self.shared.release_consumer()
            && let Err(error) = self.sync_to_device()
        {
            error!("failed to flush block cache while dropping device: {error:?}");
        }
    }
}
