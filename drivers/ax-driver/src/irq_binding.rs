use alloc::sync::Arc;

use crate::BindingInfo;

/// IRQ-safe control for one dedicated platform interrupt source.
///
/// Implementations must not allocate, sleep, acquire a sleepable lock, or
/// consult a device registry. The source mapping and MMIO lifetime must remain
/// owned by this object until every wrapped hard-IRQ handler is dropped.
pub trait IrqSourceGate: Send + Sync + 'static {
    /// Prevents new deliveries of this source before the top half publishes
    /// deferred work.
    fn mask(&self);

    /// Restores delivery without repeating source discovery or configuration.
    /// It is safe both for a hard-IRQ spurious-claim rollback and for the
    /// deferred task's drain-then-rearm transition.
    fn unmask(&self);
}

pub trait IrqBindingLease: Send + 'static {
    fn binding_info(&self) -> BindingInfo;

    fn enable_binding_irq(&self);

    fn enable_binding_source(&self, _source_id: usize) {
        self.enable_binding_irq();
    }

    /// Returns the pre-resolved IRQ-safe gate for a dedicated source.
    fn source_gate(&self, _source_id: usize) -> Option<Arc<dyn IrqSourceGate>> {
        None
    }

    fn disable_binding_irq(&self);
}
