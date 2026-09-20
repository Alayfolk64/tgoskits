use alloc::{boxed::Box, sync::Arc};

use rdif_block::{
    BlkError, BlockController, ControllerEvent, ControllerUpdate, DeviceInfo, DriverGeneric,
    HardIrqHandler, IrqAck,
};

use crate::{IrqBindingLease, IrqSourceGate};

/// Couples a portable controller to the lifetime of its platform IRQ binding.
pub struct IrqBoundBlock<T, L> {
    inner: T,
    irq_lease: L,
}

impl<T, L> IrqBoundBlock<T, L> {
    /// Creates a controller whose IRQ allocation is released on drop.
    pub const fn new(inner: T, irq_lease: L) -> Self {
        Self { inner, irq_lease }
    }

    fn bind_irq_endpoints(&self, update: ControllerUpdate) -> ControllerUpdate
    where
        L: IrqBindingLease,
    {
        update.map_irq_endpoints(|endpoint| {
            let Some(gate) = self.irq_lease.source_gate(endpoint.source_id()) else {
                return endpoint;
            };
            endpoint.map_handler(|inner| Box::new(GatedIrqHandler { inner, gate }))
        })
    }
}

struct GatedIrqHandler {
    inner: Box<dyn HardIrqHandler>,
    gate: Arc<dyn IrqSourceGate>,
}

impl HardIrqHandler for GatedIrqHandler {
    fn ack(&mut self) -> IrqAck {
        self.gate.mask();
        let ack = self.inner.ack();
        if ack.is_spurious() {
            self.gate.unmask();
            ack
        } else {
            IrqAck::masked_needs_rearm(ack.queues(), ack.control_event())
        }
    }
}

impl<T: BlockController, L: IrqBindingLease> DriverGeneric for IrqBoundBlock<T, L> {
    fn name(&self) -> &str {
        self.inner.name()
    }

    fn raw_any(&self) -> Option<&dyn core::any::Any> {
        self.inner.raw_any()
    }

    fn raw_any_mut(&mut self) -> Option<&mut dyn core::any::Any> {
        self.inner.raw_any_mut()
    }
}

impl<T: BlockController, L: IrqBindingLease> BlockController for IrqBoundBlock<T, L> {
    fn device_info(&self) -> DeviceInfo {
        self.inner.device_info()
    }

    fn max_io_queues(&self) -> usize {
        self.inner.max_io_queues()
    }

    fn advance(&mut self, event: ControllerEvent) -> Result<ControllerUpdate, BlkError> {
        let update = match event {
            ControllerEvent::Rearm { source_id } => {
                let update = self.inner.advance(event)?;
                self.irq_lease.enable_binding_source(source_id);
                update
            }
            ControllerEvent::QuiesceIrqs
            | ControllerEvent::Watchdog { .. }
            | ControllerEvent::Shutdown => {
                self.irq_lease.disable_binding_irq();
                self.inner.advance(event)?
            }
            _ => self.inner.advance(event)?,
        };
        Ok(self.bind_irq_endpoints(update))
    }
}
