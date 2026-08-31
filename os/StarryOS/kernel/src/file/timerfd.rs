//! timerfd — kernel-side timer events delivered via a file descriptor.
//!
//! Userspace creates a timerfd via `timerfd_create(clockid, flags)`, arms it
//! with `timerfd_settime(fd, flags, new, old)`, and reads the cumulative
//! number of expirations as a `u64` via `read(fd)`. The fd is epoll-pollable
//! (becomes readable when `expire_count > 0`).
//!
//! Implementation model: each `Timerfd::new` spawns exactly one long-lived
//! background task (via `ax_task::spawn_raw`) that owns a weak reference to
//! the Timerfd. The task loops, reading the current deadline under the state
//! lock, then parks on whichever fires first: the clock-domain deadline or an
//! "arm event" poked by `settime` / `Drop`. One task
//! per timerfd over its whole lifetime — no per-settime stack leak.
//!
//! Missed-tick coalescing: if the scheduler delays the task by N intervals,
//! `read` returns the full count (Linux semantics).

use alloc::{
    borrow::{Cow, ToOwned},
    sync::{Arc, Weak},
    vec::Vec,
};
use core::{
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
    task::Context,
    time::Duration,
};

use ax_lazyinit::LazyLock;
use ax_runtime::hal::time::{TimeValue, monotonic_time, wall_time};
use ax_task::future::{block_on, poll_io, timeout_at, timeout_at_wall};
use axpoll::{IoEvents, PollSet, Pollable};
use event_listener::{Event, listener};
use syscalls::Errno;

use crate::{
    StarryError, StarryResult,
    file::{FileLike, IoDst, IoSrc},
    sync::Mutex,
};

/// `clockid_t` values recognized by `timerfd_create`. Kept narrow for now —
/// musl and glibc both pass `CLOCK_REALTIME` or `CLOCK_MONOTONIC`. Other
/// values return `StarryError::InvalidInput`.
pub const CLOCK_REALTIME: u32 = 0;
pub const CLOCK_MONOTONIC: u32 = 1;
pub const CLOCK_BOOTTIME: u32 = 7;
pub const CLOCK_REALTIME_ALARM: u32 = 8;
pub const CLOCK_BOOTTIME_ALARM: u32 = 9;

/// `flags` bits for `timerfd_settime`.
pub const TFD_TIMER_ABSTIME: u32 = 1;
pub const TFD_TIMER_CANCEL_ON_SET: u32 = 2;

/// Interpretation of the initial timerfd expiration.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TimerfdSetMode {
    /// The value is an interval measured by the monotonic clock.
    Relative,
    /// The value is an absolute deadline in the timerfd's clock domain.
    Absolute,
    /// The value is an absolute deadline that is canceled by realtime changes.
    AbsoluteCancelOnSet,
}

impl TimerfdSetMode {
    fn is_absolute(self) -> bool {
        !matches!(self, Self::Relative)
    }

    fn cancel_on_set(self) -> bool {
        matches!(self, Self::AbsoluteCancelOnSet)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TimerDeadline {
    Monotonic(TimeValue),
    Realtime(TimeValue),
}

impl TimerDeadline {
    fn now(self) -> TimeValue {
        match self {
            Self::Monotonic(_) => monotonic_time(),
            Self::Realtime(_) => wall_time(),
        }
    }

    fn value(self) -> TimeValue {
        match self {
            Self::Monotonic(value) | Self::Realtime(value) => value,
        }
    }

    fn remaining(self) -> Duration {
        self.value()
            .checked_sub(self.now())
            .unwrap_or(Duration::ZERO)
    }

    fn lag(self) -> Option<Duration> {
        self.now().checked_sub(self.value())
    }

    fn saturating_add(self, duration: Duration) -> Self {
        match self {
            Self::Monotonic(value) => Self::Monotonic(value.saturating_add(duration)),
            Self::Realtime(value) => Self::Realtime(value.saturating_add(duration)),
        }
    }

    fn is_realtime(self) -> bool {
        matches!(self, Self::Realtime(_))
    }
}

/// Internal, mutex-protected state of a timerfd.
#[derive(Default)]
struct State {
    /// Time of the next expiration and its clock domain. `None` when disarmed.
    next_deadline: Option<TimerDeadline>,
    /// Interval for periodic firing. `Duration::ZERO` means one-shot.
    interval: Duration,
    /// Whether an armed realtime absolute timer is canceled by a clock step.
    cancel_on_set: bool,
    /// Whether the next read must consume a realtime clock cancellation.
    canceled: bool,
    /// When `true`, the background task should exit on its next wake.
    shutdown: bool,
}

static TIMERFD_INSTANCES: LazyLock<Mutex<Vec<Weak<Timerfd>>>> =
    LazyLock::new(|| Mutex::new(Vec::new()));

/// A timerfd. Held behind `Arc` and referenced both from the fd table and
/// from the background timer task (as a `Weak<Timerfd>`).
pub struct Timerfd {
    /// The clock domain the user passed to `timerfd_create`.
    clockid: u32,
    state: Mutex<State>,
    expire_count: AtomicU64,
    poll_rx: PollSet,
    non_blocking: AtomicBool,
    /// Pulsed by `settime` / `Drop` to wake the background task so it
    /// re-reads `state` and either re-arms or exits. `Arc` so the task
    /// can hold it independently of the Timerfd (allowing the Timerfd
    /// Arc to drop while the task is parked).
    arm_event: Arc<Event>,
}

impl Timerfd {
    /// Create a disarmed timerfd for the given clock. A single long-lived
    /// background task is spawned to serve all future arms of this fd.
    pub fn new(clockid: u32) -> StarryResult<Arc<Self>> {
        match clockid {
            CLOCK_REALTIME | CLOCK_MONOTONIC | CLOCK_BOOTTIME | CLOCK_REALTIME_ALARM
            | CLOCK_BOOTTIME_ALARM => {}
            _ => return Err(StarryError::InvalidInput),
        }
        let this = Arc::new(Self {
            clockid,
            state: Mutex::new(State::default()),
            expire_count: AtomicU64::new(0),
            poll_rx: PollSet::new(),
            non_blocking: AtomicBool::new(false),
            arm_event: Arc::new(Event::new()),
        });
        TIMERFD_INSTANCES.lock().push(Arc::downgrade(&this));
        // Hand a weak reference to the task so the Timerfd can be freed
        // (and the task told to exit) when userspace closes the fd.
        let weak = Arc::downgrade(&this);
        ax_task::spawn_raw(
            move || block_on(run_timer(weak)),
            "timerfd".to_owned(),
            ax_task::default_task_stack_size(),
        );
        Ok(this)
    }

    /// Arm or disarm the timer. Returns the previous `(interval, remaining)`.
    pub fn settime(
        &self,
        mode: TimerfdSetMode,
        new_value: Duration,
        new_interval: Duration,
    ) -> StarryResult<(Duration, Duration)> {
        let mut state = self.state.lock();
        let old_interval = state.interval;
        let old_remaining = state
            .next_deadline
            .map(TimerDeadline::remaining)
            .unwrap_or(Duration::ZERO);

        if new_value.is_zero() {
            state.next_deadline = None;
            state.interval = Duration::ZERO;
            state.cancel_on_set = false;
        } else {
            let deadline = if mode.is_absolute() {
                match self.clockid {
                    CLOCK_REALTIME | CLOCK_REALTIME_ALARM => TimerDeadline::Realtime(new_value),
                    _ => TimerDeadline::Monotonic(new_value),
                }
            } else {
                TimerDeadline::Monotonic(monotonic_time().saturating_add(new_value))
            };
            state.next_deadline = Some(deadline);
            state.interval = new_interval;
            state.cancel_on_set = mode.cancel_on_set() && deadline.is_realtime();
        }
        state.canceled = false;
        // Clear any expirations that accumulated under the previous
        // setting. man timerfd_read(2) is explicit: read returns the
        // number of expirations since "the last successful read or the
        // last timerfd_settime() that reset the timer". Without this
        // reset a `settime` rearm-without-read would let the next
        // `read` return stale ticks from the old timer.
        //
        // Done under `state` so the background task, which only adds
        // expirations after re-acquiring `state` and confirming its
        // observed deadline is still current, cannot race a stale
        // fetch_add past this clear.
        self.expire_count.store(0, Ordering::Release);
        drop(state);

        // Wake the background task so it picks up the new deadline.
        self.arm_event.notify(usize::MAX);
        Ok((old_interval, old_remaining))
    }

    /// Current `(interval, remaining)`. `remaining == 0` iff disarmed.
    pub fn gettime(&self) -> (Duration, Duration) {
        let state = self.state.lock();
        let interval = state.interval;
        let remaining = match state.next_deadline {
            None => Duration::ZERO,
            Some(deadline) => deadline.remaining(),
        };
        (interval, remaining)
    }
}

/// Marks cancel-on-set realtime timerfds after a discontinuous clock change.
pub fn notify_realtime_clock_changed() {
    let timerfds = {
        let mut registry = TIMERFD_INSTANCES.lock();
        let mut timerfds = Vec::with_capacity(registry.len());
        registry.retain(|weak| {
            let Some(timerfd) = weak.upgrade() else {
                return false;
            };
            timerfds.push(timerfd);
            true
        });
        timerfds
    };

    for timerfd in timerfds {
        let mut state = timerfd.state.lock();
        if state.cancel_on_set && state.next_deadline.is_some() {
            state.canceled = true;
            timerfd.expire_count.store(1, Ordering::Release);
            drop(state);
            timerfd.arm_event.notify(usize::MAX);
            // The cancellation marker is visible before readers are woken.
            unsafe { timerfd.poll_rx.wake(IoEvents::IN) };
        }
    }
}

impl Drop for Timerfd {
    fn drop(&mut self) {
        // Tell the background task to exit. The task holds a Weak<Timerfd>,
        // so in practice this runs only if every other ref has been released —
        // but flip the shutdown flag anyway for correctness if the last ref
        // happens to be the task's own upgrade.
        let mut state = self.state.lock();
        state.shutdown = true;
        drop(state);
        self.arm_event.notify(usize::MAX);

        // `notify_realtime_clock_changed` snapshots strong references before
        // taking any timerfd state lock, so unregistering here cannot recurse
        // into `Drop` while the registry lock is held.
        let self_ptr = core::ptr::from_ref(self);
        TIMERFD_INSTANCES
            .lock()
            .retain(|weak| weak.as_ptr() != self_ptr && weak.strong_count() != 0);
    }
}

async fn run_timer(weak: alloc::sync::Weak<Timerfd>) {
    loop {
        // Race-free arm pattern (see task/timer.rs::alarm_task):
        //   1. Upgrade, grab a standalone handle to arm_event, drop Arc.
        //   2. Register the listener.
        //   3. Re-upgrade and snapshot state. If state changed vs. anything
        //      visible before step 2, the poke was captured by the listener
        //      (or will be on next iter via `continue`).
        let arm_event = {
            let Some(tfd) = weak.upgrade() else {
                return;
            };
            tfd.arm_event.clone()
        };
        listener!(arm_event => listener);

        let (deadline, interval, shutdown) = {
            let Some(tfd) = weak.upgrade() else {
                return;
            };
            let state = tfd.state.lock();
            (state.next_deadline, state.interval, state.shutdown)
        };
        if shutdown {
            return;
        }

        match deadline {
            None => {
                // Disarmed. Wait on arm_event for the next settime.
                listener.await;
            }
            Some(dl) => {
                // Race the deadline against an arm_event (new settime,
                // cancellation, or shutdown). Relative timers stay in the
                // monotonic domain; only absolute realtime timers are rebuilt
                // after a wall-clock step.
                let fired_timer = match dl {
                    TimerDeadline::Monotonic(deadline) => {
                        timeout_at(Some(deadline), listener).await.is_err()
                    }
                    TimerDeadline::Realtime(deadline) => {
                        timeout_at_wall(Some(deadline), listener).await.is_err()
                    }
                };
                if !fired_timer {
                    // State changed; loop to re-read.
                    continue;
                }

                // Timer fired. Re-upgrade, compute missed-tick count,
                // advance deadline by N intervals, publish to state.
                let Some(tfd) = weak.upgrade() else {
                    return;
                };
                let mut expirations: u64 = 1;
                let mut next_deadline = dl;
                if !interval.is_zero() {
                    // Missed-tick coalescing: count every interval that
                    // fully elapsed past `dl`. Clamp at u32::MAX ticks so
                    // `Duration::*` multiplication cannot silently
                    // truncate; u32::MAX ticks at a 1 ns interval is still
                    // ~4 seconds of lag, which is more than any real
                    // scheduler delay we need to represent faithfully.
                    if let Some(lag) = dl.lag() {
                        let extra_ticks = lag.as_nanos() / interval.as_nanos().max(1);
                        let extra = core::cmp::min(extra_ticks, u32::MAX as u128 - 1) as u32;
                        expirations += extra as u64;
                        // saturating_mul avoids panic on pathological
                        // (interval, extra) pairs.
                        let advance = interval.saturating_mul(extra + 1);
                        next_deadline = dl.saturating_add(advance);
                    }
                }

                // Publish next deadline (or clear for one-shot) AND add
                // the expirations under the same state lock. If the
                // current next_deadline no longer matches the one we
                // just fired, someone re-armed (or disarmed) the timer
                // while we were firing — those expirations belong to
                // the now-gone timer setting, so drop them on the
                // floor. settime clears expire_count under the same
                // lock, so once we observe a stale deadline here the
                // count has already been cleared and we must not
                // re-add to it.
                let mut state = tfd.state.lock();
                if state.shutdown {
                    return;
                }
                if state.next_deadline == Some(dl) {
                    tfd.expire_count.fetch_add(expirations, Ordering::AcqRel);
                    if interval.is_zero() {
                        state.next_deadline = None;
                    } else {
                        state.next_deadline = Some(next_deadline);
                    }
                    drop(state);
                    // expire_count is published before waking readers.
                    unsafe { tfd.poll_rx.wake(IoEvents::IN) };
                }
            }
        }
    }
}

impl FileLike for Timerfd {
    fn read(&self, dst: &mut IoDst) -> StarryResult<usize> {
        if dst.remaining_mut() < core::mem::size_of::<u64>() {
            return Err(StarryError::InvalidInput);
        }
        block_on(poll_io(self, IoEvents::IN, self.nonblocking(), || {
            {
                let mut state = self.state.lock();
                if state.canceled {
                    state.canceled = false;
                    self.expire_count.store(0, Ordering::Release);
                    return Err(Errno::ECANCELED.into());
                }
            }

            // Race-free read: atomically claim the entire `expire_count`
            // snapshot via CAS so concurrent readers can't both observe
            // and copy the same ticks. Linux's `timerfd_read(2)` holds
            // the timerfd spinlock across the load + clear; we get the
            // same single-consumer guarantee from the CAS loop. A
            // simultaneous `fetch_add` from the timer task raises the
            // count past `n`, the CAS fails, and we re-snapshot before
            // copying — so newly-arrived ticks aren't dropped either.
            let n = loop {
                let observed = self.expire_count.load(Ordering::Acquire);
                if observed == 0 {
                    return Err(StarryError::WouldBlock);
                }
                if self
                    .expire_count
                    .compare_exchange(observed, 0, Ordering::AcqRel, Ordering::Acquire)
                    .is_ok()
                {
                    break observed;
                }
            };
            // Linux's timerfd_read(2): a failed read does not discard
            // expirations. Restore the claimed count on copyout failure,
            // and re-wake `poll_rx` so any reader or poller that
            // entered its wait between our CAS-to-zero and this restore
            // notices the fd is readable again.
            if let Err(e) = dst.write(&n.to_ne_bytes()) {
                self.expire_count.fetch_add(n, Ordering::AcqRel);
                // Restored expire_count is visible before re-waking readers.
                unsafe { self.poll_rx.wake(IoEvents::IN) };
                return Err(e.into());
            }
            Ok(core::mem::size_of::<u64>())
        }))
    }

    fn write(&self, _src: &mut IoSrc) -> StarryResult<usize> {
        Err(StarryError::InvalidInput)
    }

    fn nonblocking(&self) -> bool {
        self.non_blocking.load(Ordering::Acquire)
    }

    fn set_nonblocking(&self, non_blocking: bool) -> StarryResult {
        self.non_blocking.store(non_blocking, Ordering::Release);
        Ok(())
    }

    fn path(&self) -> Cow<'_, str> {
        "anon_inode:[timerfd]".into()
    }
}

impl Pollable for Timerfd {
    fn poll(&self) -> IoEvents {
        let mut events = IoEvents::empty();
        events.set(IoEvents::IN, self.expire_count.load(Ordering::Acquire) > 0);
        events
    }

    fn register(&self, context: &mut Context<'_>, events: IoEvents) {
        if events.contains(IoEvents::IN) {
            // Registration happens from file poll task context.
            unsafe { self.poll_rx.register(context.waker(), IoEvents::IN) };
        }
    }
}

#[cfg(all(test, not(axtest)))]
mod tests {
    use super::*;

    fn unspawned_timerfd() -> Arc<Timerfd> {
        Arc::new(Timerfd {
            clockid: CLOCK_REALTIME,
            state: Mutex::new(State::default()),
            expire_count: AtomicU64::new(0),
            poll_rx: PollSet::new(),
            non_blocking: AtomicBool::new(false),
            arm_event: Arc::new(Event::new()),
        })
    }

    #[test]
    fn dropping_timerfd_unregisters_clock_change_observer() {
        let timerfd = unspawned_timerfd();
        let timerfd_ptr = Arc::as_ptr(&timerfd);
        TIMERFD_INSTANCES.lock().push(Arc::downgrade(&timerfd));

        drop(timerfd);

        assert!(
            !TIMERFD_INSTANCES
                .lock()
                .iter()
                .any(|weak| weak.as_ptr() == timerfd_ptr),
            "closed timerfd remained in the realtime clock observer registry"
        );
    }
}
