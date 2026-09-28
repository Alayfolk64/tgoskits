use ax_memory_addr::{VirtAddr, VirtAddrRange};
use ax_memory_set::{MappingBackend, MappingError, MemoryArea, MemorySet};

#[test]
fn first_fit_respects_hint_alignment_and_the_upper_bound_before_any_mapping() {
    let mut set = MemorySet::new();
    let mut table = RecordingTable::default();
    map(&mut set, &mut table, 0x8000, 0x1000);
    assert_eq!(search(&set, 0x1801, 0x4000, 0x1000, 0x1000), Some(0x2000));
    assert_eq!(search(&set, 0x2801, 0x3000, 0x1000, 0x1000), None);
    assert_eq!(search(&set, 0x8000, 0x3000, 0x1000, 0x1000), None);
    assert_eq!(search(&set, 0x2000, 0x3000, 0x1000, 0x1000), Some(0x2000));
    assert_eq!(
        search(&set, usize::MAX - 1, usize::MAX, 1, 1),
        Some(usize::MAX - 1)
    );
    assert_eq!(search(&set, usize::MAX - 1, usize::MAX, 4, 4), None);
}

#[test]
fn successful_mutations_preserve_first_fit_and_coalesce_vacated_ranges() {
    let mut set = MemorySet::new();
    let mut table = RecordingTable::default();
    map(&mut set, &mut table, 0, 0x4000);
    map(&mut set, &mut table, 0x6000, 0x2000);
    set.unmap(0x1000.into(), 0x1000, &mut (), &mut table)
        .unwrap();
    assert_eq!(search(&set, 0, 0x10000, 0x1000, 0x1000), Some(0x1000));
    set.unmap_metadata(0x2000.into(), 0x2000).unwrap();
    assert_eq!(search(&set, 0, 0x10000, 0x4000, 0x1000), Some(0x1000));
    set.extend_area(0.into(), 0x1000, &mut (), &mut table)
        .unwrap();
    assert_eq!(search(&set, 0, 0x10000, 0x4000, 0x1000), Some(0x2000));
    set.map(area(0x1000, 0x6000), &mut (), &mut table, true)
        .unwrap();
    assert_eq!(search(&set, 0, 0x10000, 0x1000, 0x1000), Some(0x8000));
    set.clear(&mut (), &mut table).unwrap();
    assert_eq!(search(&set, 0, 0x10000, 0x10000, 0x1000), Some(0));
}

#[test]
fn protection_and_metadata_replacement_do_not_create_false_holes() {
    let mut set = MemorySet::new();
    let mut table = RecordingTable::default();
    map(&mut set, &mut table, 0, 0x8000);
    set.protect(0x1000.into(), 0x2000, |_| Some(2), &mut (), &mut table)
        .unwrap();
    set.replace_area_metadata(area(0x4000, 0x1000)).unwrap();
    assert_eq!(search(&set, 0, 0x10000, 0x1000, 0x1000), Some(0x8000));
    set.unmap_metadata(0x2000.into(), 0x3000).unwrap();
    assert_eq!(search(&set, 0, 0x10000, 0x3000, 0x1000), Some(0x2000));
}

#[test]
fn failed_map_extension_and_replacement_publish_only_completed_coverage() {
    let mut set = MemorySet::new();
    let mut table = RecordingTable::default();
    map(&mut set, &mut table, 0, 0x2000);
    table.fail_map = true;
    assert_eq!(
        set.map(area(0x4000, 0x1000), &mut (), &mut table, false),
        Err(MappingError::BadState)
    );
    assert_eq!(
        set.extend_area(0.into(), 0x1000, &mut (), &mut table),
        Err(MappingError::BadState)
    );
    assert_eq!(search(&set, 0, 0x10000, 0x1000, 0x1000), Some(0x2000));
    assert_eq!(
        set.map(area(0x1000, 0x2000), &mut (), &mut table, true),
        Err(MappingError::NeedsRepair)
    );
    assert_eq!(search(&set, 0, 0x10000, 0x1000, 0x1000), Some(0x2000));
    table.fail_map = false;
    set.map(area(0x1000, 0x3000), &mut (), &mut table, true)
        .unwrap();
    assert_eq!(search(&set, 0, 0x10000, 0x1000, 0x1000), Some(0x4000));
}

#[test]
fn failed_unmap_keeps_all_area_owners_and_gap_coverage() {
    let mut set = MemorySet::new();
    let mut table = RecordingTable::default();
    map(&mut set, &mut table, 0, 0x2000);
    map(&mut set, &mut table, 0x3000, 0x1000);
    table.fail_unmap_at = Some(0x1000);
    assert_eq!(
        set.unmap(0x1000.into(), 0x3000, &mut (), &mut table),
        Err(MappingError::BadState)
    );
    assert_eq!(table.unmapped, [(0x1000, 0x1000)]);
    assert_eq!(search(&set, 0, 0x10000, 0x1000, 0x1000), Some(0x2000));
    assert_eq!(set.len(), 2);
    assert_eq!(
        set.find(0x1000.into()).unwrap().end(),
        VirtAddr::from(0x2000)
    );
}

#[test]
fn failed_split_unmap_preserves_the_original_area_coverage() {
    let mut set = MemorySet::new();
    let mut table = RecordingTable::default();
    map(&mut set, &mut table, 0, 0x4000);
    table.fail_unmap_at = Some(0x1000);
    assert_eq!(
        set.unmap(0x1000.into(), 0x1000, &mut (), &mut table),
        Err(MappingError::BadState)
    );
    // Failed publication retains every backend owner until the caller's
    // mutation context confirms that stale translations can be retired.
    assert_eq!(set.find(0.into()).unwrap().end(), VirtAddr::from(0x4000));
    assert_eq!(search(&set, 0, 0x10000, 0x1000, 0x1000), Some(0x4000));
}

#[test]
fn failed_clear_keeps_the_authoritative_map_covered() {
    let mut set = MemorySet::new();
    let mut table = RecordingTable::default();
    map(&mut set, &mut table, 0, 0x1000);
    map(&mut set, &mut table, 0x1000, 0x1000);
    table.fail_unmap_at = Some(0x1000);
    assert_eq!(
        set.clear(&mut (), &mut table),
        Err(MappingError::NeedsRepair)
    );
    assert_eq!(set.len(), 2);
    assert_eq!(search(&set, 0, 0x10000, 0x1000, 0x1000), Some(0x2000));
    table.fail_unmap_at = None;
    set.clear(&mut (), &mut table).unwrap();
    assert_eq!(search(&set, 0, 0x10000, 0x1000, 0x1000), Some(0));
}

#[test]
fn mixed_real_mutations_match_an_independent_byte_coverage_oracle() {
    let mut set = MemorySet::new();
    let mut table = RecordingTable::default();
    let mut occupied = [false; 64];
    for step in 0..160 {
        let start = step * 17 % 56;
        let size = step * 7 % 8 + 1;
        if step % 3 == 0 {
            set.map(area(start, size), &mut (), &mut table, true)
                .unwrap();
            occupied[start..start + size].fill(true);
        } else if step % 3 == 1 {
            set.unmap(start.into(), size, &mut (), &mut table).unwrap();
            occupied[start..start + size].fill(false);
        } else {
            set.unmap_metadata(start.into(), size).unwrap();
            occupied[start..start + size].fill(false);
        }
        for lower in 0usize..64 {
            for length in [1, 2, 4, 8] {
                let expected = (lower..64).find(|&candidate| {
                    candidate.is_multiple_of(length)
                        && candidate + length <= 64
                        && occupied[candidate..candidate + length]
                            .iter()
                            .all(|byte| !byte)
                });
                assert_eq!(
                    search(&set, lower, 64, length, length),
                    expected,
                    "step={step}, lower={lower}, length={length}"
                );
            }
        }
    }
}

fn search(
    set: &MemorySet<RecordingBackend>,
    hint: usize,
    upper: usize,
    size: usize,
    align: usize,
) -> Option<usize> {
    set.find_free_area(
        hint.into(),
        size,
        VirtAddrRange::new(0.into(), upper.into()),
        align,
    )
    .map(usize::from)
}

fn map(
    set: &mut MemorySet<RecordingBackend>,
    table: &mut RecordingTable,
    start: usize,
    size: usize,
) {
    set.map(area(start, size), &mut (), table, false).unwrap();
}

fn area(start: usize, size: usize) -> MemoryArea<RecordingBackend> {
    MemoryArea::new(start.into(), size, 1, RecordingBackend)
}

#[derive(Default)]
struct RecordingTable {
    fail_map: bool,
    fail_unmap_at: Option<usize>,
    unmapped: Vec<(usize, usize)>,
}

#[derive(Clone)]
struct RecordingBackend;

impl MappingBackend for RecordingBackend {
    type Addr = VirtAddr;
    type Flags = u8;
    type MutationContext = ();
    type PageTable = RecordingTable;

    fn map(&self, _: VirtAddr, _: usize, _: u8, _: &mut (), table: &mut RecordingTable) -> bool {
        !table.fail_map
    }

    fn unmap(&self, start: VirtAddr, size: usize, _: &mut (), table: &mut RecordingTable) -> bool {
        table.unmapped.push((start.into(), size));
        table.fail_unmap_at != Some(start.into())
    }

    fn protect(&self, _: VirtAddr, _: usize, _: u8, _: &mut (), _: &mut RecordingTable) -> bool {
        true
    }

    fn split(&mut self, _: usize) -> Option<Self> {
        Some(Self)
    }
}
