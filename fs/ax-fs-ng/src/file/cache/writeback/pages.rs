//! One finite selection and unconditional reservation cleanup.

use super::*;

impl<'a> WritebackPages<'a> {
    pub(super) fn begin(
        shared: &'a CachedFileShared,
        requested: Option<&[u32]>,
    ) -> VfsResult<Self> {
        let mut requested_pns = requested
            .map(|requested| -> VfsResult<Vec<u32>> {
                let mut numbers = Vec::new();
                numbers
                    .try_reserve_exact(requested.len())
                    .map_err(|_| VfsError::NoMemory)?;
                numbers.extend_from_slice(requested);
                Ok(numbers)
            })
            .transpose()?;
        if let Some(pns) = requested_pns.as_mut() {
            pns.sort_unstable();
            pns.dedup();
        }
        let io = shared.io_lock.lock();
        let file_len = shared.len();
        let selected = |number: u32| {
            u64::from(number) * (PAGE_SIZE as u64) < file_len
                && requested_pns
                    .as_ref()
                    .is_none_or(|pns| pns.binary_search(&number).is_ok())
        };
        let count = shared
            .page_cache
            .lock()
            .iter()
            .filter(|(number, page)| page.dirty && selected(**number))
            .count();
        // Allocate before changing any page state, without the cache index.
        // io_lock keeps the selected membership stable between both scans.
        let mut pages = Vec::new();
        pages
            .try_reserve_exact(count)
            .map_err(|_| VfsError::NoMemory)?;
        {
            let mut cache = shared.page_cache.lock();
            for (&number, page) in cache.iter_mut() {
                if page.dirty && selected(number) {
                    page.begin_writeback();
                    pages.push(WritebackPage {
                        number,
                        pin: page.pin(),
                    });
                }
            }
        }
        drop(io);
        pages.sort_unstable_by_key(|page| page.number);
        Ok(Self {
            shared,
            file_len,
            pages,
        })
    }

    pub(super) fn protect(&self) -> VfsResult<()> {
        let listeners = self.shared.writeback_protect_listeners();
        for page in &self.pages {
            for listener in &listeners {
                if !listener(page.number) {
                    return Err(VfsError::ResourceBusy);
                }
            }
        }
        Ok(())
    }

    pub(super) fn numbers(&self) -> VfsResult<Vec<u32>> {
        let mut numbers = Vec::new();
        numbers
            .try_reserve_exact(self.pages.len())
            .map_err(|_| VfsError::NoMemory)?;
        numbers.extend(self.pages.iter().map(|page| page.number));
        Ok(numbers)
    }
}

impl Drop for WritebackPages<'_> {
    fn drop(&mut self) {
        // All per-batch and callback guards have ended before this owner is
        // dropped. On error, only already completed versions may be clean.
        let _io = self.shared.io_lock.lock();
        let mut cache = self.shared.page_cache.lock();
        for tracked in &self.pages {
            if let Some(page) = cache.get_mut(&tracked.number)
                && page.matches_pin(&tracked.pin)
            {
                page.finish_writeback();
            }
        }
    }
}
