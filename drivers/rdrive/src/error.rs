use alloc::{boxed::Box, format, string::String};

use fdt_raw::FdtError;

use crate::Phandle;

#[derive(thiserror::Error, Debug)]
pub enum DriverError {
    #[error("FDT error: {0}")]
    Fdt(String),
    #[error("unsupported driver source: {0}")]
    Unsupported(&'static str),
    #[error("Unknown driver error: {0}")]
    Unknown(String),
}

/// Failure to attach a driver capability to an FDT provider identity.
#[derive(thiserror::Error, Debug)]
pub enum RegisterFdtPhandleError {
    /// The FDT used to initialize rdrive did not define the requested phandle.
    #[error("FDT phandle {phandle:?} has no device identity")]
    UnknownPhandle { phandle: Phandle },
}

impl From<FdtError> for DriverError {
    fn from(value: FdtError) -> Self {
        Self::Fdt(format!("{value:?}"))
    }
}

impl From<Box<dyn core::error::Error>> for DriverError {
    fn from(value: Box<dyn core::error::Error>) -> Self {
        Self::Unknown(format!("{value:?}"))
    }
}
