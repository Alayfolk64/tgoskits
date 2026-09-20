//! Sleepable shared I/O admission with an exclusive durability barrier.

use crate::{
    BlockError, BlockResult,
    os::{runtime_ops, sync::IrqMutex, waiters::TaskWaiters},
};

pub(super) struct IoBarrier {
    state: IrqMutex<IoBarrierState>,
    changed: TaskWaiters,
}

struct IoBarrierState {
    readers: usize,
    waiting_writers: usize,
    writer: bool,
}

#[must_use = "retain shared I/O admission until cache and device access completes"]
pub(super) struct SharedIo<'a>(&'a IoBarrier);

#[must_use = "retain exclusive I/O admission through writeback and the device barrier"]
pub(super) struct ExclusiveIo<'a>(&'a IoBarrier);

struct WaitingWriter<'a> {
    barrier: &'a IoBarrier,
    queued: bool,
}

impl IoBarrier {
    pub(super) const fn new() -> Self {
        Self {
            state: IrqMutex::new(IoBarrierState {
                readers: 0,
                waiting_writers: 0,
                writer: false,
            }),
            changed: TaskWaiters::new(),
        }
    }

    /// Admits an operation that may run concurrently with unrelated folios.
    /// A queued durability barrier receives writer preference so a sustained
    /// stream of reads cannot starve `flush()`.
    pub(super) fn shared(&self) -> BlockResult<SharedIo<'_>> {
        loop {
            if let Some(admission) = self.try_shared()? {
                return Ok(admission);
            }
            self.wait_while(|| {
                let state = self.state.lock();
                state.writer || state.waiting_writers != 0
            })?;
        }
    }

    /// Attempts shared admission without sleeping. Allocator reclaim uses this
    /// path so it cannot overlap direct I/O yet never waits behind it.
    pub(super) fn try_shared(&self) -> BlockResult<Option<SharedIo<'_>>> {
        let mut state = self.state.lock();
        if state.writer || state.waiting_writers != 0 {
            return Ok(None);
        }
        state.readers = state
            .readers
            .checked_add(1)
            .ok_or(BlockError::InvalidState)?;
        Ok(Some(SharedIo(self)))
    }

    /// Excludes new cache/device operations and drains admitted operations.
    /// The caller can then write back every shard and issue one device flush
    /// without a later dirtying operation slipping ahead of the barrier.
    pub(super) fn exclusive(&self) -> BlockResult<ExclusiveIo<'_>> {
        {
            let mut state = self.state.lock();
            state.waiting_writers = state
                .waiting_writers
                .checked_add(1)
                .ok_or(BlockError::InvalidState)?;
        }
        let mut waiting = WaitingWriter {
            barrier: self,
            queued: true,
        };
        loop {
            {
                let mut state = self.state.lock();
                if !state.writer && state.readers == 0 {
                    state.waiting_writers -= 1;
                    state.writer = true;
                    waiting.queued = false;
                    return Ok(ExclusiveIo(self));
                }
            }
            self.wait_while(|| {
                let state = self.state.lock();
                state.writer || state.readers != 0
            })?;
        }
    }

    fn wait_while(&self, should_wait: impl FnOnce() -> bool) -> BlockResult<()> {
        if !runtime_ops().is_ok_and(|runtime| runtime.can_block()) {
            return Err(BlockError::WouldBlock);
        }
        self.changed.wait_while(should_wait)
    }
}

impl Drop for SharedIo<'_> {
    fn drop(&mut self) {
        let wake_writer = {
            let mut state = self.0.state.lock();
            assert!(state.readers != 0, "unbalanced shared I/O admission");
            state.readers -= 1;
            state.readers == 0
        };
        if wake_writer {
            self.0.changed.notify_all();
        }
    }
}

impl Drop for ExclusiveIo<'_> {
    fn drop(&mut self) {
        {
            let mut state = self.0.state.lock();
            assert!(state.writer, "unbalanced exclusive I/O admission");
            state.writer = false;
        }
        self.0.changed.notify_all();
    }
}

impl Drop for WaitingWriter<'_> {
    fn drop(&mut self) {
        if self.queued {
            {
                let mut state = self.barrier.state.lock();
                assert!(
                    state.waiting_writers != 0,
                    "unbalanced I/O barrier writer cancellation"
                );
                state.waiting_writers -= 1;
            }
            self.barrier.changed.notify_all();
        }
    }
}

#[cfg(test)]
mod tests {
    use alloc::sync::Arc;
    use std::{
        sync::mpsc,
        thread,
        time::{Duration, Instant},
    };

    use super::IoBarrier;

    #[test]
    fn queued_exclusive_drain_blocks_late_shared_admission() {
        crate::os::task::install_test_runtime_ops();
        let barrier = Arc::new(IoBarrier::new());
        let active_reader = barrier.shared().unwrap();

        let (writer_acquired_tx, writer_acquired_rx) = mpsc::channel();
        let (release_writer_tx, release_writer_rx) = mpsc::channel();
        let writer_barrier = Arc::clone(&barrier);
        let writer = thread::spawn(move || {
            let _exclusive = writer_barrier.exclusive().unwrap();
            writer_acquired_tx.send(()).unwrap();
            release_writer_rx.recv().unwrap();
        });

        let deadline = Instant::now() + Duration::from_secs(1);
        while barrier.state.lock().waiting_writers == 0 {
            assert!(
                Instant::now() < deadline,
                "exclusive admission must publish its queued state"
            );
            thread::yield_now();
        }
        assert!(
            barrier.try_shared().unwrap().is_none(),
            "allocator reclaim must not overtake a queued durability barrier"
        );

        let (reader_attempted_tx, reader_attempted_rx) = mpsc::channel();
        let (reader_acquired_tx, reader_acquired_rx) = mpsc::channel();
        let reader_barrier = Arc::clone(&barrier);
        let late_reader = thread::spawn(move || {
            reader_attempted_tx.send(()).unwrap();
            let _shared = reader_barrier.shared().unwrap();
            reader_acquired_tx.send(()).unwrap();
        });
        reader_attempted_rx.recv().unwrap();

        assert!(writer_acquired_rx.try_recv().is_err());
        assert!(reader_acquired_rx.try_recv().is_err());
        drop(active_reader);
        writer_acquired_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("exclusive admission must run after active readers drain");
        assert!(barrier.try_shared().unwrap().is_none());
        assert!(
            reader_acquired_rx.try_recv().is_err(),
            "late readers must not overtake an active durability barrier"
        );

        release_writer_tx.send(()).unwrap();
        reader_acquired_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("shared admission must resume after the barrier completes");
        writer.join().unwrap();
        late_reader.join().unwrap();
    }
}
