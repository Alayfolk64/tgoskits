//! Fixed-capacity output lanes shared by the browser-console runtime and tests.

use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Mutex;

use anyhow::{Context, Result};
use axvisor::console_mux::HostOutputQueue;
use {
    ax_std::os::arceos::modules::ax_runtime::task::sync::irq::IrqWaitCell,
    ax_std::os::arceos::modules::ax_runtime::task::sync::irq::IrqWorkerWaiter,
};

use crate::sync::MutexExt;

const OUTPUT_QUEUE_CAPACITY: usize = 64 * 1024;
pub(super) const OUTPUT_BATCH_CAPACITY: usize = 512;

pub(super) struct NetworkOutputLane {
    queue: Mutex<HostOutputQueue<OUTPUT_QUEUE_CAPACITY>>,
    connected: AtomicBool,
    session: AtomicUsize,
    ready: IrqWaitCell,
}

impl NetworkOutputLane {
    pub(super) const fn new() -> Self {
        Self {
            queue: Mutex::new(HostOutputQueue::new()),
            connected: AtomicBool::new(false),
            session: AtomicUsize::new(0),
            ready: IrqWaitCell::new(),
        }
    }

    pub(super) fn submit(&self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        // Virtual UART drains and the management shell run in task context;
        // this queue is never submitted from a hard-IRQ handler. The task
        // mutex therefore preserves FIFO transactions without extending an
        // IRQ-off critical section.
        let connected = {
            let mut queue = self.queue.lock_unpoisoned();
            queue.enqueue(bytes);
            self.connected.load(Ordering::Acquire)
        };
        if connected {
            let _result = self.ready.notify();
        }
    }

    pub(super) fn is_connected(&self) -> bool {
        self.connected.load(Ordering::Acquire)
    }

    pub(super) fn begin_session(&self) -> Option<usize> {
        let _queue = self.queue.lock_unpoisoned();
        self.connected
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .ok()?;
        // The backlog stays queued on purpose: the dispatcher replays it to
        // the browser that just attached before it waits for live output.
        Some(self.session.fetch_add(1, Ordering::AcqRel).wrapping_add(1))
    }

    pub(super) fn end_session(&self, session: usize) {
        let _queue = self.queue.lock_unpoisoned();
        if self.session.load(Ordering::Acquire) == session {
            self.connected.store(false, Ordering::Release);
            drop(_queue);
            let _result = self.ready.notify();
        }
    }

    /// Drops output and disconnects the old owner before a lane is reused.
    ///
    /// The layout lock is held by the caller while this runs. That keeps a
    /// late producer from observing the old endpoint after it was released
    /// and racing a new owner into the same lane.
    pub(super) fn reset(&self) {
        let mut queue = self.queue.lock_unpoisoned();
        self.connected.store(false, Ordering::Release);
        *queue = HostOutputQueue::new();
        drop(queue);
        let _result = self.ready.notify();
    }

    pub(super) fn take_batch(&self, session: usize) -> Option<NetworkOutputBatch> {
        let mut queue = self.queue.lock_unpoisoned();
        if !self.connected.load(Ordering::Acquire)
            || self.session.load(Ordering::Acquire) != session
        {
            return None;
        }
        let mut batch = NetworkOutputBatch::new();
        batch.dropped_bytes = queue.take_dropped_bytes();
        batch.len = queue.dequeue(&mut batch.bytes);
        (!batch.is_empty()).then_some(batch)
    }

    pub(super) fn receive(
        &self,
        session: usize,
        waiter: &IrqWorkerWaiter,
    ) -> Result<Option<NetworkOutputBatch>> {
        loop {
            if let Some(batch) = self.take_batch(session) {
                return Ok(Some(batch));
            }
            if !self.connected.load(Ordering::Acquire)
                || self.session.load(Ordering::Acquire) != session
            {
                return Ok(None);
            }
            waiter
                .wait(&self.ready)
                .context("failed to wait for browser console output")?;
        }
    }
}

pub(super) struct NetworkOutputBatch {
    pub(super) bytes: [u8; OUTPUT_BATCH_CAPACITY],
    pub(super) len: usize,
    pub(super) dropped_bytes: usize,
}

impl NetworkOutputBatch {
    const fn new() -> Self {
        Self {
            bytes: [0; OUTPUT_BATCH_CAPACITY],
            len: 0,
            dropped_bytes: 0,
        }
    }

    const fn is_empty(&self) -> bool {
        self.len == 0 && self.dropped_bytes == 0
    }
}

#[cfg(any(test, axtest))]
mod tests {
    use super::{NetworkOutputLane, OUTPUT_BATCH_CAPACITY};

    #[cfg_attr(axtest, axtest::axtest)]
    #[cfg_attr(not(axtest), test)]
    fn lane_reuse_drops_the_previous_owner_backlog() {
        let lane = NetworkOutputLane::new();
        lane.submit(b"old owner output");
        lane.reset();

        let session = lane.begin_session().expect("a reset lane is available");
        assert!(lane.take_batch(session).is_none());

        lane.submit(b"new owner output");
        let batch = lane
            .take_batch(session)
            .expect("new owner output must remain available");
        assert_eq!(&batch.bytes[..batch.len], b"new owner output");
        assert!(batch.len <= OUTPUT_BATCH_CAPACITY);
    }
}
