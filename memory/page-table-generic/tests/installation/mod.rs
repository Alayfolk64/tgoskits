//! Real descriptor, ownership and completion contracts for absent installs.

use std::{
    alloc::{Layout, alloc, dealloc},
    cell::{Cell, RefCell},
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

use page_table_generic::*;

use super::mocks::{Fram4k, MappingFlags, PteConfig, PteImpl};

thread_local! {
    static ROOT: Cell<usize> = const { Cell::new(0) };
    static EVENTS: RefCell<Vec<Event>> = const { RefCell::new(Vec::new()) };
}

#[derive(Debug, PartialEq, Eq)]
enum Event {
    Publish(VirtAddr, PhysAddr),
    Invalidate(Option<VirtAddr>),
    Retire(VirtAddr),
}

#[test]
fn single_page_unmap_completes_publication_before_reusing_the_address() {
    let allocator = TrackingAllocator::default();
    let mut table = new_table(allocator.clone());
    let address = VirtAddr::from_usize(0x20_0000);
    let physical = PhysAddr::from_usize(0x80_0000);
    install(&mut table, address, physical).unwrap();
    install(&mut table, address + 4096, physical + 4096).unwrap();
    take_events();
    assert_eq!(table.unmap_page(address).unwrap().0, physical);
    assert_eq!(table.query(address + 4096).unwrap().0, physical + 4096);
    assert_eq!(
        allocator.live(),
        4,
        "the neighbor must retain all child tables"
    );
    assert_eq!(
        take_events(),
        [Event::Retire(address)],
        "single-page removal bypassed descriptor publication/completion"
    );
    install(&mut table, address, physical + 8192).unwrap();
    assert_eq!(take_events(), [Event::Publish(address, physical + 8192)]);
}

#[test]
fn publication_observes_the_complete_leaf_and_reuses_neighbors() {
    let allocator = TrackingAllocator::default();
    let mut table = new_table(allocator.clone());
    let references = Arc::strong_count(&allocator.0);
    let address = VirtAddr::from_usize(0x20_0000);
    let physical = PhysAddr::from_usize(0x80_0000);
    install(&mut table, address, physical).unwrap();
    assert_eq!(allocator.live(), 4);
    assert_eq!(Arc::strong_count(&allocator.0), references);
    assert_eq!(take_events(), [Event::Publish(address, physical)]);

    install(&mut table, address + 4096, physical + 4096).unwrap();
    assert_eq!(allocator.live(), 4);
    assert_eq!(Arc::strong_count(&allocator.0), references);
    assert_eq!(
        take_events(),
        [Event::Publish(address + 4096, physical + 4096)]
    );
    assert_eq!(table.query(address).unwrap().0, physical);
    assert!(matches!(
        table.query(address + 8192),
        Err(PagingError::NotMapped)
    ));
    drop(table);
    assert_eq!(allocator.live(), 0);
}

#[test]
fn every_intermediate_allocation_failure_leaves_no_visible_branch_or_leak() {
    for budget in 0..3 {
        let allocator = TrackingAllocator::default();
        let mut table = new_table(allocator.clone());
        let references = Arc::strong_count(&allocator.0);
        allocator.set_budget(budget);
        let address = VirtAddr::from_usize(0x20_0000);
        let physical = PhysAddr::from_usize(0x80_0000);
        assert_eq!(
            install(&mut table, address, physical),
            Err(PagingError::NoMemory)
        );
        assert!(root_entries(&table).iter().all(PageTableEntry::unused));
        assert_eq!(allocator.live(), 1);
        assert_eq!(Arc::strong_count(&allocator.0), references);
        assert!(take_events().is_empty());

        allocator.set_budget(usize::MAX);
        install(&mut table, address, physical).unwrap();
        assert_eq!(take_events(), [Event::Publish(address, physical)]);
        drop(table);
        assert_eq!(allocator.live(), 0);
    }
}

#[test]
fn failed_new_branch_preserves_an_existing_mapping() {
    let allocator = TrackingAllocator::default();
    let mut table = new_table(allocator.clone());
    let address = VirtAddr::from_usize(0x20_0000);
    let physical = PhysAddr::from_usize(0x80_0000);
    install(&mut table, address, physical).unwrap();
    take_events();
    let before = table.query(address).unwrap();
    allocator.set_budget(0);
    assert_eq!(
        install(&mut table, address + 0x20_0000, physical + 0x20_0000),
        Err(PagingError::NoMemory)
    );
    assert_eq!(table.query(address).unwrap(), before);
    assert_eq!(allocator.live(), 4);
    assert!(take_events().is_empty());
    drop(table);
    assert_eq!(allocator.live(), 0);
}

#[test]
fn occupied_present_and_nonpresent_leaves_are_never_overwritten() {
    for flags in [MappingFlags::READ, MappingFlags::empty()] {
        let allocator = TrackingAllocator::default();
        let mut table = new_table(allocator.clone());
        let address = VirtAddr::from_usize(0x20_0000);
        let physical = PhysAddr::from_usize(0x80_0000);
        table
            .map_page(address, physical, 4096, flags.into())
            .unwrap();
        take_events();
        assert_eq!(
            install(&mut table, address, physical + 4096),
            Err(PagingError::mapping_conflict(address, physical))
        );
        assert!(take_events().is_empty());
        assert_eq!(allocator.live(), 4);
        let (removed, config, size) = table.unmap_page(address).unwrap();
        assert_eq!(removed, physical);
        assert_eq!(config, flags);
        assert_eq!(size, 4096);
    }
}

#[test]
fn a_huge_mapping_is_a_conflict_not_an_installation_target() {
    let allocator = TrackingAllocator::default();
    let mut table = new_table(allocator.clone());
    let base = VirtAddr::from_usize(0x20_0000);
    let physical = PhysAddr::from_usize(0x80_0000);
    table
        .map_page(base, physical, 0x20_0000, MappingFlags::READ.into())
        .unwrap();
    take_events();
    let address = base + 4096;
    assert_eq!(
        install(&mut table, address, physical + 0x20_0000),
        Err(PagingError::mapping_conflict(address, physical))
    );
    assert_eq!(table.query(address).unwrap().0, physical + 4096);
    assert_eq!(allocator.live(), 3);
    assert!(take_events().is_empty());
}

#[test]
fn malformed_intermediate_is_preserved_without_allocating() {
    let allocator = TrackingAllocator::default();
    let mut table = new_table(allocator.clone());
    let invalid = PteImpl::from_config(PteConfig {
        paddr: PhysAddr::from_usize(0x80_0000),
        ..Default::default()
    });
    // SAFETY: this fixture owns the aligned identity-mapped root allocation;
    // no walker runs concurrently with this intentional malformed descriptor.
    unsafe {
        (table.root_paddr().as_usize() as *mut PteImpl).write(invalid);
    }
    assert!(matches!(
        install(
            &mut table,
            VirtAddr::from_usize(0),
            PhysAddr::from_usize(0x40_0000)
        ),
        Err(PagingError::HierarchyError { .. })
    ));
    assert_eq!(root_entries(&table)[0].0, invalid.0);
    assert_eq!(allocator.live(), 1);
    assert!(take_events().is_empty());
}

#[test]
fn invalid_addresses_are_rejected_before_allocation_or_publication() {
    let allocator = TrackingAllocator::default();
    let mut table = new_table(allocator.clone());
    for (virtual_address, physical_address) in [(1, 0x8000), (0x4000, 1)] {
        assert!(matches!(
            install(&mut table, virtual_address.into(), physical_address.into()),
            Err(PagingError::AlignmentError { .. })
        ));
    }
    for (virtual_address, physical_address) in [
        (usize::MAX - 4095, 0x8000),
        (0x4000, usize::MAX - 4095),
        (1usize << 48, 0x8000),
    ] {
        assert!(matches!(
            install(&mut table, virtual_address.into(), physical_address.into()),
            Err(PagingError::AddressOverflow { .. })
        ));
    }
    assert_eq!(allocator.live(), 1);
    assert!(take_events().is_empty());
}

#[test]
fn unconfigured_metadata_keeps_invalidation_and_extended_root_geometry() {
    let allocator = TrackingAllocator::default();
    let mut table = PageTable::<ExtendedRootMeta, _>::new(allocator.clone()).unwrap();
    take_events();
    let address = VirtAddr::from_usize(1536usize << 39);
    let physical = PhysAddr::from_usize(0x80_0000);
    // SAFETY: no hardware uses this host fixture; no previous mapping exists.
    unsafe { table.install_absent_page(address, physical, MappingFlags::READ.into()) }.unwrap();
    assert_eq!(table.query(address).unwrap().0, physical);
    assert_eq!(take_events(), [Event::Invalidate(Some(address))]);
    assert_eq!(allocator.live(), 7);
    drop(table);
    assert_eq!(allocator.live(), 0);
}

fn new_table(allocator: TrackingAllocator) -> PageTable<PublishingMeta, TrackingAllocator> {
    let table = PageTable::new(allocator).unwrap();
    ROOT.set(table.root_paddr().as_usize());
    take_events();
    table
}

fn install(
    table: &mut PageTable<PublishingMeta, TrackingAllocator>,
    vaddr: VirtAddr,
    paddr: PhysAddr,
) -> PagingResult {
    // SAFETY: these host fixtures have no hardware users; descriptors only name
    // synthetic data addresses, never accessed. Mutation is serial per table.
    unsafe { table.install_absent_page(vaddr, paddr, MappingFlags::READ.into()) }
}

fn root_entries(table: &PageTable<PublishingMeta, TrackingAllocator>) -> &[PteImpl] {
    // SAFETY: TrackingAllocator keeps this identity-mapped, aligned root alive
    // for the shared table borrow. The four-level geometry has 512 root entries.
    unsafe { core::slice::from_raw_parts(table.root_paddr().as_usize() as *const PteImpl, 512) }
}

fn take_events() -> Vec<Event> {
    EVENTS.with(|events| core::mem::take(&mut *events.borrow_mut()))
}

#[derive(Clone, Copy)]
struct PublishingMeta;

impl TableMeta for PublishingMeta {
    type P = PteImpl;
    const PAGE_SIZE: usize = 4096;
    const LEVEL_BITS: &[usize] = &[9, 9, 9, 9];
    const MAX_BLOCK_LEVEL: usize = 3;
    const STRICT_ADDRESS_WIDTH: bool = true;

    fn flush(vaddr: Option<VirtAddr>) {
        EVENTS.with(|events| events.borrow_mut().push(Event::Invalidate(vaddr)));
    }

    fn flush_batch(vaddrs: &[VirtAddr]) {
        // SAFETY: ROOT names the live fixture allocation, identity mapped by
        // Fram4k. The completion hook reads only after descriptor mutation ends.
        let table = unsafe { PageTableRef::<Self, Fram4k>::from_paddr(ROOT.get().into(), Fram4k) };
        for &vaddr in vaddrs {
            assert!(matches!(table.query(vaddr), Err(PagingError::NotMapped)));
            EVENTS.with(|events| events.borrow_mut().push(Event::Retire(vaddr)));
        }
    }

    fn publish_new_mapping(vaddr: VirtAddr) {
        // The publishing call has finished all mutable descriptor accesses.
        // This read-only fixture alias verifies actual memory, not source text.
        // SAFETY: ROOT names the live fixture allocation, identity mapped by
        // Fram4k. The completion hook reads only after descriptor mutation ends.
        let table = unsafe { PageTableRef::<Self, Fram4k>::from_paddr(ROOT.get().into(), Fram4k) };
        let (physical, _, size) = table.query(vaddr).unwrap();
        assert_eq!(size, Self::PAGE_SIZE);
        EVENTS.with(|events| events.borrow_mut().push(Event::Publish(vaddr, physical)));
    }
}

#[derive(Clone, Copy)]
struct ExtendedRootMeta;

impl TableMeta for ExtendedRootMeta {
    type P = PteImpl;
    const PAGE_SIZE: usize = 4096;
    const LEVEL_BITS: &[usize] = &[11, 9, 9, 9];
    const MAX_BLOCK_LEVEL: usize = 3;
    const STRICT_ADDRESS_WIDTH: bool = true;

    fn flush(vaddr: Option<VirtAddr>) {
        PublishingMeta::flush(vaddr);
    }
}

#[derive(Clone, Default)]
struct TrackingAllocator(Arc<Mutex<Allocations>>);

struct Allocations {
    budget: usize,
    live: BTreeMap<usize, Layout>,
}

impl Default for Allocations {
    fn default() -> Self {
        Self {
            budget: usize::MAX,
            live: BTreeMap::new(),
        }
    }
}

impl TrackingAllocator {
    fn live(&self) -> usize {
        self.0
            .lock()
            .unwrap()
            .live
            .values()
            .map(|layout| layout.size() / 4096)
            .sum()
    }

    fn set_budget(&self, budget: usize) {
        self.0.lock().unwrap().budget = budget;
    }
}

impl FrameAllocator for TrackingAllocator {
    fn alloc_frame(&self) -> Option<PhysAddr> {
        self.alloc_frames(1, 4096)
    }

    fn alloc_frames(&self, frames: usize, align: usize) -> Option<PhysAddr> {
        let mut allocations = self.0.lock().unwrap();
        if allocations.budget == 0 {
            return None;
        }
        allocations.budget -= 1;
        let layout = Layout::from_size_align(frames * 4096, align).unwrap();
        // SAFETY: the layout is nonempty and page-aligned; null is checked.
        let pointer = unsafe { alloc(layout) };
        if pointer.is_null() {
            return None;
        }
        // SAFETY: fill the uniquely owned allocation so missing zeroing is visible.
        unsafe { pointer.write_bytes(0xa5, layout.size()) };
        assert!(allocations.live.insert(pointer as usize, layout).is_none());
        Some(PhysAddr::from_usize(pointer as usize))
    }

    fn dealloc_frame(&self, address: PhysAddr) {
        let layout = self
            .0
            .lock()
            .unwrap()
            .live
            .remove(&address.as_usize())
            .unwrap();
        // SAFETY: this exact allocation is removed once with its original layout.
        unsafe { dealloc(address.as_usize() as *mut u8, layout) };
    }

    fn dealloc_frames(&self, address: PhysAddr, _frames: usize, _frame_size: usize) {
        self.dealloc_frame(address);
    }

    fn phys_to_virt(&self, address: PhysAddr) -> *mut u8 {
        address.as_usize() as *mut u8
    }
}
