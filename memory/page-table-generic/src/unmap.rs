//! Range walking and bounded physical-owner retirement after TLB completion.

use core::{fmt, ops::Range};

use crate::{
    Frame, FrameAllocator, PageTable, PageTableEntry, PagingError, PhysAddr, PteConfigOf,
    TableMeta, VirtAddr,
};

const BATCH_CAPACITY: usize = 64;

/// An occupied mapping presented before its descriptor is cleared.
#[derive(Debug)]
pub struct MappedLeaf<C> {
    /// Base virtual address of the complete leaf mapping.
    pub vaddr: VirtAddr,
    /// Base physical address retained by the mapping.
    pub paddr: PhysAddr,
    /// Size represented by this leaf's page-table level.
    pub size: usize,
    /// Opaque architecture-owned configuration, including non-present state.
    pub config: C,
}

/// One table's bounded, invalidation-before-release unmap operation.
///
/// Obtain this through [`PageTable::unmap_session`]. Successful ranges share
/// pending retirement; capacity, errors, [`Self::finish`] and dropping the
/// session complete invalidation before releasing owners or table frames.
/// The caller must exclude other software walkers and mutations throughout,
/// and the metadata's flush capability must cover every hardware user.
pub struct UnmapSession<'a, T: TableMeta, A: FrameAllocator, O, R: FnMut(O)> {
    table: &'a mut PageTable<T, A>,
    retirement: Retirement<T, A, O, R>,
}

impl<'a, T: TableMeta, A: FrameAllocator, O, R: FnMut(O)> UnmapSession<'a, T, A, O, R> {
    pub(crate) fn new(table: &'a mut PageTable<T, A>, retire: R) -> Self {
        let retirement = Retirement::new(table.root.allocator.clone(), retire);
        Self { table, retirement }
    }

    /// Detaches a checked range without ending the surrounding operation.
    ///
    /// `prepare` must return a retained physical owner before the leaf is
    /// cleared. Neither preparation nor retirement callbacks may panic or
    /// re-enter this table. A successful return does not by itself revoke
    /// hardware access; keep the session until the entire operation finishes.
    ///
    /// # Errors
    ///
    /// Uses the same range and leaf checks as [`crate::PageTableRef::unmap_owned`].
    /// On any error, all previously detached mappings are invalidated and
    /// retired before returning; the failing leaf is left intact.
    pub fn unmap<E: From<PagingError>>(
        &mut self,
        range: Range<VirtAddr>,
        mut prepare: impl FnMut(MappedLeaf<PteConfigOf<T>>) -> Result<O, E>,
    ) -> Result<(), E> {
        let result = self
            .table
            .validate_owned_unmap_range(&range)
            .map_err(E::from)
            .and_then(|()| {
                if range.is_empty() {
                    return Ok(());
                }
                let retained = self.table.retained_root_entry_range();
                remove_range(
                    &mut self.table.root,
                    range,
                    Frame::<T, A>::PT_LEVEL,
                    retained,
                    &mut prepare,
                    &mut self.retirement,
                )
                .map(|_| ())
            });
        if result.is_err() {
            self.retirement.finish();
        }
        result
    }

    /// Completes pending retirement before a synchronous table operation.
    ///
    /// No unflushed table borrow escapes this boundary. If the callback
    /// replaces the owning table, adopt its allocator with an empty queue.
    pub fn with_flushed_table<V>(
        &mut self,
        operation: impl FnOnce(&mut PageTable<T, A>) -> V,
    ) -> V {
        self.retirement.finish();
        let result = operation(self.table);
        self.retirement.allocator = self.table.root.allocator.clone();
        result
    }

    /// Completes the final invalidation and consumes the operation.
    pub fn finish(mut self) {
        self.retirement.finish();
    }
}

impl<T: TableMeta, A: FrameAllocator, O, R: FnMut(O)> fmt::Debug for UnmapSession<'_, T, A, O, R> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("UnmapSession")
            .field("root", &self.table.root_paddr())
            .field("pending_descriptors", &self.retirement.addresses.len())
            .finish_non_exhaustive()
    }
}

pub(crate) fn unmap_owned<T, A, O, E>(
    root: &mut Frame<T, A>,
    range: Range<VirtAddr>,
    retained_root_entries: Option<(usize, usize)>,
    mut prepare: impl FnMut(MappedLeaf<PteConfigOf<T>>) -> Result<O, E>,
    retire: impl FnMut(O),
) -> Result<(), E>
where
    T: TableMeta,
    A: FrameAllocator,
    E: From<PagingError>,
{
    let mut gather = Retirement::<T, A, O, _>::new(root.allocator.clone(), retire);
    let result = remove_range(
        root,
        range,
        Frame::<T, A>::PT_LEVEL,
        retained_root_entries,
        &mut prepare,
        &mut gather,
    );
    // This also drains successful prefixes when preparation or walking failed.
    gather.finish();
    result.map(|_| ())
}

fn remove_range<T, A, O, E, R>(
    frame: &mut Frame<T, A>,
    range: Range<VirtAddr>,
    level: usize,
    retained_root_entries: Option<(usize, usize)>,
    prepare: &mut impl FnMut(MappedLeaf<PteConfigOf<T>>) -> Result<O, E>,
    gather: &mut Retirement<T, A, O, R>,
) -> Result<bool, E>
where
    T: TableMeta,
    A: FrameAllocator,
    E: From<PagingError>,
    R: FnMut(O),
{
    let allocator = frame.allocator.clone();
    let entries = frame.as_slice_mut();
    let level_size = Frame::<T, A>::level_size(level);
    let mut address = range.start;

    while address < range.end {
        let index = Frame::<T, A>::virt_to_index(address, level);
        let to_boundary = level_size - address.as_usize() % level_size;
        let next = address + to_boundary.min(range.end - address);
        let entry = &mut entries[index];
        if entry.unused() {
            address = next;
            continue;
        }

        if level == 1 || entry.huge(true) {
            if !address.as_usize().is_multiple_of(level_size) || next - address != level_size {
                return Err(PagingError::invalid_range(
                    "Unmap range intersects a partial huge leaf",
                )
                .into());
            }
            gather.reserve();
            let owner = prepare(MappedLeaf {
                vaddr: address,
                paddr: entry.paddr(level > 1),
                size: level_size,
                config: entry.config(level > 1),
            })?;
            entry.clear();
            // reserve() guarantees space, with no fallible step after clear.
            assert!(gather.owners.push(owner).is_ok());
            assert!(gather.addresses.push(address).is_ok());
        } else {
            if !entry.present() {
                return Err(PagingError::hierarchy_error(
                    "Non-present intermediate entry is not a leaf",
                )
                .into());
            }
            let child_paddr = entry.paddr(true);
            let mut child = Frame::<T, A>::from_paddr(child_paddr, allocator.clone());
            if remove_range(&mut child, address..next, level - 1, None, prepare, gather)?
                && !retained_root_entries.is_some_and(|(start, end)| start <= index && index < end)
            {
                gather.reserve();
                // Retain the detached frame until this parent update is covered
                // by a completed invalidation, including capacity-driven drains.
                entry.clear();
                assert!(gather.tables.push(child_paddr).is_ok());
                assert!(gather.addresses.push(address).is_ok());
            }
        }
        address = next;
    }

    // Once per visited table, not once per removed page from the VMA.
    Ok(entries.iter().all(PageTableEntry::unused))
}

struct Retirement<T: TableMeta, A: FrameAllocator, O, R: FnMut(O)> {
    addresses: heapless::Vec<VirtAddr, BATCH_CAPACITY>,
    owners: heapless::Vec<O, BATCH_CAPACITY>,
    tables: heapless::Vec<PhysAddr, BATCH_CAPACITY>,
    allocator: A,
    retire: R,
    marker: core::marker::PhantomData<T>,
}

impl<T: TableMeta, A: FrameAllocator, O, R: FnMut(O)> Retirement<T, A, O, R> {
    fn new(allocator: A, retire: R) -> Self {
        Self {
            addresses: heapless::Vec::new(),
            owners: heapless::Vec::new(),
            tables: heapless::Vec::new(),
            allocator,
            retire,
            marker: core::marker::PhantomData,
        }
    }

    fn reserve(&mut self) {
        // Every queued owner/table also has an address, so its queue cannot
        // exhaust capacity before this one. No allocation is needed to unmap.
        if self.addresses.is_full() {
            self.finish();
        }
    }

    fn finish(&mut self) {
        if self.addresses.is_empty() {
            return;
        }
        T::flush_batch(&self.addresses);
        self.addresses.clear();
        while let Some(table) = self.tables.pop() {
            self.allocator.dealloc_frame(table);
        }
        while let Some(owner) = self.owners.pop() {
            (self.retire)(owner);
        }
    }
}

impl<T: TableMeta, A: FrameAllocator, O, R: FnMut(O)> Drop for Retirement<T, A, O, R> {
    fn drop(&mut self) {
        self.finish();
    }
}
