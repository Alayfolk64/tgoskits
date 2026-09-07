//! Low-overhead in-guest CPU and off-CPU sampling for self-build profiling.

use alloc::{boxed::Box, format, string::String, vec::Vec};
use core::{
    arch::asm,
    fmt::Write,
    mem::{align_of, size_of},
    sync::atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicU64, AtomicUsize, Ordering},
};

use ax_lazyinit::LazyInit;
use ax_runtime::{
    hal::{self, cpu::UserRegisters},
    sync::ProfileEvent,
};

use crate::sync::IrqMutex;

const STACK_DEPTH: usize = 20;
const CPU_TABLE_SIZE: usize = 4096;
const WAIT_TABLE_SIZE: usize = 8192;
const PENDING_SIZE: usize = 512;
const SAMPLE_PERIOD_NS: u64 = 100_000_000;
const EXT4_SAMPLE_RATE: u32 = 16;
const PAGE_CACHE_SAMPLE_RATE: u32 = 32;
const OFFCPU_SAMPLE_RATE: u32 = 8;
const EVENT_COUNT: usize = 8;
const USER_SPACE_FRAME: usize = usize::MAX;
const TASK_NAME_LEN: usize = 16;

static ENABLED: AtomicBool = AtomicBool::new(false);
static PHASE: AtomicU8 = AtomicU8::new(0);
static PHASE_STARTED_NS: AtomicU64 = AtomicU64::new(0);
static PHASE_TOTAL_NS: [AtomicU64; 2] = [const { AtomicU64::new(0) }; 2];
static SKIPPED_CPU: AtomicU64 = AtomicU64::new(0);
static PROFILES: LazyInit<Vec<CpuProfile>> = LazyInit::new();

struct CpuProfile {
    last_sample_ns: AtomicU64,
    event_ticks: [AtomicU32; EVENT_COUNT],
    interrupted: InterruptedContextSlot,
    state: IrqMutex<ProfileState>,
}

impl CpuProfile {
    fn new() -> Self {
        Self {
            last_sample_ns: AtomicU64::new(0),
            event_ticks: [const { AtomicU32::new(0) }; EVENT_COUNT],
            interrupted: InterruptedContextSlot::new(),
            state: IrqMutex::new(ProfileState::new()),
        }
    }
}

struct InterruptedContextSlot {
    valid: AtomicBool,
    user: AtomicBool,
    ip: AtomicUsize,
    fp: AtomicUsize,
}

impl InterruptedContextSlot {
    const fn new() -> Self {
        Self {
            valid: AtomicBool::new(false),
            user: AtomicBool::new(false),
            ip: AtomicUsize::new(0),
            fp: AtomicUsize::new(0),
        }
    }

    fn store(&self, context: InterruptedContext) {
        self.user.store(context.user, Ordering::Relaxed);
        self.ip.store(context.ip, Ordering::Relaxed);
        self.fp.store(context.fp, Ordering::Relaxed);
        self.valid.store(true, Ordering::Release);
    }

    fn take(&self) -> Option<InterruptedContext> {
        self.valid
            .swap(false, Ordering::Acquire)
            .then(|| InterruptedContext {
                user: self.user.load(Ordering::Relaxed),
                ip: self.ip.load(Ordering::Relaxed),
                fp: self.fp.load(Ordering::Relaxed),
            })
    }

    fn clear(&self) {
        self.valid.store(false, Ordering::Release);
    }
}

#[derive(Clone, Copy)]
struct InterruptedContext {
    user: bool,
    ip: usize,
    fp: usize,
}

#[derive(Clone, Copy, Eq, PartialEq)]
struct TaskName {
    len: u8,
    bytes: [u8; TASK_NAME_LEN],
}

impl TaskName {
    const EMPTY: Self = Self {
        len: 0,
        bytes: [0; TASK_NAME_LEN],
    };

    fn current() -> Self {
        let mut bytes = [0; TASK_NAME_LEN];
        let len = ax_task::current_may_uninit()
            .map(|task| task.copy_name(&mut bytes))
            .unwrap_or(0);
        Self {
            len: len as u8,
            bytes,
        }
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
struct StackTrace {
    task_name: TaskName,
    len: u8,
    frames: [usize; STACK_DEPTH],
}

impl StackTrace {
    const EMPTY: Self = Self {
        task_name: TaskName::EMPTY,
        len: 0,
        frames: [0; STACK_DEPTH],
    };

    fn current() -> Self {
        Self {
            task_name: TaskName::current(),
            ..Self::EMPTY
        }
    }

    fn push(&mut self, ip: usize) {
        if ip != 0 && (self.len as usize) < STACK_DEPTH {
            self.frames[self.len as usize] = ip;
            self.len += 1;
        }
    }

    fn hash(&self, phase: u8, event: u8) -> u64 {
        let mut hash = 0xcbf2_9ce4_8422_2325u64;
        for value in [phase as u64, event as u64, self.len as u64] {
            hash ^= value;
            hash = hash.wrapping_mul(0x100_0000_01b3);
        }
        for &byte in &self.task_name.bytes[..self.task_name.len as usize] {
            hash ^= byte as u64;
            hash = hash.wrapping_mul(0x100_0000_01b3);
        }
        for &ip in &self.frames[..self.len as usize] {
            hash ^= ip as u64;
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

    fn add_wait(&mut self, phase: u8, event: u8, stack: StackTrace, duration_ns: u64, scale: u32) {
        if !add_aggregate(
            &mut self.wait,
            phase,
            event,
            stack,
            duration_ns.saturating_mul(scale as u64),
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

#[repr(C)]
#[derive(Clone, Copy)]
struct FrameRecord {
    previous_fp: usize,
    return_address: usize,
}

unsafe extern "C" {
    fn _stext();
    fn _etext();
}

fn is_kernel_text(ip: usize) -> bool {
    ip >= _stext as *const () as usize && ip < _etext as *const () as usize
}

fn unwind_kernel_stack(stack: &mut StackTrace, mut fp: usize) {
    let Some(range) = ax_task::current_may_uninit().map(|task| task.kernel_stack_range()) else {
        return;
    };

    while (stack.len as usize) < STACK_DEPTH {
        if !fp.is_multiple_of(align_of::<FrameRecord>())
            || fp < range.start
            || fp.saturating_add(size_of::<FrameRecord>()) > range.end
        {
            break;
        }
        // SAFETY: the frame record lies entirely inside the current task's
        // allocated kernel stack and is naturally aligned.
        let record = unsafe { (fp as *const FrameRecord).read() };
        let return_address = record.return_address.wrapping_sub(4);
        if !is_kernel_text(return_address) {
            break;
        }
        stack.push(return_address);
        if record.previous_fp <= fp || record.previous_fp >= range.end {
            break;
        }
        fp = record.previous_fp;
    }
}

fn capture_current_stack() -> StackTrace {
    let fp: usize;
    let lr: usize;
    // SAFETY: reading the current frame and link registers has no side effects.
    unsafe {
        asm!("mov {fp}, x29", "mov {lr}, x30", fp = out(reg) fp, lr = out(reg) lr);
    }
    let mut stack = StackTrace::current();
    stack.push(lr.wrapping_sub(4));
    unwind_kernel_stack(&mut stack, fp);
    stack
}

fn capture_trap_stack(context: InterruptedContext) -> StackTrace {
    let mut stack = StackTrace::current();
    if context.user {
        stack.push(USER_SPACE_FRAME);
        return stack;
    }
    stack.push(context.ip);
    unwind_kernel_stack(&mut stack, context.fp);
    stack
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

fn profile_irq_context(registers: &UserRegisters) {
    if !ENABLED.load(Ordering::Relaxed) {
        return;
    }
    let phase = PHASE.load(Ordering::Relaxed);
    if phase == 0 {
        return;
    }
    let cpu = hal::percpu::this_cpu_id();
    let Some(profile) = PROFILES.get().and_then(|profiles| profiles.get(cpu)) else {
        return;
    };
    profile.interrupted.store(InterruptedContext {
        user: registers.origin() == ax_runtime::hal::cpu::TrapOrigin::User,
        ip: registers.elr as usize,
        fp: registers.x[29] as usize,
    });
}

fn profile_timer_sample() {
    if !ENABLED.load(Ordering::Relaxed) {
        return;
    }
    let phase = PHASE.load(Ordering::Relaxed);
    if phase == 0 {
        return;
    }
    let cpu = hal::percpu::this_cpu_id();
    let Some(profile) = PROFILES.get().and_then(|profiles| profiles.get(cpu)) else {
        return;
    };
    let Some(context) = profile.interrupted.take() else {
        return;
    };
    let now = hal::time::monotonic_time_nanos();
    if !sample_period_elapsed(&profile.last_sample_ns, now) {
        return;
    }

    let stack = capture_trap_stack(context);
    if let Some(mut state) = profile.state.try_lock() {
        state.add_cpu(phase, stack);
    } else {
        SKIPPED_CPU.fetch_add(1, Ordering::Relaxed);
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
    let scale = match event {
        ProfileEvent::Ext4 => EXT4_SAMPLE_RATE,
        ProfileEvent::PageCache => PAGE_CACHE_SAMPLE_RATE,
        ProfileEvent::OffCpu => OFFCPU_SAMPLE_RATE,
        _ => 1,
    };
    if profile.event_ticks[event as usize].fetch_add(1, Ordering::Relaxed) % scale != 0 {
        return 0;
    }

    let stack = capture_current_stack();
    let started_ns = hal::time::monotonic_time_nanos();
    let task_id = ax_task::current().id().as_u64();
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
        || pending.task_id != ax_task::current().id().as_u64()
    {
        return;
    }
    state.pending[index].active = false;
    state.add_wait(
        pending.phase,
        pending.event,
        pending.stack,
        finished_ns.saturating_sub(pending.started_ns),
        pending.scale,
    );
}

fn profile_block_begin() -> u64 {
    profile_wait_begin(ProfileEvent::OffCpu, 0)
}

fn switch_phase(next: u8) {
    let now = hal::time::monotonic_time_nanos();
    let previous = PHASE.swap(next, Ordering::SeqCst);
    let started = PHASE_STARTED_NS.swap(if next == 0 { 0 } else { now }, Ordering::SeqCst);
    if previous != 0 && started != 0 {
        PHASE_TOTAL_NS[previous as usize].fetch_add(now.saturating_sub(started), Ordering::Relaxed);
    }
}

/// Initializes the fixed-capacity profiler storage and hot-path hooks.
pub fn init() {
    let mut profiles = Vec::with_capacity(hal::cpu_num());
    profiles.resize_with(hal::cpu_num(), CpuProfile::new);
    PROFILES.init_once(profiles);
    ax_runtime::hal::cpu::trap::set_profile_sample_handler(profile_irq_context);
    ax_task::register_timer_profile_hook(profile_timer_sample);
    ax_runtime::sync::register_profile_hooks(profile_wait_begin, profile_wait_end);
    ax_task::future::register_block_profile_hooks(profile_block_begin, profile_wait_end);
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
                    profile.interrupted.clear();
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

fn write_task_name(output: &mut String, task_name: &TaskName) {
    for byte in &task_name.bytes[..task_name.len as usize] {
        let _ = write!(output, "{byte:02x}");
    }
}

fn write_stack(output: &mut String, stack: &StackTrace) {
    for (index, ip) in stack.frames[..stack.len as usize].iter().enumerate() {
        if index != 0 {
            output.push(';');
        }
        if *ip == USER_SPACE_FRAME {
            output.push_str("user");
        } else {
            let _ = write!(output, "{ip:#x}");
        }
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
        "STARRY_PROFILE_V1 sample_hz=10 ext4_sample_rate={} page_cache_sample_rate={} \
         offcpu_sample_rate={} enabled={} phase={} prebuild_ns={} dropped_cpu={} dropped_wait={} \
         dropped_pending={} skipped_cpu={}\n",
        EXT4_SAMPLE_RATE,
        PAGE_CACHE_SAMPLE_RATE,
        OFFCPU_SAMPLE_RATE,
        ENABLED.load(Ordering::Relaxed),
        PHASE.load(Ordering::Relaxed),
        PHASE_TOTAL_NS[1].load(Ordering::Relaxed),
        dropped_cpu,
        dropped_wait,
        dropped_pending,
        SKIPPED_CPU.load(Ordering::Relaxed),
    );
    output.push_str(
        "STARRY_PHASE 1 prebuild\nSTARRY_EVENT 1 mutex_wait\nSTARRY_EVENT 2 ext4\nSTARRY_EVENT 3 \
         page_cache\nSTARRY_EVENT 4 block_read\nSTARRY_EVENT 5 block_write\nSTARRY_EVENT 6 \
         offcpu\nSTARRY_EVENT 7 ext4_lock_wait\n",
    );

    if let Some(profiles) = PROFILES.get() {
        for profile in profiles {
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
                    "STARRY_CPU phase={} samples={} task=",
                    entry.phase, entry.total
                );
                write_task_name(&mut output, &entry.stack.task_name);
                output.push_str(" stack=");
                write_stack(&mut output, &entry.stack);
                output.push('\n');
            }
            for entry in &wait_entries {
                let _ = write!(
                    output,
                    "STARRY_WAIT phase={} event={} count={} total_ns={} max_ns={} task=",
                    entry.phase, entry.event, entry.count, entry.total, entry.max
                );
                write_task_name(&mut output, &entry.stack.task_name);
                output.push_str(" stack=");
                write_stack(&mut output, &entry.stack);
                output.push('\n');
            }
        }
    }
    output
}
