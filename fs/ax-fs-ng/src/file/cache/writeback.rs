use alloc::{boxed::Box, vec::Vec};
use core::time::Duration;

use axfs_ng_vfs::{FileNodeOps, VfsError, VfsResult};

use super::{
    CacheMappingEvent, CacheMappingResult, CachedFileShared, PAGE_CACHE_SHARD_COUNT, PAGE_SIZE,
};
use crate::os::runtime_ops;

/// Upper bound for one detached writeback snapshot batch.
///
/// Linux writeback submits bounded folio/bio batches and never concatenates an
/// arbitrarily long dirty extent into a second full-size heap buffer. The VFS
/// backing interface is not scatter/gather-aware yet, so adjacent stable page
/// snapshots are copied into one bounded run before crossing into the backing
/// filesystem. This amortizes mapping preparation and lets the block layer see
/// multi-block writes without retaining unbounded dirty data across I/O.
const MAX_WRITEBACK_SNAPSHOT_PAGES: usize = 16;
const MAPPING_RETRY_INTERVAL: Duration = Duration::from_millis(1);

#[derive(Clone, Copy)]
enum MappingConflictPolicy {
    Defer,
    Wait,
}

struct DirtyPageSnapshot {
    pn: u32,
    generation: u64,
    data: Box<[u8]>,
    len: usize,
}

impl CachedFileShared {
    pub(super) fn writeback(&self) -> VfsResult<Vec<u32>> {
        let dirty_keys = self.begin_writeback_all_dirty()?;
        self.protect_dirty_pages_before_writeback_with(&dirty_keys, MappingConflictPolicy::Wait)
            .inspect_err(|_| self.cancel_writeback_tracking(&dirty_keys))?;
        let _io = self.io_lock.lock();
        let result = self.writeback_page_runs(self.len(), &dirty_keys);
        self.finish_writeback_tracking(&dirty_keys);
        result?;
        self.backing()?.sync(false)?;
        Ok(dirty_keys)
    }

    pub(super) fn writeback_pages(&self, pns: &[u32]) -> VfsResult<()> {
        let dirty_keys = self.begin_writeback_pages(pns)?;
        self.protect_dirty_pages_before_writeback_with(&dirty_keys, MappingConflictPolicy::Wait)
            .inspect_err(|_| self.cancel_writeback_tracking(&dirty_keys))?;
        let _io = self.io_lock.lock();
        let result = self.writeback_page_runs(self.len(), &dirty_keys);
        self.finish_writeback_tracking(&dirty_keys);
        result?;
        self.backing()?.sync(false)?;
        Ok(())
    }

    pub(super) fn sync(&self, data_only: bool) -> VfsResult<()> {
        let dirty_keys = self.begin_writeback_all_dirty()?;
        self.protect_dirty_pages_before_writeback_with(&dirty_keys, MappingConflictPolicy::Wait)
            .inspect_err(|_| self.cancel_writeback_tracking(&dirty_keys))?;
        let _io = self.io_lock.lock();
        let result = self.writeback_page_runs(self.len(), &dirty_keys);
        self.finish_writeback_tracking(&dirty_keys);
        result?;
        self.backing()?.sync(data_only)?;
        Ok(())
    }

    #[cfg(any(feature = "vfs", feature = "ext4"))]
    pub(super) fn writeback_dirty_for_global_sync(&self) -> VfsResult<()> {
        self.writeback_dirty_for_global_sync_with(MappingConflictPolicy::Wait)
    }

    #[cfg(feature = "ext4")]
    pub(super) fn writeback_dirty_in_background(&self) -> VfsResult<()> {
        match self.writeback_dirty_for_global_sync_with(MappingConflictPolicy::Defer) {
            Err(VfsError::ResourceBusy | VfsError::WouldBlock) => Ok(()),
            result => result,
        }
    }

    #[cfg(any(feature = "vfs", feature = "ext4"))]
    fn writeback_dirty_for_global_sync_with(
        &self,
        conflict_policy: MappingConflictPolicy,
    ) -> VfsResult<()> {
        let dirty_keys = self.begin_writeback_all_dirty()?;
        if dirty_keys.is_empty() {
            return Ok(());
        }
        self.protect_dirty_pages_before_writeback_with(&dirty_keys, conflict_policy)
            .inspect_err(|_| self.cancel_writeback_tracking(&dirty_keys))?;
        let _io = self.io_lock.lock();
        let result = self.writeback_page_runs(self.len(), &dirty_keys);
        self.finish_writeback_tracking(&dirty_keys);
        result
    }

    #[cfg(feature = "vfs")]
    pub(super) fn has_dirty_pages(&self) -> bool {
        (0..PAGE_CACHE_SHARD_COUNT).any(|index| {
            self.page_cache
                .lock_shard(index)
                .iter()
                .any(|(_, page)| page.dirty)
        })
    }

    pub(super) fn protect_dirty_pages_before_writeback(&self, pns: &[u32]) -> VfsResult<()> {
        self.protect_dirty_pages_before_writeback_with(pns, MappingConflictPolicy::Defer)
    }

    fn protect_dirty_pages_before_writeback_with(
        &self,
        pns: &[u32],
        conflict_policy: MappingConflictPolicy,
    ) -> VfsResult<()> {
        for pn in pns {
            while let Some(paddr) = {
                let mut cache = self.page_cache.lock_page(*pn);
                cache.get_mut(pn).map(|page| page.paddr()).transpose()?
            } {
                let event =
                    CacheMappingEvent::WritebackProtect(self.cache_page_identity(*pn, paddr));
                match self.publish_mapping_event(event) {
                    CacheMappingResult::Protected => break,
                    CacheMappingResult::Busy | CacheMappingResult::Quarantined
                        if matches!(conflict_policy, MappingConflictPolicy::Wait) =>
                    {
                        Self::wait_for_mapping_progress()?;
                    }
                    CacheMappingResult::Busy | CacheMappingResult::Quarantined => {
                        return Err(VfsError::ResourceBusy);
                    }
                    CacheMappingResult::Retired | CacheMappingResult::Failed => {
                        return Err(VfsError::BadState);
                    }
                }
            }
        }
        Ok(())
    }

    fn wait_for_mapping_progress() -> VfsResult<()> {
        let runtime = runtime_ops().map_err(|_| VfsError::BadState)?;
        if !runtime.can_block() {
            return Err(VfsError::WouldBlock);
        }
        let notification = runtime.notification();
        let _timed_out = notification.wait_timeout(MAPPING_RETRY_INTERVAL);
        Ok(())
    }

    fn begin_writeback_all_dirty(&self) -> VfsResult<Vec<u32>> {
        self.begin_writeback(None)
    }

    fn begin_writeback_pages(&self, pns: &[u32]) -> VfsResult<Vec<u32>> {
        self.begin_writeback(Some(pns))
    }

    fn begin_writeback(&self, requested: Option<&[u32]>) -> VfsResult<Vec<u32>> {
        let _io = self.io_lock.lock();
        let file_len = self.len();
        let mut requested_pns = if let Some(requested) = requested {
            let mut copy = Vec::new();
            copy.try_reserve_exact(requested.len())
                .map_err(|_| VfsError::NoMemory)?;
            copy.extend_from_slice(requested);
            Some(copy)
        } else {
            None
        };
        if let Some(pns) = requested_pns.as_mut() {
            pns.sort_unstable();
            pns.dedup();
        }
        let mut dirty_keys = Vec::new();
        loop {
            dirty_keys.clear();
            let required = self.page_cache.len();
            if dirty_keys.capacity() < required {
                dirty_keys
                    .try_reserve_exact(required)
                    .map_err(|_| VfsError::NoMemory)?;
            }

            if self.page_cache.len() > dirty_keys.capacity() {
                continue;
            }
            for index in 0..PAGE_CACHE_SHARD_COUNT {
                let mut guard = self.page_cache.lock_shard(index);
                for (&pn, page) in guard.iter_mut() {
                    if !page.dirty {
                        continue;
                    }
                    if let Some(requested) = requested_pns.as_ref()
                        && requested.binary_search(&pn).is_err()
                    {
                        continue;
                    }
                    let page_start = pn as u64 * PAGE_SIZE as u64;
                    let len = file_len.saturating_sub(page_start).min(PAGE_SIZE as u64);
                    if len == 0 {
                        continue;
                    }
                    page.writeback_protecting = true;
                    page.dirty_during_writeback = false;
                    dirty_keys.push(pn);
                }
            }
            break;
        }
        dirty_keys.sort_unstable();
        Ok(dirty_keys)
    }

    // The caller samples EOF only after reacquiring io_lock: mapping
    // protection runs lock-external and may race a committed truncate/write.
    fn writeback_page_runs(&self, file_len: u64, pns: &[u32]) -> VfsResult<()> {
        for batch in pns.chunks(MAX_WRITEBACK_SNAPSHOT_PAGES) {
            let snapshots = self.snapshot_dirty_pages(file_len, batch)?;
            self.writeback_snapshot_batch(&snapshots)?;
        }
        Ok(())
    }

    fn writeback_snapshot_batch(&self, snapshots: &[DirtyPageSnapshot]) -> VfsResult<()> {
        let backing = self.backing()?;
        let mut first = 0;
        while first < snapshots.len() {
            let mut end = first + 1;
            while end < snapshots.len()
                && snapshots[end - 1].pn.checked_add(1) == Some(snapshots[end].pn)
                && snapshots[end - 1].len == PAGE_SIZE
            {
                end += 1;
            }
            Self::writeback_snapshot_run(&**backing, &snapshots[first..end])?;
            first = end;
        }

        for page in snapshots {
            let mut guard = self.page_cache.lock_page(page.pn);
            if let Some(current) = guard.get_mut(&page.pn)
                && current.dirty
                && current.dirty_generation == page.generation
                && !current.dirty_during_writeback
            {
                current.dirty = false;
            }
        }
        Ok(())
    }

    fn writeback_snapshot_run(
        backing: &dyn FileNodeOps,
        pages: &[DirtyPageSnapshot],
    ) -> VfsResult<()> {
        let Some(first) = pages.first() else {
            return Ok(());
        };
        let offset = first.pn as u64 * PAGE_SIZE as u64;
        if pages.len() == 1 {
            return Self::writeback_bytes(backing, offset, &first.data[..first.len]);
        }

        let length = pages
            .iter()
            .try_fold(0usize, |total, page| total.checked_add(page.len))
            .ok_or(VfsError::ValueOverflow)?;
        let mut data = Vec::new();
        data.try_reserve_exact(length)
            .map_err(|_| VfsError::NoMemory)?;
        for page in pages {
            data.extend_from_slice(&page.data[..page.len]);
        }
        Self::writeback_bytes(backing, offset, &data)
    }

    fn writeback_bytes(backing: &dyn FileNodeOps, offset: u64, bytes: &[u8]) -> VfsResult<()> {
        let mut written = 0;
        while written < bytes.len() {
            let count = backing.write_at(&bytes[written..], offset + written as u64)?;
            if count == 0 || count > bytes.len() - written {
                return Err(VfsError::Io);
            }
            written += count;
        }
        Ok(())
    }

    fn snapshot_dirty_pages(
        &self,
        file_len: u64,
        pns: &[u32],
    ) -> VfsResult<Vec<DirtyPageSnapshot>> {
        let mut snapshots = Vec::new();
        snapshots
            .try_reserve_exact(pns.len())
            .map_err(|_| VfsError::NoMemory)?;
        for pn in pns {
            let page_start = *pn as u64 * PAGE_SIZE as u64;
            let len = file_len.saturating_sub(page_start).min(PAGE_SIZE as u64) as usize;
            if len == 0 {
                continue;
            }
            let mut data = Vec::new();
            data.try_reserve_exact(len)
                .map_err(|_| VfsError::NoMemory)?;
            let generation = {
                let mut guard = self.page_cache.lock_page(*pn);
                let Some(page) = guard.get_mut(pn) else {
                    continue;
                };
                if !page.dirty {
                    continue;
                }
                data.extend_from_slice(&page.data()[..len]);
                page.dirty_generation
            };
            if data.len() != len {
                return Err(VfsError::BadState);
            }
            snapshots.push(DirtyPageSnapshot {
                pn: *pn,
                generation,
                data: data.into_boxed_slice(),
                len,
            });
        }
        Ok(snapshots)
    }

    fn cancel_writeback_tracking(&self, pns: &[u32]) {
        let _io = self.io_lock.lock();
        self.finish_writeback_tracking(pns);
    }

    fn finish_writeback_tracking(&self, pns: &[u32]) {
        for pn in pns {
            let mut guard = self.page_cache.lock_page(*pn);
            if let Some(page) = guard.get_mut(pn) {
                page.writeback_protecting = false;
                page.dirty_during_writeback = false;
            }
        }
    }
}
