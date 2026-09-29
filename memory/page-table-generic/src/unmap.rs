//! Range walking and bounded physical-owner retirement after TLB completion.

use core::ops::Range;

use crate::{
    DeferredPageTableFrames, Frame, FrameAllocator, PageTable, PageTableEntry, PagingError,
    PagingResult, PhysAddr, PteConfigOf, TableMeta, VirtAddr,
};

const BATCH_CAPACITY: usize = 64;

/// One walk supplies either synchronous invalidation or deferred ownership.
trait RangeRetirement<T: TableMeta, A: FrameAllocator, O> {
    const RETIRE_EXISTING_EMPTY_PATHS: bool;
    fn reserve(&mut self);
    fn leaf_removed(&mut self, owner: O, address: VirtAddr);
    fn table_removed(&mut self, frame: PhysAddr, address: VirtAddr);
}

struct RangeRemoval {
    empty: bool,
    changed: bool,
}

impl<T: TableMeta, A: FrameAllocator> PageTable<T, A> {
    /// Removes complete occupied leaves without allocation or TLB invalidation.
    ///
    /// The caller retains every data-frame owner before entry and transfers
    /// returned table batches into its preallocated stage-1 retirement gather.
    /// At most one nonempty batch per cleared leaf is emitted. Existing empty
    /// paths and retained shared root entries remain installed. `retire` must
    /// not allocate, panic, reenter the table, or reclaim unconfirmed frames.
    ///
    /// Errors leave the failing leaf intact and still transfer detached table
    /// batches from the successful prefix. Return or callback completion does
    /// not revoke hardware access; only the caller's TLB receipt does so.
    pub fn unmap_range_deferred(
        &mut self,
        range: Range<VirtAddr>,
        retire: impl FnMut(DeferredPageTableFrames<A>),
    ) -> PagingResult<usize> {
        self.validate_owned_unmap_range(&range)?;
        if range.is_empty() {
            return Ok(0);
        }
        if Frame::<T, A>::PT_LEVEL > crate::table::MAX_DEFERRED_PAGE_TABLE_LEVELS {
            return Err(PagingError::hierarchy_error(
                "Page-table depth exceeds deferred reclaim capacity",
            ));
        }
        let retained = self.retained_root_entry_range();
        let mut gather = DeferredRetirement {
            tables: DeferredPageTableFrames::new(self.root.allocator.clone()),
            retire,
        };
        let mut removed = 0;
        let result = remove_range(
            &mut self.root,
            range,
            Frame::<T, A>::PT_LEVEL,
            retained,
            &mut |_leaf: MappedLeaf<PteConfigOf<T>>| -> PagingResult<()> {
                removed += 1;
                Ok(())
            },
            &mut gather,
        );
        // An empty token still reports leaf changes when no intermediate table
        // became empty. A range containing only holes changed no descriptors.
        // No table is freed at this boundary, including an error prefix.
        if removed != 0 {
            gather.finish();
        }
        result.map(|_| removed)
    }
}

struct DeferredRetirement<A: FrameAllocator, R: FnMut(DeferredPageTableFrames<A>)> {
    tables: DeferredPageTableFrames<A>,
    retire: R,
}

impl<A: FrameAllocator, R: FnMut(DeferredPageTableFrames<A>)> DeferredRetirement<A, R> {
    fn finish(&mut self) {
        let empty = DeferredPageTableFrames::new(self.tables.allocator_clone());
        (self.retire)(core::mem::replace(&mut self.tables, empty));
    }
}

impl<T: TableMeta, A: FrameAllocator, R: FnMut(DeferredPageTableFrames<A>)>
    RangeRetirement<T, A, ()> for DeferredRetirement<A, R>
{
    const RETIRE_EXISTING_EMPTY_PATHS: bool = false;

    fn reserve(&mut self) {
        if self.tables.is_full() {
            self.finish();
        }
    }

    fn leaf_removed(&mut self, _owner: (), _address: VirtAddr) {}

    fn table_removed(&mut self, frame: PhysAddr, _address: VirtAddr) {
        self.tables.push(frame);
    }
}

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

fn remove_range<T, A, O, E, G>(
    frame: &mut Frame<T, A>,
    range: Range<VirtAddr>,
    level: usize,
    retained_root_entries: Option<(usize, usize)>,
    prepare: &mut impl FnMut(MappedLeaf<PteConfigOf<T>>) -> Result<O, E>,
    gather: &mut G,
) -> Result<RangeRemoval, E>
where
    T: TableMeta,
    A: FrameAllocator,
    E: From<PagingError>,
    G: RangeRetirement<T, A, O>,
{
    let allocator = frame.allocator.clone();
    let entries = frame.as_slice_mut();
    let level_size = Frame::<T, A>::level_size(level);
    let mut address = range.start;
    let mut changed = false;

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
            changed = true;
            gather.leaf_removed(owner, address);
        } else {
            if !entry.present() {
                return Err(PagingError::hierarchy_error(
                    "Non-present intermediate entry is not a leaf",
                )
                .into());
            }
            let child_paddr = entry.paddr(true);
            let mut child = Frame::<T, A>::from_paddr(child_paddr, allocator.clone());
            let child_removed =
                remove_range(&mut child, address..next, level - 1, None, prepare, gather)?;
            changed |= child_removed.changed;
            if child_removed.empty
                && (child_removed.changed || G::RETIRE_EXISTING_EMPTY_PATHS)
                && !retained_root_entries.is_some_and(|(start, end)| start <= index && index < end)
            {
                gather.reserve();
                // Retain the detached frame until this parent update is covered
                // by a completed invalidation, including capacity-driven drains.
                entry.clear();
                changed = true;
                gather.table_removed(child_paddr, address);
            }
        }
        address = next;
    }

    // Once per visited table, not once per removed page from the VMA.
    Ok(RangeRemoval {
        empty: entries.iter().all(PageTableEntry::unused),
        changed,
    })
}

impl<T: TableMeta, A: FrameAllocator, O, R: FnMut(O)> RangeRetirement<T, A, O>
    for Retirement<T, A, O, R>
{
    const RETIRE_EXISTING_EMPTY_PATHS: bool = true;

    fn reserve(&mut self) {
        Retirement::reserve(self);
    }

    fn leaf_removed(&mut self, owner: O, address: VirtAddr) {
        assert!(self.owners.push(owner).is_ok());
        assert!(self.addresses.push(address).is_ok());
    }

    fn table_removed(&mut self, frame: PhysAddr, address: VirtAddr) {
        assert!(self.tables.push(frame).is_ok());
        assert!(self.addresses.push(address).is_ok());
    }
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
        if self.tables.is_empty() {
            T::flush_leaf_batch(&self.addresses);
        } else {
            T::flush_batch(&self.addresses);
        }
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
