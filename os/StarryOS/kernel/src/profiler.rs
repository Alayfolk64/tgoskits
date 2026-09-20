//! Low-overhead in-kernel CPU and latency profiling for self-hosted builds.

use alloc::{boxed::Box, format, string::String, vec::Vec};
use core::{
    arch::asm,
    fmt::Write,
    sync::atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicU64, Ordering},
};

use ax_lazyinit::LazyInit;
use ax_runtime::{
    hal::{self, irq::InterruptedPrivilege},
    profile::{ProfileEvent, register_profile_hooks},
    task::thread::current::current_thread_id,
};

use crate::sync::IrqMutex;

const STACK_DEPTH: usize = 24;
const CPU_TABLE_SIZE: usize = 4096;
const WAIT_TABLE_SIZE: usize = 8192;
const PENDING_SIZE: usize = 1024;
const SAMPLE_PERIOD_NS: u64 = 50_000_000;
const MUTEX_SAMPLE_RATE: u32 = 8;
const PAGE_CACHE_SAMPLE_RATE: u32 = 32;
const EXT4_LOCK_SAMPLE_RATE: u32 = 8;

static ENABLED: AtomicBool = AtomicBool::new(false);
static PHASE: AtomicU8 = AtomicU8::new(0);
static PHASE_STARTED_NS: AtomicU64 = AtomicU64::new(0);
static PHASE_TOTAL_NS: [AtomicU64; 2] = [const { AtomicU64::new(0) }; 2];
static SKIPPED_CPU: AtomicU64 = AtomicU64::new(0);
static PROFILES: LazyInit<Vec<CpuProfile>> = LazyInit::new();

struct CpuProfile {
    last_sample_ns: AtomicU64,
    event_ticks: [AtomicU32; ProfileEvent::COUNT],
    state: IrqMutex<ProfileState>,
}

impl CpuProfile {
    fn new() -> Self {
        Self {
            last_sample_ns: AtomicU64::new(0),
            event_ticks: [const { AtomicU32::new(0) }; ProfileEvent::COUNT],
            state: IrqMutex::new(ProfileState::new()),
        }
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
struct StackTrace {
    task_id: u64,
    user: bool,
    len: u8,
    frames: [u64; STACK_DEPTH],
}

impl StackTrace {
    const EMPTY: Self = Self {
        task_id: 0,
        user: false,
        len: 0,
        frames: [0; STACK_DEPTH],
    };

    fn current() -> Self {
        let fp: usize;
        let lr: usize;
        // SAFETY: reading the current frame and link registers has no side effects.
        unsafe {
            asm!("mov {fp}, x29", "mov {lr}, x30", fp = out(reg) fp, lr = out(reg) lr);
        }
        let mut trace = Self {
            task_id: current_task_id(),
            ..Self::EMPTY
        };
        trace.len = crate::perf::unwind::kernel_callchain(
            lr.wrapping_sub(4),
            fp,
            &mut trace.frames,
        ) as u8;
        trace
    }

    fn interrupted(context: ax_runtime::hal::irq::InterruptedContext) -> Self {
        let user = context.privilege == InterruptedPrivilege::User;
        let mut trace = Self {
            task_id: current_task_id(),
            user,
            ..Self::EMPTY
        };
        let len = if user {
            crate::perf::unwind::user_callchain(
                context.pc,
                context.fp,
                context.sp,
                &mut trace.frames,
            )
        } else {
            crate::perf::unwind::kernel_callchain(context.pc, context.fp, &mut trace.frames)
        };
        trace.len = len as u8;
        trace
    }

    fn hash(&self, phase: u8, event: u8) -> u64 {
        let mut hash = 0xcbf2_9ce4_8422_2325u64;
        for value in [
            phase as u64,
            event as u64,
            self.task_id,
            self.user as u64,
            self.len as u64,
        ] {
            hash ^= value;
            hash = hash.wrapping_mul(0x100_0000_01b3);
        }
        for frame in &self.frames[..self.len as usize] {
            hash ^= *frame;
            hash = hash.wrapping_mul(0x100_0000_01b3);
        }
        hash.max(1)
    }
}

#[derive(Clone, Copy)]
struct Aggregate {
    hash: u64,
    phase: u8,
    event: u8,
    stack: StackTrace,
    total: u64,
    count: u64,
    max: u64,
}

impl Aggregate {
    const EMPTY: Self = Self {
        hash: 0,
        phase: 0,
        event: 0,
        stack: StackTrace::EMPTY,
        total: 0,
        count: 0,
        max: 0,
    };
}

#[derive(Clone, Copy)]
struct Pending {
    active: bool,
    generation: u32,
    phase: u8,
    event: u8,
    scale: u32,
    task_id: u64,
    started_ns: u64,
    stack: StackTrace,
}

impl Pending {
    const EMPTY: Self = Self {
        active: false,
        generation: 0,
        phase: 0,
        event: 0,
        scale: 1,
        task_id: 0,
        started_ns: 0,
        stack: StackTrace::EMPTY,
    };
}

struct ProfileState {
    cpu: Box<[Aggregate]>,
    wait: Box<[Aggregate]>,
    pending: Box<[Pending]>,
    next_pending: usize,
    dropped_cpu: u64,
    dropped_wait: u64,
    dropped_pending: u64,
}

impl ProfileState {
    fn new() -> Self {
        Self {
            cpu: alloc::vec![Aggregate::EMPTY; CPU_TABLE_SIZE].into_boxed_slice(),
            wait: alloc::vec![Aggregate::EMPTY; WAIT_TABLE_SIZE].into_boxed_slice(),
            pending: alloc::vec![Pending::EMPTY; PENDING_SIZE].into_boxed_slice(),
            next_pending: 0,
            dropped_cpu: 0,
            dropped_wait: 0,
            dropped_pending: 0,
        }
    }

    fn clear(&mut self) {
        self.cpu.fill(Aggregate::EMPTY);
        self.wait.fill(Aggregate::EMPTY);
        self.pending.fill(Pending::EMPTY);
        self.next_pending = 0;
        self.dropped_cpu = 0;
        self.dropped_wait = 0;
        self.dropped_pending = 0;
    }

    fn add_cpu(&mut self, phase: u8, stack: StackTrace) {
        if !add_aggregate(&mut self.cpu, phase, 0, stack, 1, 1) {
            self.dropped_cpu += 1;
        }
    }

    fn add_wait(&mut self, pending: Pending, duration_ns: u64) {
        if !add_aggregate(
            &mut self.wait,
            pending.phase,
            pending.event,
            pending.stack,
            duration_ns.saturating_mul(pending.scale as u64),
            duration_ns,
        ) {
            self.dropped_wait += 1;
        }
    }
}

fn add_aggregate(
    table: &mut [Aggregate],
    phase: u8,
    event: u8,
    stack: StackTrace,
    total_value: u64,
    max_value: u64,
) -> bool {
    debug_assert!(table.len().is_power_of_two());
    let hash = stack.hash(phase, event);
    let mut index = hash as usize & (table.len() - 1);
    for _ in 0..table.len() {
        let entry = &mut table[index];
        if entry.hash == 0 {
            *entry = Aggregate {
                hash,
                phase,
                event,
                stack,
                total: total_value,
                count: 1,
                max: max_value,
            };
            return true;
        }
        if entry.hash == hash
            && entry.phase == phase
            && entry.event == event
            && entry.stack == stack
        {
            entry.total = entry.total.saturating_add(total_value);
            entry.count = entry.count.saturating_add(1);
            entry.max = entry.max.max(max_value);
            return true;
        }
        index = (index + 1) & (table.len() - 1);
    }
    false
}

fn current_task_id() -> u64 {
    current_thread_id().map(|id| id.as_u64()).unwrap_or(0)
}

fn sample_period_elapsed(last_sample: &AtomicU64, now: u64) -> bool {
    let mut previous = last_sample.load(Ordering::Relaxed);
    loop {
        if previous != 0 && now.saturating_sub(previous) < SAMPLE_PERIOD_NS {
            return false;
        }
        match last_sample.compare_exchange_weak(previous, now, Ordering::Relaxed, Ordering::Relaxed)
        {
            Ok(_) => return true,
            Err(observed) => previous = observed,
        }
    }
}

fn profile_timer_sample(context: Option<ax_runtime::hal::irq::InterruptedContext>) {
    if !ENABLED.load(Ordering::Relaxed) {
        return;
    }
    let phase = PHASE.load(Ordering::Relaxed);
    if phase == 0 {
        return;
    }
    let Some(context) = context else {
        return;
    };
    let cpu = hal::percpu::this_cpu_id();
    let Some(profile) = PROFILES.get().and_then(|profiles| profiles.get(cpu)) else {
        return;
    };
    let now = hal::time::monotonic_time_nanos();
    if !sample_period_elapsed(&profile.last_sample_ns, now) {
        return;
    }
    let stack = StackTrace::interrupted(context);
    if let Some(mut state) = profile.state.try_lock() {
        state.add_cpu(phase, stack);
    } else {
        SKIPPED_CPU.fetch_add(1, Ordering::Relaxed);
    }
}

fn sample_rate(event: ProfileEvent) -> u32 {
    match event {
        ProfileEvent::MutexWait => MUTEX_SAMPLE_RATE,
        ProfileEvent::PageCache => PAGE_CACHE_SAMPLE_RATE,
        ProfileEvent::Ext4LockWait | ProfileEvent::Ext4LockHold => EXT4_LOCK_SAMPLE_RATE,
        _ => 1,
    }
}

fn profile_wait_begin(event: ProfileEvent, _object: usize) -> u64 {
    if !ENABLED.load(Ordering::Relaxed) {
        return 0;
    }
    let phase = PHASE.load(Ordering::Relaxed);
    if phase == 0 {
        return 0;
    }
    let cpu = hal::percpu::this_cpu_id();
    let Some(profile) = PROFILES.get().and_then(|profiles| profiles.get(cpu)) else {
        return 0;
    };
    let scale = sample_rate(event);
    if profile.event_ticks[event as usize].fetch_add(1, Ordering::Relaxed) % scale != 0 {
        return 0;
    }

    let stack = StackTrace::current();
    let started_ns = hal::time::monotonic_time_nanos();
    let task_id = stack.task_id;
    let mut state = profile.state.lock();
    for offset in 0..PENDING_SIZE {
        let index = (state.next_pending + offset) % PENDING_SIZE;
        if !state.pending[index].active {
            let generation = state.pending[index].generation.wrapping_add(1).max(1);
            state.pending[index] = Pending {
                active: true,
                generation,
                phase,
                event: event as u8,
                scale,
                task_id,
                started_ns,
                stack,
            };
            state.next_pending = (index + 1) % PENDING_SIZE;
            return ((generation as u64) << 32) | ((cpu as u64) << 16) | (index as u64 + 1);
        }
    }
    state.dropped_pending += 1;
    0
}

fn profile_wait_end(token: u64) {
    let raw_index = token as u16;
    if raw_index == 0 {
        return;
    }
    let index = raw_index as usize - 1;
    let cpu = ((token >> 16) & 0xffff) as usize;
    let generation = (token >> 32) as u32;
    let Some(profile) = PROFILES.get().and_then(|profiles| profiles.get(cpu)) else {
        return;
    };
    if index >= PENDING_SIZE {
        return;
    }

    let finished_ns = hal::time::monotonic_time_nanos();
    let mut state = profile.state.lock();
    let pending = state.pending[index];
    if !pending.active
        || pending.generation != generation
        || pending.task_id != current_task_id()
    {
        return;
    }
    state.pending[index].active = false;
    state.add_wait(pending, finished_ns.saturating_sub(pending.started_ns));
}

fn switch_phase(next: u8) {
    let now = hal::time::monotonic_time_nanos();
    let previous = PHASE.swap(next, Ordering::SeqCst);
    let started = PHASE_STARTED_NS.swap(if next == 0 { 0 } else { now }, Ordering::SeqCst);
    if previous != 0 && started != 0 {
        PHASE_TOTAL_NS[previous as usize].fetch_add(now.saturating_sub(started), Ordering::Relaxed);
    }
}

/// Initializes fixed-capacity profiler storage and hot-path hooks.
pub fn init() {
    let mut profiles = Vec::with_capacity(hal::cpu_num());
    profiles.resize_with(hal::cpu_num(), CpuProfile::new);
    PROFILES.init_once(profiles);
    ax_runtime::profile::register_sample_hook(profile_timer_sample);
    register_profile_hooks(profile_wait_begin, profile_wait_end);
}

/// Applies one command written to `/proc/starry_profile`.
pub fn command(input: &[u8]) {
    let command = core::str::from_utf8(input).unwrap_or("").trim();
    match command {
        "" => {}
        "reset" => {
            ENABLED.store(false, Ordering::SeqCst);
            switch_phase(0);
            if let Some(profiles) = PROFILES.get() {
                for profile in profiles {
                    profile.state.lock().clear();
                    profile.last_sample_ns.store(0, Ordering::Relaxed);
                    for ticks in &profile.event_ticks {
                        ticks.store(0, Ordering::Relaxed);
                    }
                }
            }
            SKIPPED_CPU.store(0, Ordering::Relaxed);
            PHASE_TOTAL_NS[1].store(0, Ordering::Relaxed);
        }
        "start" | "phase=prebuild" => {
            switch_phase(1);
            ENABLED.store(true, Ordering::Release);
        }
        "stop" => {
            ENABLED.store(false, Ordering::Release);
            switch_phase(0);
        }
        _ => {}
    }
}

fn write_stack(output: &mut String, stack: &StackTrace) {
    for (index, ip) in stack.frames[..stack.len as usize].iter().enumerate() {
        if index != 0 {
            output.push(';');
        }
        let _ = write!(output, "{ip:#x}");
    }
}

fn write_event_names(output: &mut String) {
    let names = [
        "cpu",
        "mutex_wait",
        "page_cache",
        "block_read",
        "block_write",
        "block_flush",
        "ext4_lock_wait",
        "ext4_lock_hold",
        "block_admission",
        "block_dispatch",
        "block_completion_wait",
        "block_completion_drain",
    ];
    for (event, name) in names.iter().enumerate().skip(1) {
        let _ = writeln!(output, "STARRY_EVENT {event} {name}");
    }
}

/// Renders a stable raw snapshot for host-side symbolization and aggregation.
pub fn snapshot() -> String {
    let mut dropped_cpu = 0;
    let mut dropped_wait = 0;
    let mut dropped_pending = 0;
    if let Some(profiles) = PROFILES.get() {
        for profile in profiles {
            let state = profile.state.lock();
            dropped_cpu += state.dropped_cpu;
            dropped_wait += state.dropped_wait;
            dropped_pending += state.dropped_pending;
        }
    }

    let mut output = format!(
        "STARRY_PROFILE_V2 sample_hz=20 mutex_sample_rate={} page_cache_sample_rate={} \
         ext4_lock_sample_rate={} enabled={} phase={} prebuild_ns={} dropped_cpu={} \
         dropped_wait={} dropped_pending={} skipped_cpu={}\n",
        MUTEX_SAMPLE_RATE,
        PAGE_CACHE_SAMPLE_RATE,
        EXT4_LOCK_SAMPLE_RATE,
        ENABLED.load(Ordering::Relaxed),
        PHASE.load(Ordering::Relaxed),
        PHASE_TOTAL_NS[1].load(Ordering::Relaxed),
        dropped_cpu,
        dropped_wait,
        dropped_pending,
        SKIPPED_CPU.load(Ordering::Relaxed),
    );
    output.push_str("STARRY_PHASE 1 prebuild\n");
    write_event_names(&mut output);

    let block = ax_fs_ng::block::runtime::block_batch_stats();
    let _ = writeln!(
        output,
        "STARRY_BLOCK submission_batches={} submitted_requests={} commit_calls={} \
         commit_failures={} completed_requests={} failed_requests={} largest_batch={} \
         peak_inflight={}",
        block.submission_batches,
        block.submitted_requests,
        block.commit_calls,
        block.commit_failures,
        block.completed_requests,
        block.failed_requests,
        block.largest_batch,
        block.peak_inflight,
    );
    let (reads, sectors_read, writes, sectors_written) =
        ax_fs_ng::block::runtime::block_io_stats();
    let _ = writeln!(
        output,
        "STARRY_BLOCK_IO reads={reads} sectors_read={sectors_read} writes={writes} \
         sectors_written={sectors_written}"
    );
    let dma_pool = ax_fs_ng::block::runtime::dma_pool_stats();
    let _ = writeln!(
        output,
        "STARRY_DMA_POOL hits={} misses={} returns={} rejects={}",
        dma_pool.hits, dma_pool.misses, dma_pool.returns, dma_pool.rejects,
    );

    if let Some(profiles) = PROFILES.get() {
        for (cpu, profile) in profiles.iter().enumerate() {
            let mut cpu_entries = Vec::with_capacity(CPU_TABLE_SIZE);
            let mut wait_entries = Vec::with_capacity(WAIT_TABLE_SIZE);
            {
                let state = profile.state.lock();
                cpu_entries.extend(state.cpu.iter().filter(|entry| entry.hash != 0).copied());
                wait_entries.extend(state.wait.iter().filter(|entry| entry.hash != 0).copied());
            }
            for entry in &cpu_entries {
                let _ = write!(
                    output,
                    "STARRY_CPU phase={} cpu={} samples={} task={} user={} stack=",
                    entry.phase, cpu, entry.total, entry.stack.task_id, entry.stack.user as u8,
                );
                write_stack(&mut output, &entry.stack);
                output.push('\n');
            }
            for entry in &wait_entries {
                let _ = write!(
                    output,
                    "STARRY_WAIT phase={} cpu={} event={} count={} total_ns={} max_ns={} task={} \
                     stack=",
                    entry.phase,
                    cpu,
                    entry.event,
                    entry.count,
                    entry.total,
                    entry.max,
                    entry.stack.task_id,
                );
                write_stack(&mut output, &entry.stack);
                output.push('\n');
            }
        }
    }
    output
}
