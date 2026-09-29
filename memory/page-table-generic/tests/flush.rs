//! TLB flush behavior for range operations.

#![cfg(not(target_os = "none"))]

use std::sync::atomic::{AtomicUsize, Ordering};

use page_table_generic::*;

mod mocks;

use mocks::{Fram4k, MappingFlags, PteImpl};

static FULL_FLUSHES: AtomicUsize = AtomicUsize::new(0);
static ADDRESS_FLUSHES: AtomicUsize = AtomicUsize::new(0);

#[derive(Clone, Copy)]
struct CountingMeta;

impl TableMeta for CountingMeta {
    type P = PteImpl;

    const PAGE_SIZE: usize = 0x1000;
    const LEVEL_BITS: &[usize] = &[9, 9, 9, 9];
    const MAX_BLOCK_LEVEL: usize = 3;

    fn flush(vaddr: Option<VirtAddr>) {
        if vaddr.is_some() {
            ADDRESS_FLUSHES.fetch_add(1, Ordering::Relaxed);
        } else {
            FULL_FLUSHES.fetch_add(1, Ordering::Relaxed);
        }
    }
}

#[test]
fn map_region_batches_tlb_flushes() {
    FULL_FLUSHES.store(0, Ordering::Relaxed);
    ADDRESS_FLUSHES.store(0, Ordering::Relaxed);

    let mut page_table = PageTable::<CountingMeta, Fram4k>::new(Fram4k).unwrap();
    page_table
        .map_region(
            VirtAddr::from_usize(0x20_0000),
            |vaddr| PhysAddr::from_usize(vaddr.as_usize() + 0x20_0000),
            2 * CountingMeta::PAGE_SIZE,
            (MappingFlags::READ | MappingFlags::WRITE).into(),
        )
        .unwrap();

    assert_eq!(ADDRESS_FLUSHES.load(Ordering::Relaxed), 2);
    assert_eq!(FULL_FLUSHES.load(Ordering::Relaxed), 0);

    FULL_FLUSHES.store(0, Ordering::Relaxed);
    ADDRESS_FLUSHES.store(0, Ordering::Relaxed);

    let mut page_table = PageTable::<CountingMeta, Fram4k>::new(Fram4k).unwrap();
    page_table
        .map_region(
            VirtAddr::from_usize(0x40_0000),
            |vaddr| PhysAddr::from_usize(vaddr.as_usize() + 0x20_0000),
            128 * CountingMeta::PAGE_SIZE,
            (MappingFlags::READ | MappingFlags::WRITE).into(),
        )
        .unwrap();

    assert_eq!(ADDRESS_FLUSHES.load(Ordering::Relaxed), 0);
    assert_eq!(FULL_FLUSHES.load(Ordering::Relaxed), 1);
}

static LEAF_COMPLETIONS: AtomicUsize = AtomicUsize::new(0);
static TABLE_COMPLETIONS: AtomicUsize = AtomicUsize::new(0);
static BREAK_BEFORE_MAKE_COMPLETIONS: AtomicUsize = AtomicUsize::new(0);
static REPLACEMENT_COMPLETIONS: AtomicUsize = AtomicUsize::new(0);

#[derive(Clone, Copy)]
struct RoutedMeta;

impl TableMeta for RoutedMeta {
    type P = PteImpl;

    const PAGE_SIZE: usize = 0x1000;
    const LEVEL_BITS: &[usize] = &[9, 9, 9, 9];
    const MAX_BLOCK_LEVEL: usize = 3;

    fn flush(_vaddr: Option<VirtAddr>) {}

    fn flush_batch(_vaddrs: &[VirtAddr]) {
        TABLE_COMPLETIONS.fetch_add(1, Ordering::Relaxed);
    }

    fn flush_leaf_batch(vaddrs: &[VirtAddr]) {
        LEAF_COMPLETIONS.fetch_add(vaddrs.len(), Ordering::Relaxed);
    }

    fn flush_before_make(_vaddr: VirtAddr, _page_size: usize) {
        BREAK_BEFORE_MAKE_COMPLETIONS.fetch_add(1, Ordering::Relaxed);
    }

    fn complete_replaced_leaf(_vaddr: VirtAddr) {
        REPLACEMENT_COMPLETIONS.fetch_add(1, Ordering::Relaxed);
    }
}

#[test]
fn leaf_updates_and_table_detach_use_their_required_completion() {
    let mut page_table = PageTable::<RoutedMeta, Fram4k>::new(Fram4k).unwrap();
    let first = VirtAddr::from_usize(0x20_0000);
    let second = first + RoutedMeta::PAGE_SIZE;
    let original = PhysAddr::from_usize(0x40_0000);
    let replacement = PhysAddr::from_usize(0x50_0000);
    let read = MappingFlags::READ.into();

    page_table
        .map_page(first, original, RoutedMeta::PAGE_SIZE, read)
        .unwrap();
    page_table
        .map_page(
            second,
            original + RoutedMeta::PAGE_SIZE,
            RoutedMeta::PAGE_SIZE,
            read,
        )
        .unwrap();
    LEAF_COMPLETIONS.store(0, Ordering::Relaxed);
    TABLE_COMPLETIONS.store(0, Ordering::Relaxed);
    BREAK_BEFORE_MAKE_COMPLETIONS.store(0, Ordering::Relaxed);
    REPLACEMENT_COMPLETIONS.store(0, Ordering::Relaxed);

    page_table.protect_page(first, read).unwrap();
    page_table.remap_page(first, original, read).unwrap();
    assert_eq!(LEAF_COMPLETIONS.load(Ordering::Relaxed), 2);
    assert_eq!(TABLE_COMPLETIONS.load(Ordering::Relaxed), 0);

    page_table.remap_page(first, replacement, read).unwrap();
    assert_eq!(LEAF_COMPLETIONS.load(Ordering::Relaxed), 2);
    assert_eq!(BREAK_BEFORE_MAKE_COMPLETIONS.load(Ordering::Relaxed), 1);
    assert_eq!(REPLACEMENT_COMPLETIONS.load(Ordering::Relaxed), 1);

    page_table.unmap_page(first).unwrap();
    assert_eq!(LEAF_COMPLETIONS.load(Ordering::Relaxed), 3);
    assert_eq!(TABLE_COMPLETIONS.load(Ordering::Relaxed), 0);

    page_table.unmap_page(second).unwrap();
    assert_eq!(TABLE_COMPLETIONS.load(Ordering::Relaxed), 1);
}
