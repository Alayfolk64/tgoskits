//! Hard-IRQ acknowledgement for a CQHCI engine sharing an SD/MMC IRQ line.

use alloc::sync::Arc;
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use rdif_block::{ControlEvent, HardIrqHandler, IrqAck, IrqQueueMask};
use sdmmc_protocol::sdio::host::{HostEvent, HostEventKind, SdMmcIrqHandle};

use crate::{read_u32, regs::*, write_u32};

pub(crate) struct IrqState {
    base: usize,
    active: AtomicBool,
    status: AtomicU32,
    completed: AtomicU32,
}

impl IrqState {
    pub(crate) fn new(base: usize) -> Self {
        Self {
            base,
            active: AtomicBool::new(false),
            status: AtomicU32::new(0),
            completed: AtomicU32::new(0),
        }
    }

    pub(crate) fn set_active(&self, active: bool) {
        self.active.store(active, Ordering::Release);
    }

    fn is_active(&self) -> bool {
        self.active.load(Ordering::Acquire)
    }

    pub(crate) fn take(&self) -> (u32, u32) {
        let status = self.status.swap(0, Ordering::AcqRel);
        let completed = self.completed.swap(0, Ordering::AcqRel);
        (status, completed)
    }

    fn record_legacy_error(&self) {
        self.status.fetch_or(IS_RESPONSE_ERROR, Ordering::Release);
    }

    fn acknowledge(&self) -> bool {
        if !self.is_active() {
            return false;
        }
        let status = read_u32(self.base, IS);
        if status == 0 {
            return false;
        }
        write_u32(self.base, IS, status);
        let completed = if status & IS_TASK_COMPLETE != 0 {
            let completed = read_u32(self.base, TCN);
            write_u32(self.base, TCN, completed);
            completed
        } else {
            0
        };
        self.completed.fetch_or(completed, Ordering::Release);
        self.status.fetch_or(status, Ordering::Release);
        true
    }
}

/// IRQ endpoint that acknowledges both legacy SDHCI and CQHCI state.
pub struct CqhciIrqHandle<L> {
    legacy: L,
    state: Arc<IrqState>,
}

impl<L> CqhciIrqHandle<L> {
    pub(crate) fn new(legacy: L, state: Arc<IrqState>) -> Self {
        Self { legacy, state }
    }
}

impl<L> HardIrqHandler for CqhciIrqHandle<L>
where
    L: SdMmcIrqHandle,
{
    fn ack(&mut self) -> IrqAck {
        let legacy = self.legacy.handle_irq();
        let cqhci = self.state.acknowledge();
        if legacy.kind() == HostEventKind::Error {
            self.state.record_legacy_error();
        }
        if !cqhci
            && matches!(
                legacy.kind(),
                HostEventKind::None | HostEventKind::CardInterrupt
            )
        {
            return IrqAck::spurious(0);
        }
        let control_bits = u64::from(!self.state.is_active());
        IrqAck::cleared(
            IrqQueueMask::from_queue(0),
            ControlEvent::new(0, control_bits),
        )
    }
}
