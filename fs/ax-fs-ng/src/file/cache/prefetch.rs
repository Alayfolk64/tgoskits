//! Bounded speculative I/O. Demand uses the same validated fill ownership.

use alloc::{boxed::Box, collections::VecDeque, string::String, sync::Arc};
use core::ops::Range;

use super::CachedFile;
use crate::os::{BlockNotification, BlockRuntimeOps, BlockThread, runtime_ops, sync::Mutex};

const MAX_QUEUED_WINDOWS: usize = 16;

struct PrefetchJob {
    file: CachedFile,
    pages: Range<u64>,
}

impl PrefetchJob {
    fn run(self) {
        let Ok(first_page) = u32::try_from(self.pages.start) else {
            return;
        };
        let Some(window_pages) = self
            .pages
            .end
            .checked_sub(self.pages.start)
            .filter(|count| *count != 0)
            .and_then(|count| usize::try_from(count).ok())
        else {
            return;
        };
        // Do not displace the demand window in a cache with a tiny retention
        // target. No cache lock survives the backing read.
        if self.file.shared.page_cache.lock().cap().get()
            < 2 * super::readahead::MAX_READAHEAD_PAGES
        {
            return;
        }
        if let Err(error) = self.file.prefetch_page_window(first_page, window_pages) {
            // Speculation must not make a later demand inherit a stale error.
            // Its demand path retries backing I/O and reports any real failure.
            log::debug!("file readahead deferred: {error:?}");
        }
    }
}

struct WorkerResources {
    notification: Arc<dyn BlockNotification>,
    _thread: Box<dyn BlockThread>,
}

struct PrefetchWorker {
    queue: Mutex<VecDeque<PrefetchJob>>,
    resources: Mutex<Option<WorkerResources>>,
}

impl PrefetchWorker {
    const fn new() -> Self {
        Self {
            queue: Mutex::new(VecDeque::new()),
            resources: Mutex::new(None),
        }
    }

    fn start(&'static self, runtime: &'static dyn BlockRuntimeOps) -> bool {
        let mut resources = self.resources.lock();
        if resources.is_some() {
            return true;
        }
        if runtime.online_cpu_count() == 0 {
            return false;
        }
        let notification = runtime.notification();
        let waiter = Arc::clone(&notification);
        let entry = Box::new(move || {
            loop {
                waiter.wait();
                loop {
                    let job = self.queue.lock().pop_front();
                    let Some(job) = job else { break };
                    job.run();
                }
            }
        });
        let cpu = runtime.current_cpu() % runtime.online_cpu_count();
        let Ok(thread) = runtime.spawn_pinned(String::from("axfs-readahead"), cpu, entry) else {
            return false;
        };
        *resources = Some(WorkerResources {
            notification,
            _thread: thread,
        });
        true
    }

    fn push(&self, job: PrefetchJob) -> bool {
        let mut queue = self.queue.lock();
        if queue.len() == MAX_QUEUED_WINDOWS || queue.try_reserve(1).is_err() {
            return false;
        }
        queue.push_back(job);
        true
    }

    fn request(&self, job: PrefetchJob) {
        let notification = self
            .resources
            .lock()
            .as_ref()
            .map(|r| Arc::clone(&r.notification));
        if let Some(notification) = notification
            && self.push(job)
        {
            notification.notify();
        }
    }
}

static PREFETCH: PrefetchWorker = PrefetchWorker::new();

pub(crate) fn start_background_readahead(runtime: &'static dyn BlockRuntimeOps) -> bool {
    PREFETCH.start(runtime)
}

impl CachedFile {
    pub(super) fn prefetch_after_read(&self, number: u32) {
        if self.in_memory || !runtime_ops().is_ok_and(|runtime| runtime.can_block()) {
            return;
        }
        let pages = self.shared.len().div_ceil(super::PAGE_SIZE as u64);
        let Some(pages) = self.readahead.lock().take_prefetch(number, pages) else {
            return;
        };
        let job = PrefetchJob {
            file: self.clone(),
            pages,
        };
        #[cfg(not(test))]
        PREFETCH.request(job);
        #[cfg(test)]
        tests::enqueue(job);
    }
}

#[cfg(test)]
pub(super) mod tests {
    use core::cell::RefCell;

    use super::*;

    std::thread_local! {
        static QUEUED: RefCell<Option<Arc<PrefetchWorker>>> = const { RefCell::new(None) };
    }

    pub(super) fn enqueue(job: PrefetchJob) {
        QUEUED.with(|queue| {
            if let Some(worker) = queue.borrow().as_ref() {
                worker.push(job);
            } else {
                PREFETCH.request(job);
            }
        });
    }

    pub(in super::super) fn with_queue(run: impl FnOnce(&dyn Fn())) {
        struct Reset;
        impl Drop for Reset {
            fn drop(&mut self) {
                QUEUED.with(|queue| *queue.borrow_mut() = None);
            }
        }
        let worker = Arc::new(PrefetchWorker::new());
        QUEUED.with(|queue| *queue.borrow_mut() = Some(Arc::clone(&worker)));
        let _reset = Reset;
        run(&|| {
            let job = worker
                .queue
                .lock()
                .pop_front()
                .expect("read did not queue readahead");
            job.run();
        });
    }
}
