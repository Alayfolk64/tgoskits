//! CQHCI tag allocation, submission, completion, and recovery.

use alloc::{boxed::Box, sync::Arc, vec::Vec};
use core::sync::atomic::{Ordering, fence};

use dma_api::{DeviceDma, InFlightDma};
use log::{info, warn};
use rdif_block::{
    BatchSubmitDisposition, BatchSubmitResult, BlkError, CompletedRequest, CompletionSink,
    OwnedRequest, OwnedRequestBatch, QueueInfo, RequestId, RequestOp, SubmissionSink,
    validate_owned_request,
};
use sdmmc_protocol::rdif::{BlockConfig, CommandQueueAccelerator, CommandQueueActivation};

use crate::{
    CqhciPlatform, descriptor::DescriptorTables, irq::IrqState, read_u32, regs::*, write_u32,
};

const MMC_SWITCH_WRITE_BYTE: u32 = 0b11;
const MMC_FLUSH_CACHE_INDEX: u32 = 32;
const MMC_FLUSH_CACHE_TRIGGER: u32 = 1;
const MMC_CMD6: u8 = 6;
const MMC_CMD13: u8 = 13;

struct RequestSlot {
    request_id: Option<RequestId>,
    dma: Option<InFlightDma>,
}

struct FlushSlot {
    request_id: RequestId,
}

/// One CQHCI hardware queue with a single task-context owner.
pub struct CqhciQueue {
    base: usize,
    platform: Box<dyn CqhciPlatform>,
    irq: Arc<IrqState>,
    descriptors: Option<DescriptorTables>,
    slots: Vec<RequestSlot>,
    free_tags: Vec<usize>,
    flush: Option<FlushSlot>,
    staged: u32,
    active: bool,
    depth: usize,
    next_flush_id: usize,
    cache_enabled: bool,
    info: Option<QueueInfo>,
}

impl CqhciQueue {
    /// Creates a disabled command queue over one CQHCI register file.
    ///
    /// # Safety
    ///
    /// `base` must address an exclusively owned CQHCI register file for the
    /// same physical eMMC host represented by `platform` and `dma`.
    pub(crate) unsafe fn new(
        base: core::ptr::NonNull<u8>,
        dma: DeviceDma,
        platform: Box<dyn CqhciPlatform>,
    ) -> Result<(Self, Arc<IrqState>), BlkError> {
        let descriptors = DescriptorTables::allocate(&dma)?;
        let irq = Arc::new(IrqState::new(base.as_ptr() as usize));
        let mut slots = Vec::with_capacity(DATA_SLOTS);
        slots.resize_with(DATA_SLOTS, || RequestSlot {
            request_id: None,
            dma: None,
        });
        let queue = Self {
            base: base.as_ptr() as usize,
            platform,
            irq: Arc::clone(&irq),
            descriptors: Some(descriptors),
            slots,
            free_tags: Vec::new(),
            flush: None,
            staged: 0,
            active: false,
            depth: 0,
            next_flush_id: usize::MAX / 2,
            cache_enabled: false,
            info: None,
        };
        Ok((queue, irq))
    }

    fn stage_next(&mut self, requests: &mut OwnedRequestBatch) -> Result<RequestId, BlkError> {
        let Some(mut request) = requests.pop_front() else {
            return Err(BlkError::InvalidRequest);
        };
        let info = self.info.ok_or(BlkError::InvalidRequest)?;
        if let Err(error) = validate_owned_request(info, &request) {
            requests.push_front(request);
            return Err(error);
        }
        if request.op == RequestOp::Flush {
            return self.stage_flush(requests, request);
        }
        if self.flush.is_some() {
            requests.push_front(request);
            return Err(BlkError::Retry);
        }
        let Some(tag) = self.free_tags.pop() else {
            requests.push_front(request);
            return Err(BlkError::Retry);
        };
        let Some(segment) = request.data.as_ref().map(|data| data.segment()) else {
            self.free_tags.push(tag);
            requests.push_front(request);
            return Err(BlkError::InvalidRequest);
        };
        let Some(descriptors) = self.descriptors.as_mut() else {
            self.free_tags.push(tag);
            requests.push_front(request);
            return Err(BlkError::InvalidRequest);
        };
        if let Err(error) =
            descriptors.prepare_data(tag, request.op, request.lba, request.block_count, segment)
        {
            self.free_tags.push(tag);
            requests.push_front(request);
            return Err(error);
        }
        let dma = request.data.take().map(|dma| {
            // SAFETY: the request is installed in its tag before the staged
            // doorbell bit can be published to hardware.
            unsafe { dma.into_in_flight() }
        });
        let id = RequestId::new(tag);
        self.slots[tag].request_id = Some(id);
        self.slots[tag].dma = dma;
        self.staged |= 1 << tag;
        Ok(id)
    }

    fn stage_flush(
        &mut self,
        requests: &mut OwnedRequestBatch,
        request: OwnedRequest,
    ) -> Result<RequestId, BlkError> {
        if self.staged != 0
            || self.flush.is_some()
            || self.slots.iter().any(|slot| slot.request_id.is_some())
        {
            requests.push_front(request);
            return Err(BlkError::Retry);
        }
        let id = RequestId::new(self.next_flush_id);
        self.next_flush_id = self.next_flush_id.wrapping_add(1);
        let descriptors = self.descriptors.as_mut().ok_or(BlkError::InvalidRequest)?;
        if self.cache_enabled {
            let argument = MMC_SWITCH_WRITE_BYTE << 24
                | MMC_FLUSH_CACHE_INDEX << 16
                | MMC_FLUSH_CACHE_TRIGGER << 8;
            descriptors.prepare_direct_command(MMC_CMD6, argument, true);
        } else {
            let rca = u32::from(read_u32(self.base, SSC2) as u16);
            descriptors.prepare_direct_command(MMC_CMD13, rca << 16, false);
        }
        self.flush = Some(FlushSlot { request_id: id });
        self.staged |= 1 << DIRECT_COMMAND_SLOT;
        Ok(id)
    }

    fn complete_tag(
        &mut self,
        tag: usize,
        result: Result<(), BlkError>,
        sink: &mut dyn CompletionSink,
    ) -> Result<(), BlkError> {
        if tag == DIRECT_COMMAND_SLOT {
            let flush = self.flush.take().ok_or(BlkError::Io)?;
            if let Some(descriptors) = self.descriptors.as_mut() {
                descriptors.clear_task(tag);
            }
            sink.complete(CompletedRequest::new(flush.request_id, result, None));
            return Ok(());
        }
        let slot = self.slots.get_mut(tag).ok_or(BlkError::Io)?;
        let id = slot.request_id.take().ok_or(BlkError::Io)?;
        let dma = slot.dma.take().map(|dma| {
            // SAFETY: TCN is the CQHCI terminal handoff for this tag.
            unsafe { dma.complete_after_quiesce() }
        });
        if let Some(descriptors) = self.descriptors.as_mut() {
            descriptors.clear_task(tag);
        }
        self.free_tags.push(tag);
        sink.complete(CompletedRequest::new(id, result, dma));
        Ok(())
    }

    fn recover_all(&mut self, sink: &mut dyn CompletionSink) -> Result<(), BlkError> {
        if !self.halt() {
            self.quarantine_all(sink);
            return Err(BlkError::Io);
        }
        write_u32(self.base, CTL, CTL_HALT | CTL_CLEAR_ALL_TASKS);
        for _ in 0..10_000 {
            if read_u32(self.base, CTL) & CTL_CLEAR_ALL_TASKS == 0 {
                self.complete_all_after_quiesce(sink);
                self.staged = 0;
                return Err(BlkError::Io);
            }
            core::hint::spin_loop();
        }
        self.quarantine_all(sink);
        Err(BlkError::Io)
    }

    fn halt(&self) -> bool {
        write_u32(self.base, CTL, CTL_HALT);
        for _ in 0..10_000 {
            if read_u32(self.base, CTL) & CTL_HALT != 0 {
                return true;
            }
            core::hint::spin_loop();
        }
        false
    }

    fn complete_all_after_quiesce(&mut self, sink: &mut dyn CompletionSink) {
        for tag in 0..self.depth {
            if self.slots[tag].request_id.is_some() {
                let _ = self.complete_tag(tag, Err(BlkError::Io), sink);
            }
        }
        if self.flush.is_some() {
            let _ = self.complete_tag(DIRECT_COMMAND_SLOT, Err(BlkError::Io), sink);
        }
    }

    fn quarantine_all(&mut self, sink: &mut dyn CompletionSink) {
        for tag in 0..self.depth {
            let slot = &mut self.slots[tag];
            let Some(id) = slot.request_id.take() else {
                continue;
            };
            if let Some(dma) = slot.dma.take() {
                let _quarantined = dma.quarantine();
            }
            sink.complete(CompletedRequest::new(id, Err(BlkError::Io), None));
        }
        if let Some(flush) = self.flush.take() {
            sink.complete(CompletedRequest::new(
                flush.request_id,
                Err(BlkError::Io),
                None,
            ));
        }
        self.staged = 0;
    }
}

impl CommandQueueAccelerator for CqhciQueue {
    fn provisioned_depth(&self) -> usize {
        DATA_SLOTS
    }

    fn activate(
        &mut self,
        activation: CommandQueueActivation,
        config: &mut BlockConfig,
    ) -> Result<(), BlkError> {
        if self.active {
            return Err(BlkError::InvalidRequest);
        }
        let depth = activation.card_depth.min(DATA_SLOTS);
        if depth == 0 {
            return Err(BlkError::NotSupported);
        }
        let task_list = self
            .descriptors
            .as_ref()
            .ok_or(BlkError::InvalidRequest)?
            .task_list_dma();
        self.platform.prepare(self.base)?;
        let mut cfg = read_u32(self.base, CFG);
        cfg &= !(CFG_ENABLE | CFG_TASK_DESC_128 | CFG_CRYPTO_GENERAL_ENABLE);
        cfg |= CFG_DCMD;
        write_u32(self.base, CFG, cfg);
        let stale_status = read_u32(self.base, IS);
        if stale_status != 0 {
            write_u32(self.base, IS, stale_status);
        }
        let stale_completions = read_u32(self.base, TCN);
        if stale_completions != 0 {
            write_u32(self.base, TCN, stale_completions);
        }
        write_u32(self.base, TDLBA, task_list as u32);
        write_u32(self.base, TDLBAU, (task_list >> 32) as u32);
        write_u32(self.base, SSC2, u32::from(activation.rca));
        write_u32(self.base, ISTE, 0);
        write_u32(self.base, ISGE, 0);
        write_u32(self.base, CFG, cfg | CFG_ENABLE);
        write_u32(self.base, CTL, 0);
        if let Err(error) = self.platform.enable_data_path() {
            write_u32(self.base, CFG, cfg);
            let _ = self.platform.disable_data_path();
            return Err(error);
        }
        fence(Ordering::Release);
        write_u32(self.base, ISTE, IS_RUNTIME_MASK);
        write_u32(self.base, ISGE, IS_RUNTIME_MASK);

        self.depth = depth;
        self.free_tags = (0..depth).rev().collect();
        self.cache_enabled = activation.cache_enabled;
        self.active = true;
        self.irq.set_active(true);

        config.limits.max_inflight = depth;
        config.limits.max_submit_batch = depth;
        config.limits.max_segments = 1;
        config.limits.supports_flush = true;
        let info = QueueInfo {
            id: 0,
            device: config.device,
            limits: config.limits,
        };
        self.info = Some(info);
        info!(
            "cqhci: enabled version={:#x} caps={:#x} depth={} task_list={:#x}",
            read_u32(self.base, VER),
            read_u32(self.base, CAP),
            depth,
            task_list
        );
        Ok(())
    }

    fn is_active(&self) -> bool {
        self.active
    }

    fn submit_batch_owned(
        &mut self,
        requests: &mut OwnedRequestBatch,
        sink: &mut dyn SubmissionSink,
    ) -> BatchSubmitResult {
        let mut accepted = 0;
        while accepted < self.depth && !requests.is_empty() {
            match self.stage_next(requests) {
                Ok(id) => {
                    sink.accepted(id);
                    accepted += 1;
                }
                Err(BlkError::Retry) => {
                    return BatchSubmitResult::new(accepted, BatchSubmitDisposition::QueueFull);
                }
                Err(error) => {
                    return BatchSubmitResult::new(accepted, BatchSubmitDisposition::Fatal(error));
                }
            }
        }
        BatchSubmitResult::new(accepted, BatchSubmitDisposition::Continue)
    }

    fn commit_submissions(&mut self) -> Result<(), BlkError> {
        if self.staged == 0 {
            return Ok(());
        }
        fence(Ordering::Release);
        write_u32(self.base, TDBR, self.staged);
        self.staged = 0;
        Ok(())
    }

    fn drain_completions(&mut self, sink: &mut dyn CompletionSink) -> Result<(), BlkError> {
        let (status, completed) = self.irq.take();
        fence(Ordering::Acquire);
        if status & IS_ERROR_MASK != 0 {
            warn!(
                "cqhci: error status={status:#x} terri={:#x} tdpe={:#x}",
                read_u32(self.base, TERRI),
                read_u32(self.base, TDPE)
            );
            return self.recover_all(sink);
        }
        let mut remaining = completed;
        let mut first_error = None;
        while remaining != 0 {
            let tag = remaining.trailing_zeros() as usize;
            remaining &= !(1 << tag);
            if let Err(error) = self.complete_tag(tag, Ok(()), sink) {
                first_error.get_or_insert(error);
            }
        }
        first_error.map_or(Ok(()), Err)
    }

    fn shutdown(&mut self, sink: &mut dyn CompletionSink) -> Result<(), BlkError> {
        self.irq.set_active(false);
        write_u32(self.base, ISGE, 0);
        write_u32(self.base, ISTE, 0);
        let halted = self.halt();
        if halted {
            self.complete_all_after_quiesce(sink);
        } else {
            self.quarantine_all(sink);
        }
        self.platform.disable_data_path()?;
        let cfg = read_u32(self.base, CFG) & !CFG_ENABLE;
        write_u32(self.base, CFG, cfg);
        self.active = false;
        if halted { Ok(()) } else { Err(BlkError::Io) }
    }
}

#[cfg(test)]
mod tests {
    use alloc::{boxed::Box, vec, vec::Vec};
    use core::{
        alloc::Layout,
        num::NonZeroUsize,
        ptr::NonNull,
        sync::atomic::{AtomicU64, Ordering},
    };
    use std::alloc::{alloc_zeroed, dealloc};

    use dma_api::{
        CpuDmaBuffer, DeviceDma, DmaAllocHandle, DmaCoherency, DmaConstraints, DmaDeviceInfo,
        DmaDirection, DmaDomainId, DmaError, DmaMapHandle, DmaOp,
    };
    use rdif_block::{BatchSubmitDisposition, CompletedRequest, RequestFlags, SubmissionSink};
    use sdmmc_protocol::sdio::host::SdMmcIrqHandle;

    use super::*;

    #[test]
    fn batch_doorbell_and_out_of_order_completion_preserve_dma_ownership() {
        let mut registers = [0u32; 32];
        let base = NonNull::new(registers.as_mut_ptr().cast::<u8>()).unwrap();
        let dma = test_dma();
        let (mut queue, mut irq) = unsafe {
            crate::queue_and_irq(base, dma.clone(), Box::new(TestPlatform), TestLegacyIrq)
        }
        .unwrap();
        let mut config = BlockConfig::dma("cqhci-test", 1024, &dma);
        queue
            .activate(
                CommandQueueActivation {
                    rca: 1,
                    card_depth: 2,
                    cache_enabled: true,
                },
                &mut config,
            )
            .unwrap();
        let mut requests = vec![read_request(&dma, 0), read_request(&dma, 1)]
            .into_iter()
            .collect::<OwnedRequestBatch>();
        let mut submitted = SubmittedIds::default();

        let result = queue.submit_batch_owned(&mut requests, &mut submitted);
        assert_eq!(result.accepted(), 2);
        assert_eq!(result.disposition(), BatchSubmitDisposition::Continue);
        assert!(requests.is_empty());
        assert_eq!(submitted.0, vec![RequestId::new(0), RequestId::new(1)]);
        assert_eq!(read_u32(base.as_ptr() as usize, TDBR), 0);

        queue.commit_submissions().unwrap();
        assert_eq!(read_u32(base.as_ptr() as usize, TDBR), 0b11);

        write_u32(base.as_ptr() as usize, IS, IS_TASK_COMPLETE);
        write_u32(base.as_ptr() as usize, TCN, 1 << 1);
        let ack = irq.ack();
        assert!(ack.control_event().is_empty());
        write_u32(base.as_ptr() as usize, IS, 0);
        write_u32(base.as_ptr() as usize, TCN, 0);
        let mut completed = CompletedRequests::default();
        queue.drain_completions(&mut completed).unwrap();
        assert_eq!(completed.0.len(), 1);
        assert_eq!(completed.0[0].id, RequestId::new(1));
        assert!(completed.0[0].result.is_ok());
        assert!(completed.0[0].data.is_some());

        write_u32(base.as_ptr() as usize, IS, IS_TASK_COMPLETE);
        write_u32(base.as_ptr() as usize, TCN, 1);
        let _ = irq.ack();
        write_u32(base.as_ptr() as usize, IS, 0);
        write_u32(base.as_ptr() as usize, TCN, 0);
        queue.drain_completions(&mut completed).unwrap();
        assert_eq!(completed.0.len(), 2);
        assert_eq!(completed.0[1].id, RequestId::new(0));
        assert!(completed.0[1].result.is_ok());
        assert!(completed.0[1].data.is_some());

        write_u32(base.as_ptr() as usize, TDBR, 0);
        let mut barrier_batch = vec![read_request(&dma, 2), flush_request()]
            .into_iter()
            .collect::<OwnedRequestBatch>();
        let result = queue.submit_batch_owned(&mut barrier_batch, &mut submitted);
        assert_eq!(result.accepted(), 1);
        assert_eq!(result.disposition(), BatchSubmitDisposition::QueueFull);
        assert_eq!(barrier_batch.len(), 1);
        assert_eq!(barrier_batch.front().unwrap().op, RequestOp::Flush);
        queue.commit_submissions().unwrap();
        assert_eq!(read_u32(base.as_ptr() as usize, TDBR), 1);

        write_u32(base.as_ptr() as usize, IS, IS_TASK_COMPLETE);
        write_u32(base.as_ptr() as usize, TCN, 1);
        let _ = irq.ack();
        write_u32(base.as_ptr() as usize, IS, 0);
        write_u32(base.as_ptr() as usize, TCN, 0);
        queue.drain_completions(&mut completed).unwrap();

        write_u32(base.as_ptr() as usize, TDBR, 0);
        let result = queue.submit_batch_owned(&mut barrier_batch, &mut submitted);
        assert_eq!(result.accepted(), 1);
        assert_eq!(result.disposition(), BatchSubmitDisposition::Continue);
        queue.commit_submissions().unwrap();
        assert_eq!(
            read_u32(base.as_ptr() as usize, TDBR),
            1 << DIRECT_COMMAND_SLOT
        );

        write_u32(base.as_ptr() as usize, IS, IS_TASK_COMPLETE);
        write_u32(base.as_ptr() as usize, TCN, 1 << DIRECT_COMMAND_SLOT);
        let _ = irq.ack();
        write_u32(base.as_ptr() as usize, IS, 0);
        write_u32(base.as_ptr() as usize, TCN, 0);
        queue.drain_completions(&mut completed).unwrap();
        assert_eq!(completed.0.len(), 4);
        assert!(completed.0[2].data.is_some());
        assert!(completed.0[3].result.is_ok());
        assert!(completed.0[3].data.is_none());
    }

    fn read_request(dma: &DeviceDma, lba: u64) -> OwnedRequest {
        let data = CpuDmaBuffer::new_zero(
            dma,
            NonZeroUsize::new(512).unwrap(),
            512,
            DmaDirection::FromDevice,
        )
        .unwrap()
        .prepare_for_device();
        OwnedRequest {
            op: RequestOp::Read,
            lba,
            block_count: 1,
            data: Some(data),
            flags: RequestFlags::NONE,
        }
    }

    fn flush_request() -> OwnedRequest {
        OwnedRequest {
            op: RequestOp::Flush,
            lba: 0,
            block_count: 0,
            data: None,
            flags: RequestFlags::NONE,
        }
    }

    #[derive(Default)]
    struct SubmittedIds(Vec<RequestId>);

    impl SubmissionSink for SubmittedIds {
        fn accepted(&mut self, id: RequestId) {
            self.0.push(id);
        }
    }

    #[derive(Default)]
    struct CompletedRequests(Vec<CompletedRequest>);

    impl CompletionSink for CompletedRequests {
        fn complete(&mut self, request: CompletedRequest) {
            self.0.push(request);
        }
    }

    struct TestPlatform;

    impl CqhciPlatform for TestPlatform {
        fn prepare(&mut self, _cqhci_base: usize) -> Result<(), BlkError> {
            Ok(())
        }

        fn enable_data_path(&mut self) -> Result<(), BlkError> {
            Ok(())
        }

        fn disable_data_path(&mut self) -> Result<(), BlkError> {
            Ok(())
        }
    }

    struct TestLegacyIrq;

    impl SdMmcIrqHandle for TestLegacyIrq {
        type Event = ();

        fn handle_irq(&mut self) -> Self::Event {}
    }

    struct TestDma;

    static TEST_DMA: TestDma = TestDma;
    static NEXT_DMA_ADDRESS: AtomicU64 = AtomicU64::new(0x1000_0000);

    impl DmaOp for TestDma {
        fn page_size(&self) -> usize {
            4096
        }

        unsafe fn alloc_contiguous(
            &self,
            constraints: DmaConstraints,
            layout: Layout,
        ) -> Option<DmaAllocHandle> {
            allocate_test_dma(constraints, layout)
        }

        unsafe fn dealloc_contiguous(&self, handle: DmaAllocHandle) {
            unsafe { dealloc(handle.as_ptr().as_ptr(), handle.layout()) };
        }

        unsafe fn alloc_coherent(
            &self,
            constraints: DmaConstraints,
            layout: Layout,
        ) -> Option<DmaAllocHandle> {
            allocate_test_dma(constraints, layout)
        }

        unsafe fn dealloc_coherent(&self, handle: DmaAllocHandle) -> Result<(), DmaError> {
            unsafe { self.dealloc_contiguous(handle) };
            Ok(())
        }

        unsafe fn map_streaming(
            &self,
            _constraints: DmaConstraints,
            _addr: NonNull<u8>,
            _size: NonZeroUsize,
            _direction: DmaDirection,
        ) -> Result<DmaMapHandle, DmaError> {
            Err(DmaError::NoMemory)
        }

        unsafe fn unmap_streaming(&self, _handle: DmaMapHandle) {}
    }

    fn allocate_test_dma(constraints: DmaConstraints, layout: Layout) -> Option<DmaAllocHandle> {
        let alignment = constraints.align.max(layout.align());
        let layout = Layout::from_size_align(layout.size(), alignment).ok()?;
        // SAFETY: `layout` is non-zero and valid; the returned pointer is
        // released with the same layout by the test DMA backend.
        let ptr = NonNull::new(unsafe { alloc_zeroed(layout) })?;
        let dma_address = NEXT_DMA_ADDRESS.fetch_add(0x0100_0000, Ordering::Relaxed);
        // SAFETY: this fake device defines the synthetic aligned bus address
        // as the mapping of the live allocation for the duration of the test.
        Some(unsafe { DmaAllocHandle::new(ptr, ptr, dma_address.into(), layout) })
    }

    fn test_dma() -> DeviceDma {
        DeviceDma::new(
            DmaDeviceInfo::new(
                DmaDomainId::Direct,
                DmaCoherency::NonCoherent,
                DmaConstraints::new(u32::MAX as u64),
            ),
            &TEST_DMA,
        )
    }
}
