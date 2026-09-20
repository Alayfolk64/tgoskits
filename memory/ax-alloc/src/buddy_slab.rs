//! Memory allocator implementation backed by `buddy-slab-allocator`.

use core::{
    alloc::{GlobalAlloc, Layout},
    ptr::NonNull,
    slice,
    sync::atomic::{AtomicIsize, Ordering},
};

use ax_sync::SpinLock;
use buddy_slab_allocator::{
    GlobalAllocator as InnerAllocator, RemoteFreeHint, SizeClass, SlabAllocResult, SlabAllocator,
    SlabDeallocResult, SlabPoolTrait, SlabTrait, interface::BuddySlabIf,
};
use strum::VariantArray;

use super::{AllocResult, AllocatorOps, UsageKind, Usages};

/// The global allocator instance for buddy-slab mode.
#[cfg_attr(
    all(any(target_os = "none", feature = "global-allocator"), not(test)),
    global_allocator
)]
static GLOBAL_ALLOCATOR: GlobalAllocator = GlobalAllocator::new();

/// The default byte allocator for buddy-slab mode.
pub type DefaultByteAllocator = buddy_slab_allocator::SlabAllocator<PAGE_SIZE>;

const PAGE_SIZE: usize = 0x1000;
const PERCPU_PAGE_CACHE_CAPACITY: usize = 64;
const PERCPU_PAGE_REFILL_BATCH: usize = 32;
const PERCPU_PAGE_DRAIN_BATCH: usize = 32;

fn retry_after_cached_order0_drain<T>(
    mut attempt: impl FnMut() -> AllocResult<T>,
    drain: impl FnOnce() -> usize,
) -> AllocResult<T> {
    match attempt() {
        Ok(value) => return Ok(value),
        Err(crate::AllocError::NoMemory) => {}
        Err(error) => return Err(error),
    }
    if drain() == 0 {
        return Err(crate::AllocError::NoMemory);
    }
    attempt()
}

#[ax_percpu::def_percpu]
static PERCPU_SLAB: PercpuSlab<PAGE_SIZE> = PercpuSlab::new_uninit();

static SLAB_POOL: SlabPool = SlabPool;

struct PercpuSlab<const PAGE_SIZE: usize = 0x1000> {
    cpu_id: Option<u16>,
    inner: SpinLock<SlabAllocator<PAGE_SIZE>>,
    remote_hint: RemoteFreeHint,
    page_cache: SpinLock<PercpuPageCache>,
    usages: PercpuUsages,
}

struct PercpuPageCache {
    pages: [usize; PERCPU_PAGE_CACHE_CAPACITY],
    len: usize,
}

impl PercpuPageCache {
    const fn new() -> Self {
        Self {
            pages: [0; PERCPU_PAGE_CACHE_CAPACITY],
            len: 0,
        }
    }

    fn pop(&mut self) -> Option<usize> {
        self.len.checked_sub(1).map(|index| {
            self.len = index;
            self.pages[index]
        })
    }

    fn push(&mut self, page: usize) {
        assert!(self.len < self.pages.len(), "per-CPU page cache overflow");
        self.pages[self.len] = page;
        self.len += 1;
    }

    fn drain_into(&mut self, output: &mut [usize]) -> usize {
        let count = output.len().min(self.len);
        for slot in &mut output[..count] {
            *slot = self.pop().expect("drain count is bounded by cache length");
        }
        count
    }
}

struct PercpuUsages([AtomicIsize; UsageKind::VARIANTS.len()]);

impl PercpuUsages {
    const fn new() -> Self {
        Self([const { AtomicIsize::new(0) }; UsageKind::VARIANTS.len()])
    }

    fn reset(&mut self) {
        self.0 = [const { AtomicIsize::new(0) }; UsageKind::VARIANTS.len()];
    }

    fn alloc(&self, kind: UsageKind, size: usize) {
        // Rust Layout guarantees that a successful allocation fits isize.
        self.0[kind as usize].fetch_add(size as isize, Ordering::Relaxed);
    }

    fn dealloc(&self, kind: UsageKind, size: usize) {
        // A task may free an allocation created on another CPU. Signed local
        // deltas permit that ownership transfer; the aggregate remains the
        // exact process-wide usage outside a concurrent snapshot window.
        self.0[kind as usize].fetch_sub(size as isize, Ordering::Relaxed);
    }
}

impl<const PAGE_SIZE: usize> PercpuSlab<PAGE_SIZE> {
    const fn new_uninit() -> Self {
        Self {
            cpu_id: None,
            inner: SpinLock::new(SlabAllocator::new()),
            remote_hint: RemoteFreeHint::new(),
            page_cache: SpinLock::new(PercpuPageCache::new()),
            usages: PercpuUsages::new(),
        }
    }

    fn init_during_cpu_bringup(&mut self, cpu_id: usize) {
        let cpu_id = u16::try_from(cpu_id).expect("CPU id exceeds per-CPU slab range");
        assert!(
            self.cpu_id.is_none(),
            "per-CPU slab is already initialized on this CPU",
        );
        self.cpu_id = Some(cpu_id);
        *self.inner.get_mut() = SlabAllocator::new();
        self.remote_hint.clear();
        *self.page_cache.get_mut() = PercpuPageCache::new();
        self.usages.reset();
    }

    fn cpu_id_checked(&self) -> u16 {
        self.cpu_id
            .expect("per-CPU slab is not initialized on this CPU")
    }
}

impl<const PAGE_SIZE: usize> SlabTrait for PercpuSlab<PAGE_SIZE> {
    fn cpu_id(&self) -> usize {
        self.cpu_id_checked() as usize
    }

    fn page_size(&self) -> usize {
        PAGE_SIZE
    }

    fn alloc(&self, layout: Layout) -> buddy_slab_allocator::AllocResult<SlabAllocResult> {
        self.inner
            .lock_irqsave()
            .alloc_hinted(layout, Some(&self.remote_hint))
    }

    fn add_slab(&self, size_class: SizeClass, base: usize, bytes: usize) {
        self.inner
            .lock_irqsave()
            .add_slab(size_class, base, bytes, self.cpu_id_checked());
    }

    fn dealloc_local(&self, ptr: NonNull<u8>, layout: Layout) -> SlabDeallocResult {
        self.inner.lock_irqsave().dealloc(ptr, layout)
    }

    fn remote_free_hint(&self) -> Option<&RemoteFreeHint> {
        Some(&self.remote_hint)
    }
}

fn current_percpu_slab() -> NonNull<PercpuSlab<PAGE_SIZE>> {
    // SAFETY: GlobalAllocator::with_runtime_state disables local IRQs and
    // preemption before upstream buddy-slab-allocator calls this hook. CPU
    // areas live until shutdown and PercpuSlab serializes later mutation.
    unsafe { ax_percpu::with_cpu_pin(|pin| PERCPU_SLAB.current_ptr(pin)) }
        .expect("allocator access requires an installed CPU area")
}

fn remote_percpu_slab(cpu_idx: usize) -> NonNull<PercpuSlab<PAGE_SIZE>> {
    let cpu_index = ax_percpu::CpuIndex::try_from(cpu_idx)
        .expect("allocator CPU index must fit the CPU-local ABI");
    let area = ax_percpu::area(cpu_index)
        .expect("allocator CPU index must name an initialized CPU-local area");
    PERCPU_SLAB.remote_ptr(area)
}

struct SlabPool;

impl SlabPoolTrait for SlabPool {
    fn current_slab(&self) -> &dyn SlabTrait {
        // SAFETY: CPU areas outlive the global pool, and the allocator's outer
        // guard pins the current CPU while the returned trait borrow is used.
        unsafe { current_percpu_slab().as_ref() }
    }

    fn owner_slab(&self, cpu_idx: usize) -> &dyn SlabTrait {
        // SAFETY: the selected area is permanent and PercpuSlab serializes all
        // local and remote interior mutation through its IRQ-safe lock.
        unsafe { remote_percpu_slab(cpu_idx).as_ref() }
    }
}

struct BuddySlabIfImpl;

#[ax_crate_interface::impl_interface]
impl BuddySlabIf for BuddySlabIfImpl {
    fn virt_to_phys(vaddr: usize) -> usize {
        ax_plat::mem::virt_to_phys(vaddr.into()).as_usize()
    }

    fn slab_pool() -> &'static dyn SlabPoolTrait {
        &SLAB_POOL
    }
}

/// The global allocator used by ArceOS when `buddy-slab` is enabled.
pub struct GlobalAllocator {
    inner: InnerAllocator<PAGE_SIZE>,
}

impl Default for GlobalAllocator {
    fn default() -> Self {
        Self::new()
    }
}

impl GlobalAllocator {
    /// Creates an empty [`GlobalAllocator`].
    pub const fn new() -> Self {
        Self {
            inner: InnerAllocator::<PAGE_SIZE>::new(),
        }
    }

    /// Runs one allocator backend operation without permitting CPU migration
    /// or same-CPU interrupt re-entry.
    ///
    /// The composed allocator already owns the only cross-CPU locks it needs:
    /// a buddy lock for page state and one lock per CPU slab. Keeping another
    /// global wrapper lock here would serialize every slab fast path. This
    /// guard supplies only the execution-context invariant required by the
    /// backend's raw buddy lock and per-CPU selection.
    fn with_runtime_state<R>(
        &self,
        operation: impl FnOnce(&InnerAllocator<PAGE_SIZE>, &PercpuSlab<PAGE_SIZE>) -> R,
    ) -> R {
        let _context = ax_sync::PreemptIrqSaveGuard::new();
        let slab = current_percpu_slab();
        // SAFETY: the context guard prevents migration and local re-entry;
        // per-CPU areas are permanent, while each remotely inspected field
        // supplies its own lock or atomic synchronization.
        operation(&self.inner, unsafe { slab.as_ref() })
    }

    fn with_backend<R>(&self, operation: impl FnOnce(&InnerAllocator<PAGE_SIZE>) -> R) -> R {
        self.with_runtime_state(|inner, _| operation(inner))
    }

    fn alloc_cached_order0(&self, kind: UsageKind) -> AllocResult<usize> {
        self.with_runtime_state(|inner, slab| {
            if let Some(page) = slab.page_cache.lock_irqsave().pop() {
                slab.usages.alloc(kind, PAGE_SIZE);
                return Ok(page);
            }

            let mut refill = [0; PERCPU_PAGE_REFILL_BATCH];
            let count = inner
                .alloc_order0_batch(&mut refill)
                .map_err(crate::AllocError::from)?;
            if count == 0 {
                return Err(crate::AllocError::NoMemory);
            }
            let page = refill[0];
            let mut cache = slab.page_cache.lock_irqsave();
            for cached in &refill[1..count] {
                cache.push(*cached);
            }
            slab.usages.alloc(kind, PAGE_SIZE);
            Ok(page)
        })
    }

    fn dealloc_cached_order0(&self, page: usize, kind: UsageKind) {
        self.with_runtime_state(|inner, slab| {
            let mut drained = [0; PERCPU_PAGE_DRAIN_BATCH];
            let drain_count = {
                let mut cache = slab.page_cache.lock_irqsave();
                let count = if cache.len == PERCPU_PAGE_CACHE_CAPACITY {
                    cache.drain_into(&mut drained)
                } else {
                    0
                };
                cache.push(page);
                count
            };
            if drain_count != 0 {
                inner.dealloc_order0_batch(&drained[..drain_count]);
            }
            slab.usages.dealloc(kind, PAGE_SIZE);
        });
    }

    /// Returns cached order-0 pages to the buddy allocator under pressure.
    fn drain_cached_order0_pages(&self, target: usize) -> usize {
        self.with_backend(|inner| {
            let Ok(layout) = ax_percpu::layout() else {
                return 0;
            };
            let mut total = 0;
            for cpu in 0..layout.area_count() {
                if total >= target {
                    break;
                }
                let cpu = ax_percpu::CpuIndex::try_from(cpu as usize)
                    .expect("per-CPU layout index must fit CpuIndex");
                // SAFETY: the frozen layout keeps every area alive and the
                // page-cache lock serializes this remote drain with its owner.
                let slab = unsafe { remote_percpu_slab(cpu.as_usize()).as_ref() };
                while total < target {
                    let mut pages = [0; PERCPU_PAGE_DRAIN_BATCH];
                    let limit = pages.len().min(target - total);
                    let count = slab
                        .page_cache
                        .lock_irqsave()
                        .drain_into(&mut pages[..limit]);
                    if count == 0 {
                        break;
                    }
                    inner.dealloc_order0_batch(&pages[..count]);
                    total += count;
                }
            }
            total
        })
    }

    fn cached_order0_pages(&self) -> usize {
        let Ok(layout) = ax_percpu::layout() else {
            return 0;
        };
        let mut total = 0usize;
        for cpu in 0..layout.area_count() {
            let cpu = ax_percpu::CpuIndex::try_from(cpu as usize)
                .expect("per-CPU layout index must fit CpuIndex");
            // SAFETY: the frozen layout keeps every area alive and the lock
            // supplies a coherent remote length snapshot.
            let slab = unsafe { remote_percpu_slab(cpu.as_usize()).as_ref() };
            total = total.saturating_add(slab.page_cache.lock_irqsave().len);
        }
        total
    }

    /// Returns the name of the allocator.
    pub const fn name(&self) -> &'static str {
        "buddy-slab-allocator"
    }

    /// Initializes the allocator with the given region.
    pub fn init(&self, start_vaddr: usize, size: usize) -> AllocResult {
        info!(
            "Initialize global memory allocator, start_vaddr: {:#x}, size: {:#x}",
            start_vaddr, size
        );
        let region = unsafe { slice::from_raw_parts_mut(start_vaddr as *mut u8, size) };
        self.with_backend(|inner| unsafe { inner.init(region) })
            .map_err(Into::into)
    }

    /// Add the given region to the allocator.
    pub fn add_memory(&self, start_vaddr: usize, size: usize) -> AllocResult {
        info!(
            "Add memory region, start_vaddr: {:#x}, size: {:#x}",
            start_vaddr, size
        );
        let region = unsafe { slice::from_raw_parts_mut(start_vaddr as *mut u8, size) };
        self.with_backend(|inner| unsafe { inner.add_region(region) })
            .map_err(Into::into)
    }

    /// Allocate arbitrary number of bytes. Returns the left bound of the
    /// allocated region.
    pub fn alloc(&self, layout: Layout) -> AllocResult<NonNull<u8>> {
        crate::retry_after_registered_reclaim(crate::layout_reclaim_pages(layout), || {
            retry_after_cached_order0_drain(
                || {
                    self.with_runtime_state(|inner, slab| {
                        let result = inner.alloc(layout).map_err(crate::AllocError::from);
                        if result.is_ok() {
                            slab.usages.alloc(UsageKind::RustHeap, layout.size());
                        }
                        result
                    })
                },
                || self.drain_cached_order0_pages(usize::MAX),
            )
        })
    }

    /// Gives back the allocated region to the byte allocator.
    pub fn dealloc(&self, pos: NonNull<u8>, layout: Layout) {
        self.with_runtime_state(|inner, slab| {
            unsafe { inner.dealloc(pos, layout) };
            slab.usages.dealloc(UsageKind::RustHeap, layout.size());
        });
    }

    /// Allocates contiguous pages.
    pub fn alloc_pages(
        &self,
        num_pages: usize,
        alignment: usize,
        kind: UsageKind,
    ) -> AllocResult<usize> {
        if num_pages == 1 && matches!(alignment, 0 | PAGE_SIZE) {
            return crate::retry_after_registered_reclaim(1, || {
                match self.alloc_cached_order0(kind) {
                    Err(crate::AllocError::NoMemory)
                        if self.drain_cached_order0_pages(PERCPU_PAGE_DRAIN_BATCH) != 0 =>
                    {
                        self.alloc_cached_order0(kind)
                    }
                    result => result,
                }
            });
        }
        crate::retry_after_registered_reclaim(num_pages, || {
            retry_after_cached_order0_drain(
                || {
                    self.with_runtime_state(|inner, slab| {
                        let result = inner
                            .alloc_pages(num_pages, alignment)
                            .map_err(crate::AllocError::from);
                        if result.is_ok() {
                            slab.usages.alloc(kind, num_pages * PAGE_SIZE);
                        }
                        result
                    })
                },
                || self.drain_cached_order0_pages(usize::MAX),
            )
        })
    }

    /// Allocates contiguous low-memory pages (physical address < 4 GiB).
    pub fn alloc_dma32_pages(
        &self,
        num_pages: usize,
        alignment: usize,
        kind: UsageKind,
    ) -> AllocResult<usize> {
        crate::retry_after_registered_reclaim(num_pages, || {
            retry_after_cached_order0_drain(
                || {
                    self.with_runtime_state(|inner, slab| {
                        let result = inner
                            .alloc_pages_lowmem(num_pages, alignment)
                            .map_err(crate::AllocError::from);
                        if result.is_ok() {
                            slab.usages.alloc(kind, num_pages * PAGE_SIZE);
                        }
                        result
                    })
                },
                || self.drain_cached_order0_pages(usize::MAX),
            )
        })
    }

    /// Allocates contiguous pages starting from the given address.
    pub fn alloc_pages_at(
        &self,
        _start: usize,
        _num_pages: usize,
        _alignment: usize,
        _kind: UsageKind,
    ) -> AllocResult<usize> {
        unimplemented!("buddy-slab allocator does not support alloc_pages_at")
    }

    /// Gives back the allocated pages starts from `pos` to the page allocator.
    pub fn dealloc_pages(&self, pos: usize, num_pages: usize, kind: UsageKind) {
        // Keep DMA32 pages visible to the constrained low-memory allocator;
        // a generic per-CPU cache cannot satisfy that address contract.
        if num_pages == 1 && kind != UsageKind::Dma {
            self.dealloc_cached_order0(pos, kind);
            return;
        }
        self.with_runtime_state(|inner, slab| {
            inner.dealloc_pages(pos, num_pages);
            slab.usages.dealloc(kind, num_pages * PAGE_SIZE);
        });
    }

    /// Returns the number of allocated bytes in the allocator backend.
    pub fn used_bytes(&self) -> usize {
        self.with_backend(InnerAllocator::allocated_bytes)
            .saturating_sub(self.cached_order0_pages().saturating_mul(PAGE_SIZE))
    }

    /// Returns the number of available bytes in the allocator backend.
    pub fn available_bytes(&self) -> usize {
        let (managed, allocated) =
            self.with_backend(|inner| (inner.managed_bytes(), inner.allocated_bytes()));
        let cached = self.cached_order0_pages().saturating_mul(PAGE_SIZE);
        managed.saturating_sub(allocated.saturating_sub(cached))
    }

    /// Returns the number of allocated pages in the allocator backend.
    pub fn used_pages(&self) -> usize {
        self.used_bytes() / PAGE_SIZE
    }

    /// Returns the number of available pages in the allocator backend.
    pub fn available_pages(&self) -> usize {
        self.available_bytes() / PAGE_SIZE
    }

    /// Returns the usage statistics of the allocator.
    pub fn usages(&self) -> Usages {
        let mut totals = [0i128; UsageKind::VARIANTS.len()];
        if let Ok(layout) = ax_percpu::layout() {
            for cpu in 0..layout.area_count() {
                let cpu = ax_percpu::CpuIndex::try_from(cpu as usize)
                    .expect("per-CPU layout index must fit CpuIndex");
                // SAFETY: the frozen layout keeps every area alive. Atomics
                // allow a lock-free snapshot while remote CPUs update their
                // signed ownership deltas.
                let slab = unsafe { remote_percpu_slab(cpu.as_usize()).as_ref() };
                for (total, usage) in totals.iter_mut().zip(&slab.usages.0) {
                    *total += usage.load(Ordering::Relaxed) as i128;
                }
            }
        }

        let mut usages = Usages::new();
        for &kind in UsageKind::VARIANTS {
            let total = totals[kind as usize].clamp(0, usize::MAX as i128) as usize;
            usages.alloc(kind, total);
        }
        usages
    }
}

impl AllocatorOps for GlobalAllocator {
    fn name(&self) -> &'static str {
        GlobalAllocator::name(self)
    }

    fn init(&self, start_vaddr: usize, size: usize) -> AllocResult {
        GlobalAllocator::init(self, start_vaddr, size)
    }

    fn add_memory(&self, start_vaddr: usize, size: usize) -> AllocResult {
        GlobalAllocator::add_memory(self, start_vaddr, size)
    }

    fn alloc(&self, layout: Layout) -> AllocResult<NonNull<u8>> {
        GlobalAllocator::alloc(self, layout)
    }

    fn dealloc(&self, pos: NonNull<u8>, layout: Layout) {
        GlobalAllocator::dealloc(self, pos, layout)
    }

    fn alloc_pages(
        &self,
        num_pages: usize,
        alignment: usize,
        kind: UsageKind,
    ) -> AllocResult<usize> {
        GlobalAllocator::alloc_pages(self, num_pages, alignment, kind)
    }

    fn alloc_dma32_pages(
        &self,
        num_pages: usize,
        alignment: usize,
        kind: UsageKind,
    ) -> AllocResult<usize> {
        GlobalAllocator::alloc_dma32_pages(self, num_pages, alignment, kind)
    }

    fn alloc_pages_at(
        &self,
        start: usize,
        num_pages: usize,
        alignment: usize,
        kind: UsageKind,
    ) -> AllocResult<usize> {
        GlobalAllocator::alloc_pages_at(self, start, num_pages, alignment, kind)
    }

    fn dealloc_pages(&self, pos: usize, num_pages: usize, kind: UsageKind) {
        GlobalAllocator::dealloc_pages(self, pos, num_pages, kind)
    }

    fn used_bytes(&self) -> usize {
        GlobalAllocator::used_bytes(self)
    }

    fn available_bytes(&self) -> usize {
        GlobalAllocator::available_bytes(self)
    }

    fn used_pages(&self) -> usize {
        GlobalAllocator::used_pages(self)
    }

    fn available_pages(&self) -> usize {
        GlobalAllocator::available_pages(self)
    }

    fn usages(&self) -> Usages {
        GlobalAllocator::usages(self)
    }
}

/// Returns the reference to the global allocator.
pub fn global_allocator() -> &'static GlobalAllocator {
    &GLOBAL_ALLOCATOR
}

/// Initializes the per-CPU slab for the current CPU during CPU bring-up.
///
/// Must run after per-CPU storage is initialized and before scheduler, IPI, or
/// IRQ paths can allocate on this CPU.
pub fn init_percpu_slab(cpu_id: usize) {
    // SAFETY: CPU bring-up excludes migration, IRQ/re-entry, and remote access
    // until this CPU-local slab has been initialized.
    unsafe {
        ax_percpu::with_cpu_pin(|pin| {
            ax_percpu::with_exclusive_cpu(pin, |exclusive| {
                PERCPU_SLAB.with_current_mut(exclusive, |slab| slab.init_during_cpu_bringup(cpu_id))
            })
        })
    }
    .expect("per-CPU slab initialization requires an installed CPU area");
}

/// Initializes the global allocator with the given memory region.
pub fn global_init(start_vaddr: usize, size: usize) -> AllocResult {
    debug!(
        "initialize global allocator at: [{:#x}, {:#x})",
        start_vaddr,
        start_vaddr + size
    );
    GLOBAL_ALLOCATOR.init(start_vaddr, size)?;
    info!("global allocator initialized");
    Ok(())
}

/// Add the given memory region to the global allocator.
pub fn global_add_memory(start_vaddr: usize, size: usize) -> AllocResult {
    debug!(
        "add a memory region to global allocator: [{:#x}, {:#x})",
        start_vaddr,
        start_vaddr + size
    );
    GLOBAL_ALLOCATOR.add_memory(start_vaddr, size)
}

unsafe impl GlobalAlloc for GlobalAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let inner = move || {
            if let Ok(ptr) = GlobalAllocator::alloc(self, layout) {
                ptr.as_ptr()
            } else {
                // Let fallible containers observe allocation failure. The
                // standard library still calls its allocation-error handler
                // for infallible Box/Vec/Arc construction after a null result.
                core::ptr::null_mut()
            }
        };

        #[cfg(feature = "tracking")]
        {
            crate::tracking::with_state(|state| match state {
                None => inner(),
                Some(state) => {
                    let ptr = inner();
                    if ptr.is_null() {
                        return ptr;
                    }
                    let generation = state.generation;
                    state.generation += 1;
                    state.map.insert(
                        ptr as usize,
                        crate::tracking::AllocationInfo {
                            layout,
                            backtrace: axbacktrace::Backtrace::capture(),
                            generation,
                        },
                    );
                    ptr
                }
            })
        }

        #[cfg(not(feature = "tracking"))]
        inner()
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        let ptr = NonNull::new(ptr).expect("dealloc null ptr");
        let inner = || GlobalAllocator::dealloc(self, ptr, layout);

        #[cfg(feature = "tracking")]
        crate::tracking::with_state(|state| match state {
            None => inner(),
            Some(state) => {
                let address = ptr.as_ptr() as usize;
                state.map.remove(&address);
                inner()
            }
        });

        #[cfg(not(feature = "tracking"))]
        inner();
    }
}

impl From<buddy_slab_allocator::AllocError> for super::AllocError {
    fn from(value: buddy_slab_allocator::AllocError) -> Self {
        match value {
            buddy_slab_allocator::AllocError::InvalidParam => Self::InvalidParam,
            buddy_slab_allocator::AllocError::AlreadyInitialized => Self::AlreadyInitialized,
            buddy_slab_allocator::AllocError::MemoryOverlap => Self::MemoryOverlap,
            buddy_slab_allocator::AllocError::NoMemory => Self::NoMemory,
            buddy_slab_allocator::AllocError::NotAllocated => Self::NotAllocated,
            buddy_slab_allocator::AllocError::NotInitialized => Self::NotInitialized,
            buddy_slab_allocator::AllocError::NotFound => Self::NotFound,
        }
    }
}

#[cfg(test)]
mod tests {
    use core::cell::Cell;

    use super::*;

    #[test]
    fn backend_allocation_retries_after_cached_pages_are_drained() {
        let attempts = Cell::new(0);
        let drains = Cell::new(0);

        let result = retry_after_cached_order0_drain(
            || {
                attempts.set(attempts.get() + 1);
                if attempts.get() == 1 {
                    Err(crate::AllocError::NoMemory)
                } else {
                    Ok(0x1000)
                }
            },
            || {
                drains.set(drains.get() + 1);
                32
            },
        );

        assert_eq!(result, Ok(0x1000));
        assert_eq!(attempts.get(), 2);
        assert_eq!(drains.get(), 1);
    }

    #[test]
    fn per_cpu_usage_accepts_cross_cpu_free_deltas() {
        let first = PercpuUsages::new();
        let second = PercpuUsages::new();

        first.alloc(UsageKind::RustHeap, 4096);
        second.dealloc(UsageKind::RustHeap, 1024);

        let total = first.0[UsageKind::RustHeap as usize].load(Ordering::Relaxed)
            + second.0[UsageKind::RustHeap as usize].load(Ordering::Relaxed);
        assert_eq!(total, 3072);
    }

    #[test]
    fn per_cpu_page_cache_drains_a_bounded_batch() {
        let mut cache = PercpuPageCache::new();
        for page in 1..=PERCPU_PAGE_CACHE_CAPACITY {
            cache.push(page * PAGE_SIZE);
        }
        let mut drained = [0; PERCPU_PAGE_DRAIN_BATCH];

        assert_eq!(cache.drain_into(&mut drained), drained.len());
        assert_eq!(cache.len, PERCPU_PAGE_CACHE_CAPACITY - drained.len());
        assert_eq!(drained[0], PERCPU_PAGE_CACHE_CAPACITY * PAGE_SIZE);
        assert_eq!(drained[drained.len() - 1], 33 * PAGE_SIZE);
    }
}
