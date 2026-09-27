//! Clean physical blocks below rsext4's journal and mutable staging caches.

use alloc::{vec, vec::Vec};
use core::num::NonZeroUsize;

use lru::LruCache;
use rsext4::{bmalloc::AbsoluteBN, config::BLOCK_SIZE};

const CACHE_BLOCKS: NonZeroUsize = NonZeroUsize::new(64).unwrap();

/// Owned by one exclusive Ext4Disk; independent readers never publish here.
/// Every device write must update these copies on success or clear them on error.
pub(super) struct CleanBlockCache {
    blocks: LruCache<AbsoluteBN, Vec<u8>>,
}

impl CleanBlockCache {
    pub(super) fn new() -> Self {
        Self {
            blocks: LruCache::new(CACHE_BLOCKS),
        }
    }

    /// Non-mutating query for the journal's lock-free pre-read preparation.
    pub(super) fn peek(&self, block: AbsoluteBN) -> Option<&[u8]> {
        self.blocks.peek(&block).map(Vec::as_slice)
    }

    /// The caller requests exactly one filesystem block.
    pub(super) fn copy_block(&mut self, block: AbsoluteBN, buffer: &mut [u8]) -> bool {
        let Some(bytes) = self.blocks.get(&block) else {
            return false;
        };
        buffer.copy_from_slice(bytes);
        true
    }

    /// Called only after a successful, single-block device read.
    pub(super) fn remember_read(&mut self, block: AbsoluteBN, buffer: &[u8]) {
        let mut bytes = if self.blocks.len() == CACHE_BLOCKS.get() {
            self.blocks
                .pop_lru()
                .expect("a full nonzero-capacity cache contains an eviction victim")
                .1
        } else {
            vec![0; BLOCK_SIZE]
        };
        bytes.copy_from_slice(buffer);
        self.blocks.put(block, bytes);
    }

    /// Refresh existing copies only; streaming writes do not populate the cache.
    pub(super) fn update_written(&mut self, first: AbsoluteBN, buffer: &[u8]) {
        let count = buffer.len() / BLOCK_SIZE;
        for (block, cached) in self.blocks.iter_mut() {
            let Some(offset) = block.raw().checked_sub(first.raw()) else {
                continue;
            };
            if offset < count as u64 {
                let start = offset as usize * BLOCK_SIZE;
                cached.copy_from_slice(&buffer[start..start + BLOCK_SIZE]);
            }
        }
    }

    /// A failed write may have partially modified any of its target blocks.
    pub(super) fn discard_after_write_error(&mut self) {
        self.blocks.clear();
    }
}
