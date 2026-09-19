//! Portable Command Queue Host Controller Interface (CQHCI) driver core.

#![no_std]

extern crate alloc;
#[cfg(test)]
extern crate std;

use alloc::{boxed::Box, sync::Arc};
use core::ptr::NonNull;

use dma_api::DeviceDma;
use rdif_block::{BlkError, HardIrqHandler};
use sdmmc_protocol::sdio::host::SdMmcIrqHandle;

mod descriptor;
mod irq;
mod queue;
mod regs;

pub use queue::CqhciQueue;

/// Platform operations around the standardized CQHCI register block.
pub trait CqhciPlatform: Send + 'static {
    /// Programs vendor state required before CQHCI is enabled.
    fn prepare(&mut self, cqhci_base: usize) -> Result<(), BlkError>;

    /// Routes the SDHCI data path and shared IRQ to CQHCI.
    fn enable_data_path(&mut self) -> Result<(), BlkError>;

    /// Restores a quiesced legacy data path during shutdown or recovery.
    fn disable_data_path(&mut self) -> Result<(), BlkError>;
}

/// Constructs a CQHCI accelerator and the matching shared IRQ endpoint.
///
/// # Safety
///
/// `base` must cover the exclusively owned CQHCI register block belonging to
/// the same physical host as `legacy_irq` and `platform`.
pub unsafe fn queue_and_irq<L>(
    base: NonNull<u8>,
    dma: DeviceDma,
    platform: Box<dyn CqhciPlatform>,
    legacy_irq: L,
) -> Result<(CqhciQueue, Box<dyn HardIrqHandler>), BlkError>
where
    L: SdMmcIrqHandle,
{
    let (queue, irq_state) = unsafe { CqhciQueue::new(base, dma, platform)? };
    let irq = irq::CqhciIrqHandle::new(legacy_irq, Arc::clone(&irq_state));
    Ok((queue, Box::new(irq)))
}

pub(crate) fn read_u32(base: usize, offset: usize) -> u32 {
    // SAFETY: every caller holds the lifetime ownership established by the
    // constructor and accesses an aligned CQHCI register.
    unsafe { core::ptr::read_volatile((base + offset) as *const u32) }
}

pub(crate) fn write_u32(base: usize, offset: usize, value: u32) {
    // SAFETY: every caller holds the lifetime ownership established by the
    // constructor and accesses an aligned CQHCI register.
    unsafe { core::ptr::write_volatile((base + offset) as *mut u32, value) }
}
