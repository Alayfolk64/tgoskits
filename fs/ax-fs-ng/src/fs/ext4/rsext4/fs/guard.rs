//! Publishes mount failures and profiling intervals at the state-lock boundary.

use core::ops::{Deref, DerefMut};

use super::{Ext4State, admission::Admission};
use crate::os::sync::{SleepMutex, SleepMutexGuard};

pub(crate) struct Ext4Guard<'a> {
    cached_writes: &'a Admission,
    // End the hold interval before unlocking, then the inclusive interval.
    #[cfg(feature = "profile")]
    _hold: ax_sync::ProfileScope,
    inner: SleepMutexGuard<'a, Ext4State>,
    #[cfg(feature = "profile")]
    _operation: ax_sync::ProfileScope,
}

impl<'a> Ext4Guard<'a> {
    pub(super) fn acquire(mutex: &'a SleepMutex<Ext4State>, cached_writes: &'a Admission) -> Self {
        #[cfg(feature = "profile")]
        let operation =
            ax_sync::ProfileScope::new(ax_sync::ProfileEvent::Ext4, mutex as *const _ as usize);
        #[cfg(feature = "profile")]
        let inner = mutex.try_lock().unwrap_or_else(|| {
            let _wait = ax_sync::ProfileScope::new(
                ax_sync::ProfileEvent::Ext4LockWait,
                mutex as *const _ as usize,
            );
            mutex.lock()
        });
        #[cfg(not(feature = "profile"))]
        let inner = mutex.lock();
        Self {
            cached_writes,
            #[cfg(feature = "profile")]
            _hold: ax_sync::ProfileScope::new(
                ax_sync::ProfileEvent::Ext4LockHold,
                mutex as *const _ as usize,
            ),
            inner,
            #[cfg(feature = "profile")]
            _operation: operation,
        }
    }
}

impl Drop for Ext4Guard<'_> {
    fn drop(&mut self) {
        // Publish the authoritative core cause before unlocking state. This
        // covers detached commits, synchronous fallback and mutation aborts,
        // without adding a filesystem lock to every cached overwrite. The
        // admission lock never calls back or wakes while this guard is held.
        if let Some(cause) = self.inner.ext4.writeback_failure() {
            self.cached_writes.fail(cause);
        }
    }
}

impl Deref for Ext4Guard<'_> {
    type Target = Ext4State;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl DerefMut for Ext4Guard<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}
