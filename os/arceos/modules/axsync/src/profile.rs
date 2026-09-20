//! Optional latency profiling hooks shared by synchronization and I/O layers.

use core::{
    mem,
    sync::atomic::{AtomicUsize, Ordering},
};

/// A kernel wait or latency class recorded by an optional guest profiler.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum ProfileEvent {
    /// Time spent waiting to acquire a contended sleepable mutex.
    MutexWait            = 1,
    /// Time spent inside a serialized page-cache operation.
    PageCache            = 2,
    /// End-to-end synchronous block read latency.
    BlockRead            = 3,
    /// End-to-end synchronous block write latency.
    BlockWrite           = 4,
    /// Time spent waiting for a block flush barrier.
    BlockFlush           = 5,
    /// Time spent waiting to acquire the filesystem-wide ext4 mutex.
    Ext4LockWait         = 6,
    /// Time owning the filesystem-wide ext4 mutex.
    Ext4LockHold         = 7,
    /// Time spent admitting a request into the software block queues.
    BlockAdmission       = 8,
    /// Time spent submitting and publishing a hardware request batch.
    BlockDispatch        = 9,
    /// Time from software submission until terminal device completion.
    BlockCompletionWait  = 10,
    /// Task-context work used to drain acknowledged device completions.
    BlockCompletionDrain = 11,
}

impl ProfileEvent {
    /// Number of representable profiling event values, including zero.
    pub const COUNT: usize = Self::BlockCompletionDrain as usize + 1;
}

type BeginHook = fn(ProfileEvent, usize) -> u64;
type EndHook = fn(u64);

static BEGIN_HOOK: AtomicUsize = AtomicUsize::new(0);
static END_HOOK: AtomicUsize = AtomicUsize::new(0);

/// Installs the callbacks used to record profiling intervals.
pub fn register_profile_hooks(begin: BeginHook, end: EndHook) {
    BEGIN_HOOK.store(begin as usize, Ordering::Release);
    END_HOOK.store(end as usize, Ordering::Release);
}

/// Starts one profiling interval and returns its opaque token.
#[inline]
pub fn profile_wait_begin(event: ProfileEvent, object: usize) -> u64 {
    let hook = BEGIN_HOOK.load(Ordering::Acquire);
    if hook == 0 {
        return 0;
    }
    // SAFETY: `register_profile_hooks` stores this exact function type.
    let hook: BeginHook = unsafe { mem::transmute(hook) };
    hook(event, object)
}

/// Completes a profiling interval created by [`profile_wait_begin`].
#[inline]
pub fn profile_wait_end(token: u64) {
    if token == 0 {
        return;
    }
    let hook = END_HOOK.load(Ordering::Acquire);
    if hook == 0 {
        return;
    }
    // SAFETY: `register_profile_hooks` stores this exact function type.
    let hook: EndHook = unsafe { mem::transmute(hook) };
    hook(token);
}

/// Records the lifetime of one lexically scoped profiling event.
pub struct ProfileScope(u64);

impl ProfileScope {
    /// Starts a new interval for `event`.
    #[inline]
    pub fn new(event: ProfileEvent, object: usize) -> Self {
        Self(profile_wait_begin(event, object))
    }
}

impl Drop for ProfileScope {
    #[inline]
    fn drop(&mut self) {
        profile_wait_end(self.0);
    }
}
