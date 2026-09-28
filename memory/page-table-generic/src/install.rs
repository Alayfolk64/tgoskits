//! Absent base-page installation with unpublished intermediate-table ownership.

use core::sync::atomic::{Ordering, fence};

use crate::{
    Frame, FrameAllocator, PageTableEntry, PageTableRef, PagingError, PagingResult, PhysAddr,
    PteConfigOf, TableMeta, VirtAddr,
};

impl<T: TableMeta, A: FrameAllocator> PageTableRef<T, A> {
    /// Installs one aligned base page and completes its initial publication.
    ///
    /// Missing intermediate tables are built off-tree and linked only after
    /// the complete branch is initialized. Failure leaves existing descriptors
    /// unchanged and releases every newly allocated table, never the data page.
    /// Unlike [`Self::map_page`], addresses are not rounded down.
    ///
    /// # Errors
    ///
    /// Rejects unaligned/overflowing addresses, occupied leaves (including
    /// non-present leaves), huge mappings and malformed intermediate entries.
    /// Returns [`PagingError::NoMemory`] if a child table cannot be allocated.
    ///
    /// # Safety
    ///
    /// The caller must exclude other software mutations, supply initialized
    /// backing with the required cache attributes/maintenance, and retain it
    /// through completion of any subsequent removal. Any previous valid mapping
    /// of this address must already be invalidated on every hardware user of
    /// the table: an unused PTE alone does not prove that no stale TLB entry
    /// remains. In particular, unfinished deferred unmaps cannot use this path.
    pub unsafe fn install_absent_page(
        &mut self,
        vaddr: VirtAddr,
        paddr: PhysAddr,
        config: PteConfigOf<T>,
    ) -> PagingResult {
        self.validate_mapping(vaddr, paddr, T::PAGE_SIZE)?;
        let entry = T::P::new_page(paddr, config, false);
        if entry.unused() {
            return Err(PagingError::invalid_range(
                "Mapping encodes an unused page-table entry",
            ));
        }
        install_entry(&mut self.root, vaddr, entry, Frame::<T, A>::PT_LEVEL)?;
        T::publish_new_mapping(vaddr);
        Ok(())
    }
}

fn install_entry<T: TableMeta, A: FrameAllocator>(
    frame: &mut Frame<T, A>,
    vaddr: VirtAddr,
    entry: T::P,
    level: usize,
) -> PagingResult {
    let index = Frame::<T, A>::virt_to_index(vaddr, level);
    let current = frame.as_slice()[index];
    if level == 1 {
        if !current.unused() {
            return Err(PagingError::mapping_conflict(vaddr, current.paddr(false)));
        }
        // Publish initialized backing before hardware can observe the leaf.
        // Architecture completion is performed once at the operation boundary.
        fence(Ordering::Release);
        frame.as_slice_mut()[index] = entry;
        return Ok(());
    }

    if current.unused() {
        let mut branch = UnpublishedBranch::<T, A>::new(frame.allocator.clone(), level - 1)?;
        install_entry(branch.frame(), vaddr, entry, level - 1)?;
        branch.publish(&mut frame.as_slice_mut()[index]);
        return Ok(());
    }
    if current.huge(true) {
        return Err(PagingError::mapping_conflict(vaddr, current.paddr(true)));
    }
    if !current.present() {
        return Err(PagingError::hierarchy_error(
            "Non-present intermediate entry is not a leaf",
        ));
    }
    let mut child = Frame::<T, A>::from_paddr(current.paddr(true), frame.allocator.clone());
    install_entry(&mut child, vaddr, entry, level - 1)
}

/// Owns only table frames that no hardware walker or other software can reach.
struct UnpublishedBranch<T: TableMeta, A: FrameAllocator> {
    frame: Option<Frame<T, A>>,
    level: usize,
}

impl<T: TableMeta, A: FrameAllocator> UnpublishedBranch<T, A> {
    fn new(allocator: A, level: usize) -> PagingResult<Self> {
        Ok(Self {
            frame: Some(Frame::new(allocator)?),
            level,
        })
    }

    fn frame(&mut self) -> &mut Frame<T, A> {
        self.frame
            .as_mut()
            .expect("the branch is not published yet")
    }

    fn publish(mut self, parent: &mut T::P) {
        let descriptor = T::P::new_table(self.frame().paddr);
        // All child initialization precedes the first reachable descriptor.
        // Software mutation remains excluded by the owning table's caller.
        fence(Ordering::Release);
        *parent = descriptor;
        // Disarm table reclamation, but still drop the allocator clone.
        drop(self.frame.take());
    }
}

impl<T: TableMeta, A: FrameAllocator> Drop for UnpublishedBranch<T, A> {
    fn drop(&mut self) {
        // No invalidation is needed: this branch was never reachable. The
        // recursive helper releases tables only, not the caller's data page.
        if let Some(frame) = &mut self.frame {
            frame.deallocate_recursive(self.level);
        }
    }
}
