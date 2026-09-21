use crate::os::memory::PAGE_SIZE;

const INITIAL_READAHEAD_PAGES: usize = 4;
const MAX_READAHEAD_PAGES: usize = 32;
const MMAP_LOTS_OF_MISSES: u16 = 100;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct ReadAheadPlan {
    pub(super) window_pages: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct MmapReadAroundPlan {
    pub(super) start_page: u32,
    pub(super) window_pages: usize,
}

pub(super) struct ReadAheadState {
    next_offset: u64,
    window_pages: usize,
    mmap_misses: u16,
    initialized: bool,
}

impl ReadAheadState {
    pub(super) const fn new() -> Self {
        Self {
            next_offset: 0,
            window_pages: INITIAL_READAHEAD_PAGES,
            mmap_misses: 0,
            initialized: false,
        }
    }

    pub(super) fn plan(&mut self, offset: u64, end: u64) -> ReadAheadPlan {
        let sequential = if self.initialized {
            offset == self.next_offset
        } else {
            offset == 0
        };
        let first_page = offset / PAGE_SIZE as u64;
        let end_page = end.div_ceil(PAGE_SIZE as u64);
        let requested_pages =
            usize::try_from(end_page.saturating_sub(first_page)).unwrap_or(usize::MAX);
        let window_pages = if sequential {
            self.window_pages
                .max(requested_pages)
                .min(MAX_READAHEAD_PAGES)
        } else {
            requested_pages.min(MAX_READAHEAD_PAGES)
        };

        self.next_offset = end;
        self.initialized = true;
        self.window_pages = if sequential {
            self.window_pages.saturating_mul(2).min(MAX_READAHEAD_PAGES)
        } else {
            INITIAL_READAHEAD_PAGES
        };
        ReadAheadPlan { window_pages }
    }

    /// Plans Linux-style mmap read-around for one cache miss.
    ///
    /// The default backing-device window is 128 KiB (32 base pages), centered
    /// on the fault where possible. Repeated cache misses without intervening
    /// hits eventually disable speculative pages for a random-access mapping.
    pub(super) fn plan_mmap_fault(
        &mut self,
        fault_page: u32,
        file_pages: u64,
    ) -> MmapReadAroundPlan {
        self.mmap_misses = self.mmap_misses.saturating_add(1);
        if self.mmap_misses > MMAP_LOTS_OF_MISSES {
            return MmapReadAroundPlan {
                start_page: fault_page,
                window_pages: 1,
            };
        }

        let start_page = fault_page.saturating_sub((MAX_READAHEAD_PAGES / 2) as u32);
        let available = file_pages.saturating_sub(u64::from(start_page));
        let window_pages = usize::try_from(available)
            .unwrap_or(MAX_READAHEAD_PAGES)
            .clamp(1, MAX_READAHEAD_PAGES);
        MmapReadAroundPlan {
            start_page,
            window_pages,
        }
    }

    pub(super) fn record_mmap_hit(&mut self) {
        self.mmap_misses = self.mmap_misses.saturating_sub(1);
    }
}

#[cfg(test)]
mod tests {
    use super::{
        INITIAL_READAHEAD_PAGES, MAX_READAHEAD_PAGES, MMAP_LOTS_OF_MISSES, ReadAheadState,
    };
    use crate::os::memory::PAGE_SIZE;

    #[test]
    fn sequential_reads_grow_a_bounded_window_and_random_reads_reset_it() {
        let mut state = ReadAheadState::new();

        assert_eq!(
            state.plan(0, PAGE_SIZE as u64).window_pages,
            INITIAL_READAHEAD_PAGES
        );
        assert_eq!(
            state
                .plan(PAGE_SIZE as u64, (PAGE_SIZE * 2) as u64)
                .window_pages,
            INITIAL_READAHEAD_PAGES * 2
        );
        assert_eq!(
            state
                .plan(17 * PAGE_SIZE as u64, 18 * PAGE_SIZE as u64)
                .window_pages,
            1
        );
        assert_eq!(
            state
                .plan(18 * PAGE_SIZE as u64, 19 * PAGE_SIZE as u64)
                .window_pages,
            INITIAL_READAHEAD_PAGES
        );

        for page in 19..64 {
            let _ = state.plan(page * PAGE_SIZE as u64, (page + 1) * PAGE_SIZE as u64);
        }
        assert_eq!(
            state
                .plan(64 * PAGE_SIZE as u64, 65 * PAGE_SIZE as u64)
                .window_pages,
            MAX_READAHEAD_PAGES
        );
    }

    #[test]
    fn mmap_faults_read_around_until_repeated_misses_disable_speculation() {
        let mut state = ReadAheadState::new();
        let plan = state.plan_mmap_fault(40, 64);
        assert_eq!(plan.start_page, 24);
        assert_eq!(plan.window_pages, MAX_READAHEAD_PAGES);

        for _ in 1..MMAP_LOTS_OF_MISSES {
            let _ = state.plan_mmap_fault(40, 64);
        }
        let random = state.plan_mmap_fault(40, 64);
        assert_eq!(random.start_page, 40);
        assert_eq!(random.window_pages, 1);

        state.record_mmap_hit();
        state.record_mmap_hit();
        assert_eq!(
            state.plan_mmap_fault(40, 64).window_pages,
            MAX_READAHEAD_PAGES
        );
    }
}
