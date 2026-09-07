//! Optional observations around scheduler-backed future waits.

use core::sync::atomic::{AtomicUsize, Ordering};

static BEGIN: AtomicUsize = AtomicUsize::new(0);
static END: AtomicUsize = AtomicUsize::new(0);

/// Installs wait observers before profiling begins.
///
/// Both callbacks must avoid allocating and sleeping. `end` consumes the token
/// returned by `begin`, including when the waiting thread migrates.
pub fn register_block_profile_hooks(begin: fn() -> u64, end: fn(u64)) {
    END.store(end as usize, Ordering::Release);
    BEGIN.store(begin as usize, Ordering::Release);
}

pub(crate) struct BlockProfile {
    token: u64,
}

impl BlockProfile {
    pub(crate) fn new() -> Self {
        let begin = BEGIN.load(Ordering::Acquire);
        let token = if begin == 0 {
            0
        } else {
            // SAFETY: registration stores only this exact function signature.
            let begin: fn() -> u64 = unsafe { core::mem::transmute(begin) };
            begin()
        };
        Self { token }
    }
}

impl Drop for BlockProfile {
    fn drop(&mut self) {
        if self.token == 0 {
            return;
        }
        let end = END.load(Ordering::Acquire);
        if end != 0 {
            // SAFETY: registration publishes the end callback before begin.
            let end: fn(u64) = unsafe { core::mem::transmute(end) };
            end(self.token);
        }
    }
}
