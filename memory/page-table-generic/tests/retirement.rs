//! Production page-table retirement ordering, observed at capability boundaries.

#![cfg(not(target_os = "none"))]

mod retirement_support;
mod session_retirement;

use page_table_generic::*;
use retirement_support::*;

#[test]
fn base_page_conflict_reports_existing_physical_address() {
    assert_conflict_preserves_mapping(4096);
}

#[test]
fn huge_page_conflict_reports_existing_physical_address() {
    assert_conflict_preserves_mapping(0x20_0000);
}

#[test]
fn detached_tables_are_invalidated_before_allocator_reuse() {
    let mut table =
        PageTable::<RetirementMeta, RetirementAllocator>::new(RetirementAllocator).unwrap();
    table
        .map_page(0x20_0000.into(), 0x40_0000.into(), 4096, 1)
        .unwrap();
    EVENTS.with_borrow_mut(Vec::clear);

    table.unmap_page(0x20_0000.into()).unwrap();

    EVENTS.with_borrow(|events| {
        let mut pending_clear = false;
        let mut frees = 0;
        for event in events {
            match event {
                Event::Clear => pending_clear = true,
                Event::Flush => pending_clear = false,
                Event::Batch(_) | Event::Retire(_) => {}
                Event::Free => {
                    assert!(
                        !pending_clear,
                        "table freed before its unlink was invalidated: {events:?}"
                    );
                    frees += 1;
                }
            }
        }
        assert_eq!(frees, 3, "all child tables must be reclaimed");
    });
}

#[test]
fn occupied_leaf_query_retains_inaccessible_mapping_geometry() {
    for size in [4096, 0x20_0000] {
        let mut table = new_table();
        let address = VirtAddr::from(0x20_0000);
        let physical = PhysAddr::from(0x80_0000);
        assert!(matches!(
            table.query_occupied_leaf(address),
            Err(PagingError::NotMapped)
        ));
        table.map_page(address, physical, size, 1).unwrap();
        table.protect_page(address, 0).unwrap();
        assert_eq!(table.query(address), Err(PagingError::NotMapped));
        reset_events();
        let leaf = table.query_occupied_leaf(address + size - 1).unwrap();
        assert_eq!(leaf.vaddr, address);
        assert_eq!(leaf.paddr, physical);
        assert_eq!(leaf.size, size);
        assert_eq!(leaf.config, 0);
        EVENTS.with_borrow(|events| assert!(events.is_empty()));
        assert_eq!(table.unmap_page(address).unwrap(), (physical, 0, size));
        assert!(matches!(
            table.query_occupied_leaf(address),
            Err(PagingError::NotMapped)
        ));
    }
}

#[test]
fn changed_frame_remap_invalidates_absent_leaf_before_make() {
    for size in [4096, 0x20_0000] {
        let mut table = new_table();
        let address = VirtAddr::from(0x20_0000);
        table.map_page(address, 0x80_0000.into(), size, 1).unwrap();
        let _observation = LeafObservation::new(&table, address);

        assert_eq!(table.remap_page(address, 0xa0_0000.into(), 1), Ok(size));

        FLUSHED_LEAVES.with_borrow(|leaves| {
            assert!(
                leaves.len() >= 2 && leaves[0] == 0,
                "replacement became visible before invalidating the absent old leaf: {leaves:x?}"
            );
            assert_eq!(
                leaves.last().copied(),
                Some(0xa0_0001 | if size > 4096 { 2 } else { 0 }),
                "the final completion must observe the replacement descriptor"
            );
        });
        assert_eq!(table.query(address).unwrap().0, PhysAddr::from(0xa0_0000));
    }
}

#[test]
fn same_frame_remap_does_not_break_the_leaf() {
    let mut table = new_table();
    let address = VirtAddr::from(0x20_0000);
    table.map_page(address, 0x80_0000.into(), 4096, 1).unwrap();
    let _observation = LeafObservation::new(&table, address);
    assert_eq!(table.remap_page(address, 0x80_0000.into(), 0), Ok(4096));
    FLUSHED_LEAVES.with_borrow(|leaves| assert_eq!(leaves.as_slice(), &[0x80_0000]));
    // The retained descriptor is intentionally non-present; query translates
    // only present mappings, unlike the occupied-leaf ownership walker.
    assert_eq!(table.query(address), Err(PagingError::NotMapped));
}

#[test]
fn protect_completes_descriptor_publication_before_cow_sharing() {
    let mut table = new_table();
    let address = VirtAddr::from(0x20_0000);
    table.map_page(address, 0x80_0000.into(), 4096, 1).unwrap();
    let _observation = LeafObservation::new(&table, address);
    reset_events();
    assert_eq!(table.protect_page(address, 0), Ok(4096));
    FLUSHED_LEAVES.with_borrow(|leaves| assert_eq!(leaves.as_slice(), &[0x80_0000]));
    EVENTS.with_borrow(|events| {
        assert!(
            events.iter().any(|event| matches!(event, Event::Batch(1))),
            "permission downgrade lacks descriptor-publication completion: {events:?}"
        );
    });
}

#[test]
fn dense_unmap_walks_each_table_once_and_batches_retirement() {
    let mut table = new_table();
    let start = VirtAddr::from(0x20_0000);
    let pages = 1024;
    for index in 0..pages {
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

    table
        .unmap_owned(
            start..start + pages * 4096,
            Ok::<_, PagingError>,
            retire_leaf,
        )
        .unwrap();

    assert!(
        WALKS.get() <= 12,
        "walk restarted per page: {}",
        WALKS.get()
    );
    let retired = retired_addresses();
    assert_eq!(retired.len(), pages);
    assert_eq!(
        retired,
        (0..pages)
            .map(|index| start.as_usize() + index * 4096)
            .collect::<Vec<_>>()
    );
    EVENTS.with_borrow(|events| {
        let batches: Vec<_> = events
            .iter()
            .filter_map(|event| match event {
                Event::Batch(size) => Some(*size),
                _ => None,
            })
            .collect();
        assert!(
            batches.len() < pages / 16,
            "per-page completion: {batches:?}"
        );
        assert!(batches.iter().all(|size| *size > 0 && *size <= 64));
    });
    assert_retirement_order();
    assert!(root_entries(&table).iter().all(PageTableEntry::unused));
}

#[test]
fn sparse_unmap_skips_holes_and_retains_nonpresent_leaf_ownership() {
    let mut table = new_table();
    let start = VirtAddr::from(0x20_0000);
    let far = start + (1usize << 40);
    table.map_page(start, 0x80_0000.into(), 4096, 0).unwrap();
    table.map_page(far, 0x90_0000.into(), 4096, 1).unwrap();
    reset_events();
    table
        .unmap_owned(
            start..far + 4096,
            |leaf| {
                if leaf.vaddr == start {
                    assert_eq!(leaf.config, 0);
                    assert_eq!(leaf.paddr, PhysAddr::from(0x80_0000));
                }
                Ok::<_, PagingError>(leaf)
            },
            retire_leaf,
        )
        .unwrap();
    assert!(WALKS.get() <= 12, "sparse range traversed page by page");
    assert_eq!(retired_addresses(), vec![start.as_usize(), far.as_usize()]);
    assert_retirement_order();
}

#[test]
fn prepare_error_preserves_failed_leaf_and_retires_successful_prefix() {
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
    let result = table.unmap_owned(
        start..start + 3 * 4096,
        |leaf| {
            if leaf.vaddr == start + 4096 {
                Err(PagingError::NoMemory)
            } else {
                Ok(leaf)
            }
        },
        retire_leaf,
    );
    assert_eq!(result, Err(PagingError::NoMemory));
    assert_eq!(retired_addresses(), vec![start.as_usize()]);
    assert!(matches!(table.query(start), Err(PagingError::NotMapped)));
    assert_eq!(
        table.query(start + 4096).unwrap().0,
        PhysAddr::from(0x80_1000)
    );
    assert_eq!(
        table.query(start + 8192).unwrap().0,
        PhysAddr::from(0x80_2000)
    );
    assert_retirement_order();
}

#[test]
fn partial_huge_unmap_does_not_remove_neighbors() {
    let mut table = new_table();
    let start = VirtAddr::from(0x20_0000);
    table
        .map_page(start, 0x80_0000.into(), 0x20_0000, 1)
        .unwrap();
    reset_events();
    let result = table.unmap_owned(
        start + 4096..start + 8192,
        Ok::<_, PagingError>,
        retire_leaf,
    );
    assert!(matches!(result, Err(PagingError::InvalidRange { .. })));
    assert!(retired_addresses().is_empty());
    assert_eq!(table.query(start).unwrap().0, PhysAddr::from(0x80_0000));
    assert_eq!(
        table.query(start + 8192).unwrap().0,
        PhysAddr::from(0x80_2000)
    );
    table
        .unmap_owned(
            start..start + 0x20_0000,
            |leaf| {
                assert_eq!(leaf.size, 0x20_0000);
                Ok::<_, PagingError>(leaf)
            },
            retire_leaf,
        )
        .unwrap();
    assert_eq!(retired_addresses(), vec![start.as_usize()]);
    assert_retirement_order();
}

#[test]
fn empty_or_invalid_ranges_do_not_retire_mappings() {
    let mut table = new_table();
    let start = VirtAddr::from(0x20_0000);
    table.map_page(start, 0x80_0000.into(), 4096, 1).unwrap();
    reset_events();
    table
        .unmap_owned(start..start, Ok::<_, PagingError>, retire_leaf)
        .unwrap();
    assert!(
        table
            .unmap_owned(start + 4096..start, Ok::<_, PagingError>, retire_leaf)
            .is_err()
    );
    assert!(
        table
            .unmap_owned(start + 1..start + 4096, Ok::<_, PagingError>, retire_leaf)
            .is_err()
    );
    assert!(retired_addresses().is_empty());
    EVENTS.with_borrow(|events| assert!(events.is_empty()));
    assert!(table.query(start).is_ok());
}

#[test]
fn deferred_leaf_flush_still_invalidates_retired_tables() {
    let mut table = new_table();
    table
        .map_page(0x20_0000.into(), 0x80_0000.into(), 4096, 1)
        .unwrap();
    reset_events();
    table
        .unmap_with_config(&UnmapConfig {
            start_vaddr: 0x20_0000.into(),
            size: 4096,
            flush: false,
        })
        .unwrap();
    assert_retirement_order();
    EVENTS.with_borrow(|events| {
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event, Event::Free))
                .count(),
            3
        )
    });
}

fn assert_conflict_preserves_mapping(page_size: usize) {
    let mut table = new_table();
    let vaddr = VirtAddr::from(0x20_0000);
    let existing_paddr = PhysAddr::from(0x40_0000);
    let replacement = PhysAddr::from(0x60_0000);
    table.map_page(vaddr, existing_paddr, page_size, 1).unwrap();
    reset_events();

    let result = table.map_page(vaddr, replacement, page_size, 0);
    assert_eq!(table.query(vaddr).unwrap(), (existing_paddr, 1, page_size));
    EVENTS.with_borrow(|events| {
        assert!(
            events.is_empty(),
            "conflict must not mutate or retire the occupied mapping: {events:?}"
        );
    });
    assert_eq!(
        result,
        Err(PagingError::MappingConflict {
            vaddr,
            existing_paddr,
        }),
    );
}

#[test]
fn owned_unmap_retains_preallocated_shared_root_directories() {
    for scoped in [false, true] {
        let mut table = new_table();
        let start = VirtAddr::from(0x20_0000);
        table.preallocate_shared_root_entries(start, 4096).unwrap();
        let root_entry = root_entries(&table)[0];
        table.map_page(start, 0x80_0000.into(), 4096, 1).unwrap();
        if scoped {
            let mut session = table.unmap_session(retire_leaf);
            session
                .unmap(start..start + 4096, Ok::<_, PagingError>)
                .unwrap();
            session.finish();
        } else {
            table
                .unmap_owned(start..start + 4096, Ok::<_, PagingError>, retire_leaf)
                .unwrap();
        }
        let retained = root_entries(&table)[0];
        assert!(
            !retained.unused(),
            "borrowed process roots still need this directory"
        );
        assert_eq!(retained.paddr(true), root_entry.paddr(true));
        table.map_page(start, 0x90_0000.into(), 4096, 1).unwrap();
        assert_eq!(root_entries(&table)[0].paddr(true), root_entry.paddr(true));
        assert_eq!(table.query(start).unwrap().0, PhysAddr::from(0x90_0000));
    }
}
