#![no_std]

extern crate alloc;

use core::time::Duration;

pub use rdif_base::DriverGeneric;
use rdif_base::def_driver;

/// The timeout selected by a watchdog after applying its hardware granularity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ArmedTimeout {
    requested: Duration,
    actual: Duration,
}

impl ArmedTimeout {
    pub const fn new(requested: Duration, actual: Duration) -> Self {
        Self { requested, actual }
    }

    pub const fn requested(self) -> Duration {
        self.requested
    }

    pub const fn actual(self) -> Duration {
        self.actual
    }
}

#[derive(thiserror::Error, Clone, Copy, Debug, PartialEq, Eq)]
pub enum WatchdogError {
    #[error("watchdog timeout is invalid or not representable")]
    InvalidTimeout,
    #[error("watchdog is not armed")]
    NotArmed,
    #[error("watchdog operation is unsupported")]
    Unsupported,
    #[error("watchdog hardware access failed")]
    Hardware,
}

/// Reset-mode watchdog operations needed by kernel recovery policies.
pub trait Interface: DriverGeneric {
    /// Programs and starts the watchdog in reset mode.
    ///
    /// Hardware is allowed to round the requested duration up. The returned
    /// value records both durations so the runtime can log the effective reset
    /// deadline.
    fn arm_reset(&mut self, timeout: Duration) -> Result<ArmedTimeout, WatchdogError>;

    /// Reloads an armed watchdog counter.
    fn ping(&mut self) -> Result<(), WatchdogError>;

    /// Returns the currently programmed timeout.
    fn programmed_timeout(&self) -> Result<Duration, WatchdogError>;

    /// Returns the remaining duration reported by hardware.
    fn time_left(&self) -> Result<Duration, WatchdogError>;

    fn is_armed(&self) -> bool;
}

def_driver!(Watchdog, Interface);

#[cfg(test)]
mod tests {
    use super::*;

    struct RecordingWatchdog {
        armed: bool,
        programmed: Duration,
        remaining: Duration,
        pings: usize,
    }

    impl DriverGeneric for RecordingWatchdog {
        fn name(&self) -> &str {
            "recording-watchdog"
        }
    }

    impl Interface for RecordingWatchdog {
        fn arm_reset(&mut self, timeout: Duration) -> Result<ArmedTimeout, WatchdogError> {
            self.armed = true;
            self.programmed = timeout + Duration::from_secs(1);
            Ok(ArmedTimeout::new(timeout, self.programmed))
        }

        fn ping(&mut self) -> Result<(), WatchdogError> {
            if !self.armed {
                return Err(WatchdogError::NotArmed);
            }
            self.pings += 1;
            Ok(())
        }

        fn programmed_timeout(&self) -> Result<Duration, WatchdogError> {
            self.armed
                .then_some(self.programmed)
                .ok_or(WatchdogError::NotArmed)
        }

        fn time_left(&self) -> Result<Duration, WatchdogError> {
            self.armed
                .then_some(self.remaining)
                .ok_or(WatchdogError::NotArmed)
        }

        fn is_armed(&self) -> bool {
            self.armed
        }
    }

    #[test]
    fn wrapper_dispatches_reset_mode_operations() {
        let mut watchdog = Watchdog::new(RecordingWatchdog {
            armed: false,
            programmed: Duration::ZERO,
            remaining: Duration::from_secs(7),
            pings: 0,
        });

        assert_eq!(watchdog.ping(), Err(WatchdogError::NotArmed));
        let armed = watchdog.arm_reset(Duration::from_secs(5)).unwrap();
        assert_eq!(armed.requested(), Duration::from_secs(5));
        assert_eq!(armed.actual(), Duration::from_secs(6));
        assert_eq!(watchdog.programmed_timeout(), Ok(Duration::from_secs(6)));
        assert_eq!(watchdog.time_left(), Ok(Duration::from_secs(7)));
        watchdog.ping().unwrap();

        let inner = watchdog.typed_ref::<RecordingWatchdog>().unwrap();
        assert!(inner.armed);
        assert_eq!(inner.pings, 1);
    }
}
