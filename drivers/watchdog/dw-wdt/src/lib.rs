#![no_std]

use core::time::Duration;

use mmio_api::MmioRaw;

pub const MMIO_SIZE: usize = 0x100;
pub const NUM_TIMEOUTS: usize = 16;

const CONTROL: usize = 0x00;
const CONTROL_ENABLE: u32 = 1 << 0;
const CONTROL_RESPONSE_MODE: u32 = 1 << 1;
const TIMEOUT_RANGE: usize = 0x04;
const TIMEOUT_INITIAL_SHIFT: u32 = 4;
const CURRENT_COUNT: usize = 0x08;
const COUNTER_RESTART: usize = 0x0c;
const COUNTER_RESTART_KICK: u32 = 0x76;
const COMPONENT_PARAMETERS_1: usize = 0xf4;
const COMPONENT_PARAMETERS_1_FIXED_TOP: u32 = 1 << 6;

const FIXED_TIMEOUT_COUNTS: [u32; NUM_TIMEOUTS] = [
    1 << 16,
    1 << 17,
    1 << 18,
    1 << 19,
    1 << 20,
    1 << 21,
    1 << 22,
    1 << 23,
    1 << 24,
    1 << 25,
    1 << 26,
    1 << 27,
    1 << 28,
    1 << 29,
    1 << 30,
    1 << 31,
];

#[derive(thiserror::Error, Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    #[error("DesignWare watchdog MMIO window is smaller than 0x100 bytes")]
    MmioTooSmall,
    #[error("DesignWare watchdog timer clock rate is zero")]
    InvalidClockRate,
    #[error("custom DesignWare watchdog timeout counts are required")]
    MissingTimeoutCounts,
    #[error("DesignWare watchdog timeout count is zero")]
    InvalidTimeoutCount,
    #[error("watchdog timeout must be non-zero and no greater than the hardware maximum")]
    TimeoutOutOfRange,
    #[error("watchdog is not armed")]
    NotArmed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct TimeoutEntry {
    top: u8,
    count: u32,
    duration: Duration,
}

impl TimeoutEntry {
    const EMPTY: Self = Self {
        top: 0,
        count: 0,
        duration: Duration::ZERO,
    };
}

/// Register-level Synopsys DesignWare watchdog controller.
///
/// Once armed, this IP cannot generally be stopped without asserting its reset
/// line. This type deliberately exposes no stop operation.
pub struct DwWatchdog {
    mmio: MmioRaw,
    clock_hz: u64,
    timeouts: [TimeoutEntry; NUM_TIMEOUTS],
}

impl DwWatchdog {
    /// Creates a controller from an existing kernel-lifetime MMIO mapping.
    ///
    /// `custom_timeout_counts` is required only when the synthesized component
    /// reports that fixed TOP values are disabled. Each array element is the
    /// counter value selected by the corresponding raw TOP index.
    pub fn new(
        mmio: MmioRaw,
        clock_hz: u64,
        custom_timeout_counts: Option<[u32; NUM_TIMEOUTS]>,
    ) -> Result<Self, Error> {
        if mmio.size() < MMIO_SIZE {
            return Err(Error::MmioTooSmall);
        }
        if clock_hz == 0 {
            return Err(Error::InvalidClockRate);
        }

        let parameters = mmio.read::<u32>(COMPONENT_PARAMETERS_1);
        let counts = if parameters & COMPONENT_PARAMETERS_1_FIXED_TOP != 0 {
            FIXED_TIMEOUT_COUNTS
        } else {
            custom_timeout_counts.ok_or(Error::MissingTimeoutCounts)?
        };
        let timeouts = sorted_timeouts(counts, clock_hz)?;

        Ok(Self {
            mmio,
            clock_hz,
            timeouts,
        })
    }

    /// Programs the closest representable timeout not shorter than `requested`
    /// and starts the controller in reset mode.
    pub fn arm_reset(&mut self, requested: Duration) -> Result<Duration, Error> {
        let selected = self.select_timeout(requested)?;
        let top = u32::from(selected.top);

        let reset_mode = self.mmio.read::<u32>(CONTROL) & !CONTROL_RESPONSE_MODE;
        self.mmio.write(CONTROL, reset_mode);
        self.mmio
            .write(TIMEOUT_RANGE, top | (top << TIMEOUT_INITIAL_SHIFT));
        self.reload_counter();
        self.mmio.write(CONTROL, reset_mode | CONTROL_ENABLE);

        Ok(selected.duration)
    }

    pub fn ping(&mut self) -> Result<(), Error> {
        if !self.is_armed() {
            return Err(Error::NotArmed);
        }
        self.reload_counter();
        Ok(())
    }

    pub fn is_armed(&self) -> bool {
        self.mmio.read::<u32>(CONTROL) & CONTROL_ENABLE != 0
    }

    pub fn programmed_timeout(&self) -> Result<Duration, Error> {
        if !self.is_armed() {
            return Err(Error::NotArmed);
        }
        let selected_top = (self.mmio.read::<u32>(TIMEOUT_RANGE) & 0xf) as u8;
        self.timeouts
            .iter()
            .find(|entry| entry.top == selected_top)
            .map(|entry| entry.duration)
            .ok_or(Error::TimeoutOutOfRange)
    }

    pub fn time_left(&self) -> Result<Duration, Error> {
        if !self.is_armed() {
            return Err(Error::NotArmed);
        }
        Ok(duration_from_count(
            self.mmio.read::<u32>(CURRENT_COUNT),
            self.clock_hz,
        ))
    }

    fn select_timeout(&self, requested: Duration) -> Result<TimeoutEntry, Error> {
        if requested.is_zero() || requested > self.timeouts[NUM_TIMEOUTS - 1].duration {
            return Err(Error::TimeoutOutOfRange);
        }

        self.timeouts
            .iter()
            .copied()
            .find(|entry| entry.duration >= requested)
            .ok_or(Error::TimeoutOutOfRange)
    }

    fn reload_counter(&self) {
        self.mmio.write(COUNTER_RESTART, COUNTER_RESTART_KICK);
    }
}

fn sorted_timeouts(
    counts: [u32; NUM_TIMEOUTS],
    clock_hz: u64,
) -> Result<[TimeoutEntry; NUM_TIMEOUTS], Error> {
    let mut entries = [TimeoutEntry::EMPTY; NUM_TIMEOUTS];

    for (top, count) in counts.into_iter().enumerate() {
        if count == 0 {
            return Err(Error::InvalidTimeoutCount);
        }
        let entry = TimeoutEntry {
            top: top as u8,
            count,
            duration: duration_from_count(count, clock_hz),
        };
        let mut position = top;
        while position > 0 && entry.duration < entries[position - 1].duration {
            entries[position] = entries[position - 1];
            position -= 1;
        }
        entries[position] = entry;
    }

    Ok(entries)
}

fn duration_from_count(count: u32, clock_hz: u64) -> Duration {
    let count = u64::from(count);
    let seconds = count / clock_hz;
    let remainder = count % clock_hz;
    let nanos = (u128::from(remainder) * 1_000_000_000 / u128::from(clock_hz)) as u32;
    Duration::new(seconds, nanos)
}

#[cfg(test)]
mod tests {
    extern crate alloc;

    use alloc::boxed::Box;
    use core::ptr::NonNull;

    use mmio_api::MmioAddr;

    use super::*;

    const WORDS: usize = MMIO_SIZE / size_of::<u32>();

    fn device_with_counts(
        clock_hz: u64,
        fixed: bool,
        custom: Option<[u32; NUM_TIMEOUTS]>,
    ) -> (DwWatchdog, Box<[u32; WORDS]>) {
        let mut registers = Box::new([0_u32; WORDS]);
        if fixed {
            registers[COMPONENT_PARAMETERS_1 / 4] = COMPONENT_PARAMETERS_1_FIXED_TOP;
        }
        let base = NonNull::new(registers.as_mut_ptr().cast::<u8>()).unwrap();
        let mmio = unsafe { MmioRaw::new(MmioAddr::from(0_u64), base, MMIO_SIZE) };
        let watchdog = DwWatchdog::new(mmio, clock_hz, custom).unwrap();
        (watchdog, registers)
    }

    #[test]
    fn arm_rounds_up_and_programs_reset_mode_sequence() {
        let (mut watchdog, mut registers) = device_with_counts(1 << 16, true, None);
        registers[CONTROL / 4] = CONTROL_RESPONSE_MODE;

        let actual = watchdog.arm_reset(Duration::from_millis(1500)).unwrap();

        assert_eq!(actual, Duration::from_secs(2));
        assert_eq!(registers[TIMEOUT_RANGE / 4], 0x11);
        assert_eq!(registers[COUNTER_RESTART / 4], COUNTER_RESTART_KICK);
        assert_eq!(registers[CONTROL / 4] & CONTROL_ENABLE, CONTROL_ENABLE);
        assert_eq!(registers[CONTROL / 4] & CONTROL_RESPONSE_MODE, 0);
        assert_eq!(watchdog.programmed_timeout(), Ok(Duration::from_secs(2)));
    }

    #[test]
    fn ping_requires_an_armed_controller() {
        let (mut watchdog, _) = device_with_counts(1 << 16, true, None);
        assert_eq!(watchdog.ping(), Err(Error::NotArmed));
    }

    #[test]
    fn invalid_requested_timeouts_are_rejected() {
        let (mut watchdog, _) = device_with_counts(1 << 16, true, None);
        assert_eq!(
            watchdog.arm_reset(Duration::ZERO),
            Err(Error::TimeoutOutOfRange)
        );
        assert_eq!(
            watchdog.arm_reset(Duration::from_secs(1 << 16)),
            Err(Error::TimeoutOutOfRange)
        );
    }

    #[test]
    fn custom_timeout_counts_are_sorted_without_losing_top_values() {
        let mut counts = FIXED_TIMEOUT_COUNTS;
        counts.swap(0, 1);
        let (mut watchdog, registers) = device_with_counts(1 << 16, false, Some(counts));

        assert_eq!(
            watchdog.arm_reset(Duration::from_millis(500)),
            Ok(Duration::from_secs(1))
        );
        assert_eq!(registers[TIMEOUT_RANGE / 4], 0x11);
    }

    #[test]
    fn custom_timeout_counts_are_required_when_fixed_top_is_disabled() {
        let mut registers = Box::new([0_u32; WORDS]);
        let base = NonNull::new(registers.as_mut_ptr().cast::<u8>()).unwrap();
        let mmio = unsafe { MmioRaw::new(MmioAddr::from(0_u64), base, MMIO_SIZE) };

        assert_eq!(
            DwWatchdog::new(mmio, 24_000_000, None).err(),
            Some(Error::MissingTimeoutCounts)
        );
    }

    #[test]
    fn constructor_rejects_an_undersized_mmio_window() {
        let mut registers = Box::new([0_u32; WORDS]);
        let base = NonNull::new(registers.as_mut_ptr().cast::<u8>()).unwrap();
        let mmio = unsafe { MmioRaw::new(MmioAddr::from(0_u64), base, MMIO_SIZE - 1) };

        assert_eq!(
            DwWatchdog::new(mmio, 24_000_000, None).err(),
            Some(Error::MmioTooSmall)
        );
    }

    #[test]
    fn time_left_uses_the_timer_clock() {
        let (mut watchdog, mut registers) = device_with_counts(1 << 16, true, None);
        watchdog.arm_reset(Duration::from_secs(1)).unwrap();
        registers[CURRENT_COUNT / 4] = (1 << 16) + (1 << 15);

        assert_eq!(watchdog.time_left(), Ok(Duration::from_millis(1500)));
    }
}
