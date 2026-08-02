use pcie::PcieController;
pub use rdif_base::DriverGeneric;

use crate::{Descriptor, Phandle, error::RegisterFdtPhandleError};

pub struct Empty;

impl DriverGeneric for Empty {
    fn name(&self) -> &str {
        "Empty Driver"
    }
}

pub struct PlatformDevice {
    pub descriptor: Descriptor,
}

impl PlatformDevice {
    pub(crate) fn new(descriptor: Descriptor) -> Self {
        Self { descriptor }
    }

    pub fn descriptor(&self) -> &Descriptor {
        &self.descriptor
    }

    /// Register a device to the driver manager.
    ///
    /// # Panics
    /// This method will panic if the device with the same ID is already added
    pub fn register<T: DriverGeneric>(&self, driver: T) {
        crate::edit(|manager| {
            manager
                .dev_container
                .insert(self.descriptor.clone(), driver);
        });
    }

    /// Registers a capability against an existing FDT provider phandle.
    ///
    /// This is intended for firmware transports whose child protocol node is
    /// the provider referenced by consumers, while the parent transport node
    /// owns probe and initialization.
    ///
    /// # Errors
    ///
    /// Returns [`RegisterFdtPhandleError::UnknownPhandle`] when `phandle` was
    /// not present in the FDT used to initialize rdrive.
    ///
    /// # Panics
    ///
    /// Panics when the same interface type is registered twice for the target
    /// phandle, matching [`Self::register`].
    pub fn register_fdt_phandle<T: DriverGeneric>(
        &self,
        phandle: Phandle,
        driver: T,
    ) -> Result<(), RegisterFdtPhandleError> {
        let device_id = crate::fdt_phandle_to_device_id(phandle)
            .ok_or(RegisterFdtPhandleError::UnknownPhandle { phandle })?;
        let mut descriptor = self.descriptor.clone();
        descriptor.device_id = device_id;
        descriptor.irq_parent = None;
        crate::edit(|manager| manager.dev_container.insert(descriptor, driver));
        Ok(())
    }

    pub fn register_pcie(&self, drv: PcieController) {
        crate::edit(|manager| {
            manager.dev_container.insert(self.descriptor.clone(), drv);
        });
    }
}
