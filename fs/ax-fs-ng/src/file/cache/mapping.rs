//! A retained physical page is distinct from permission to publish its mapping.

use core::sync::atomic::Ordering;

use axfs_ng_vfs::VfsResult;

use super::{CachedFile, PAGE_SIZE};
use crate::file::CachedPagePin;

impl CachedFile {
    /// Pins an initialized, complete file page for a read-only mapping.
    ///
    /// Returns None when the page is not wholly within current EOF. Partial
    /// pages need the consumer's existing bounded initialization path. The
    /// pin retains physical storage but must be revalidated with
    /// `with_current_read_page` before publishing a PTE after sleeping.
    /// Backing I/O and allocation failures are propagated, not converted to None.
    pub fn pin_read_page(&self, number: u32) -> VfsResult<Option<CachedPagePin>> {
        #[cfg(feature = "profile")]
        let _profile = self.profile_scope();
        let start = u64::from(number) * PAGE_SIZE as u64;
        let end = start + PAGE_SIZE as u64;
        let file = self.inner.entry().as_file()?;
        let window = self.readahead.lock().plan(start, end).window_pages;
        loop {
            {
                let mut cache = self.shared.page_cache.lock();
                if !self.shared.updating.load(Ordering::Acquire) {
                    if end > self.shared.len() {
                        return Ok(None);
                    }
                    if let Some(page) = cache.get_mut(&number) {
                        return Ok(Some(page.pin()));
                    }
                }
            }
            self.populate_page_window(file, number, window)?;
        }
    }

    /// Publishes a consumer mapping only while this pin is the current full page.
    ///
    /// Holds stable cache-index exclusion while calling `publish`. The callback
    /// must not reenter the cache, perform I/O, access faultable memory or
    /// acquire a mapping lock: the caller must already own its mapping state.
    /// None means tentative contents, a stale/replaced page or changed EOF;
    /// the callback is not run. This never waits for the cached I/O owner.
    /// A successful mapping must retain its physical pin until translation
    /// invalidation completes. This operation never grants writable access.
    pub fn with_current_read_page<T>(
        &self,
        number: u32,
        pin: &CachedPagePin,
        publish: impl FnOnce() -> VfsResult<T>,
    ) -> VfsResult<Option<T>> {
        let mut cache = self.shared.page_cache.lock();
        let end = (u64::from(number) + 1) * PAGE_SIZE as u64;
        if self.shared.updating.load(Ordering::Acquire) || end > self.shared.len() {
            return Ok(None);
        }
        if !cache
            .get_mut(&number)
            .is_some_and(|page| page.matches_pin(pin))
        {
            return Ok(None);
        }
        publish().map(Some)
    }
}
