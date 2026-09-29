//! Non-fixed mapping placement: try the hint before restarting at the base.

use ax_memory_addr::VirtAddr;

use crate::{StarryError, StarryResult};

pub(super) fn find_mapping_start(
    hint: VirtAddr,
    base: VirtAddr,
    mut search: impl FnMut(VirtAddr) -> Option<VirtAddr>,
) -> StarryResult<VirtAddr> {
    search(hint)
        .or_else(|| search(base))
        .ok_or(StarryError::NoMemory)
}

#[cfg(test)]
mod tests {
    use alloc::vec::Vec;

    use ax_memory_addr::VirtAddrRange;
    use ax_memory_set::{MappingBackend, MemoryArea, MemorySet};

    use super::*;

    #[cfg_attr(axtest, axtest::axtest)]
    #[cfg_attr(not(axtest), test)]
    fn successful_hint_does_not_restart_the_area_search() {
        let mappings = occupied_middle();
        let limit = VirtAddrRange::new(0x1000.into(), 0x5000.into());
        let mut searches = Vec::new();
        let result = find_mapping_start(0x4000.into(), limit.start, |hint| {
            searches.push(hint);
            mappings.find_free_area(hint, 0x1000, limit, 0x1000)
        });
        assert_eq!(result.unwrap(), VirtAddr::from(0x4000));
        assert_eq!(searches, [VirtAddr::from(0x4000)]);
    }

    #[cfg_attr(axtest, axtest::axtest)]
    #[cfg_attr(not(axtest), test)]
    fn unavailable_hint_restarts_at_the_base() {
        let mappings = occupied_middle();
        let limit = VirtAddrRange::new(0x1000.into(), 0x5000.into());
        let mut searches = Vec::new();
        let result = find_mapping_start(limit.end, limit.start, |hint| {
            searches.push(hint);
            mappings.find_free_area(hint, 0x1000, limit, 0x1000)
        });
        assert_eq!(result.unwrap(), limit.start);
        assert_eq!(searches, [limit.end, limit.start]);
    }

    #[cfg_attr(axtest, axtest::axtest)]
    #[cfg_attr(not(axtest), test)]
    fn exhausted_address_range_preserves_no_memory() {
        let mappings = occupied_middle();
        let limit = VirtAddrRange::new(0x2000.into(), 0x4000.into());
        let result = find_mapping_start(limit.end, limit.start, |hint| {
            mappings.find_free_area(hint, 0x1000, limit, 0x1000)
        });
        assert!(matches!(result, Err(StarryError::NoMemory)));
    }

    fn occupied_middle() -> MemorySet<MetadataBackend> {
        let mut mappings = MemorySet::new();
        mappings
            .map(
                MemoryArea::new(0x2000.into(), 0x2000, (), MetadataBackend),
                &mut (),
                &mut (),
                false,
            )
            .unwrap();
        mappings
    }

    #[derive(Clone)]
    struct MetadataBackend;

    impl MappingBackend for MetadataBackend {
        type Addr = VirtAddr;
        type Flags = ();
        type MutationContext = ();
        type PageTable = ();

        fn map(&self, _: VirtAddr, _: usize, _: (), _: &mut (), _: &mut ()) -> bool {
            true
        }

        fn unmap(&self, _: VirtAddr, _: usize, _: &mut (), _: &mut ()) -> bool {
            true
        }

        fn protect(&self, _: VirtAddr, _: usize, _: (), _: &mut (), _: &mut ()) -> bool {
            true
        }

        fn split(&mut self, _: usize) -> Option<Self> {
            Some(Self)
        }
    }
}
