//! Future support.

use alloc::{sync::Arc, task::Wake};
#[cfg(feature = "profile")]
use core::sync::atomic::{AtomicUsize, Ordering};
use core::{
    future::poll_fn,
    pin::pin,
    task::{Context, Poll, Waker},
};

use crate::{
    AxTaskRef, WeakAxTaskRef, current, current_run_queue, select_wake_run_queue,
    sync::{PreemptIrqSaveState, SpinLock},
};

mod poll;
pub use poll::*;

mod time;
pub use time::*;

#[cfg(feature = "profile")]
type ProfileBeginHook = fn() -> u64;
#[cfg(feature = "profile")]
type ProfileEndHook = fn(u64);

#[cfg(feature = "profile")]
static PROFILE_BEGIN_HOOK: AtomicUsize = AtomicUsize::new(0);
#[cfg(feature = "profile")]
static PROFILE_END_HOOK: AtomicUsize = AtomicUsize::new(0);

/// Installs callbacks around intervals in which `block_on` removes a task from a CPU.
#[cfg(feature = "profile")]
pub fn register_block_profile_hooks(begin: ProfileBeginHook, end: ProfileEndHook) {
    PROFILE_BEGIN_HOOK.store(begin as usize, Ordering::Release);
    PROFILE_END_HOOK.store(end as usize, Ordering::Release);
}

#[cfg(feature = "profile")]
#[inline]
fn profile_block_begin() -> u64 {
    let hook = PROFILE_BEGIN_HOOK.load(Ordering::Acquire);
    if hook == 0 {
        return 0;
    }
    // SAFETY: `register_block_profile_hooks` stores this exact function type.
    let hook: ProfileBeginHook = unsafe { core::mem::transmute(hook) };
    hook()
}

#[cfg(feature = "profile")]
#[inline]
fn profile_block_end(token: u64) {
    if token == 0 {
        return;
    }
    let hook = PROFILE_END_HOOK.load(Ordering::Acquire);
    if hook != 0 {
        // SAFETY: `register_block_profile_hooks` stores this exact function type.
        let hook: ProfileEndHook = unsafe { core::mem::transmute(hook) };
        hook(token);
    }
}

/// Errors owned by task waiting and notification operations.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum TaskError {
    /// A signal or explicit task notification interrupted the wait.
    #[error(transparent)]
    Interrupted(#[from] Interrupted),
    /// A task wait exceeded its deadline.
    #[error(transparent)]
    Elapsed(#[from] Elapsed),
    /// A nonblocking task operation cannot currently make progress.
    #[error("task operation would block")]
    WouldBlock,
    /// An IRQ operation used by a task-owned waker failed.
    #[cfg(feature = "irq")]
    #[error(transparent)]
    Irq(#[from] ax_hal::irq::IrqError),
}

/// A result returned by a task-domain operation.
pub type TaskResult<T = ()> = Result<T, TaskError>;

/// Error capability required by [`poll_io`].
///
/// The caller keeps ownership of its domain error while the task layer only
/// asks how to recognize retryable I/O and how to publish an interruption.
pub trait PollIoError {
    /// Returns whether this error means the I/O operation should be retried.
    fn is_would_block(&self) -> bool;

    /// Creates the caller's domain error for an interrupted blocking wait.
    fn interrupted(error: Interrupted) -> Self;
}

impl PollIoError for TaskError {
    fn is_would_block(&self) -> bool {
        matches!(self, Self::WouldBlock)
    }

    fn interrupted(error: Interrupted) -> Self {
        error.into()
    }
}

struct AxWaker {
    task: WeakAxTaskRef,
    woke: SpinLock<bool>,
}

impl AxWaker {
    fn new(task: &AxTaskRef) -> Arc<Self> {
        Arc::new(AxWaker {
            task: Arc::downgrade(task),
            woke: SpinLock::new(false),
        })
    }
}

impl Wake for AxWaker {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        if let Some(task) = self.task.upgrade() {
            let mut rq = select_wake_run_queue::<PreemptIrqSaveState>(&task);
            *self.woke.lock_irqsave() = true;
            rq.unblock_task(task, true);
        }
    }
}

/// Blocks the current task until the given future is resolved or the task
/// is interrupted by a signal.
///
/// When the task's `interrupted` flag is set (by `task.interrupt()`, typically
/// from signal delivery), this function yields the CPU to allow signal
/// processing on the return-to-userspace path. The future will be re-polled
/// after the yield.
#[track_caller]
pub fn block_on<F: IntoFuture>(f: F) -> F::Output {
    crate::api::might_sleep();

    let mut fut = pin!(f.into_future());

    let curr = current();
    let task = curr.clone();

    let axwaker = AxWaker::new(&task);
    let waker = Waker::from(axwaker.clone());
    let mut cx = Context::from_waker(&waker);

    loop {
        match fut.as_mut().poll(&mut cx) {
            Poll::Pending => {
                // Before sleeping, check if a signal has arrived. If so,
                // yield instead of blocking so that the future's
                // interruptible wrapper or poll_interrupt can observe
                // the flag on the next poll. Use a non-consuming read
                // to avoid stealing the flag from consumers that call
                // poll_interrupt / take_interrupt themselves.
                if task.interrupted() {
                    crate::yield_now();
                    continue;
                }

                let mut rq = current_run_queue::<PreemptIrqSaveState>();
                let mut woke = axwaker.woke.lock_irqsave();
                if !*woke {
                    #[cfg(feature = "profile")]
                    let profile_token = profile_block_begin();
                    rq.future_blocked_resched(woke);
                    #[cfg(feature = "profile")]
                    profile_block_end(profile_token);
                } else {
                    *woke = false;
                    drop(woke);
                    drop(rq);
                    crate::yield_now();
                }
            }
            Poll::Ready(output) => break output,
        }
    }
}

/// Error returned by [`interruptible`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("task wait was interrupted")]
pub struct Interrupted;

/// Makes a future interruptible.
pub async fn interruptible<F: IntoFuture>(f: F) -> Result<F::Output, Interrupted> {
    let mut f = pin!(f.into_future());
    let curr = current();
    poll_fn(|cx| {
        if curr.poll_interrupt(cx).is_ready() {
            return Poll::Ready(Err(Interrupted));
        }
        f.as_mut().poll(cx).map(Ok)
    })
    .await
}
