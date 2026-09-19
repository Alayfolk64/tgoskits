use alloc::sync::Arc;

use ax_fs_ng::vfs::{CachedFile, FileBackend};
use ax_memory_addr::{PAGE_SIZE_4K, PhysAddr, VirtAddr};

use super::file::FilePageDomain;
use super::super::objects::PageObject;
use crate::StarryResult;

/// File coordinates and cache-page ownership for one private mapping.
///
/// Full, aligned cached pages may be installed read-only into a private VMA.
/// The first write still replaces them with an anonymous page in the COW
/// backend. Unaligned ELF boundaries and direct backends stay on the copying
/// path because they cannot represent one complete cache-page identity.
#[derive(Clone)]
pub(super) struct PrivateFileBacking {
    backend: FileBackend,
    vaddr_base: VirtAddr,
    file_start: u64,
    file_end: Option<u64>,
    page_domain: Option<Arc<FilePageDomain>>,
}

impl PrivateFileBacking {
    pub(super) fn new(
        backend: FileBackend,
        vaddr_base: VirtAddr,
        file_start: u64,
        file_end: Option<u64>,
    ) -> StarryResult<Self> {
        let page_domain = match &backend {
            FileBackend::Cached(cache) => Some(FilePageDomain::get_or_create(cache)?),
            FileBackend::Direct(_) => None,
        };
        Ok(Self {
            backend,
            vaddr_base,
            file_start,
            file_end,
            page_domain,
        })
    }

    pub(super) const fn backend(&self) -> &FileBackend {
        &self.backend
    }

    pub(super) const fn vaddr_base(&self) -> VirtAddr {
        self.vaddr_base
    }

    pub(super) const fn file_start(&self) -> u64 {
        self.file_start
    }

    pub(super) const fn file_end(&self) -> Option<u64> {
        self.file_end
    }

    pub(super) fn page_cache_resident(&self, vaddr: VirtAddr) -> bool {
        let FileBackend::Cached(cache) = &self.backend else {
            return false;
        };
        let relative = vaddr
            .as_usize()
            .saturating_sub(self.vaddr_base.as_usize()) as u64;
        let Some(offset) = self.file_start.checked_add(relative) else {
            return false;
        };
        if self.file_end.is_some_and(|end| offset >= end) {
            return false;
        }
        let Ok(page_number) = u32::try_from(offset / PAGE_SIZE_4K as u64) else {
            return false;
        };
        cache.is_page_cached(page_number)
    }

    pub(super) fn prepare_cached_read_page(
        &self,
        vaddr: VirtAddr,
        leaf_size: usize,
    ) -> StarryResult<Option<Arc<PageObject>>> {
        let Some((cache, page_domain)) = self.cached_parts() else {
            return Ok(None);
        };
        let Some(page_number) = self.cache_page_number_at(vaddr, leaf_size) else {
            return Ok(None);
        };
        let file_offset = u64::from(page_number) * PAGE_SIZE_4K as u64;
        if file_offset >= cache.file_len()? {
            return Ok(None);
        }
        let pin = cache.pin_page_or_insert(page_number)?;
        page_domain
            .reserve_page(cache.mapping_epoch(), page_number, pin)
            .map(Some)
    }

    pub(super) fn cached_page_at(
        &self,
        vaddr: VirtAddr,
        paddr: PhysAddr,
    ) -> Option<Arc<PageObject>> {
        let (cache, page_domain) = self.cached_parts()?;
        let page_number = self.cache_page_number_at(vaddr, PAGE_SIZE_4K)?;
        page_domain.page_if_matches(cache.mapping_epoch(), page_number, paddr)
    }

    pub(super) fn owns_cached_page(&self, vaddr: VirtAddr, page: &Arc<PageObject>) -> bool {
        let Some((_, page_domain)) = self.cached_parts() else {
            return false;
        };
        let Some(page_number) = self.cache_page_number_at(vaddr, PAGE_SIZE_4K) else {
            return false;
        };
        page_domain.owns_page(page_number, page)
    }

    pub(super) fn finish_page_publication(
        &self,
        vaddr: VirtAddr,
        page: &Arc<PageObject>,
    ) -> StarryResult<bool> {
        let Some((cache, page_domain)) = self.cached_parts() else {
            return Ok(false);
        };
        let Some(page_number) = self.cache_page_number_at(vaddr, PAGE_SIZE_4K) else {
            return Ok(false);
        };
        if !page_domain.owns_page(page_number, page) {
            return Ok(false);
        }
        page_domain.finish_page_publication(cache.mapping_epoch(), page_number, page)?;
        Ok(true)
    }

    pub(super) fn cancel_page_publication(
        &self,
        vaddr: VirtAddr,
        page: &Arc<PageObject>,
    ) -> StarryResult<bool> {
        let Some((_, page_domain)) = self.cached_parts() else {
            return Ok(false);
        };
        let Some(page_number) = self.cache_page_number_at(vaddr, PAGE_SIZE_4K) else {
            return Ok(false);
        };
        if !page_domain.owns_page(page_number, page) {
            return Ok(false);
        }
        page_domain.cancel_page_publication(page_number, page)?;
        Ok(true)
    }

    pub(super) fn ensure_page_identity(
        &self,
        vaddr: VirtAddr,
        page: &Arc<PageObject>,
    ) -> StarryResult<bool> {
        let Some((cache, page_domain)) = self.cached_parts() else {
            return Ok(false);
        };
        let Some(page_number) = self.cache_page_number_at(vaddr, PAGE_SIZE_4K) else {
            return Ok(false);
        };
        if !page_domain.owns_page(page_number, page) {
            return Ok(false);
        }
        page_domain.ensure_page_identity(cache.mapping_epoch(), page_number, page)?;
        Ok(true)
    }

    fn cached_parts(&self) -> Option<(&CachedFile, &FilePageDomain)> {
        let FileBackend::Cached(cache) = &self.backend else {
            return None;
        };
        Some((cache, self.page_domain.as_deref()?))
    }

    fn cache_page_number_at(&self, vaddr: VirtAddr, leaf_size: usize) -> Option<u32> {
        if leaf_size != PAGE_SIZE_4K || vaddr.as_usize() < self.vaddr_base.as_usize() {
            return None;
        }
        let relative = vaddr.as_usize() - self.vaddr_base.as_usize();
        let file_offset = self.file_start.checked_add(relative as u64)?;
        if !file_offset.is_multiple_of(PAGE_SIZE_4K as u64) {
            return None;
        }
        let page_end = file_offset.checked_add(PAGE_SIZE_4K as u64)?;
        if self.file_end.is_some_and(|end| page_end > end) {
            return None;
        }
        u32::try_from(file_offset / PAGE_SIZE_4K as u64).ok()
    }
}
