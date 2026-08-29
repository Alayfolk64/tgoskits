use alloc::{format, vec::Vec};
use core::time::Duration;

use dw_wdt::{DwWatchdog, Error as DwWatchdogError, MMIO_SIZE, NUM_TIMEOUTS};
use log::info;
use mmio_api::{MmioAddr, MmioRaw};
use rdif_watchdog::{
    ArmedTimeout, DriverGeneric, Interface as WatchdogInterface, Watchdog, WatchdogError,
};
use rdrive::{
    probe::{OnProbeError, fdt::ResourcePrepareConfig},
    register::ProbeFdt,
};

use crate::mmio::iomap;

crate::model_register!(
    name: "Synopsys DesignWare watchdog",
    level: ProbeLevel::PostKernel,
    priority: ProbePriority::DEFAULT,
    probe_kinds: &[
        ProbeKind::Fdt {
            compatibles: &["snps,dw-wdt"],
            on_probe: probe
        }
    ],
);

struct DesignWareWatchdogDevice {
    inner: DwWatchdog,
}

impl DriverGeneric for DesignWareWatchdogDevice {
    fn name(&self) -> &str {
        "dw-wdt"
    }
}

impl WatchdogInterface for DesignWareWatchdogDevice {
    fn arm_reset(&mut self, timeout: Duration) -> Result<ArmedTimeout, WatchdogError> {
        let actual = self.inner.arm_reset(timeout).map_err(map_driver_error)?;
        Ok(ArmedTimeout::new(timeout, actual))
    }

    fn ping(&mut self) -> Result<(), WatchdogError> {
        self.inner.ping().map_err(map_driver_error)
    }

    fn programmed_timeout(&self) -> Result<Duration, WatchdogError> {
        self.inner.programmed_timeout().map_err(map_driver_error)
    }

    fn time_left(&self) -> Result<Duration, WatchdogError> {
        self.inner.time_left().map_err(map_driver_error)
    }

    fn is_armed(&self) -> bool {
        self.inner.is_armed()
    }
}

fn map_driver_error(error: DwWatchdogError) -> WatchdogError {
    match error {
        DwWatchdogError::TimeoutOutOfRange => WatchdogError::InvalidTimeout,
        DwWatchdogError::NotArmed => WatchdogError::NotArmed,
        DwWatchdogError::MmioTooSmall
        | DwWatchdogError::InvalidClockRate
        | DwWatchdogError::MissingTimeoutCounts
        | DwWatchdogError::InvalidTimeoutCount => WatchdogError::Hardware,
    }
}

fn probe(probe: ProbeFdt<'_>) -> Result<(), OnProbeError> {
    let (info, platform_device) = probe.into_parts();
    let register = info
        .node
        .regs()
        .into_iter()
        .next()
        .ok_or_else(|| OnProbeError::other(format!("[{}] has no reg", info.node.name())))?;
    let mmio_size = register.size.unwrap_or(MMIO_SIZE as u64) as usize;
    if mmio_size < MMIO_SIZE {
        return Err(OnProbeError::other(format!(
            "[{}] watchdog MMIO size {mmio_size:#x} is smaller than {MMIO_SIZE:#x}",
            info.node.name()
        )));
    }

    let resources = info.prepare_resources(
        ResourcePrepareConfig::default()
            .without_assigned_clocks()
            .with_named_clock_rate("tclk"),
    )?;
    let timer_clock_hz = resources.clock_rate("tclk").ok_or_else(|| {
        OnProbeError::other(format!("[{}] missing tclk clock rate", info.node.name()))
    })?;
    if timer_clock_hz == 0 {
        return Err(OnProbeError::other(format!(
            "[{}] tclk clock rate is zero",
            info.node.name()
        )));
    }

    let custom_timeout_counts = custom_timeout_counts(info.node.as_node())?;
    let mapped = iomap(register.address as usize, mmio_size)?;
    // SAFETY: `iomap` returned a valid mapping for this FDT register window.
    // rdrive retains the registered device for the kernel lifetime, so the raw
    // mapping remains owned and does not outlive the platform device.
    let mmio = unsafe { MmioRaw::new(MmioAddr::from(register.address), mapped, mmio_size) };
    let inner = DwWatchdog::new(mmio, timer_clock_hz, custom_timeout_counts).map_err(|error| {
        OnProbeError::other(format!(
            "[{}] failed to initialize DesignWare watchdog: {error}",
            info.node.name()
        ))
    })?;

    platform_device.register(Watchdog::new(DesignWareWatchdogDevice { inner }));
    info!(
        "DesignWare watchdog registered: node={} base={:#x} tclk={}Hz",
        info.node.name(),
        register.address,
        timer_clock_hz
    );
    Ok(())
}

fn custom_timeout_counts(
    node: &fdt_edit::Node,
) -> Result<Option<[u32; NUM_TIMEOUTS]>, OnProbeError> {
    let Some(property) = node.get_property("snps,watchdog-tops") else {
        return Ok(None);
    };
    let values = property.get_u32_iter().collect::<Vec<_>>();
    let count = values.len();
    let counts = values.try_into().map_err(|_| {
        OnProbeError::other(format!(
            "[{}] snps,watchdog-tops has {count} values, expected {NUM_TIMEOUTS}",
            node.name()
        ))
    })?;
    Ok(Some(counts))
}

#[derive(thiserror::Error, Debug)]
pub enum ControlError {
    #[error("no hardware watchdog is registered")]
    NotFound,
    #[error("hardware watchdog is unavailable")]
    Unavailable,
    #[error(transparent)]
    Operation(#[from] WatchdogError),
}

pub fn arm_reset(timeout: Duration) -> Result<ArmedTimeout, ControlError> {
    let device = rdrive::get_one::<Watchdog>().ok_or(ControlError::NotFound)?;
    let mut watchdog = device.lock().map_err(|_| ControlError::Unavailable)?;
    watchdog.arm_reset(timeout).map_err(ControlError::from)
}

pub fn ping() -> Result<(), ControlError> {
    let device = rdrive::get_one::<Watchdog>().ok_or(ControlError::NotFound)?;
    let mut watchdog = device.lock().map_err(|_| ControlError::Unavailable)?;
    watchdog.ping().map_err(ControlError::from)
}

pub fn time_left() -> Result<Duration, ControlError> {
    let device = rdrive::get_one::<Watchdog>().ok_or(ControlError::NotFound)?;
    let watchdog = device.lock().map_err(|_| ControlError::Unavailable)?;
    watchdog.time_left().map_err(ControlError::from)
}
