use std::{cell::Cell, cmp::Ordering};

use ax_memory_addr::AddrRange;
use ax_memory_set::{MappingBackend, MemoryArea, MemorySet};

#[test]
fn unmap_only_searches_near_the_removed_range() {
    let mut set = populated_set();
    let mut unmapped = Vec::new();
    COMPARISONS.set(0);
    set.unmap(8192.into(), 2, &mut (), &mut unmapped).unwrap();
    assert_bounded_search();
    assert_eq!(unmapped, [(8192, 2)]);
    assert_eq!(set.len(), 4095);
    assert!(set.find(8188.into()).is_some());
    assert!(set.find(8192.into()).is_none());
    assert!(set.find(8196.into()).is_some());
}

#[test]
fn unmap_metadata_only_searches_near_the_removed_range() {
    let mut set = populated_set();
    COMPARISONS.set(0);
    set.unmap_metadata(8192.into(), 2).unwrap();
    assert_bounded_search();
    assert_eq!(set.len(), 4095);
}

#[test]
fn overlapping_query_is_bounded_and_includes_the_left_predecessor() {
    let set = populated_set();
    COMPARISONS.set(0);
    let starts: Vec<usize> = set
        .iter_overlapping(AddrRange::new(8193.into(), 8197.into()))
        .map(|area| area.start().0)
        .collect();
    assert_bounded_search();
    assert_eq!(starts, [8192, 8196]);
    for (start, end) in [(8193, 8193), (8194, 8196), (16384, 16388)] {
        assert_eq!(
            set.iter_overlapping(AddrRange::new(start.into(), end.into()))
                .count(),
            0
        );
    }
}

#[test]
fn first_fit_skips_thousands_of_too_small_holes() {
    let set = populated_set();
    COMPARISONS.set(0);
    let result = set.find_free_area(0.into(), 3, AddrRange::new(0.into(), 17000.into()), 1);
    assert_eq!(result.unwrap().0, 16382);
    assert_bounded_search();
}

fn assert_bounded_search() {
    let comparisons = COMPARISONS.get();
    assert!(
        comparisons < 200,
        "a local operation must not scan 4096 unrelated areas: {comparisons} comparisons"
    );
}

fn populated_set() -> MemorySet<RecordingBackend> {
    let mut set = MemorySet::new();
    for index in 0..4096 {
        set.map(
            MemoryArea::new((index * 4).into(), 2, (), RecordingBackend),
            &mut (),
            &mut Vec::new(),
            false,
        )
        .unwrap();
    }
    set
}

thread_local! {
    static COMPARISONS: Cell<usize> = const { Cell::new(0) };
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct CountedAddr(usize);

impl Ord for CountedAddr {
    fn cmp(&self, other: &Self) -> Ordering {
        COMPARISONS.set(COMPARISONS.get() + 1);
        self.0.cmp(&other.0)
    }
}

impl PartialOrd for CountedAddr {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl From<usize> for CountedAddr {
    fn from(value: usize) -> Self {
        Self(value)
    }
}

impl From<CountedAddr> for usize {
    fn from(value: CountedAddr) -> Self {
        value.0
    }
}

#[derive(Clone)]
struct RecordingBackend;

impl MappingBackend for RecordingBackend {
    type Addr = CountedAddr;
    type Flags = ();
    type MutationContext = ();
    type PageTable = Vec<(usize, usize)>;

    fn map(&self, _: CountedAddr, _: usize, _: (), _: &mut (), _: &mut Self::PageTable) -> bool {
        true
    }

    fn unmap(&self, start: CountedAddr, size: usize, _: &mut (), pt: &mut Self::PageTable) -> bool {
        pt.push((start.0, size));
        true
    }

    fn protect(
        &self,
        _: CountedAddr,
        _: usize,
        _: (),
        _: &mut (),
        _: &mut Self::PageTable,
    ) -> bool {
        true
    }

    fn split(&mut self, _: usize) -> Option<Self> {
        Some(Self)
    }
}
