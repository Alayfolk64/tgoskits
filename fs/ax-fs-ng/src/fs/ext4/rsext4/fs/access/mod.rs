//! Sleepable ownership admission with optional per-inode hot metadata.

use alloc::sync::Arc;
use core::sync::atomic::{AtomicU8, AtomicU64, AtomicUsize, Ordering};

use axfs_ng_vfs::{VfsError, VfsResult, WritebackPolicy};

use crate::{
    error::block_error_to_vfs_error,
    os::{sync::IrqMutex, waiters::TaskWaiters},
};

pub(crate) struct AccessGate {
    state: IrqMutex<AccessState>,
    changed: TaskWaiters,
    lifetime_state: AtomicUsize,
    regular_file_size: AtomicU64,
    regular_file_layout: AtomicU8,
    writeback_policy: AtomicU8,
}

const ZERO_LINK: usize = 1 << (usize::BITS - 1);
const LIFETIME_REFS: usize = !ZERO_LINK;
const UNKNOWN_REGULAR_FILE_SIZE: u64 = u64::MAX;
const UNKNOWN_REGULAR_FILE_LAYOUT: u8 = 0;
const LEGACY_REGULAR_FILE_LAYOUT: u8 = 1;
const EXTENT_REGULAR_FILE_LAYOUT: u8 = 2;
const UNKNOWN_WRITEBACK_POLICY: u8 = u8::MAX;

struct AccessState {
    readers: usize,
    waiting_writers: usize,
    writer: bool,
}

#[must_use = "retain shared access until all protected reads complete"]
pub(crate) struct ReadAccess(Arc<AccessGate>);
#[must_use = "retain exclusive access until the protected mutation completes"]
pub(crate) struct WriteAccess(Arc<AccessGate>);

struct WaitingWriter {
    gate: Arc<AccessGate>,
    queued: bool,
}

impl AccessGate {
    pub(crate) fn new() -> Self {
        Self {
            state: IrqMutex::new(AccessState {
                readers: 0,
                waiting_writers: 0,
                writer: false,
            }),
            changed: TaskWaiters::new(),
            lifetime_state: AtomicUsize::new(0),
            regular_file_size: AtomicU64::new(UNKNOWN_REGULAR_FILE_SIZE),
            regular_file_layout: AtomicU8::new(UNKNOWN_REGULAR_FILE_LAYOUT),
            writeback_policy: AtomicU8::new(UNKNOWN_WRITEBACK_POLICY),
        }
    }

    /// Retains one allocated-inode owner without taking mount state.
    pub(crate) fn retain_lifetime(&self) {
        self.lifetime_state
            .try_update(Ordering::AcqRel, Ordering::Acquire, |state| {
                ((state & LIFETIME_REFS) != LIFETIME_REFS).then_some(state + 1)
            })
            .expect("inode lifetime reference count overflow");
    }

    /// Releases one allocated-inode owner and reports whether an unlinked
    /// inode reached zero owners and needs the reap worker.
    pub(crate) fn release_lifetime(&self) -> bool {
        let previous = self
            .lifetime_state
            .try_update(Ordering::AcqRel, Ordering::Acquire, |state| {
                ((state & LIFETIME_REFS) != 0).then_some(state - 1)
            })
            .expect("inode lifetime reference count underflow");
        previous & LIFETIME_REFS == 1 && previous & ZERO_LINK != 0
    }

    pub(crate) fn lifetime_refs(&self) -> usize {
        self.lifetime_state.load(Ordering::Acquire) & LIFETIME_REFS
    }

    /// Publishes unlink in the same atomic word as the owner count. If the
    /// final release precedes this update, the publisher observes zero owners;
    /// if it follows, the release observes `ZERO_LINK` and wakes the worker.
    pub(crate) fn publish_zero_link(&self) {
        self.lifetime_state.fetch_or(ZERO_LINK, Ordering::AcqRel);
    }

    /// Clears publication only after successful reap, before this gate can be
    /// reused for a newly allocated inode with the same number.
    pub(crate) fn clear_zero_link(&self) {
        let previous = self
            .lifetime_state
            .fetch_and(LIFETIME_REFS, Ordering::AcqRel);
        assert_eq!(
            previous, ZERO_LINK,
            "successful inode reap must have zero lifetime references"
        );
    }

    /// Initializes a regular inode's hot size without overwriting a value
    /// published by a concurrent content writer.
    pub(crate) fn initialize_regular_file_size(&self, size: u64) {
        debug_assert_ne!(size, UNKNOWN_REGULAR_FILE_SIZE);
        let _ = self.regular_file_size.compare_exchange(
            UNKNOWN_REGULAR_FILE_SIZE,
            size,
            Ordering::Release,
            Ordering::Acquire,
        );
    }

    pub(crate) fn regular_file_size(&self) -> Option<u64> {
        let size = self.regular_file_size.load(Ordering::Acquire);
        (size != UNKNOWN_REGULAR_FILE_SIZE).then_some(size)
    }

    /// Publishes after a successful mutation and before content-write access
    /// is released, matching Linux's locked `i_size_write()` contract.
    pub(crate) fn publish_regular_file_size(&self, size: u64) {
        debug_assert_ne!(size, UNKNOWN_REGULAR_FILE_SIZE);
        self.regular_file_size.store(size, Ordering::Release);
    }

    /// Initializes the immutable block-mapping layout of a regular inode.
    pub(crate) fn initialize_regular_file_layout(&self, uses_extents: bool) {
        let layout = if uses_extents {
            EXTENT_REGULAR_FILE_LAYOUT
        } else {
            LEGACY_REGULAR_FILE_LAYOUT
        };
        let _ = self.regular_file_layout.compare_exchange(
            UNKNOWN_REGULAR_FILE_LAYOUT,
            layout,
            Ordering::Release,
            Ordering::Acquire,
        );
    }

    pub(crate) fn regular_file_uses_extents(&self) -> Option<bool> {
        match self.regular_file_layout.load(Ordering::Acquire) {
            LEGACY_REGULAR_FILE_LAYOUT => Some(false),
            EXTENT_REGULAR_FILE_LAYOUT => Some(true),
            _ => None,
        }
    }

    pub(crate) fn initialize_writeback_policy(&self, policy: WritebackPolicy) {
        debug_assert_ne!(policy.bits(), UNKNOWN_WRITEBACK_POLICY);
        let _ = self.writeback_policy.compare_exchange(
            UNKNOWN_WRITEBACK_POLICY,
            policy.bits(),
            Ordering::Release,
            Ordering::Acquire,
        );
    }

    pub(crate) fn writeback_policy(&self) -> Option<WritebackPolicy> {
        let policy = self.writeback_policy.load(Ordering::Acquire);
        (policy != UNKNOWN_WRITEBACK_POLICY).then(|| WritebackPolicy::from_bits_retain(policy))
    }

    #[cfg(test)]
    pub(crate) fn is_zero_link(&self) -> bool {
        self.lifetime_state.load(Ordering::Acquire) & ZERO_LINK != 0
    }

    pub(crate) fn read(self: &Arc<Self>) -> VfsResult<ReadAccess> {
        if let Some(access) = self.try_read()? {
            return Ok(access);
        }
        self.read_with(|| {
            self.changed
                .wait_while(|| {
                    let state = self.state.lock();
                    state.writer || state.waiting_writers != 0
                })
                .map_err(block_error_to_vfs_error)
        })
    }

    fn read_with(
        self: &Arc<Self>,
        mut wait: impl FnMut() -> VfsResult<()>,
    ) -> VfsResult<ReadAccess> {
        loop {
            if let Some(access) = self.try_read()? {
                return Ok(access);
            }
            wait()?;
        }
    }

    /// Readers share stable mappings but never bypass an already queued writer.
    pub(crate) fn try_read(self: &Arc<Self>) -> VfsResult<Option<ReadAccess>> {
        let mut state = self.state.lock();
        if state.writer || state.waiting_writers != 0 {
            return Ok(None);
        }
        state.readers = state
            .readers
            .checked_add(1)
            .ok_or(VfsError::ValueOverflow)?;
        Ok(Some(ReadAccess(self.clone())))
    }

    pub(crate) fn write(self: &Arc<Self>) -> VfsResult<WriteAccess> {
        if let Some(access) = self.try_write() {
            return Ok(access);
        }
        self.write_with(|| {
            self.changed
                .wait_while(|| {
                    let state = self.state.lock();
                    state.writer || state.readers != 0
                })
                .map_err(block_error_to_vfs_error)
        })
    }

    /// Claims idle access without overtaking a writer already waiting for it.
    pub(crate) fn try_write(self: &Arc<Self>) -> Option<WriteAccess> {
        let mut state = self.state.lock();
        if state.writer || state.readers != 0 || state.waiting_writers != 0 {
            return None;
        }
        state.writer = true;
        Some(WriteAccess(self.clone()))
    }

    fn write_with(
        self: &Arc<Self>,
        mut wait: impl FnMut() -> VfsResult<()>,
    ) -> VfsResult<WriteAccess> {
        {
            let mut state = self.state.lock();
            state.waiting_writers = state
                .waiting_writers
                .checked_add(1)
                .ok_or(VfsError::ValueOverflow)?;
        }
        let mut waiting = WaitingWriter {
            gate: self.clone(),
            queued: true,
        };
        loop {
            {
                let mut state = self.state.lock();
                if !state.writer && state.readers == 0 {
                    state.waiting_writers -= 1;
                    state.writer = true;
                    waiting.queued = false;
                    return Ok(WriteAccess(self.clone()));
                }
            }
            wait()?;
        }
    }
}

impl Drop for ReadAccess {
    fn drop(&mut self) {
        let last = {
            let mut state = self.0.state.lock();
            assert!(state.readers != 0, "unbalanced access read release");
            state.readers -= 1;
            state.readers == 0
        };
        if last {
            self.0.changed.notify_all();
        }
    }
}

impl Drop for WriteAccess {
    fn drop(&mut self) {
        {
            let mut state = self.0.state.lock();
            assert!(state.writer, "unbalanced access write release");
            state.writer = false;
        }
        self.0.changed.notify_all();
    }
}

impl Drop for WaitingWriter {
    fn drop(&mut self) {
        if self.queued {
            {
                let mut state = self.gate.state.lock();
                assert!(
                    state.waiting_writers != 0,
                    "unbalanced access writer cancellation"
                );
                state.waiting_writers -= 1;
            }
            self.gate.changed.notify_all();
        }
    }
}

#[cfg(test)]
mod tests;
