//! Reconcile only the authoritative area coverage touched by partial unmaps.

use alloc::collections::BTreeMap;

use ax_memory_addr::AddrRange;

use crate::{MappingBackend, MemoryArea, gaps::GapIndex};

pub(super) struct GapMutation<'a, B: MappingBackend> {
    pub(super) areas: &'a mut BTreeMap<B::Addr, MemoryArea<B>>,
    gaps: &'a mut GapIndex,
    affected: AddrRange<B::Addr>,
}

impl<'a, B: MappingBackend> GapMutation<'a, B> {
    pub(super) fn new(
        areas: &'a mut BTreeMap<B::Addr, MemoryArea<B>>,
        gaps: &'a mut GapIndex,
        mut affected: AddrRange<B::Addr>,
    ) -> Self {
        // A failing backend shrink can leave a whole removed area absent, or a
        // split left part shortened before the error. Include those old bounds,
        // not just the requested hole, when deriving the post-operation gaps.
        if let Some((_, first)) = areas.range(..=affected.start).next_back()
            && first.end() > affected.start
        {
            affected.start = first.start();
        }
        if let Some((_, last)) = areas.range(..affected.end).next_back() {
            affected.end = affected.end.max(last.end());
        }
        Self {
            areas,
            gaps,
            affected,
        }
    }
}

impl<B: MappingBackend> Drop for GapMutation<'_, B> {
    fn drop(&mut self) {
        // This guard also covers early Err returns. MemoryArea remains the
        // source of truth; never publish a free range over a surviving area.
        self.gaps
            .release(self.affected.start.into()..self.affected.end.into());
        for area in self
            .areas
            .range(self.affected.start..self.affected.end)
            .map(|(_, area)| area)
        {
            self.gaps.occupy(area.start().into()..area.end().into());
        }
    }
}
