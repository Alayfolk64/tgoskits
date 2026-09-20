use alloc::{collections::VecDeque, vec::Vec};
use core::iter::Peekable;

use log::warn;
use rdif_block::{
    BlkError, CompletedRequest, OwnedRequest, OwnedRequestBatch, QueueInfo, RequestFlags,
    RequestOp, TransferChunk, TransferPlan, TransferPlanner, TransferRuntimeCaps,
};

use super::{BlockDeviceHandle, block_io_error, request_cannot_block};
use crate::{BlockError, BlockResult};

const MAX_RUNTIME_TRANSFER_BYTES: usize = 4 * 1024 * 1024;
const SOFTWARE_PIPELINE_WINDOWS: usize = 2;

struct ReadWindow {
    chunks: Vec<TransferChunk>,
    completions: super::super::CompletionGroup,
    limits: rdif_block::QueueLimits,
}

#[cfg(any(feature = "ext4", feature = "fat"))]
struct WriteWindow {
    chunks: Vec<TransferChunk>,
    completions: super::super::CompletionGroup,
    limits: rdif_block::QueueLimits,
}

pub(super) fn read_blocks(
    device: &BlockDeviceHandle,
    block_id: u64,
    buffer: &mut [u8],
) -> BlockResult {
    if buffer.is_empty() {
        return Ok(());
    }
    ensure_sleepable()?;
    let info = device.inner.selected_queue_info().ok_or(BlockError::Io)?;
    let mut plan = transfer_plan(info, block_id, buffer.len(), RequestOp::Read)?.peekable();
    let window_limit = submission_window_limit(info);
    let mut pending = VecDeque::with_capacity(SOFTWARE_PIPELINE_WINDOWS);

    while plan.peek().is_some() || !pending.is_empty() {
        while pending.len() < SOFTWARE_PIPELINE_WINDOWS && plan.peek().is_some() {
            pending.push_back(submit_read_window(
                device,
                info,
                take_window(&mut plan, window_limit)?,
            )?);
        }
        let window = pending.pop_front().ok_or(BlockError::InvalidState)?;
        let first_lba = window.chunks[0].lba;
        let completions = window
            .completions
            .recv()
            .map_err(|error| block_io_error("receive window", RequestOp::Read, first_lba, error))?;
        complete_read_window(
            &device.inner.dma_pool,
            window.limits,
            &window.chunks,
            completions,
            buffer,
        )?;
    }
    Ok(())
}

#[cfg(any(feature = "ext4", feature = "fat"))]
pub(super) fn write_blocks(
    device: &BlockDeviceHandle,
    block_id: u64,
    buffer: &[u8],
) -> BlockResult {
    write_blocks_with_flags(device, block_id, buffer, RequestFlags::NONE)
}

#[cfg(feature = "ext4")]
pub(super) fn write_blocks_fua(
    device: &BlockDeviceHandle,
    block_id: u64,
    buffer: &[u8],
) -> BlockResult {
    write_blocks_with_flags(device, block_id, buffer, RequestFlags::FUA)
}

#[cfg(any(feature = "ext4", feature = "fat"))]
fn write_blocks_with_flags(
    device: &BlockDeviceHandle,
    block_id: u64,
    buffer: &[u8],
    flags: RequestFlags,
) -> BlockResult {
    if buffer.is_empty() {
        return Ok(());
    }
    ensure_sleepable()?;
    let info = device.inner.selected_queue_info().ok_or(BlockError::Io)?;
    let mut plan = transfer_plan(info, block_id, buffer.len(), RequestOp::Write)?.peekable();
    let window_limit = submission_window_limit(info);
    let mut pending = VecDeque::with_capacity(SOFTWARE_PIPELINE_WINDOWS);
    let mut first_error = None;

    while plan.peek().is_some() || !pending.is_empty() {
        while first_error.is_none()
            && pending.len() < SOFTWARE_PIPELINE_WINDOWS
            && plan.peek().is_some()
        {
            let window = take_window(&mut plan, window_limit)
                .and_then(|chunks| submit_write_window(device, info, chunks, buffer, flags));
            match window {
                Ok(window) => pending.push_back(window),
                Err(error) => first_error = Some(error),
            }
        }
        let Some(window) = pending.pop_front() else {
            break;
        };
        let first_lba = window.chunks[0].lba;
        let result = window
            .completions
            .recv()
            .map_err(|error| block_io_error("receive window", RequestOp::Write, first_lba, error))
            .and_then(|completions| {
                complete_write_window(
                    &device.inner.dma_pool,
                    window.limits,
                    &window.chunks,
                    completions,
                )
            });
        if let Err(error) = result
            && first_error.is_none()
        {
            first_error = Some(error);
        }
    }
    first_error.map_or(Ok(()), Err)
}

fn submit_read_window(
    device: &BlockDeviceHandle,
    info: QueueInfo,
    chunks: Vec<TransferChunk>,
) -> Result<ReadWindow, BlockError> {
    let first_lba = chunks[0].lba;
    let requests = prepare_read_requests(&device.inner.dma_pool, info, &chunks)?;
    let completions = device.submit_batch_owned(requests).map_err(|error| {
        let source = error.error;
        recycle_unsubmitted(&device.inner.dma_pool, info.limits, error.into_batch());
        block_io_error("submit window", RequestOp::Read, first_lba, source)
    })?;
    Ok(ReadWindow {
        chunks,
        completions,
        limits: info.limits,
    })
}

#[cfg(any(feature = "ext4", feature = "fat"))]
fn submit_write_window(
    device: &BlockDeviceHandle,
    info: QueueInfo,
    chunks: Vec<TransferChunk>,
    buffer: &[u8],
    flags: RequestFlags,
) -> Result<WriteWindow, BlockError> {
    let first_lba = chunks[0].lba;
    let requests = prepare_write_requests(&device.inner.dma_pool, info, &chunks, buffer, flags)?;
    let completions = device.submit_batch_owned(requests).map_err(|error| {
        let source = error.error;
        recycle_unsubmitted(&device.inner.dma_pool, info.limits, error.into_batch());
        block_io_error("submit window", RequestOp::Write, first_lba, source)
    })?;
    Ok(WriteWindow {
        chunks,
        completions,
        limits: info.limits,
    })
}

fn transfer_plan(
    info: QueueInfo,
    block_id: u64,
    byte_len: usize,
    op: RequestOp,
) -> Result<TransferPlan, BlockError> {
    let boundary_cap = info
        .limits
        .dma
        .constraints()
        .boundary
        .unwrap_or(MAX_RUNTIME_TRANSFER_BYTES);
    let planner = TransferPlanner::new(
        info.device,
        info.limits,
        TransferRuntimeCaps::new(MAX_RUNTIME_TRANSFER_BYTES.min(boundary_cap), 1),
    )
    .map_err(|error| block_io_error("create transfer plan", op, block_id, error))?;
    planner
        .plan(block_id, byte_len)
        .map_err(|error| block_io_error("plan transfer", op, block_id, error))
}

fn submission_window_limit(info: QueueInfo) -> usize {
    info.limits
        .max_inflight
        .min(info.limits.max_submit_batch)
        .max(1)
}

fn take_window(
    plan: &mut Peekable<TransferPlan>,
    limit: usize,
) -> Result<Vec<TransferChunk>, BlockError> {
    let mut chunks = Vec::new();
    chunks
        .try_reserve_exact(limit)
        .map_err(|_| BlockError::NoMemory)?;
    chunks.extend(plan.by_ref().take(limit));
    if chunks.is_empty() {
        return Err(BlockError::InvalidRequest);
    }
    Ok(chunks)
}

fn prepare_read_requests(
    pool: &super::super::dma::DmaBufferPool,
    info: QueueInfo,
    chunks: &[TransferChunk],
) -> Result<OwnedRequestBatch, BlockError> {
    let mut requests = OwnedRequestBatch::with_capacity(chunks.len());
    for chunk in chunks {
        let data = match pool.prepare_read(info.limits, chunk.byte_len) {
            Ok(data) => data,
            Err(error) => {
                recycle_unsubmitted(pool, info.limits, requests);
                return Err(block_io_error(
                    "prepare DMA",
                    RequestOp::Read,
                    chunk.lba,
                    error,
                ));
            }
        };
        requests.push_back(OwnedRequest {
            op: RequestOp::Read,
            lba: chunk.lba,
            block_count: chunk.block_count,
            data: Some(data),
            flags: RequestFlags::NONE,
        });
    }
    Ok(requests)
}

#[cfg(any(feature = "ext4", feature = "fat"))]
fn prepare_write_requests(
    pool: &super::super::dma::DmaBufferPool,
    info: QueueInfo,
    chunks: &[TransferChunk],
    buffer: &[u8],
    flags: RequestFlags,
) -> Result<OwnedRequestBatch, BlockError> {
    let mut requests = OwnedRequestBatch::with_capacity(chunks.len());
    for chunk in chunks {
        let range = chunk.byte_offset..chunk.byte_offset + chunk.byte_len;
        let data = match pool.prepare_write(info.limits, &buffer[range]) {
            Ok(data) => data,
            Err(error) => {
                recycle_unsubmitted(pool, info.limits, requests);
                return Err(block_io_error(
                    "prepare DMA",
                    RequestOp::Write,
                    chunk.lba,
                    error,
                ));
            }
        };
        requests.push_back(OwnedRequest {
            op: RequestOp::Write,
            lba: chunk.lba,
            block_count: chunk.block_count,
            data: Some(data),
            flags,
        });
    }
    Ok(requests)
}

fn complete_read_window(
    pool: &super::super::dma::DmaBufferPool,
    limits: rdif_block::QueueLimits,
    chunks: &[TransferChunk],
    mut completions: Vec<CompletedRequest>,
    buffer: &mut [u8],
) -> BlockResult {
    if completions.len() != chunks.len() {
        recycle_completions(pool, limits, completions);
        return Err(block_io_error(
            "match completion window",
            RequestOp::Read,
            chunks[0].lba,
            BlkError::Io,
        ));
    }

    let mut first_error = None;
    for (chunk, mut completion) in chunks.iter().zip(completions.drain(..)) {
        if let Err(error) = completion.result {
            warn!(
                "block Read completion at LBA {} reported {error:?}",
                chunk.lba
            );
            first_error.get_or_insert((chunk.lba, error));
            if let Some(data) = completion.data.take() {
                pool.recycle(limits, data);
            }
            continue;
        }
        let Some(data) = completion.data.take() else {
            warn!(
                "block Read completion at LBA {} returned no DMA data",
                chunk.lba
            );
            first_error.get_or_insert((chunk.lba, BlkError::Io));
            continue;
        };
        if data.len().get() != chunk.byte_len {
            warn!(
                "block Read completion at LBA {} returned {} bytes, expected {}",
                chunk.lba,
                data.len(),
                chunk.byte_len
            );
            first_error.get_or_insert((chunk.lba, BlkError::Io));
            pool.recycle(limits, data);
            continue;
        }
        let range = chunk.byte_offset..chunk.byte_offset + chunk.byte_len;
        data.copy_to_slice_cpu(&mut buffer[range]);
        pool.recycle(limits, data);
    }

    if let Some((lba, error)) = first_error {
        Err(block_io_error(
            "complete window",
            RequestOp::Read,
            lba,
            error,
        ))
    } else {
        Ok(())
    }
}

#[cfg(any(feature = "ext4", feature = "fat"))]
fn complete_write_window(
    pool: &super::super::dma::DmaBufferPool,
    limits: rdif_block::QueueLimits,
    chunks: &[TransferChunk],
    mut completions: Vec<CompletedRequest>,
) -> BlockResult {
    if completions.len() != chunks.len() {
        recycle_completions(pool, limits, completions);
        return Err(block_io_error(
            "match completion window",
            RequestOp::Write,
            chunks[0].lba,
            BlkError::Io,
        ));
    }

    let mut first_error = None;
    for (chunk, mut completion) in chunks.iter().zip(completions.drain(..)) {
        if let Err(error) = completion.result {
            first_error.get_or_insert((chunk.lba, error));
        }
        if let Some(data) = completion.data.take() {
            pool.recycle(limits, data);
        }
    }
    if let Some((lba, error)) = first_error {
        Err(block_io_error(
            "complete window",
            RequestOp::Write,
            lba,
            error,
        ))
    } else {
        Ok(())
    }
}

fn recycle_unsubmitted(
    pool: &super::super::dma::DmaBufferPool,
    limits: rdif_block::QueueLimits,
    requests: OwnedRequestBatch,
) {
    for request in requests {
        if let Some(data) = super::super::dma::complete_without_submit(request.data) {
            pool.recycle(limits, data);
        }
    }
}

fn recycle_completions(
    pool: &super::super::dma::DmaBufferPool,
    limits: rdif_block::QueueLimits,
    completions: Vec<CompletedRequest>,
) {
    for completion in completions {
        if let Some(data) = completion.data {
            pool.recycle(limits, data);
        }
    }
}

fn ensure_sleepable() -> BlockResult {
    if request_cannot_block() {
        Err(BlockError::WouldBlock)
    } else {
        Ok(())
    }
}
