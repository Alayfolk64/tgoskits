//! Optional in-kernel sampling bridge owned by the final operating system.

use core::{
    mem,
    sync::atomic::{AtomicUsize, Ordering},
};

use ax_hal::irq::InterruptedContext;
pub use ax_sync::{ProfileEvent, ProfileScope, register_profile_hooks};

type SampleHook = fn(Option<InterruptedContext>);

static SAMPLE_HOOK: AtomicUsize = AtomicUsize::new(0);

/// Installs the callback invoked by the local clockevent interrupt.
pub fn register_sample_hook(hook: SampleHook) {
    SAMPLE_HOOK.store(hook as usize, Ordering::Release);
}

pub(crate) fn sample(context: Option<InterruptedContext>) {
    let hook = SAMPLE_HOOK.load(Ordering::Acquire);
    if hook == 0 {
        return;
    }
    // SAFETY: `register_sample_hook` stores this exact function type.
    let hook: SampleHook = unsafe { mem::transmute(hook) };
    hook(context);
}
