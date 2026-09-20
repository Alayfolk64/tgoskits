use alloc::vec::Vec;
use core::num::NonZeroUsize;
#[cfg(feature = "profile")]
use core::sync::atomic::{AtomicU64, Ordering};

use dma_api::{CompletedDma, CpuDmaBuffer, DeviceDma, DmaDeviceInfo, DmaDirection, PreparedDma};
use rdif_block::{BlkError, QueueLimits};

use crate::os::{dma_op, sync::IrqMutex};

const MAX_POOL_BUFFERS: usize = 128;
const MAX_POOL_BYTES: usize = 8 * 1024 * 1024;

#[cfg(feature = "profile")]
static DMA_POOL_HITS: AtomicU64 = AtomicU64::new(0);
#[cfg(feature = "profile")]
static DMA_POOL_MISSES: AtomicU64 = AtomicU64::new(0);
#[cfg(feature = "profile")]
static DMA_POOL_RETURNS: AtomicU64 = AtomicU64::new(0);
#[cfg(feature = "profile")]
static DMA_POOL_REJECTS: AtomicU64 = AtomicU64::new(0);

/// Cumulative transfer-buffer pool counters for profiling builds.
#[cfg(feature = "profile")]
pub struct DmaPoolStats {
    pub hits: u64,
    pub misses: u64,
    pub returns: u64,
    pub rejects: u64,
}

#[cfg(feature = "profile")]
pub fn dma_pool_stats() -> DmaPoolStats {
    DmaPoolStats {
        hits: DMA_POOL_HITS.load(Ordering::Relaxed),
        misses: DMA_POOL_MISSES.load(Ordering::Relaxed),
        returns: DMA_POOL_RETURNS.load(Ordering::Relaxed),
        rejects: DMA_POOL_REJECTS.load(Ordering::Relaxed),
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
struct DmaBufferKey {
    device: DmaDeviceInfo,
    len: usize,
    align: usize,
    direction: DmaDirection,
}

struct PooledDmaBuffer {
    key: DmaBufferKey,
    buffer: CpuDmaBuffer,
}

struct DmaBufferPoolState {
    buffers: Vec<PooledDmaBuffer>,
    bytes: usize,
}

/// Device-local bounded cache of CPU-owned DMA transfer buffers.
///
/// Buffers cross this boundary only after device ownership has ended. Exact
/// matching by DMA identity, alignment, direction, and length preserves the
/// constraints used for the original allocation. The count follows the
/// blk-mq queue depth while the byte ceiling prevents large transfers from
/// pinning an unbounded amount of contiguous memory.
pub(super) struct DmaBufferPool {
    state: IrqMutex<DmaBufferPoolState>,
}

impl DmaBufferPool {
    pub(super) fn try_new() -> Result<Self, BlkError> {
        let mut buffers = Vec::new();
        buffers
            .try_reserve_exact(MAX_POOL_BUFFERS)
            .map_err(|_| BlkError::NoMemory)?;
        Ok(Self {
            state: IrqMutex::new(DmaBufferPoolState { buffers, bytes: 0 }),
        })
    }

    pub(super) fn prepare_read(
        &self,
        limits: QueueLimits,
        len: usize,
    ) -> Result<PreparedDma, BlkError> {
        self.acquire(limits, len, DmaDirection::FromDevice)
            .map(CpuDmaBuffer::prepare_for_device)
    }

    #[cfg(any(feature = "ext4", feature = "fat"))]
    pub(super) fn prepare_write(
        &self,
        limits: QueueLimits,
        source: &[u8],
    ) -> Result<PreparedDma, BlkError> {
        let mut buffer = self.acquire(limits, source.len(), DmaDirection::ToDevice)?;
        buffer.copy_from_slice_cpu(source);
        Ok(buffer.prepare_for_device())
    }

    pub(super) fn recycle(&self, limits: QueueLimits, completed: CompletedDma) {
        let buffer = completed.into_cpu_buffer();
        let len = buffer.len().get();
        let key = Self::key(limits, len, buffer.direction());
        let max_buffers = limits
            .max_inflight
            .saturating_mul(2)
            .clamp(1, MAX_POOL_BUFFERS);
        let mut entry = Some(PooledDmaBuffer { key, buffer });
        {
            let mut state = self.state.lock();
            if state.buffers.len() < max_buffers
                && state.bytes.saturating_add(len) <= MAX_POOL_BYTES
            {
                state.bytes += len;
                state.buffers.push(
                    entry
                        .take()
                        .expect("pool entry is present before insertion"),
                );
                #[cfg(feature = "profile")]
                DMA_POOL_RETURNS.fetch_add(1, Ordering::Relaxed);
            }
        }
        #[cfg(feature = "profile")]
        if entry.is_some() {
            DMA_POOL_REJECTS.fetch_add(1, Ordering::Relaxed);
        }
        // A buffer rejected by the bounded pool is released after dropping the
        // IRQ-safe state lock; contiguous deallocation may enter the allocator.
        drop(entry);
    }

    fn acquire(
        &self,
        limits: QueueLimits,
        len: usize,
        direction: DmaDirection,
    ) -> Result<CpuDmaBuffer, BlkError> {
        let key = Self::key(limits, len, direction);
        let pooled = {
            let mut state = self.state.lock();
            let index = state.buffers.iter().rposition(|entry| entry.key == key);
            index.map(|index| {
                let entry = state.buffers.swap_remove(index);
                state.bytes -= entry.key.len;
                entry.buffer
            })
        };
        #[cfg(feature = "profile")]
        if pooled.is_some() {
            DMA_POOL_HITS.fetch_add(1, Ordering::Relaxed);
        } else {
            DMA_POOL_MISSES.fetch_add(1, Ordering::Relaxed);
        }
        pooled.map_or_else(|| allocate(limits, len, direction), Ok)
    }

    fn key(limits: QueueLimits, len: usize, direction: DmaDirection) -> DmaBufferKey {
        DmaBufferKey {
            device: limits.dma,
            len,
            align: limits.dma.constraints().align,
            direction,
        }
    }
}

#[cfg(test)]
pub(super) fn prepare_read(limits: QueueLimits, len: usize) -> Result<PreparedDma, BlkError> {
    allocate(limits, len, DmaDirection::FromDevice).map(CpuDmaBuffer::prepare_for_device)
}

pub(super) fn complete_without_submit(data: Option<PreparedDma>) -> Option<CompletedDma> {
    data.map(PreparedDma::complete_without_device)
}

fn allocate(
    limits: QueueLimits,
    len: usize,
    direction: DmaDirection,
) -> Result<CpuDmaBuffer, BlkError> {
    let constraints = limits.dma.constraints();
    if constraints.align == 0
        || limits.dma_length_alignment == 0
        || !len.is_multiple_of(limits.dma_length_alignment)
    {
        return Err(BlkError::InvalidRequest);
    }
    let dma_op = dma_op().ok_or(BlkError::Io)?;
    if let Some(boundary) = constraints.boundary
        && !boundary.is_power_of_two()
    {
        return Err(BlkError::InvalidRequest);
    }
    let device = DeviceDma::new(limits.dma, dma_op);
    CpuDmaBuffer::new_zero(
        &device,
        NonZeroUsize::new(len).ok_or(BlkError::InvalidRequest)?,
        constraints.align,
        direction,
    )
    .map_err(BlkError::from)
}
