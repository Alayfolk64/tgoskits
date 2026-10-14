//! Per-device accounting owned by request completion, independent of waiters.

use alloc::sync::Arc;
use core::time::Duration;

use rdif_block::RequestOp;

use crate::os::{monotonic_time, sync::RawSpinLock};

/// One device's cumulative dispatch-to-completion I/O statistics.
///
/// Durations use monotonic nanoseconds; sector counts use 512-byte sectors.
/// In-flight time begins when a driver accepts ownership and ends when its
/// terminal completion is consumed. It excludes software admission/queue wait.
/// Busy time is the union of in-flight intervals, while weighted time integrates
/// the number of in-flight requests. Neither is CPU execution time.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct BlockIoStats {
    pub reads: u64,
    pub sectors_read: u64,
    pub read_ns: u64,
    pub writes: u64,
    pub sectors_written: u64,
    pub write_ns: u64,
    pub flushes: u64,
    pub flush_ns: u64,
    pub failed_requests: u64,
    pub inflight: u64,
    pub busy_ns: u64,
    pub weighted_ns: u64,
}

#[derive(Default)]
struct IoState {
    counters: BlockIoStats,
    updated: Duration,
}

#[derive(Default)]
pub(super) struct DeviceIoMetrics {
    state: RawSpinLock<IoState>,
}

impl DeviceIoMetrics {
    pub(super) fn snapshot(&self) -> BlockIoStats {
        let mut state = self.state.lock_irqsave();
        state.advance(monotonic_time());
        state.counters
    }
}

impl IoState {
    fn advance(&mut self, now: Duration) {
        let elapsed = nanos(now.saturating_sub(self.updated));
        if self.counters.inflight != 0 {
            self.counters.busy_ns = self.counters.busy_ns.saturating_add(elapsed);
            self.counters.weighted_ns = self
                .counters
                .weighted_ns
                .saturating_add(elapsed.saturating_mul(self.counters.inflight));
        }
        self.updated = self.updated.max(now);
    }

    fn begin(&mut self, now: Duration) {
        self.advance(now);
        self.counters.inflight += 1;
    }

    fn complete(
        &mut self,
        now: Duration,
        started: Duration,
        op: RequestOp,
        sectors: u64,
        failed: bool,
    ) {
        self.advance(now);
        self.counters.inflight -= 1;
        let elapsed = nanos(now.saturating_sub(started));
        let sectors = if failed { 0 } else { sectors };
        if failed {
            self.counters.failed_requests += 1;
        }
        match op {
            RequestOp::Read => {
                self.counters.reads += 1;
                self.counters.sectors_read = self.counters.sectors_read.saturating_add(sectors);
                self.counters.read_ns = self.counters.read_ns.saturating_add(elapsed);
            }
            RequestOp::Write => {
                self.counters.writes += 1;
                self.counters.sectors_written =
                    self.counters.sectors_written.saturating_add(sectors);
                self.counters.write_ns = self.counters.write_ns.saturating_add(elapsed);
            }
            RequestOp::Flush => {
                self.counters.flushes += 1;
                self.counters.flush_ns = self.counters.flush_ns.saturating_add(elapsed);
            }
        }
    }
}

fn nanos(duration: Duration) -> u64 {
    u64::try_from(duration.as_nanos()).unwrap_or(u64::MAX)
}

/// Moves with the sole completion sender. Dropping a receiver cannot stop
/// accounting; rejecting a request before dispatch never starts accounting.
pub(super) struct RequestAccounting {
    metrics: Arc<DeviceIoMetrics>,
    op: RequestOp,
    sectors: u64,
    started: Option<Duration>,
}

impl RequestAccounting {
    pub(super) fn new(metrics: Arc<DeviceIoMetrics>, op: RequestOp, sectors: u64) -> Self {
        Self {
            metrics,
            op,
            sectors,
            started: None,
        }
    }

    pub(super) fn start(&mut self) {
        if self.started.is_none() {
            let mut state = self.metrics.state.lock_irqsave();
            let now = monotonic_time();
            state.begin(now);
            self.started = Some(now);
        }
    }

    pub(super) fn finish(&mut self, failed: bool) {
        if let Some(started) = self.started.take() {
            let mut state = self.metrics.state.lock_irqsave();
            state.complete(monotonic_time(), started, self.op, self.sectors, failed);
        }
    }
}

impl Drop for RequestAccounting {
    fn drop(&mut self) {
        self.finish(true);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlap_counts_union_and_weighted_time_without_billing_idle_time() {
        let ms = Duration::from_millis;
        let mut state = IoState::default();
        state.begin(ms(10));
        state.begin(ms(20));
        state.advance(ms(25));
        assert_eq!(state.counters.busy_ns, 15_000_000);
        assert_eq!(state.counters.weighted_ns, 20_000_000);
        state.complete(ms(30), ms(10), RequestOp::Read, 8, false);
        state.complete(ms(40), ms(20), RequestOp::Write, 16, true);
        state.advance(ms(100));
        assert_eq!(state.counters.busy_ns, 30_000_000);
        assert_eq!(state.counters.weighted_ns, 40_000_000);
        assert_eq!(state.counters.inflight, 0);
        assert_eq!(state.counters.read_ns, 20_000_000);
        assert_eq!(state.counters.write_ns, 20_000_000);
        assert_eq!(state.counters.sectors_read, 8);
        assert_eq!(state.counters.sectors_written, 0);
        assert_eq!(state.counters.failed_requests, 1);
    }
}
