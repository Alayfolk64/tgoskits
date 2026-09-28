//! Cross-range completion through the production owning-table session.

use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

use super::*;

#[test]
fn disjoint_ranges_share_one_completion_and_retain_owners_until_finish() {
    let mut table = new_table();
    let start = VirtAddr::from(0x20_0000);
    for index in 0..3 {
        table
            .map_page(
                start + index * 8192,
                (0x80_0000 + index * 4096).into(),
                4096,
                1,
            )
            .unwrap();
    }
    reset_events();
    let mut session = table.unmap_session(retire_leaf);
    for index in 0..3 {
        let address = start + index * 8192;
        session
            .unmap(address..address + 4096, Ok::<_, PagingError>)
            .unwrap();
        assert!(
            retired_addresses().is_empty(),
            "range ended the surrounding operation"
        );
        EVENTS.with_borrow(|events| {
            assert!(events.iter().all(|event| matches!(event, Event::Clear)))
        });
    }
    session.unmap(start..start, Ok::<_, PagingError>).unwrap();
    assert!(retired_addresses().is_empty());
    session.finish();
    assert_eq!(
        retired_addresses(),
        (0..3)
            .map(|index| start.as_usize() + index * 8192)
            .collect::<Vec<_>>()
    );
    assert_eq!(batch_sizes(), [6]); // Three leaves and three detached tables.
    assert_retirement_order();
    assert!(root_entries(&table).iter().all(PageTableEntry::unused));
}

#[test]
fn capacity_completion_spans_ranges_without_unbounded_storage() {
    let mut table = new_table();
    let start = VirtAddr::from(0x20_0000);
    for index in 0..130 {
        table
            .map_page(
                start + index * 8192,
                (0x80_0000 + index * 4096).into(),
                4096,
                1,
            )
            .unwrap();
    }
    reset_events();
    let mut session = table.unmap_session(retire_leaf);
    for index in 0..130 {
        let address = start + index * 8192;
        session
            .unmap(address..address + 4096, Ok::<_, PagingError>)
            .unwrap();
    }
    assert_eq!(retired_addresses().len(), 128);
    session.finish();
    assert_eq!(retired_addresses().len(), 130);
    assert_eq!(batch_sizes(), [64, 64, 5]);
    assert_retirement_order();
}

#[test]
fn malformed_range_drains_earlier_work_before_returning_error() {
    let start = VirtAddr::from(0x20_0000);
    for invalid in [
        start + 4096..start,
        start + 1..start + 4096,
        start..start + 1,
    ] {
        let mut table = new_table();
        table.map_page(start, 0x80_0000.into(), 4096, 1).unwrap();
        reset_events();
        let mut session = table.unmap_session(retire_leaf);
        session
            .unmap(start..start + 4096, Ok::<_, PagingError>)
            .unwrap();
        assert!(retired_addresses().is_empty());
        assert!(session.unmap(invalid, Ok::<_, PagingError>).is_err());
        assert_eq!(retired_addresses(), [start.as_usize()]);
        assert_retirement_order();
        session.finish();
        assert_eq!(batch_sizes().len(), 1);
    }
}

#[test]
fn preparation_error_drains_prior_ranges_and_current_prefix_only() {
    let mut table = new_table();
    let start = VirtAddr::from(0x20_0000);
    for index in 0..3 {
        table
            .map_page(
                start + index * 4096,
                (0x80_0000 + index * 4096).into(),
                4096,
                1,
            )
            .unwrap();
    }
    reset_events();
    let mut session = table.unmap_session(retire_leaf);
    session
        .unmap(start..start + 4096, Ok::<_, PagingError>)
        .unwrap();
    assert_eq!(
        session.unmap(start + 4096..start + 3 * 4096, |leaf| {
            if leaf.vaddr == start + 8192 {
                Err(PagingError::NoMemory)
            } else {
                Ok(leaf)
            }
        }),
        Err(PagingError::NoMemory)
    );
    assert_eq!(
        retired_addresses(),
        [start.as_usize(), start.as_usize() + 4096]
    );
    assert_retirement_order();
    session.with_flushed_table(|table| {
        assert!(matches!(table.query(start), Err(PagingError::NotMapped)));
        assert_eq!(
            table.query(start + 8192).unwrap().0,
            PhysAddr::from(0x80_2000)
        );
    });
}

#[test]
fn table_handoff_completes_old_retirement_before_replacement() {
    let mut table = new_table();
    let start = VirtAddr::from(0x20_0000);
    table.map_page(start, 0x80_0000.into(), 4096, 1).unwrap();
    let mut replacement = new_table();
    replacement
        .map_page(start, 0x90_0000.into(), 4096, 1)
        .unwrap();
    let replacement_root = replacement.root_paddr();
    reset_events();
    let mut session = table.unmap_session(retire_leaf);
    session
        .unmap(start..start + 4096, Ok::<_, PagingError>)
        .unwrap();
    session.with_flushed_table(|table| {
        assert_eq!(retired_addresses(), [start.as_usize()]);
        assert_retirement_order();
        *table = replacement;
    });
    session
        .unmap(start..start + 4096, Ok::<_, PagingError>)
        .unwrap();
    session.finish();
    assert_eq!(table.root_paddr(), replacement_root);
    assert_eq!(batch_sizes(), [4, 4]);
    assert_eq!(retired_addresses(), [start.as_usize(), start.as_usize()]);
    assert_retirement_order();
}

#[test]
fn scope_exit_completes_huge_and_inaccessible_mapping_retirement() {
    let mut table = new_table();
    let start = VirtAddr::from(0x20_0000);
    table
        .map_page(start, 0x80_0000.into(), 0x20_0000, 0)
        .unwrap();
    table
        .map_page(start + 0x40_0000, 0xa0_0000.into(), 4096, 0)
        .unwrap();
    reset_events();
    {
        let mut session = table.unmap_session(retire_leaf);
        session
            .unmap(start..start + 0x20_0000, |leaf| {
                assert_eq!(leaf.size, 0x20_0000);
                assert_eq!(leaf.config, 0);
                Ok::<_, PagingError>(leaf)
            })
            .unwrap();
        session
            .unmap(start + 0x40_0000..start + 0x40_1000, Ok::<_, PagingError>)
            .unwrap();
        assert!(retired_addresses().is_empty());
    }
    assert_eq!(
        retired_addresses(),
        [start.as_usize(), start.as_usize() + 0x40_0000]
    );
    assert_eq!(batch_sizes(), [5]);
    assert_retirement_order();
}

#[test]
fn partial_huge_error_drains_prior_ranges_without_removing_the_huge_owner() {
    let mut table = new_table();
    let start = VirtAddr::from(0x20_0000);
    let huge = start + 0x20_0000;
    table.map_page(start, 0x80_0000.into(), 4096, 1).unwrap();
    table
        .map_page(huge, 0xa0_0000.into(), 0x20_0000, 1)
        .unwrap();
    reset_events();
    let mut session = table.unmap_session(retire_leaf);
    session
        .unmap(start..start + 4096, Ok::<_, PagingError>)
        .unwrap();
    assert!(matches!(
        session.unmap(huge + 4096..huge + 8192, Ok::<_, PagingError>),
        Err(PagingError::InvalidRange { .. })
    ));
    assert_eq!(retired_addresses(), [start.as_usize()]);
    assert_retirement_order();
    session.with_flushed_table(|table| {
        assert_eq!(table.query(huge).unwrap().0, PhysAddr::from(0xa0_0000))
    });
    // The same session remains usable after an error drained the earlier work.
    session
        .unmap(huge..huge + 0x20_0000, Ok::<_, PagingError>)
        .unwrap();
    session.finish();
    assert_eq!(retired_addresses(), [start.as_usize(), huge.as_usize()]);
    assert_retirement_order();
}

#[test]
fn replacement_table_retires_frames_through_its_own_allocator() {
    let owners = Arc::new(Mutex::new(BTreeMap::new()));
    let mut table = PageTable::<RetirementMeta, _>::new(TaggedAllocator {
        tag: 1,
        owners: owners.clone(),
    })
    .unwrap();
    let mut replacement = PageTable::<RetirementMeta, _>::new(TaggedAllocator {
        tag: 2,
        owners: owners.clone(),
    })
    .unwrap();
    let start = VirtAddr::from(0x20_0000);
    table.map_page(start, 0x80_0000.into(), 4096, 1).unwrap();
    replacement
        .map_page(start, 0x90_0000.into(), 4096, 1)
        .unwrap();
    reset_events();
    let mut session = table.unmap_session(retire_leaf);
    session
        .unmap(start..start + 4096, Ok::<_, PagingError>)
        .unwrap();
    session.with_flushed_table(|table| *table = replacement);
    session
        .unmap(start..start + 4096, Ok::<_, PagingError>)
        .unwrap();
    session.finish();
    assert_eq!(owners.lock().unwrap().len(), 1); // The replacement root remains owned.
    drop(table);
    assert!(owners.lock().unwrap().is_empty());
    assert_retirement_order();
}

fn batch_sizes() -> Vec<usize> {
    EVENTS.with_borrow(|events| {
        events
            .iter()
            .filter_map(|event| match event {
                Event::Batch(size) => Some(*size),
                _ => None,
            })
            .collect()
    })
}

#[derive(Clone)]
struct TaggedAllocator {
    tag: usize,
    owners: Arc<Mutex<BTreeMap<PhysAddr, usize>>>,
}

impl FrameAllocator for TaggedAllocator {
    fn alloc_frame(&self) -> Option<PhysAddr> {
        let address = RetirementAllocator.alloc_frame()?;
        assert!(
            self.owners
                .lock()
                .unwrap()
                .insert(address, self.tag)
                .is_none()
        );
        Some(address)
    }

    fn dealloc_frame(&self, address: PhysAddr) {
        assert_eq!(
            self.owners.lock().unwrap().remove(&address),
            Some(self.tag),
            "detached table was returned to another allocator"
        );
        RetirementAllocator.dealloc_frame(address);
    }

    fn phys_to_virt(&self, address: PhysAddr) -> *mut u8 {
        RetirementAllocator.phys_to_virt(address)
    }
}
